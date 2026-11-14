use std::sync::Arc;

use crate::benchmarks::workloads_calls::{make_int_map_obj, make_rows_obj};
use crate::benchmarks::{Workload, WorkloadState};
use crate::frontend::compile_source;
use crate::ir::Module;
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj};

fn compile_workload(src: &str, entry: &str) -> (Module, u32) {
    let module = compile_source(src).unwrap_or_else(|e| panic!("compile error in {entry}: {e}"));
    let fid = module
        .func_id(entry)
        .unwrap_or_else(|| panic!("missing function {entry}"));
    (module, fid)
}

pub fn collection_workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "collections.copy_list",
            category: "Collections",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(20.807),
            php_us: Some(0.010459),
            go_us: Some(1.885),
            rust_us: Some(0.398602),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func copy_list(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             copy := items\n\
                             acc = acc + copy.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "copy_list",
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
            name: "collections.copy_records",
            category: "Collections",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(641.5),
            php_us: Some(0.010445),
            go_us: Some(104.2),
            rust_us: Some(456.8),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "struct Row {\n\
                         id: Int\n\
                         name: String\n\
                         tags: [String]\n\
                     }\n\
                     func copy_records(iterations: Int, rows: [Row]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             copy := rows\n\
                             acc = acc + copy.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "copy_records",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![make_rows_obj(4096)],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.for_list",
            category: "Collections",
            size: 4096,
            target_us: Some(10.0),
            old_conduit_us: Some(152.169),
            php_us: Some(18.450),
            go_us: Some(4.324),
            rust_us: Some(1.277),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func for_list(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var sum = 0\n\
                             for elem in items {\n\
                                 sum = sum + elem\n\
                             }\n\
                             acc = acc + sum\n\
                         }\n\
                         return acc\n\
                     }",
                    "for_list",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 4096 * 4095 / 2,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.append_unique",
            category: "Collections",
            size: 4096,
            target_us: Some(15.0),
            old_conduit_us: Some(305.228),
            php_us: Some(23.886),
            go_us: Some(10.956),
            rust_us: Some(2.384),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func append_unique(iterations: Int) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out: [Int] = []\n\
                             for j in 0..4096 {\n\
                                 out.append(j)\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "append_unique",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.append_shared",
            category: "Collections",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(21.158),
            php_us: Some(4.961),
            go_us: Some(4.429),
            rust_us: Some(0.924811),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func append_shared(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out = items\n\
                             out.append(4096)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func append_shared_verify(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out = items\n\
                             out.append(4096)\n\
                             var expected = 0\n\
                             for x in out {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 1\n\
                             }\n\
                             if expected != 4097 {\n\
                                 return -1\n\
                             }\n\
                             expected = 0\n\
                             for x in items {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 1\n\
                             }\n\
                             if expected != 4096 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "append_shared",
                );
                let vid = module.func_id("append_shared_verify").unwrap();
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 4097,
                    after_check: Some(Box::new(|objs| {
                        let expected = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                        if objs[0] != expected {
                            return Err("append_shared mutated original list".into());
                        }
                        Ok(())
                    })),
                }
            }),
        },
        Workload {
            name: "collections.set_unique",
            category: "Collections",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(681.982),
            php_us: Some(28.763),
            go_us: Some(178.9),
            rust_us: Some(175.4),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func set_unique(iterations: Int) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out = [Int: Int]()\n\
                             for j in 0..4096 {\n\
                                 out.set(j, j)\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "set_unique",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: bid,
                    scalar_args: vec![],
                    obj_args: vec![],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.set_shared",
            category: "Collections",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(35.992),
            php_us: Some(4.934),
            go_us: Some(71.869),
            rust_us: Some(3.251),
            setup: Box::new(|_| {
                let (module, bid) = compile_workload(
                    "func set_shared(iterations: Int, m: [Int: Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out = m\n\
                             out.set(0, 99)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func set_shared_verify(iterations: Int, m: [Int: Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             var out = m\n\
                             out.set(0, 99)\n\
                             got := out.get(0) ?? -1\n\
                             got_orig := m.get(0) ?? -1\n\
                             if got != 99 || got_orig != 0 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "set_shared",
                );
                let vid = module.func_id("set_shared_verify").unwrap();
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![make_int_map_obj(4096)],
                    expected_per_op: 4096,
                    after_check: Some(Box::new(|objs| {
                        match &objs[0] {
                            Obj::Map(arc) => {
                                if arc.get(&MapKey::Int(0)) != Some(&MapVal::Scalar(0)) {
                                    return Err("set_shared mutated original map".into());
                                }
                            }
                            _ => return Err("expected Map".into()),
                        }
                        Ok(())
                    })),
                }
            }),
        },
    ]
}
