use std::sync::Arc;

use crate::benchmarks::{Workload, WorkloadState};
use crate::frontend::compile_source;
use crate::ir::Module;
use crate::vm::Vm;
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj, OrderedMap, StructData};

pub fn make_rows_obj(size: usize) -> Obj {
    let label = Arc::new("A record with a nontrivial string payload: București".into());
    let tags = Obj::List(ListStorage::Obj(Arc::new(vec![
        Obj::str("benchmark"),
        Obj::str("collections"),
        Obj::str("closures"),
    ])));
    let mut rows = Vec::with_capacity(size);
    for i in 0..size {
        let mut s = StructData::new(1, 1, 2);
        s.set_scalar(0, i as u64);
        s.set_obj(0, Obj::Str(Arc::clone(&label)));
        s.set_obj(1, tags.clone());
        rows.push(Obj::Record(Arc::new(s)));
    }
    Obj::List(ListStorage::Obj(Arc::new(rows)))
}

pub fn make_int_map_obj(size: usize) -> Obj {
    let mut m = OrderedMap::new();
    for i in 0..size {
        m.set(MapKey::Int(i as i64), MapVal::Scalar(i as u64));
    }
    Obj::Map(Arc::new(m))
}

fn compile_workload(src: &str, entry: &str) -> (Module, u32) {
    let module = compile_source(src).unwrap_or_else(|e| panic!("compile error in {entry}: {e}"));
    let fid = module
        .func_id(entry)
        .unwrap_or_else(|| panic!("missing function {entry}"));
    (module, fid)
}

