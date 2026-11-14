use std::sync::Arc;

use crate::benchmarks::workloads_calls::make_rows_obj;
use crate::benchmarks::{Workload, WorkloadState};
use crate::frontend::compile_source;
use crate::ir::Module;
use crate::vm::value::{ListStorage, Obj};

fn compile_pipeline(src: &str, bench_entry: &str, verify_entry: &str) -> (Module, u32, u32) {
    let module =
        compile_source(src).unwrap_or_else(|e| panic!("compile error in {bench_entry}: {e}"));
    let bid = module
        .func_id(bench_entry)
        .unwrap_or_else(|| panic!("missing bench function {bench_entry}"));
    let vid = module
        .func_id(verify_entry)
        .unwrap_or_else(|| panic!("missing verify function {verify_entry}"));
    (module, bid, vid)
}

pub fn pipeline_workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "collections.map_ints",
            category: "Pipelines",
            size: 4096,
            target_us: Some(25.0),
            old_conduit_us: Some(217.886),
            php_us: Some(79.037),
            go_us: Some(7.280),
            rust_us: Some(3.666),
            setup: Box::new(|_| {
                let (module, bid, vid) = compile_pipeline(
                    "func increment(x: Int) -> Int {\n\
                         return x + 1\n\
                     }\n\
                     func map_ints(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> map(increment)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func map_ints_verify(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> map(increment)\n\
                             var expected = 1\n\
                             for x in out {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 1\n\
                             }\n\
                             if expected != 4097 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "map_ints",
                    "map_ints_verify",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.filter_ints",
            category: "Pipelines",
            size: 4096,
            target_us: Some(25.0),
            old_conduit_us: Some(295.886),
            php_us: Some(102.6),
            go_us: Some(11.745),
            rust_us: Some(6.195),
            setup: Box::new(|_| {
                let (module, bid, vid) = compile_pipeline(
                    "func even(x: Int) -> Bool {\n\
                         return x % 2 == 0\n\
                     }\n\
                     func filter_ints(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> filter(even)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func filter_ints_verify(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> filter(even)\n\
                             var expected = 0\n\
                             for x in out {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 2\n\
                             }\n\
                             if expected != 4096 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "filter_ints",
                    "filter_ints_verify",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 2048,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.map_records",
            category: "Pipelines",
            size: 4096,
            target_us: Some(140.0),
            old_conduit_us: Some(636.196),
            php_us: Some(241.8),
            go_us: Some(115.7),
            rust_us: Some(456.6),
            setup: Box::new(|_| {
                let (module, bid, vid) = compile_pipeline(
                    "struct ReservedSchema0 { x: Int }\n\
                     struct Row {\n\
                         id: Int\n\
                         name: String\n\
                         tags: [String]\n\
                     }\n\
                     func bump_row(r: Row) -> Row {\n\
                         return Row(..r, id: r.id + 1)\n\
                     }\n\
                     func map_records(iterations: Int, rows: [Row]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := rows |> map(bump_row)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func map_records_verify(iterations: Int, rows: [Row]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := rows |> map(bump_row)\n\
                             if out.len() != 4096 {\n\
                                 return -1\n\
                             }\n\
                             var expected_id = 1\n\
                             for r in out {\n\
                                 if r.id != expected_id {\n\
                                     return -1\n\
                                 }\n\
                                 expected_id = expected_id + 1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "map_records",
                    "map_records_verify",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![make_rows_obj(4096)],
                    expected_per_op: 4096,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.filter_records",
            category: "Pipelines",
            size: 4096,
            target_us: None,
            old_conduit_us: Some(3859.5),
            php_us: Some(128.7),
            go_us: Some(89.009),
            rust_us: Some(234.7),
            setup: Box::new(|_| {
                let (module, bid, vid) = compile_pipeline(
                    "struct ReservedSchema0 { x: Int }\n\
                     struct Row {\n\
                         id: Int\n\
                         name: String\n\
                         tags: [String]\n\
                     }\n\
                     func even_row(r: Row) -> Bool {\n\
                         return r.id % 2 == 0\n\
                     }\n\
                     func filter_records(iterations: Int, rows: [Row]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := rows |> filter(even_row)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func filter_records_verify(iterations: Int, rows: [Row]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := rows |> filter(even_row)\n\
                             if out.len() != 2048 {\n\
                                 return -1\n\
                             }\n\
                             var expected_id = 0\n\
                             for r in out {\n\
                                 if r.id != expected_id {\n\
                                     return -1\n\
                                 }\n\
                                 expected_id = expected_id + 2\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "filter_records",
                    "filter_records_verify",
                );
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![make_rows_obj(4096)],
                    expected_per_op: 2048,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.pipeline",
            category: "Pipelines",
            size: 4096,
            target_us: Some(35.0),
            old_conduit_us: Some(499.361),
            php_us: Some(181.8),
            go_us: Some(19.480),
            rust_us: Some(9.871),
            setup: Box::new(|_| {
                let (module, bid, vid) = compile_pipeline(
                    "func pipeline(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> map(x => x + 1) |> filter(x => x % 2 == 0)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func pipeline_verify(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> map(x => x + 1) |> filter(x => x % 2 == 0)\n\
                             var expected = 2\n\
                             for x in out {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 2\n\
                             }\n\
                             if expected != 4098 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "pipeline",
                    "pipeline_verify",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: 2048,
                    after_check: None,
                }
            }),
        },
        Workload {
            name: "collections.pipeline_3stage",
            category: "Pipelines",
            size: 4096,
            target_us: None,
            old_conduit_us: None,
            php_us: None,
            go_us: None,
            rust_us: None,
            setup: Box::new(|_| {
                let expected_vec: Vec<i64> = (0..4096)
                    .filter(|x| x % 2 == 0)
                    .map(|x| x + 1)
                    .filter(|x| *x > 1000)
                    .collect();
                let exp_len = expected_vec.len() as i64;
                let (module, bid, vid) = compile_pipeline(
                    "func even(x: Int) -> Bool {\n\
                         return x % 2 == 0\n\
                     }\n\
                     func increment(x: Int) -> Int {\n\
                         return x + 1\n\
                     }\n\
                     func gt_1000(x: Int) -> Bool {\n\
                         return x > 1000\n\
                     }\n\
                     func pipeline_3stage(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> filter(even) |> map(increment) |> filter(gt_1000)\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }\n\
                     func pipeline_3stage_verify(iterations: Int, items: [Int]) -> Int {\n\
                         var acc = 0\n\
                         for _ in 0..iterations {\n\
                             out := items |> filter(even) |> map(increment) |> filter(gt_1000)\n\
                             var expected = 1001\n\
                             for x in out {\n\
                                 if x != expected {\n\
                                     return -1\n\
                                 }\n\
                                 expected = expected + 2\n\
                             }\n\
                             if expected != 4097 {\n\
                                 return -1\n\
                             }\n\
                             acc = acc + out.len()\n\
                         }\n\
                         return acc\n\
                     }",
                    "pipeline_3stage",
                    "pipeline_3stage_verify",
                );
                let items = Obj::List(ListStorage::Int(Arc::new((0..4096).collect())));
                WorkloadState {
                    module,
                    bench_func: bid,
                    verify_func: vid,
                    scalar_args: vec![],
                    obj_args: vec![items],
                    expected_per_op: exp_len,
                    after_check: None,
                }
            }),
        },
    ]
}
