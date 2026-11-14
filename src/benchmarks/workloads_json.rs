use std::sync::Arc;

use crate::benchmarks::{Workload, WorkloadState};
use crate::frontend::compile_source;
use crate::ir::Module;
use crate::native::json_decode::decode_json_with_plan;
use crate::native::json_encode::encode_json_obj;
use crate::native::{JsonDecodePlan, JsonType, StructFieldDef, StructSchema};
use crate::vm::Vm;
use crate::vm::value::{ListStorage, Obj};

const SCHEMA_ADDRESS: u32 = 0;
const SCHEMA_RECORD: u32 = 1;

const JSON_TYPES_CDT: &str = "\
import { decode, encode } from \"cdt.json\"
struct Address {
    city: String
    zip: String
}
struct Record {
    id: Int
    name: String @json(\"display_name\")
    email: String
    active: Bool
    score: Float
    tags: [String]
    address: Address
    note: String?
}
";

pub fn json_schemas() -> Vec<StructSchema> {
    let address = StructSchema::new(
        SCHEMA_ADDRESS,
        "Address",
        vec![
            StructFieldDef {
                name: "city".into(),
                json_name: "city".into(),
                ty: JsonType::String,
            },
            StructFieldDef {
                name: "zip".into(),
                json_name: "zip".into(),
                ty: JsonType::String,
            },
        ],
    );
    let record = StructSchema::new(
        SCHEMA_RECORD,
        "Record",
        vec![
            StructFieldDef {
                name: "id".into(),
                json_name: "id".into(),
                ty: JsonType::Int,
            },
            StructFieldDef {
                name: "name".into(),
                json_name: "display_name".into(),
                ty: JsonType::String,
            },
            StructFieldDef {
                name: "email".into(),
                json_name: "email".into(),
                ty: JsonType::String,
            },
            StructFieldDef {
                name: "active".into(),
                json_name: "active".into(),
                ty: JsonType::Bool,
            },
            StructFieldDef {
                name: "score".into(),
                json_name: "score".into(),
                ty: JsonType::Float,
            },
            StructFieldDef {
                name: "tags".into(),
                json_name: "tags".into(),
                ty: JsonType::List(Box::new(JsonType::String)),
            },
            StructFieldDef {
                name: "address".into(),
                json_name: "address".into(),
                ty: JsonType::Struct(SCHEMA_ADDRESS),
            },
            StructFieldDef {
                name: "note".into(),
                json_name: "note".into(),
                ty: JsonType::Optional(Box::new(JsonType::String)),
            },
        ],
    );
    vec![address, record]
}

pub fn build_fixture_json(count: usize) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(count * 212 + 2);
    out.push('[');
    for i in 0..count {
        if i > 0 {
            out.push(',');
        }
        let id = i + 1;
        let active = i % 2 == 0;
        let group = i % 5;
        let zip = 1000 + i;
        let _ = write!(
            out,
            "{{\"id\":{},\"display_name\":\"User {:03}\",\"email\":\"user{:03}@example.com\",\"active\":{},\"score\":{}.5,\"tags\":[\"benchmark\",\"json\",\"group_{}\"],\"address\":{{\"city\":\"București\",\"zip\":\"{:06}\"}},\"note\":",
            id, id, id, active, i, group, zip
        );
        if i % 3 == 0 {
            out.push_str("null}");
        } else {
            let _ = write!(out, "\"Note {}: \\\"hello\\\"\\nnext line\"}}", i);
        }
    }
    out.push(']');
    out
}

fn check_fixture(count: usize, expected_bytes: usize) -> (String, Obj) {
    let schemas = json_schemas();
    let list_ty = JsonType::List(Box::new(JsonType::Struct(SCHEMA_RECORD)));
    let plan = JsonDecodePlan::compile(&list_ty, &schemas);

    let raw = build_fixture_json(count);
    assert_eq!(
        raw.len(),
        expected_bytes,
        "fixture byte length mismatch for count {}",
        count
    );
    let decoded = decode_json_with_plan(&raw, &plan).expect("fixture decode must succeed");
    let roundtrip = encode_json_obj(&decoded, &schemas).expect("fixture encode must succeed");
    assert_eq!(roundtrip, raw, "fixture roundtrip mismatch for count {}", count);
    (raw, decoded)
}

fn compile_json_module(extra_src: &str, entry: &str) -> (Module, u32) {
    let src = format!("{JSON_TYPES_CDT}{extra_src}");
    let module = compile_source(&src).unwrap_or_else(|e| panic!("compile error in {entry}: {e}"));
    let fid = module
        .func_id(entry)
        .unwrap_or_else(|| panic!("missing function {entry}"));
    (module, fid)
}

