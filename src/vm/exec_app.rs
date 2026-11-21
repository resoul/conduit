use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ir::{AppPlan, Module};
use crate::native::cron::CronSchedulerState;
use crate::native::env::exec_env_load_checked;
use crate::native::validation::is_err_or_nil_obj;
use crate::vm::exec_cold::err_msg;
use crate::vm::exec_cron::{start_cron_scheduler, stop_cron_scheduler};
use crate::vm::exec_http::{SIGNAL_STOP, install_signal_handlers, serve_http_listener};
use crate::vm::value::{
    Obj, SCHEMA_APP_ERROR, SCHEMA_APP_HANDLE, SCHEMA_ENV_ERROR, SCHEMA_HTTP_ROUTER, StructData,
};
use crate::vm::{RuntimeError, Vm};

pub const APP_RUN: u8 = 0;
pub const APP_START: u8 = 1;
pub const APP_STOP: u8 = 2;
pub const APP_IS_RUNNING: u8 = 3;

pub(crate) struct AppHandleState {
    pub http_stop: Option<(usize, Arc<AtomicBool>, Option<JoinHandle<()>>)>,
    pub cron: Option<CronSchedulerState>,
    pub shutdown_ms: u64,
    pub on_stop_funcs: Vec<u32>,
    pub running: Arc<AtomicBool>,
}

pub fn make_app_error(kind: &str, message: &str) -> Obj {
    let mut s = StructData::new(SCHEMA_APP_ERROR, 0, 2);
    s.set_obj(0, Obj::str(kind));
    s.set_obj(1, Obj::str(message));
    Obj::Record(Arc::new(s))
}

pub fn make_app_handle(id: i64) -> Obj {
    let mut s = StructData::new(SCHEMA_APP_HANDLE, 1, 0);
    s.set_scalar(0, id as u64);
    Obj::Record(Arc::new(s))
}

fn format_config_violations(violations: &[Obj]) -> String {
    let mut parts = Vec::with_capacity(violations.len());
    for v in violations {
        if let Obj::Record(rec) = v
            && rec.obj_len >= 3
        {
            let path = rec.get_obj(0).format_obj();
            let msg = rec.get_obj(2).format_obj();
            if path.is_empty() {
                parts.push(msg);
            } else {
                parts.push(format!("{}: {}", path, msg));
            }
        }
    }
    if parts.is_empty() {
        format!("Config validation failed ({} issue(s))", violations.len())
    } else {
        format!("Config validation failed: {}", parts.join("; "))
    }
}

