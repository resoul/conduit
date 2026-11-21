use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ir::{Module, SchedulePlan};
use crate::native::cron::{
    CronExpr, CronSchedulerState, make_cron_error, make_cron_scheduler, parse_cron_expr,
};
use crate::native::validation::is_err_or_nil_obj;
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{Obj, SCHEMA_CRON_SCHEDULER};
use crate::vm::{ExecResult, Permissions, RuntimeError, SpawnTracker, Vm};

pub const CRON_RUN: u8 = 0;
pub const CRON_START: u8 = 1;
pub const CRON_RUN_ONCE: u8 = 2;
pub const CRON_VALIDATE: u8 = 3;
pub const CRON_STOP: u8 = 4;
pub const CRON_IS_RUNNING: u8 = 5;

enum JobTiming {
    Cron { expr: CronExpr, next_unix: u64 },
    Interval { every: Duration, next_at: Instant },
}

pub(crate) struct CompiledJob {
    name: String,
    func_id: u32,
    timing: JobTiming,
    in_flight: Arc<AtomicBool>,
}

pub(crate) fn build_compiled_jobs(plan: &SchedulePlan) -> Result<Vec<CompiledJob>, String> {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now_inst = Instant::now();
    let mut out = Vec::with_capacity(plan.jobs.len());
    for j in &plan.jobs {
        let timing = if let Some(raw) = &j.cron_expr {
            let expr = parse_cron_expr(raw)?;
            let next_unix = expr
                .next_tick_after(now_unix)
                .ok_or_else(|| format!("Cron '{}' has no upcoming tick", raw))?;
            JobTiming::Cron { expr, next_unix }
        } else if let Some(ms) = j.interval_ms {
            let every = Duration::from_millis(ms.max(1));
            JobTiming::Interval {
                every,
                next_at: now_inst + every,
            }
        } else {
            return Err(format!("Job '{}' has neither cron expression nor interval", j.name));
        };
        out.push(CompiledJob {
            name: j.name.clone(),
            func_id: j.func_id,
            timing,
            in_flight: Arc::new(AtomicBool::new(false)),
        });
    }
    Ok(out)
}

pub(crate) fn start_cron_scheduler(
    module: Arc<Module>,
    plan: &SchedulePlan,
    spawn_tracker: Option<Arc<SpawnTracker>>,
    permissions: Arc<Permissions>,
) -> Result<CronSchedulerState, String> {
    let jobs = build_compiled_jobs(plan)?;
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let stop_bg = Arc::clone(&stop);
    let running_bg = Arc::clone(&running);
    let handle = std::thread::spawn(move || {
        scheduler_loop(module, jobs, stop_bg, running_bg, spawn_tracker, permissions);
    });
    Ok(CronSchedulerState {
        stop,
        running,
        handle: Some(handle),
    })
}

pub(crate) fn stop_cron_scheduler(mut st: CronSchedulerState) {
    st.stop.store(true, Ordering::Release);
    if let Some(h) = st.handle.take() {
        let _ = h.join();
    }
}

fn run_job_in_fresh_vm(
    module: &Module,
    job_name: &str,
    func_id: u32,
    permissions: &Arc<Permissions>,
) -> Result<(), String> {
    let mut vm = Vm::new();
    vm.host.permissions = Arc::clone(permissions);
    match vm.call(module, func_id, &[], &[]) {
        Ok(ExecResult::Obj(obj)) if is_err_or_nil_obj(module, &obj) && !matches!(obj, Obj::Empty) => {
            Err(format!("cron job '{}' returned error: {}", job_name, obj.format_obj()))
        }
        Ok(_) => Ok(()),
        Err(e) => Err(format!("cron job '{}' runtime error: {}", job_name, e.message)),
    }
}

struct CronJobGuard {
    flag: Arc<AtomicBool>,
    tracker: Option<Arc<SpawnTracker>>,
}

impl Drop for CronJobGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
        if let Some(tracker) = &self.tracker {
            tracker.on_finish();
        }
    }
}

