use crate::benchmarks::{Workload, WorkloadState};
use crate::frontend::compile_source;

fn compile_compute(src: &str, fn_name: &str) -> (crate::ir::Module, u32) {
    let m = compile_source(src).unwrap_or_else(|e| panic!("compile error in {}: {}", fn_name, e));
    let bid = m
        .func_id(fn_name)
        .unwrap_or_else(|| panic!("missing function {}", fn_name));
    (m, bid)
}

pub fn compute_workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "control.while",
            category: "Control",
            size: 1,
            target_us: None,
            old_conduit_us: Some(0.097),
            php_us: Some(0.004287),
            go_us: Some(0.001444),
            rust_us: Some(0.000360),
            setup: Box::new(|_| {
                let (m, bid) = compile_compute(
                    r#"
                    func control_while(iterations: Int) -> Int {
                        var total = 0
                        for _ in 0..iterations {
                            x := 41
                            total = total + (x + 1)
                        }
                        return total
                    }
                    "#,
                    "control_while",
                );
                WorkloadState {
                    module: m,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: 42,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "compute.for_range",
            category: "Compute",
            size: 4096,
            target_us: Some(10.0),
            old_conduit_us: Some(150.0),
            php_us: Some(22.880),
            go_us: Some(3.679),
            rust_us: Some(1.260),
            setup: Box::new(|_| {
                let (m, bid) = compile_compute(
                    r#"
                    func compute_for_range(iterations: Int) -> Int {
                        var total = 0
                        for _ in 0..iterations {
                            var score = 0
                            for item in 0..4096 {
                                score = score + item
                            }
                            total = total + score
                        }
                        return total
                    }
                    "#,
                    "compute_for_range",
                );
                WorkloadState {
                    module: m,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: 4096 * 4095 / 2,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "compute.float_poly",
            category: "Compute",
            size: 4096,
            target_us: Some(15.0),
            old_conduit_us: Some(190.0),
            php_us: Some(28.0),
            go_us: Some(4.5),
            rust_us: Some(2.0),
            setup: Box::new(|_| {
                let (m, bid) = compile_compute(
                    r#"
                    func compute_float_poly(iterations: Int) -> Int {
                        var total = 0
                        cm := 1.5
                        ca := 2.25
                        for _ in 0..iterations {
                            var acc = 0.0
                            for i in 0..4096 {
                                acc = acc + (i.to_float() * cm + ca)
                            }
                            if acc == 12589056.0 {
                                total = total + 12589056
                            }
                        }
                        return total
                    }
                    "#,
                    "compute_float_poly",
                );
                WorkloadState {
                    module: m,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: 12_589_056,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "compute.arith_mix",
            category: "Compute",
            size: 4096,
            target_us: None,
            old_conduit_us: None,
            php_us: None,
            go_us: None,
            rust_us: None,
            setup: Box::new(|_| {
                let (m, bid) = compile_compute(
                    r#"
                    func compute_arith_mix(iterations: Int) -> Int {
                        var total = 0
                        for _ in 0..iterations {
                            var score = 0
                            for i in 0..4096 {
                                score = score + (((i * 3 + 7) / 2) % 97)
                            }
                            total = total + score
                        }
                        return total
                    }
                    "#,
                    "compute_arith_mix",
                );
                let expected: i64 = (0..4096).map(|i| ((i * 3 + 7) / 2) % 97).sum();
                WorkloadState {
                    module: m,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: expected,
                    after_check: None,
                }
            }),
        },
    ]
}
