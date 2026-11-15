use std::sync::Arc;

use crate::benchmarks::{all_workloads, builder::FnBuilder, builder::ModuleExt, workloads_json};
use crate::ir::{
    CallableRef, ClosureProto, ElemKind, Module, PipelinePlan, PipelineStage, RecordDesc,
};
use crate::native::json_decode::decode_json_with_plan;
use crate::native::json_encode::encode_json_obj;
use crate::native::{JsonDecodePlan, JsonType, MAX_JSON_DEPTH, StructFieldDef, StructSchema};
use crate::vm::Vm;
use crate::vm::opcode::Op;
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj, OrderedMap, StructData};

#[test]
fn checked_int_overflow_and_div_zero() {
    let mut m = Module::new();
    let c_max = m.add_int(i64::MAX);
    let c_min = m.add_int(i64::MIN);

    // 1. AddInt overflow
    let mut fb = FnBuilder::new("add_ov", 0, 0, false);
    let a = fb.alloc_s();
    let b = fb.alloc_s();
    let dst = fb.alloc_s();
    fb.emit(Op::LoadConstInt { dst: a, idx: c_max });
    fb.emit(Op::LoadImmInt { dst: b, imm: 1 });
    fb.emit(Op::AddInt { dst, a, b });
    fb.emit(Op::ReturnScalar { src: dst });
    let f_add = m.add_fn(fb.finish()) as u32;

    // 2. DivInt by zero
    let mut fb2 = FnBuilder::new("div_zero", 0, 0, false);
    let a = fb2.alloc_s();
    let b = fb2.alloc_s();
    let dst = fb2.alloc_s();
    fb2.emit(Op::LoadImmInt { dst: a, imm: 10 });
    fb2.emit(Op::LoadImmInt { dst: b, imm: 0 });
    fb2.emit(Op::DivInt { dst, a, b });
    fb2.emit(Op::ReturnScalar { src: dst });
    let f_div = m.add_fn(fb2.finish()) as u32;

    // 3. NegInt overflow (i64::MIN)
    let mut fb3 = FnBuilder::new("neg_ov", 0, 0, false);
    let a = fb3.alloc_s();
    let dst = fb3.alloc_s();
    fb3.emit(Op::LoadConstInt { dst: a, idx: c_min });
    fb3.emit(Op::NegInt { dst, src: a });
    fb3.emit(Op::ReturnScalar { src: dst });
    let f_neg = m.add_fn(fb3.finish()) as u32;

    // 4. DivFloat by zero
    let mut fb4 = FnBuilder::new("fdiv_zero", 0, 0, false);
    let cf0 = m.add_float(0.0);
    let cf1 = m.add_float(1.0);
    let a = fb4.alloc_s();
    let b = fb4.alloc_s();
    let dst = fb4.alloc_s();
    fb4.emit(Op::LoadConstFloat { dst: a, idx: cf1 });
    fb4.emit(Op::LoadConstFloat { dst: b, idx: cf0 });
    fb4.emit(Op::DivFloat { dst, a, b });
    fb4.emit(Op::ReturnScalar { src: dst });
    let f_fdiv = m.add_fn(fb4.finish()) as u32;

    m.validate().unwrap();
    let mut vm = Vm::new();
    assert!(vm.call_scalar(&m, f_add, &[], &[]).unwrap_err().message.contains("overflow"));
    assert!(vm.call_scalar(&m, f_div, &[], &[]).unwrap_err().message.contains("zero"));
    assert!(vm.call_scalar(&m, f_neg, &[], &[]).unwrap_err().message.contains("overflow"));
    assert!(vm.call_scalar(&m, f_fdiv, &[], &[]).unwrap_err().message.contains("zero"));
}