impl Vm {
    fn extract_app_handle_id(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected app.App handle"));
        };
        if rec.schema_id != SCHEMA_APP_HANDLE {
            return Err(err_msg("Expected app.App handle"));
        }
        Ok(rec.get_scalar(0) as usize)
    }

    fn extract_optional_router_id(&self, obj: &Obj) -> Result<Option<usize>, RuntimeError> {
        match obj {
            Obj::Empty => Ok(None),
            Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_ROUTER => {
                Ok(Some(rec.get_scalar(0) as usize))
            }
            _ => Err(err_msg("Expected http.Router or Empty in cdt.app")),
        }
    }

    fn validate_app_config(
        &mut self,
        module: &Module,
        plan: &AppPlan,
        s_top: usize,
        o_top: usize,
    ) -> Result<Option<String>, RuntimeError> {
        let Some(env_idx) = plan.env_plan_idx else {
            return Ok(None);
        };
        let env_plan = module
            .env_plans
            .get(env_idx as usize)
            .ok_or_else(|| err_msg("Missing EnvLoadPlan in app"))?;
        let cfg_obj = exec_env_load_checked(env_plan, &self.host.permissions)
            .map_err(|e| err_msg(&e))?;
        if let Obj::Record(rec) = &cfg_obj
            && rec.schema_id == SCHEMA_ENV_ERROR
        {
            let name = rec.get_obj(0).format_obj();
            let msg = rec.get_obj(1).format_obj();
            return Ok(Some(format!("{}: {}", name, msg)));
        }
        let mut violations = Vec::new();
        self.validate_dyn_val(module, s_top, o_top, &cfg_obj, "", &mut violations, 0)?;
        if !violations.is_empty() {
            return Ok(Some(format_config_violations(&violations)));
        }
        Ok(None)
    }

    fn run_app_hook_list(
        &mut self,
        module: &Module,
        func_ids: &[u32],
        reverse: bool,
        s_top: usize,
        o_top: usize,
        label: &str,
    ) -> Option<String> {
        let mut first_err: Option<String> = None;
        let iter: Box<dyn Iterator<Item = &u32>> = if reverse {
            Box::new(func_ids.iter().rev())
        } else {
            Box::new(func_ids.iter())
        };
        for &fid in iter {
            match self.invoke_zero_arg_cb(module, s_top, o_top, &Obj::Func(fid), 0) {
                Ok(obj) if is_err_or_nil_obj(module, &obj) && !matches!(obj, Obj::Empty) => {
                    let msg = format!("{} hook failed: {}", label, obj.format_obj());
                    if !reverse {
                        return Some(msg);
                    }
                    if first_err.is_none() {
                        first_err = Some(msg);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if !reverse {
                        return Some(e.message);
                    }
                    if first_err.is_none() {
                        first_err = Some(e.message);
                    }
                }
            }
        }
        crate::vm::exec_map::flush_stdout();
        first_err
    }

    fn run_app_startup_hooks(
        &mut self,
        module: &Module,
        plan: &AppPlan,
        rid: Option<usize>,
        s_top: usize,
        o_top: usize,
    ) -> Option<String> {
        if !module.on_start_funcs.is_empty()
            && !self.host.lifecycle_started
            && let Err(msg) = self.run_module_on_start(module, s_top, o_top)
        {
            return Some(msg);
        }
        if let Some(msg) = self.run_app_hook_list(
            module,
            &plan.on_start_funcs,
            false,
            s_top,
            o_top,
            "app on start",
        ) {
            return Some(msg);
        }
        if let Some(r_id) = rid {
            let router_hooks = self.host.routers[r_id].on_start.clone();
            for hook in router_hooks {
                match self.invoke_zero_arg_cb(module, s_top, o_top, &hook, 0) {
                    Ok(obj) if is_err_or_nil_obj(module, &obj) && !matches!(obj, Obj::Empty) => {
                        return Some(format!("router on_start hook failed: {}", obj.format_obj()));
                    }
                    Ok(_) => {}
                    Err(e) => return Some(e.message),
                }
            }
        }
        crate::vm::exec_map::flush_stdout();
        None
    }

    fn finish_app_shutdown(
        &mut self,
        module: &Module,
        rid: Option<usize>,
        shutdown_ms: u64,
        on_stop_funcs: &[u32],
        s_top: usize,
        o_top: usize,
    ) -> Option<String> {
        if self.host.spawn_tracker.active_count() > 0 {
            self.host
                .spawn_tracker
                .wait_idle(Duration::from_millis(shutdown_ms));
        }
        let mut stop_err: Option<String> = None;
        if let Some(r_id) = rid {
            let router_hooks = self.host.routers[r_id].on_stop.clone();
            for hook in router_hooks {
                match self.invoke_zero_arg_cb(module, s_top, o_top, &hook, 0) {
                    Ok(obj) if is_err_or_nil_obj(module, &obj) && !matches!(obj, Obj::Empty) => {
                        if stop_err.is_none() {
                            stop_err = Some(format!(
                                "router on_stop hook failed: {}",
                                obj.format_obj()
                            ));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if stop_err.is_none() {
                            stop_err = Some(e.message);
                        }
                    }
                }
            }
        }
        if let Some(msg) =
            self.run_app_hook_list(module, on_stop_funcs, true, s_top, o_top, "app on stop")
            && stop_err.is_none()
        {
            stop_err = Some(msg);
        }
        if !module.on_stop_funcs.is_empty()
            && !self.host.lifecycle_stopped
            && let Err(msg) = self.run_module_on_stop(module, s_top, o_top)
            && stop_err.is_none()
        {
            stop_err = Some(msg);
        }
        crate::vm::exec_map::flush_stdout();
        stop_err
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_app_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            APP_RUN | APP_START => {
                let plan = module
                    .apps
                    .get(b as usize)
                    .ok_or_else(|| err_msg("Missing AppPlan in module"))?
                    .clone();
                let rid = self.extract_optional_router_id(&self.objs[o_base + c as usize])?;
                if let Some(msg) = self.validate_app_config(module, &plan, s_top, o_top)? {
                    self.objs[o_base + a as usize] = make_app_error("config_error", &msg);
                    return Ok(());
                }
                if let Some(msg) = self.run_app_startup_hooks(module, &plan, rid, s_top, o_top) {
                    self.objs[o_base + a as usize] = make_app_error("startup_error", &msg);
                    return Ok(());
                }

                let shared_mod = self
                    .host
                    .shared_module
                    .get_or_insert_with(|| Arc::new(module.clone()))
                    .clone();
                let perms = Arc::clone(&self.host.permissions);

                if let (Some(_), Some(addr)) = (rid, &plan.http_addr) {
                    self.host
                        .permissions
                        .check_net_addr(addr.as_str())
                        .map_err(|e| err_msg(&e))?;
                }

                let cron_state = if let Some(sched_idx) = plan.schedule_idx {
                    let sched = &module.schedules[sched_idx as usize];
                    match start_cron_scheduler(
                        Arc::clone(&shared_mod),
                        sched,
                        Some(Arc::clone(&self.host.spawn_tracker)),
                        Arc::clone(&perms),
                    ) {
                        Ok(st) => Some(st),
                        Err(msg) => {
                            self.objs[o_base + a as usize] =
                                make_app_error("schedule_error", &msg);
                            return Ok(());
                        }
                    }
                } else {
                    None
                };

                let listener = match (rid, &plan.http_addr) {
                    (Some(_), Some(addr)) => match TcpListener::bind(addr.as_str()) {
                        Ok(l) => Some(l),
                        Err(e) => {
                            if let Some(st) = cron_state {
                                stop_cron_scheduler(st);
                            }
                            self.objs[o_base + a as usize] =
                                make_app_error("bind_error", &e.to_string());
                            return Ok(());
                        }
                    },
                    _ => None,
                };

                if aux == APP_RUN {
                    if let (Some(r_id), Some(l)) = (rid, listener) {
                        let stop_flag = Arc::clone(&self.host.routers[r_id].stop_flag);
                        serve_http_listener(
                            Arc::clone(&shared_mod),
                            self.host.routers.clone(),
                            self.host.http_clients.clone(),
                            self.host.db_pools.clone(),
                            self.host.db_pool_free.clone(),
                            Arc::clone(&self.host.spawn_tracker),
                            self.script_args.clone(),
                            Arc::clone(&perms),
                            r_id,
                            l,
                            stop_flag,
                            true,
                        );
                    } else {
                        SIGNAL_STOP.store(false, Ordering::Release);
                        install_signal_handlers();
                        while !SIGNAL_STOP.load(Ordering::Acquire) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }

                    if let Some(st) = cron_state {
                        stop_cron_scheduler(st);
                    }
                    let stop_err = self.finish_app_shutdown(
                        module,
                        rid,
                        plan.shutdown_ms,
                        &plan.on_stop_funcs,
                        s_top,
                        o_top,
                    );
                    self.objs[o_base + a as usize] = match stop_err {
                        Some(msg) => make_app_error("shutdown_error", &msg),
                        None => Obj::Empty,
                    };
                } else {
                    let http_stop = if let (Some(r_id), Some(l)) = (rid, listener) {
                        let stop_flag = Arc::clone(&self.host.routers[r_id].stop_flag);
                        let stop_bg = Arc::clone(&stop_flag);
                        let routers = self.host.routers.clone();
                        let http_clients = self.host.http_clients.clone();
                        let db_pools = self.host.db_pools.clone();
                        let db_pool_free = self.host.db_pool_free.clone();
                        let spawn_tracker = Arc::clone(&self.host.spawn_tracker);
                        let script_args = self.script_args.clone();
                        let perms_bg = Arc::clone(&perms);
                        let mod_bg = Arc::clone(&shared_mod);
                        let handle = std::thread::spawn(move || {
                            serve_http_listener(
                                mod_bg,
                                routers,
                                http_clients,
                                db_pools,
                                db_pool_free,
                                spawn_tracker,
                                script_args,
                                perms_bg,
                                r_id,
                                l,
                                stop_bg,
                                false,
                            );
                        });
                        Some((r_id, stop_flag, Some(handle)))
                    } else {
                        None
                    };

                    let id = self.host.app_handles.len();
                    self.host.app_handles.push(Some(AppHandleState {
                        http_stop,
                        cron: cron_state,
                        shutdown_ms: plan.shutdown_ms,
                        on_stop_funcs: plan.on_stop_funcs,
                        running: Arc::new(AtomicBool::new(true)),
                    }));
                    self.objs[o_base + a as usize] = make_app_handle(id as i64);
                }
            }
            APP_STOP => {
                let id = self.extract_app_handle_id(&self.objs[o_base + b as usize])?;
                let stop_plan = if let Some(Some(state)) = self.host.app_handles.get_mut(id) {
                    if !state.running.swap(false, Ordering::AcqRel) {
                        None
                    } else {
                        let http_stop = state.http_stop.take();
                        let cron = state.cron.take();
                        let shutdown_ms = state.shutdown_ms;
                        let on_stop_funcs = state.on_stop_funcs.clone();
                        Some((http_stop, cron, shutdown_ms, on_stop_funcs))
                    }
                } else {
                    None
                };

                let stop_err = if let Some((http_stop, cron, shutdown_ms, on_stop_funcs)) = stop_plan {
                    let mut rid_opt = None;
                    if let Some((rid, stop_flag, handle)) = http_stop {
                        rid_opt = Some(rid);
                        stop_flag.store(true, Ordering::Release);
                        self.wake_router_listener(rid);
                        if let Some(h) = handle {
                            let _ = h.join();
                        }
                    }
                    if let Some(st) = cron {
                        stop_cron_scheduler(st);
                    }
                    self.finish_app_shutdown(
                        module,
                        rid_opt,
                        shutdown_ms,
                        &on_stop_funcs,
                        s_top,
                        o_top,
                    )
                } else {
                    None
                };
                self.objs[o_base + a as usize] = match stop_err {
                    Some(msg) => make_app_error("shutdown_error", &msg),
                    None => Obj::Empty,
                };
            }
            APP_IS_RUNNING => {
                let id = self.extract_app_handle_id(&self.objs[o_base + b as usize])?;
                let is_running = self
                    .host
                    .app_handles
                    .get(id)
                    .and_then(|slot| slot.as_ref())
                    .is_some_and(|st| st.running.load(Ordering::Acquire));
                self.scalars[s_base + a as usize] = u64::from(is_running);
            }
            _ => return Err(err_msg("Unknown app sub-op")),
        }
        Ok(())
    }
}