pub fn call_and_closure_workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "calls.direct",
            category: "Calls",
            size: 1,
            target_us: Some(0.015),
            old_conduit_us: Some(0.200),
            php_us: Some(0.017137),
            go_us: Some(0.002044),
            rust_us: Some(0.000879),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func increment(x: Int) -> Int {\n\
                         return x + 1\n\
                     }\n\
                     func calls_direct(iterations: Int) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + increment(41)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_direct",
                );
                WorkloadState {
                    module,
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
            name: "calls.function_value",
            category: "Calls",
            size: 1,
            target_us: None,
            old_conduit_us: None,
            php_us: Some(0.033320),
            go_us: Some(0.002062),
            rust_us: Some(0.000886),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func increment(x: Int) -> Int {\n\
                         return x + 1\n\
                     }\n\
                     func calls_fn_val(iterations: Int, f: (Int) -> Int) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + f(41)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_fn_val",
                );
                let inc_id = module.func_id("increment").unwrap();
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![Obj::Func(inc_id)],
                    expected_per_op: 42,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "calls.list_argument",
            category: "Calls",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(41.226),
            php_us: Some(0.022437),
            go_us: Some(0.001497),
            rust_us: Some(0.000892),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func list_size(items: [Int]) -> Int {\n\
                         return items.len()\n\
                     }\n\
                     func calls_list_arg(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + list_size(items)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_list_arg",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "calls.record_argument",
            category: "Calls",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(39.849),
            php_us: Some(0.026342),
            go_us: Some(0.001490),
            rust_us: Some(0.000891),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "struct Payload {\n\
                         items: [Int]\n\
                         label: String\n\
                     }\n\
                     func payload_size(p: Payload) -> Int {\n\
                         return p.items.len()\n\
                     }\n\
                     func calls_rec_arg(iterations: Int, p: Payload) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + payload_size(p)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_rec_arg",
                );
                let mut payload = StructData::new(0, 0, 2);
                payload.set_obj(0, Obj::List(ListStorage::Int(Arc::new((0..4096).collect()))));
                payload.set_obj(1, Obj::str("payload"));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![Obj::Record(Arc::new(payload))],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "calls.string_argument",
            category: "Calls",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(1.437),
            php_us: Some(0.022147),
            go_us: Some(0.001490),
            rust_us: Some(0.000888),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func string_size(s: String) -> Int {\n\
                         return s.len()\n\
                     }\n\
                     func calls_str_arg(iterations: Int, s: String) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + string_size(s)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_str_arg",
                );
                let text = Obj::str("x".repeat(4096));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![text],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "calls.map_argument",
            category: "Calls",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(70.693),
            php_us: Some(0.022182),
            go_us: Some(0.001490),
            rust_us: Some(0.000892),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func map_size(m: [Int: Int]) -> Int {\n\
                         return m.len()\n\
                     }\n\
                     func calls_map_arg(iterations: Int, m: [Int: Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             acc = acc + map_size(m)\n\
                         }\n\
                         return acc\n\
                     }",
                    "calls_map_arg",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![make_int_map_obj(4096)],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "closures.invoke_empty",
            category: "Closures",
            size: 1,
            target_us: None,
            old_conduit_us: Some(0.771633),
            php_us: Some(0.019560),
            go_us: Some(0.002089),
            rust_us: Some(0.000891),
            setup: Box::new(|vm| make_invoke_closure_workload(vm, false)),
        },
        Workload {
            name: "closures.invoke_scalar",
            category: "Closures",
            size: 1,
            target_us: None,
            old_conduit_us: Some(0.219),
            php_us: Some(0.021320),
            go_us: Some(0.002084),
            rust_us: Some(0.002017),
            setup: Box::new(|vm| make_invoke_closure_workload(vm, true)),
        },
        Workload {
            name: "closures.invoke_list",
            category: "Closures",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(21.087),
            php_us: Some(0.024812),
            go_us: Some(1.819),
            rust_us: Some(0.372201),
            setup: Box::new(make_invoke_list_closure_workload),
        },
        Workload {
            name: "closures.create_empty",
            category: "Closures",
            size: 1,
            target_us: None,
            old_conduit_us: Some(1.428),
            php_us: Some(0.046690),
            go_us: Some(0.002678),
            rust_us: Some(0.001240),
            setup: Box::new(|_| make_create_closure_workload(false, false, false)),
        },
        Workload {
            name: "closures.create_scalar",
            category: "Closures",
            size: 1,
            target_us: None,
            old_conduit_us: Some(0.754),
            php_us: Some(0.063780),
            go_us: Some(0.002680),
            rust_us: Some(0.019592),
            setup: Box::new(|_| make_create_closure_workload(true, false, false)),
        },
        Workload {
            name: "closures.create_unused_list",
            category: "Closures",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(22.010),
            php_us: Some(0.063563),
            go_us: Some(0.002673),
            rust_us: Some(0.019631),
            setup: Box::new(|_| make_create_closure_workload(true, true, false)),
        },
        Workload {
            name: "closures.create_unused_records",
            category: "Closures",
            size: 4096,
            target_us: Some(0.050),
            old_conduit_us: Some(639.3),
            php_us: Some(0.063267),
            go_us: Some(0.002678),
            rust_us: Some(0.017493),
            setup: Box::new(|_| make_create_closure_workload(true, false, true)),
        },
    ]
}

fn make_invoke_closure_workload(vm: &mut Vm, capture_scalar: bool) -> WorkloadState {
    let src = if capture_scalar {
        "func setup_closure(base: Int) -> (Int) -> Int {\n\
             return func(x: Int) -> Int { return base + x }\n\
         }\n\
         func closures_invoke(iterations: Int, cb: (Int) -> Int) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 acc = acc + cb(1)\n\
             }\n\
             return acc\n\
         }"
    } else {
        "func setup_closure() -> (Int) -> Int {\n\
             return func(x: Int) -> Int { return x + 1 }\n\
         }\n\
         func closures_invoke(iterations: Int, cb: (Int) -> Int) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 acc = acc + cb(41)\n\
             }\n\
             return acc\n\
         }"
    };
    let (module, bid) = compile_workload(src, "closures_invoke");
    let setup_fid = module.func_id("setup_closure").unwrap();
    let closure_obj = vm
        .call_obj(&module, setup_fid, if capture_scalar { &[41] } else { &[] }, &[])
        .unwrap();

    WorkloadState {
        module,
        bench_func: bid,
        verify_func: bid,
        scalar_args: vec![],
        obj_args: vec![closure_obj],
        expected_per_op: 42,
        after_check: None,
    }
}