#[test]
fn recursive_fib_and_call_depth_limit() {
    let mut m = Module::new();
    // fib(n: Int) -> Int
    let mut fb = FnBuilder::new("fib", 1, 0, false);
    let two = fb.alloc_s();
    let cond = fb.alloc_s();
    let r1 = fb.alloc_s();
    let r2 = fb.alloc_s();
    let sum = fb.alloc_s();
    let arg = fb.alloc_s();
    fb.emit(Op::LoadImmInt { dst: two, imm: 2 });
    fb.emit(Op::LtInt {
        dst: cond,
        a: 0,
        b: two,
    });
    let base_j = fb.emit(Op::JumpIfFalse { cond, target: 0 });
    fb.emit(Op::ReturnScalar { src: 0 });
    let rec = fb.pc();
    fb.patch_target(base_j, rec);
    fb.emit(Op::AddIntImm {
        dst: arg,
        a: 0,
        imm: -1,
    });
    fb.emit(Op::CallDirect {
        func_id: 0,
        arg_base_s: arg,
        arg_base_o: 0,
        ret_reg: r1,
    });
    fb.emit(Op::AddIntImm {
        dst: arg,
        a: 0,
        imm: -2,
    });
    fb.emit(Op::CallDirect {
        func_id: 0,
        arg_base_s: arg,
        arg_base_o: 0,
        ret_reg: r2,
    });
    fb.emit(Op::AddInt {
        dst: sum,
        a: r1,
        b: r2,
    });
    fb.emit(Op::ReturnScalar { src: sum });
    let fib_id = m.add_fn(fb.finish()) as u32;

    // Infinite recursion function
    let mut inf = FnBuilder::new("inf", 0, 0, false);
    let r = inf.alloc_s();
    inf.emit(Op::CallDirect {
        func_id: 1,
        arg_base_s: 0,
        arg_base_o: 0,
        ret_reg: r,
    });
    inf.emit(Op::ReturnScalar { src: r });
    let inf_id = m.add_fn(inf.finish()) as u32;

    m.validate().unwrap();
    let mut vm = Vm::new();
    assert_eq!(vm.call_scalar(&m, fib_id, &[20], &[]).unwrap(), 6765);
    assert!(
        vm.call_scalar(&m, inf_id, &[], &[])
            .unwrap_err()
            .message
            .contains("depth")
    );
}

#[test]
fn closure_captures_snapshot_and_isolation() {
    let mut m = Module::new();
    // fn(x) { return upval_s0 + x + upval_o0.len() }
    let mut cf = FnBuilder::new("cls_fn", 1, 0, false);
    let u0 = cf.alloc_s();
    let uo0 = cf.alloc_o();
    let len = cf.alloc_s();
    let t1 = cf.alloc_s();
    let t2 = cf.alloc_s();
    cf.emit(Op::LoadUpvalueScalar { dst: u0, idx: 0 });
    cf.emit(Op::LoadUpvalueObj { dst: uo0, idx: 0 });
    cf.emit(Op::ListLen {
        dst_s: len,
        list_obj: uo0,
    });
    cf.emit(Op::AddInt {
        dst: t1,
        a: u0,
        b: 0,
    });
    cf.emit(Op::AddInt {
        dst: t2,
        a: t1,
        b: len,
    });
    cf.emit(Op::ReturnScalar { src: t2 });
    let cf_id = m.add_fn(cf.finish()) as u32;

    let cp = m.add_closure(ClosureProto {
        func_id: cf_id,
        capture_scalars: Box::new([0]),
        capture_objs: Box::new([0]),
    });

    // builder(base: Int, items: [Int]) -> Int
    let mut main_f = FnBuilder::new("test_cls", 1, 1, false);
    let cls = main_f.alloc_o();
    let arg_o = main_f.alloc_o();
    let arg_s = main_f.alloc_s();
    let ret = main_f.alloc_s();
    let extra = main_f.alloc_s();
    main_f.emit(Op::MakeClosure {
        dst_obj: cls,
        proto_idx: cp,
    });
    // Mutate outer base & items after closure creation; closure snapshot must stay unchanged!
    main_f.emit(Op::LoadImmInt { dst: 0, imm: 999 });
    main_f.emit(Op::LoadImmInt {
        dst: extra,
        imm: 77,
    });
    main_f.emit(Op::ListAppendInt {
        list_obj: 0,
        val_s: extra,
    });
    main_f.emit(Op::LoadImmInt {
        dst: arg_s,
        imm: 5,
    });
    main_f.emit(Op::CallCallable {
        callee_obj: cls,
        arg_base_s: arg_s,
        arg_base_o: arg_o,
        ret_reg: ret,
    });
    main_f.emit(Op::ReturnScalar { src: ret });
    let mid = m.add_fn(main_f.finish()) as u32;

    m.validate().unwrap();
    let mut vm = Vm::new();
    let items = Obj::List(ListStorage::Int(Arc::new(vec![10, 20, 30])));
    // Captured base=10, items len=3, arg=5 -> 18 (not affected by 999 or append(77))
    assert_eq!(vm.call_scalar(&m, mid, &[10], &[items]).unwrap(), 18);
}