fn setup_decode(count: usize, expected_bytes: usize) -> WorkloadState {
    let (raw, _) = check_fixture(count, expected_bytes);
    let (module, func_id) = compile_json_module(
        "func json_decode_bench(iterations: Int, raw: String) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 decoded := decode<[Record]>(raw)?\n\
                 acc = acc + decoded.len()\n\
             }\n\
             return acc\n\
         }",
        "json_decode_bench",
    );
    WorkloadState {
        module,
        bench_func: func_id,
        verify_func: func_id,
        scalar_args: vec![],
        obj_args: vec![Obj::Str(Arc::new(raw))],
        expected_per_op: count as i64,
        after_check: Some(Box::new(move |objs| match &objs[0] {
            Obj::Str(s) if s.len() == expected_bytes => Ok(()),
            _ => Err("JSON input string was mutated or corrupted".into()),
        })),
    }
}

fn setup_encode(count: usize, expected_bytes: usize) -> WorkloadState {
    let (_, decoded) = check_fixture(count, expected_bytes);
    let (module, func_id) = compile_json_module(
        "func json_encode_bench(iterations: Int, records: [Record]) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 encoded := encode(records)?\n\
                 acc = acc + encoded.len()\n\
             }\n\
             return acc\n\
         }",
        "json_encode_bench",
    );
    WorkloadState {
        module,
        bench_func: func_id,
        verify_func: func_id,
        scalar_args: vec![],
        obj_args: vec![decoded],
        expected_per_op: expected_bytes as i64,
        after_check: Some(Box::new(move |objs| match &objs[0] {
            Obj::List(ListStorage::Obj(v)) if v.len() == count => Ok(()),
            _ => Err("Decoded records list was mutated".into()),
        })),
    }
}

fn setup_json_etl(count: usize, expected_raw_bytes: usize) -> WorkloadState {
    let (raw, decoded) = check_fixture(count, expected_raw_bytes);
    let (module, func_id) = compile_json_module(
        "func is_active(r: Record) -> Bool {\n\
             return r.active\n\
         }\n\
         func add_bonus_score(r: Record) -> Record {\n\
             return Record(..r, score: r.score + 10.0)\n\
         }\n\
         func etl_ref(records: [Record]) -> String {\n\
             active := records |> filter(is_active) |> map(add_bonus_score)\n\
             return encode(active)?\n\
         }\n\
         func json_etl_bench(iterations: Int, raw: String) -> Int {\n\
             var acc = 0\n\
             for _ in 0..iterations {\n\
                 records := decode<[Record]>(raw)?\n\
                 active := records |> filter(is_active) |> map(add_bonus_score)\n\
                 encoded := encode(active)?\n\
                 acc = acc + encoded.len()\n\
             }\n\
             return acc\n\
         }",
        "json_etl_bench",
    );
    let ref_id = module.func_id("etl_ref").unwrap();

    module.validate().expect("ETL reference module must validate");
    let mut tmp_vm = Vm::new();
    let Obj::Str(ref_out) = tmp_vm
        .call_obj(&module, ref_id, &[], &[decoded])
        .expect("ETL reference execution must succeed")
    else {
        panic!("ETL reference output must be String");
    };
    assert!(ref_out.contains("\"score\":10.5"));
    assert!(ref_out.contains("\"score\":1008.5"));
    assert!(!ref_out.contains("\"active\":false"));
    let expected_etl_bytes = ref_out.len() as i64;

    WorkloadState {
        module,
        bench_func: func_id,
        verify_func: func_id,
        scalar_args: vec![],
        obj_args: vec![Obj::Str(Arc::new(raw))],
        expected_per_op: expected_etl_bytes,
        after_check: Some(Box::new(move |objs| match &objs[0] {
            Obj::Str(s) if s.len() == expected_raw_bytes => Ok(()),
            _ => Err("JSON ETL input string was mutated".into()),
        })),
    }
}

pub fn json_workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "json.decode_100",
            category: "json",
            size: 100,
            target_us: None,
            old_conduit_us: Some(88.752),
            php_us: Some(93.526),
            go_us: Some(113.7),
            rust_us: Some(54.467),
            setup: Box::new(|_| setup_decode(100, 20_809)),
        },
        Workload {
            name: "json.encode_100",
            category: "json",
            size: 100,
            target_us: None,
            old_conduit_us: Some(23.875),
            php_us: Some(37.203),
            go_us: Some(49.345),
            rust_us: Some(24.563),
            setup: Box::new(|_| setup_encode(100, 20_809)),
        },
        Workload {
            name: "json.decode",
            category: "json",
            size: 1000,
            target_us: Some(600.0),
            old_conduit_us: Some(859.3),
            php_us: Some(936.8),
            go_us: Some(1171.1),
            rust_us: Some(540.4),
            setup: Box::new(|_| setup_decode(1000, 210_862)),
        },
        Workload {
            name: "json.encode",
            category: "json",
            size: 1000,
            target_us: Some(200.0),
            old_conduit_us: Some(220.3),
            php_us: Some(382.8),
            go_us: Some(500.4),
            rust_us: Some(232.4),
            setup: Box::new(|_| setup_encode(1000, 210_862)),
        },
        Workload {
            name: "pipeline.json_etl",
            category: "json",
            size: 1000,
            target_us: Some(750.0),
            old_conduit_us: None,
            php_us: None,
            go_us: None,
            rust_us: None,
            setup: Box::new(|_| setup_json_etl(1000, 210_862)),
        },
    ]
}