fn make_invoke_list_closure_workload(vm: &mut Vm) -> WorkloadState {
    let (module, bid) = compile_workload(
        "func setup_list_closure(items: [Int]) -> () -> [Int] {\n\
             return func() -> [Int] { return items }\n\
         }\n\
         func closures_invoke_list(iterations: Int, cb: () -> [Int]) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 out := cb()\n\
                 acc = acc + out.len()\n\
             }\n\
             return acc\n\
         }",
        "closures_invoke_list",
    );
    let setup_fid = module.func_id("setup_list_closure").unwrap();
    let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
    let closure_obj = vm.call_obj(&module, setup_fid, &[], &[items]).unwrap();

    WorkloadState {
        module,
        bench_func: bid,
        verify_func: bid,
        scalar_args: vec![],
        obj_args: vec![closure_obj],
        expected_per_op: 4096,
        after_check: None,
    }
}

fn make_create_closure_workload(
    capture_scalar: bool,
    with_unused_list: bool,
    with_unused_records: bool,
) -> WorkloadState {
    let src = match (capture_scalar, with_unused_list, with_unused_records) {
        (false, _, _) => {
            "func closures_create(iterations: Int) -> Int {\n\
                 var acc = 0\n\
                 for _ in 0..iterations {\n\
                     cb := func(x: Int) -> Int { return x + 1 }\n\
                     acc = acc + cb(41)\n\
                 }\n\
                 return acc\n\
             }"
        }
        (true, false, false) => {
            "func closures_create(iterations: Int, base: Int) -> Int {\n\
                 var acc = 0\n\
                 for _ in 0..iterations {\n\
                     cb := func(x: Int) -> Int { return base + x }\n\
                     acc = acc + cb(1)\n\
                 }\n\
                 return acc\n\
             }"
        }
        (true, true, false) => {
            "func closures_create(iterations: Int, base: Int, unused_items: [Int]) -> Int {\n\
                 var acc = 0\n\
                 for _ in 0..iterations {\n\
                     cb := func(x: Int) -> Int { return base + x }\n\
                     acc = acc + cb(1)\n\
                 }\n\
                 return acc\n\
             }"
        }
        (true, _, true) => {
            "struct Row {\n\
                 id: Int\n\
                 name: String\n\
                 tags: [String]\n\
             }\n\
             func closures_create(iterations: Int, base: Int, unused_rows: [Row]) -> Int {\n\
                 var acc = 0\n\
                 for _ in 0..iterations {\n\
                     cb := func(x: Int) -> Int { return base + x }\n\
                     acc = acc + cb(1)\n\
                 }\n\
                 return acc\n\
             }"
        }
    };
    let (module, bid) = compile_workload(src, "closures_create");

    let scalar_args = if capture_scalar { vec![41] } else { vec![] };
    let obj_args = if with_unused_list {
        vec![Obj::List(ListStorage::Int(Arc::new((0..4096).collect())))]
    } else if with_unused_records {
        vec![make_rows_obj(4096)]
    } else {
        vec![]
    };

    let after_check: Option<super::AfterCheckFn> = if with_unused_list {
        Some(Box::new(|objs| {
            let expected = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
            if objs[0] != expected {
                return Err("unused list was mutated".into());
            }
            Ok(())
        }))
    } else if with_unused_records {
        Some(Box::new(|objs| {
            if objs[0] != make_rows_obj(4096) {
                return Err("unused records were mutated".into());
            }
            Ok(())
        }))
    } else {
        None
    };

    WorkloadState {
        module,
        bench_func: bid,
        verify_func: bid,
        scalar_args,
        obj_args,
        expected_per_op: 42,
        after_check,
    }
}