#[test]
fn cow_list_and_map_isolation() {
    let mut l1 = ListStorage::Int(Arc::new(vec![1, 2, 3]));
    let mut l2 = l1.clone();
    l2.append_int(4);
    assert_eq!(l1.len(), 3);
    assert_eq!(l2.len(), 4);
    l1.append_int(9);
    assert_eq!(l1, ListStorage::Int(Arc::new(vec![1, 2, 3, 9])));
    assert_eq!(l2, ListStorage::Int(Arc::new(vec![1, 2, 3, 4])));

    let mut m1 = OrderedMap::new();
    m1.set(MapKey::Int(0), MapVal::Scalar(10));
    m1.set(MapKey::Int(1), MapVal::Scalar(20));
    // Non-dense key transitions to hash index cleanly
    m1.set(MapKey::Int(5), MapVal::Scalar(50));
    assert_eq!(m1.get(&MapKey::Int(0)), Some(&MapVal::Scalar(10)));
    assert_eq!(m1.get(&MapKey::Int(5)), Some(&MapVal::Scalar(50)));
    assert_eq!(m1.get(&MapKey::Int(2)), None);
}

#[test]
fn large_struct_inline_and_overflow_slots() {
    let mut s = StructData::new(42, 6, 6);
    for i in 0..6 {
        s.set_scalar(i, (i as u64) * 10);
        s.set_obj(i, Obj::str(format!("f{}", i)));
    }
    for i in 0..6 {
        assert_eq!(s.get_scalar(i), (i as u64) * 10);
        assert_eq!(s.get_obj(i), &Obj::str(format!("f{}", i)));
    }
}

#[test]
fn non_leaf_and_float_pipelines() {
    let mut m = Module::new();
    // helper(x) = x + 1.0
    let c1 = m.add_float(1.0);
    let mut f_helper = FnBuilder::new("f_inc", 1, 0, false);
    let one = f_helper.alloc_s();
    let out = f_helper.alloc_s();
    f_helper.emit(Op::LoadConstFloat { dst: one, idx: c1 });
    f_helper.emit(Op::AddFloat {
        dst: out,
        a: 0,
        b: one,
    });
    f_helper.emit(Op::ReturnScalar { src: out });
    let helper_id = m.add_fn(f_helper.finish());

    // non-leaf stage calling helper
    let mut f_non_leaf = FnBuilder::new("non_leaf", 1, 0, false);
    let arg = f_non_leaf.alloc_s();
    let res = f_non_leaf.alloc_s();
    f_non_leaf.emit(Op::MoveScalar { dst: arg, src: 0 });
    f_non_leaf.emit(Op::CallDirect {
        func_id: helper_id,
        arg_base_s: arg,
        arg_base_o: 0,
        ret_reg: res,
    });
    f_non_leaf.emit(Op::ReturnScalar { src: res });
    let nl_id = m.add_fn(f_non_leaf.finish()) as u32;

    // filter: x > 2.5
    let c25 = m.add_float(2.5);
    let mut f_filt = FnBuilder::new("gt_25", 1, 0, false);
    let lim = f_filt.alloc_s();
    let cond = f_filt.alloc_s();
    f_filt.emit(Op::LoadConstFloat { dst: lim, idx: c25 });
    f_filt.emit(Op::LtFloat {
        dst: cond,
        a: lim,
        b: 0,
    });
    f_filt.emit(Op::ReturnScalar { src: cond });
    let filt_id = m.add_fn(f_filt.finish()) as u32;

    let plan_idx = m.add_pipeline(PipelinePlan {
        stages: Box::new([
            PipelineStage::MapScalarToScalar(CallableRef::DirectFunc(nl_id)),
            PipelineStage::FilterScalar(CallableRef::DirectFunc(filt_id)),
        ]),
        elem_kind: ElemKind::Float,
    });

    let mut runner = FnBuilder::new("run_fp", 0, 1, true);
    let dst = runner.alloc_o();
    runner.emit(Op::RunPipeline {
        dst_obj: dst,
        src_obj: 0,
        plan_idx,
    });
    runner.emit(Op::ReturnObj { src: dst });
    let run_id = m.add_fn(runner.finish()) as u32;

    m.validate().unwrap();
    let mut vm = Vm::new();
    let input = Obj::List(ListStorage::Float(Arc::new(vec![1.0, 2.0, 3.0])));
    let res = vm.call_obj(&m, run_id, &[], &[input]).unwrap();
    assert_eq!(
        res,
        Obj::List(ListStorage::Float(Arc::new(vec![3.0, 4.0])))
    );
}