pub(crate) fn scheduler_loop(
    module: Arc<Module>,
    mut jobs: Vec<CompiledJob>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    spawn_tracker: Option<Arc<SpawnTracker>>,
    permissions: Arc<Permissions>,
) {
    running.store(true, Ordering::Release);
    while !stop.load(Ordering::Acquire) {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let now_inst = Instant::now();
        let mut sleep_dur = Duration::from_millis(500);

        for job in &mut jobs {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let due = match &mut job.timing {
                JobTiming::Cron { expr, next_unix } => {
                    if now_unix >= *next_unix {
                        *next_unix = expr.next_tick_after(now_unix).unwrap_or(now_unix + 60);
                        true
                    } else {
                        let wait = Duration::from_secs((*next_unix - now_unix).min(1));
                        if wait < sleep_dur {
                            sleep_dur = wait;
                        }
                        false
                    }
                }
                JobTiming::Interval { every, next_at } => {
                    if now_inst >= *next_at {
                        *next_at = now_inst + *every;
                        true
                    } else {
                        let wait = next_at.saturating_duration_since(now_inst);
                        if wait < sleep_dur {
                            sleep_dur = wait;
                        }
                        false
                    }
                }
            };

            if due && job.in_flight.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                let m = Arc::clone(&module);
                let jname = job.name.clone();
                let fid = job.func_id;
                let flag = Arc::clone(&job.in_flight);
                let tracker = spawn_tracker.clone();
                let perms = Arc::clone(&permissions);
                if let Some(ref t) = tracker {
                    t.on_spawn();
                }
                std::thread::spawn(move || {
                    let _guard = CronJobGuard { flag, tracker };
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_job_in_fresh_vm(&m, &jname, fid, &perms)
                    })) {
                        Ok(Ok(())) => {}
                        Ok(Err(msg)) => eprintln!("[cdt.cron] {}", msg),
                        Err(_) => eprintln!("[cdt.cron] job '{}' panicked", jname),
                    }
                });
            }
        }

        if !stop.load(Ordering::Acquire) {
            std::thread::sleep(sleep_dur.min(Duration::from_millis(250)));
        }
    }
    running.store(false, Ordering::Release);
}

impl Vm {
    fn extract_scheduler_id(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected cron.Scheduler"));
        };
        if rec.schema_id != SCHEMA_CRON_SCHEDULER {
            return Err(err_msg("Expected cron.Scheduler"));
        }
        Ok(rec.get_scalar(0) as usize)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_cron_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            CRON_VALIDATE => {
                let Obj::Str(s) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String in cron.validate()"));
                };
                self.scalars[s_base + a as usize] =
                    u64::from(parse_cron_expr(s.as_str()).is_ok());
            }
            CRON_RUN => {
                let plan = &module.schedules[b as usize];
                match build_compiled_jobs(plan) {
                    Ok(jobs) => {
                        let stop = Arc::new(AtomicBool::new(false));
                        let running = Arc::new(AtomicBool::new(true));
                        scheduler_loop(
                            Arc::new(module.clone()),
                            jobs,
                            stop,
                            running,
                            Some(Arc::clone(&self.host.spawn_tracker)),
                            Arc::clone(&self.host.permissions),
                        );
                        self.objs[o_base + a as usize] = Obj::Empty;
                    }
                    Err(msg) => {
                        self.objs[o_base + a as usize] = make_cron_error("invalid_schedule", &msg);
                    }
                }
            }
            CRON_START => {
                let plan = &module.schedules[b as usize];
                match start_cron_scheduler(
                    Arc::new(module.clone()),
                    plan,
                    Some(Arc::clone(&self.host.spawn_tracker)),
                    Arc::clone(&self.host.permissions),
                ) {
                    Ok(state) => {
                        let id = self.host.cron_schedulers.len();
                        self.host.cron_schedulers.push(Some(state));
                        self.objs[o_base + a as usize] = make_cron_scheduler(id as i64);
                    }
                    Err(msg) => {
                        self.objs[o_base + a as usize] = make_cron_error("invalid_schedule", &msg);
                    }
                }
            }
            CRON_RUN_ONCE => {
                let plan = &module.schedules[b as usize];
                let Obj::Str(job_name) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected String job name in cron.run_once()"));
                };
                let res = match plan.jobs.iter().find(|j| j.name == job_name.as_str()) {
                    Some(job) => match run_job_in_fresh_vm(
                        module,
                        &job.name,
                        job.func_id,
                        &self.host.permissions,
                    ) {
                        Ok(()) => Obj::Empty,
                        Err(msg) => make_cron_error("job_failed", &msg),
                    },
                    None => make_cron_error(
                        "unknown_job",
                        &format!("Job '{}' not found in schedule '{}'", job_name, plan.name),
                    ),
                };
                self.objs[o_base + a as usize] = res;
            }
            CRON_STOP => {
                let id = self.extract_scheduler_id(&self.objs[o_base + b as usize])?;
                if let Some(Some(st)) = self.host.cron_schedulers.get_mut(id).map(|slot| slot.take()) {
                    stop_cron_scheduler(st);
                }
            }
            CRON_IS_RUNNING => {
                let id = self.extract_scheduler_id(&self.objs[o_base + b as usize])?;
                let is_running = self
                    .host
                    .cron_schedulers
                    .get(id)
                    .and_then(|s| s.as_ref())
                    .is_some_and(|st| st.running.load(Ordering::Acquire));
                self.scalars[s_base + a as usize] = u64::from(is_running);
            }
            _ => return Err(err_msg("Unknown cron sub-op")),
        }
        Ok(())
    }
}
