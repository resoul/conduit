pub mod builder;
pub mod workloads_calls;
pub mod workloads_collections;
pub mod workloads_compute;
pub mod workloads_json;
pub mod workloads_pipelines;

use std::time::Instant;

use crate::ir::Module;
use crate::vm::Vm;
use crate::vm::value::Obj;

pub type AfterCheckFn = Box<dyn Fn(&[Obj]) -> Result<(), String>>;

pub struct WorkloadState {
    pub module: Module,
    pub bench_func: u32,
    pub verify_func: u32,
    pub scalar_args: Vec<u64>,
    pub obj_args: Vec<Obj>,
    pub expected_per_op: i64,
    pub after_check: Option<AfterCheckFn>,
}

pub struct Workload {
    pub name: &'static str,
    pub category: &'static str,
    pub size: usize,
    pub target_us: Option<f64>,
    pub old_conduit_us: Option<f64>,
    pub php_us: Option<f64>,
    pub go_us: Option<f64>,
    pub rust_us: Option<f64>,
    pub setup: Box<dyn Fn(&mut Vm) -> WorkloadState>,
}

pub struct BenchResult {
    pub name: &'static str,
    pub size: usize,
    pub median_us: f64,
    pub mad_us: f64,
    pub target_us: Option<f64>,
    pub old_conduit_us: Option<f64>,
    pub php_us: Option<f64>,
    pub go_us: Option<f64>,
    pub rust_us: Option<f64>,
    pub gate_passed: Option<bool>,
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
    }
}

fn median_and_mad(samples: &[f64]) -> (f64, f64) {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let med = median(&s);
    let mut devs: Vec<f64> = s.iter().map(|&x| (x - med).abs()).collect();
    devs.sort_by(|a, b| a.total_cmp(b));
    (med, median(&devs))
}

pub fn all_workloads() -> Vec<Workload> {
    let mut all = workloads_compute::compute_workloads();
    all.extend(workloads_calls::call_and_closure_workloads());
    all.extend(workloads_collections::collection_workloads());
    all.extend(workloads_pipelines::pipeline_workloads());
    all.extend(workloads_json::json_workloads());
    all
}

pub fn run_workload(w: &Workload) -> Result<BenchResult, String> {
    let mut vm = Vm::new();
    let state = (w.setup)(&mut vm);
    state.module.validate()?;

    // 1. Semantic verification pass (outside timer).
    let mut verify_scalars = Vec::with_capacity(1 + state.scalar_args.len());
    verify_scalars.push(1u64);
    verify_scalars.extend_from_slice(&state.scalar_args);
    let verify_checksum = vm
        .call_scalar(&state.module, state.verify_func, &verify_scalars, &state.obj_args)
        .map_err(|e| format!("{}: verify failed: {}", w.name, e))? as i64;
    if verify_checksum != state.expected_per_op {
        return Err(format!(
            "{}: verify checksum mismatch: expected {}, got {}",
            w.name, state.expected_per_op, verify_checksum
        ));
    }

    // 2. Calibrate iterations so each batch takes >= 40 ms.
    let mut iters: u64 = 1;
    loop {
        let mut s_args = Vec::with_capacity(1 + state.scalar_args.len());
        s_args.push(iters);
        s_args.extend_from_slice(&state.scalar_args);
        let t0 = Instant::now();
        let sum = vm
            .call_scalar(&state.module, state.bench_func, &s_args, &state.obj_args)
            .map_err(|e| format!("{}: calibrate failed: {}", w.name, e))? as i64;
        let dt = t0.elapsed();
        let expected_sum = (iters as i64).wrapping_mul(state.expected_per_op);
        if sum != expected_sum {
            return Err(format!(
                "{}: calibrate checksum mismatch: expected {}, got {}",
                w.name, expected_sum, sum
            ));
        }
        if dt.as_millis() >= 40 || iters >= 4_000_000 {
            if dt.as_millis() < 20 && iters < 4_000_000 {
                iters *= 4;
            }
            break;
        }
        iters = (iters * 4).min(4_000_000);
    }

    let mut run_scalars = Vec::with_capacity(1 + state.scalar_args.len());
    run_scalars.push(iters);
    run_scalars.extend_from_slice(&state.scalar_args);

    // 3. Warmup (2 batches).
    for _ in 0..2 {
        let _ = vm
            .call_scalar(&state.module, state.bench_func, &run_scalars, &state.obj_args)
            .map_err(|e| format!("{}: warmup failed: {}", w.name, e))?;
    }

    // 4. Collect 7 timed samples.
    let mut samples = Vec::with_capacity(7);
    let expected_sum = (iters as i64).wrapping_mul(state.expected_per_op);
    for _ in 0..7 {
        let t0 = Instant::now();
        let sum = vm
            .call_scalar(&state.module, state.bench_func, &run_scalars, &state.obj_args)
            .map_err(|e| format!("{}: run failed: {}", w.name, e))? as i64;
        let ns = t0.elapsed().as_nanos() as f64;
        if sum != expected_sum {
            return Err(format!(
                "{}: checksum mismatch: expected {}, got {}",
                w.name, expected_sum, sum
            ));
        }
        samples.push((ns / 1000.0) / (iters as f64));
    }

    // 5. Post-run immutability / invariant check.
    if let Some(check) = &state.after_check {
        check(&state.obj_args)?;
    }

    let (median_us, mad_us) = median_and_mad(&samples);
    let gate_passed = w.target_us.map(|target| median_us <= target);

    Ok(BenchResult {
        name: w.name,
        size: w.size,
        median_us,
        mad_us,
        target_us: w.target_us,
        old_conduit_us: w.old_conduit_us,
        php_us: w.php_us,
        go_us: w.go_us,
        rust_us: w.rust_us,
        gate_passed,
    })
}

fn fmt_opt_us(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{:.3}", x),
        None => "-".to_string(),
    }
}

fn fmt_speedup(base: Option<f64>, current: f64) -> String {
    match base {
        Some(b) if current > 0.0 => format!("{:.1}x", b / current),
        _ => "-".to_string(),
    }
}

pub fn run_all(filter: Option<&str>) -> Result<bool, String> {
    let workloads = all_workloads();
    println!(
        "{:<30} {:>5} {:>10} {:>8} {:>9} {:>6} {:>9} {:>9} {:>9}",
        "Workload", "Size", "Med(us)", "MAD(us)", "Target", "Gate", "vs Old", "vs PHP", "vs Go"
    );
    println!("{}", "-".repeat(106));

    let mut all_gates_passed = true;
    for w in &workloads {
        if let Some(pat) = filter
            && !w.name.contains(pat)
        {
            continue;
        }
        let r = run_workload(w)?;
        let gate_str = match r.gate_passed {
            Some(true) => "PASS",
            Some(false) => {
                all_gates_passed = false;
                "FAIL"
            }
            None => "-",
        };
        println!(
            "{:<30} {:>5} {:>10.4} {:>8.4} {:>9} {:>6} {:>9} {:>9} {:>9}",
            r.name,
            r.size,
            r.median_us,
            r.mad_us,
            fmt_opt_us(r.target_us),
            gate_str,
            fmt_speedup(r.old_conduit_us, r.median_us),
            fmt_speedup(r.php_us, r.median_us),
            fmt_speedup(r.go_us, r.median_us),
        );
    }
    Ok(all_gates_passed)
}