#[test]
fn verify_all_benchmark_workloads_semantics() {
    for w in all_workloads() {
        let mut vm = Vm::new();
        let state = (w.setup)(&mut vm);
        state
            .module
            .validate()
            .unwrap_or_else(|e| panic!("{} validate error: {}", w.name, e));
        let mut s_args = vec![1u64];
        s_args.extend_from_slice(&state.scalar_args);
        let got = vm
            .call_scalar(&state.module, state.verify_func, &s_args, &state.obj_args)
            .unwrap_or_else(|e| panic!("{} verify error: {}", w.name, e)) as i64;
        assert_eq!(
            got, state.expected_per_op,
            "{}: verify checksum mismatch",
            w.name
        );
        if let Some(after) = &state.after_check {
            after(&state.obj_args).unwrap_or_else(|e| panic!("{} after error: {}", w.name, e));
        }
    }
}

#[test]
fn json_lists_maps_scalars_and_escaping() {
    // 1. Direct [Int] & [Float] ListStorage decode/encode without boxing
    let schemas: Vec<StructSchema> = vec![];
    let int_plan = JsonDecodePlan::compile(&JsonType::List(Box::new(JsonType::Int)), &schemas);
    let ints = decode_json_with_plan("[0, -0, 42, 9223372036854775807, -9223372036854775808]", &int_plan).unwrap();
    assert_eq!(
        ints,
        Obj::List(ListStorage::Int(Arc::new(vec![0, 0, 42, i64::MAX, i64::MIN])))
    );
    assert_eq!(
        encode_json_obj(&ints, &schemas).unwrap(),
        "[0,0,42,9223372036854775807,-9223372036854775808]"
    );

    let float_plan = JsonDecodePlan::compile(&JsonType::List(Box::new(JsonType::Float)), &schemas);
    let floats = decode_json_with_plan("[-0.0, 0, 1.5, -0.5, -1.5, 1e2, 9007199254740993.0]", &float_plan).unwrap();
    match &floats {
        Obj::List(ListStorage::Float(v)) => {
            assert_eq!(v[0].to_bits(), (-0.0f64).to_bits());
            assert_eq!(v[1], 0.0);
            assert_eq!(v[2], 1.5);
            assert_eq!(v[3], -0.5);
            assert_eq!(v[4], -1.5);
            assert_eq!(v[5], 100.0);
            assert_eq!(v[6], 9007199254740992.0);
        }
        other => panic!("expected Float list, got {:?}", other),
    }
    assert_eq!(
        encode_json_obj(&floats, &schemas).unwrap(),
        "[-0.0,0.0,1.5,-0.5,-1.5,100.0,9007199254740992.0]"
    );

    // 2. Escaped strings, control bytes 0..=0x1F, UTF-16 surrogates, Map<String, ...>
    let all_ctrl: String = (0u8..=0x1F).map(char::from).collect();
    let encoded_ctrl = encode_json_obj(&Obj::str(all_ctrl.clone()), &schemas).unwrap();
    let str_plan = JsonDecodePlan::compile(&JsonType::String, &schemas);
    assert_eq!(decode_json_with_plan(&encoded_ctrl, &str_plan).unwrap(), Obj::str(all_ctrl));

    let surr = decode_json_with_plan(
        r#""é\n世界\t\"\\\/\b\f\r\uD83D\uDE00終""#,
        &str_plan,
    )
    .unwrap();
    assert_eq!(surr, Obj::str("é\n世界\t\"\\/\u{0008}\u{000C}\r😀終"));

    let map_float_plan = JsonDecodePlan::compile(&JsonType::Map(Box::new(JsonType::Float)), &schemas);
    let map_val = decode_json_with_plan(r#"{"a":1.5,"b":-0.5}"#, &map_float_plan).unwrap();
    assert_eq!(encode_json_obj(&map_val, &schemas).unwrap(), r#"{"a":1.5,"b":-0.5}"#);

    let map_bool_plan = JsonDecodePlan::compile(&JsonType::Map(Box::new(JsonType::Bool)), &schemas);
    let map_bool = decode_json_with_plan(r#"{"ok":true,"no":false}"#, &map_bool_plan).unwrap();
    assert_eq!(encode_json_obj(&map_bool, &schemas).unwrap(), r#"{"ok":true,"no":false}"#);
}

#[test]
fn json_struct_errors_depth_and_fixture_vm_ops() {
    let schemas = vec![StructSchema::new(
        0,
        "Item",
        vec![
            StructFieldDef {
                name: "id".into(),
                json_name: "id".into(),
                ty: JsonType::Int,
            },
            StructFieldDef {
                name: "dot_key".into(),
                json_name: "a.b".into(),
                ty: JsonType::Optional(Box::new(JsonType::Struct(0))),
            },
        ],
    )];
    let item_plan = JsonDecodePlan::compile(&JsonType::Struct(0), &schemas);

    let err_missing = decode_json_with_plan(r#"{"a.b":null}"#, &item_plan).unwrap_err();
    assert_eq!(err_missing.path, "id");
    assert!(err_missing.message.contains("Missing required field 'id'"));

    let err_unknown = decode_json_with_plan(r#"{"id":1,"extra":2}"#, &item_plan).unwrap_err();
    assert_eq!(err_unknown.path, "extra");
    assert!(err_unknown.message.contains("Unknown field 'extra'"));

    let err_nested = decode_json_with_plan(r#"{"id":1,"a.b":{"id":1.5}}"#, &item_plan).unwrap_err();
    assert_eq!(err_nested.path, "[\"a.b\"].id");
    assert!(err_nested.message.contains("Expected Int"));

    let list_int_plan = JsonDecodePlan::compile(&JsonType::List(Box::new(JsonType::Int)), &schemas);
    let err_ov = decode_json_with_plan("[9223372036854775808]", &list_int_plan).unwrap_err();
    assert_eq!(err_ov.path, "[0]");
    assert!(err_ov.message.contains("overflows 64-bit Int"));

    let list_float_plan = JsonDecodePlan::compile(&JsonType::List(Box::new(JsonType::Float)), &schemas);
    let err_inf = decode_json_with_plan("[1e400]", &list_float_plan).unwrap_err();
    assert_eq!(err_inf.path, "[0]");
    assert!(err_inf.message.contains("overflows finite IEEE 754"));

    // Depth limit: nested optional struct list
    let nested_plan = JsonDecodePlan::compile(
        &JsonType::List(Box::new(JsonType::Optional(Box::new(JsonType::List(
            Box::new(JsonType::Int),
        ))))),
        &schemas,
    );
    let deep = format!("{}0{}", "[".repeat(MAX_JSON_DEPTH + 1), "]".repeat(MAX_JSON_DEPTH + 1));
    assert!(
        decode_json_with_plan(&deep, &nested_plan)
            .unwrap_err()
            .message
            .contains("Maximum JSON nesting depth (128)")
    );

    // Encode errors: non-finite Float and non-String map key
    let bad_floats = Obj::List(ListStorage::Float(Arc::new(vec![1.0, f64::NAN])));
    let enc_err = encode_json_obj(&bad_floats, &schemas).unwrap_err();
    assert_eq!(enc_err.path, "[1]");
    let mut bad_map = OrderedMap::new();
    bad_map.set(MapKey::Int(7), MapVal::Scalar(1));
    assert!(
        encode_json_obj(&Obj::Map(Arc::new(bad_map)), &schemas)
            .unwrap_err()
            .message
            .contains("String map keys")
    );

    // VM Op::JsonDecode -> Op::JsonEncode roundtrip with full 100-record fixture
    let mut m = Module::new();
    for s in workloads_json::json_schemas() {
        m.add_struct_schema(s);
    }
    let rec_plan = m.add_json_plan(JsonDecodePlan::compile(
        &JsonType::List(Box::new(JsonType::Struct(1))),
        &m.struct_schemas,
    ));
    let mut fb = FnBuilder::new("vm_json_rt", 0, 1, true);
    let dec = fb.alloc_o();
    let enc = fb.alloc_o();
    fb.emit(Op::JsonDecode {
        dst_obj: dec,
        src_obj: 0,
        plan_idx: rec_plan,
    });
    fb.emit(Op::JsonEncode {
        dst_obj: enc,
        src_obj: dec,
    });
    fb.emit(Op::ReturnObj { src: enc });
    let fid = m.add_fn(fb.finish()) as u32;
    m.validate().unwrap();

    let raw = workloads_json::build_fixture_json(100);
    assert_eq!(raw.len(), 20_809);
    let mut vm = Vm::new();
    let res = vm.call_obj(&m, fid, &[], &[Obj::str(raw.clone())]).unwrap();
    assert_eq!(res, Obj::str(raw));
}

#[test]
fn string_methods_e2e() {
    let src = r#"
func main() -> String {
    s := "  hello,conduit,world  ".trim()
    if "".is_empty() && !s.is_empty() && s.contains("conduit") && s.starts_with("hello") && s.ends_with("world") {
        parts := s.split(",")
        joined := parts.join("-")
        unchanged := joined.replace("", "x")
        return unchanged.replace("conduit", "cdt")
    }
    return "fail"
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    module.validate().unwrap();
    let mut vm = Vm::new();
    let fid = module.func_id("main").unwrap();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("hello-cdt-world"));
}

#[test]
fn list_reduce_and_number_methods_e2e() {
    let src = r#"
func main() -> String {
    nums := [5, -2, 10, 3, 1]
    sorted := nums.sort()
    words := ["pear", "apple", "fig"].sort()
    floats := [3.5, -1.25, 2.0].sort()

    var empty: [Int] = []
    if !empty.is_empty() || nums.is_empty() {
        return "fail_empty"
    }

    f := sorted.first() ?? -999
    l := sorted.last() ?? -999
    ef := empty.first() ?? -999
    el := empty.last() ?? -999

    found := sorted.find((x) => x > 4) ?? -1
    none := sorted.find((x) => x > 100) ?? -1

    has_big := sorted.any((x) => x >= 10)
    all_pos := sorted.all((x) => x > 0)

    sum := sorted
        |> filter((x) => x > 0)
        |> map((x) => x * 2)
        |> reduce(0, (acc, x) => acc + x)

    ia := (-42).abs()
    im := 10.min(3).max(5).clamp(0, 4)
    fa := (-3.5).abs()
    fm := 1.5.min(2.5).max(0.5).clamp(1.0, 2.0)

    w0 := words.first() ?? ""
    f0 := floats.first() ?? 0.0

    return "{f},{l},{ef},{el},{found},{none},{has_big},{all_pos},{sum},{ia},{im},{fa},{fm},{w0},{f0}"
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    module.validate().unwrap();
    let mut vm = Vm::new();
    let fid = module.func_id("main").unwrap();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(
        res,
        Obj::str("-2,10,-999,-999,5,-1,true,false,38,42,4,3.5,1.5,apple,-1.25")
    );
}

#[test]
fn optional_chaining_and_defaults_e2e() {
    let src = r#"
import * as validation from "cdt.validation"

struct Addr {
    city: String = "Kyiv"
}

struct Config {
    host: String @validation.min(1) = "localhost"
    port: Int = 8080
    addr: Addr? = nil
}

func greet(name: String, prefix: String = "Hello") -> String {
    return "{prefix}, {name}!"
}

func main() -> String {
    c1 := Config()
    c2 := Config(port: 9090, addr: Addr())
    c3 := Config("0.0.0.0")

    var opt_none: Config? = nil
    var opt_some: Config? = c2

    p1 := opt_none?.port ?? -1
    p2 := opt_some?.port ?? -1
    city1 := opt_none?.addr?.city ?? "none"
    city2 := opt_some?.addr?.city ?? "none"

    g1 := greet("Alice")
    g2 := greet("Bob", "Hi")

    bad := Config(host: "", port: 80)
    var msg: String = "ok"
    switch validation.check(bad) {
        case Config(_):
        case validation.ValidationError(e):
            msg = e.issues.first()?.path ?? "empty"
    }

    return "{c1.host}:{c1.port}|{c3.host}:{c3.port}|{p1},{p2}|{city1},{city2}|{g1}|{g2}|{msg}"
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    module.validate().unwrap();
    let mut vm = Vm::new();
    let fid = module.func_id("main").unwrap();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(
        res,
        Obj::str("localhost:8080|0.0.0.0:8080|-1,9090|none,Kyiv|Hello, Alice!|Hi, Bob!|host")
    );
}

#[test]
fn cdt_env_and_load_e2e() {
    unsafe {
        std::env::set_var("CONDUIT_TEST_PORT", "9090");
        std::env::set_var("CONDUIT_TEST_DEBUG", "true");
        std::env::remove_var("CONDUIT_TEST_MISSING");
    }

    let src = r#"
import * as env from "cdt.env"
import * as validation from "cdt.validation"

struct Config {
    conduit_test_port: Int @env @validation.min(1024)
    debug: Bool @env("CONDUIT_TEST_DEBUG")
    host: String @env = "127.0.0.1"
    opt_tag: String? @env("CONDUIT_TEST_MISSING")
}

func run_env() -> String | env.EnvError | validation.ValidationError {
    cfg := env.load<Config>()?
    checked := validation.check(cfg)?
    got := env.get("CONDUIT_TEST_PORT") ?? "none"
    req := env.require("CONDUIT_TEST_PORT")?
    tag := checked.opt_tag ?? "no-tag"
    args_len := env.args().len()
    has_args := args_len >= 0
    return "{checked.host}:{checked.conduit_test_port}:{checked.debug}:{got}:{req}:{tag}:{has_args}"
}

func main() -> String {
    switch run_env() {
        case String(s): return s
        case env.EnvError(e): return "env_err:{e.name}:{e.message}"
        case validation.ValidationError(e): return "val_err:{e.issues.len()}"
    }
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    module.validate().unwrap();
    let mut vm = Vm::new();
    let fid = module.func_id("main").unwrap();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();

    unsafe {
        std::env::remove_var("CONDUIT_TEST_PORT");
        std::env::remove_var("CONDUIT_TEST_DEBUG");
    }

    assert_eq!(
        res,
        Obj::str("127.0.0.1:9090:true:9090:9090:no-tag:true")
    );
}

#[allow(dead_code)]
fn _keep_record_desc(_: RecordDesc) {}
