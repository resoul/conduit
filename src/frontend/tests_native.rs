use std::collections::HashMap;
use std::sync::Arc;

use crate::frontend::{check_source, compile_source, compile_with_modules};
use crate::vm::Vm;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_TIME_ERROR, SCHEMA_VALIDATION_ERROR,
    StructData,
};

#[test]
fn cdt_time_now_monotonic_elapsed_and_conversions() {
    let src = r#"
import "cdt.time" as time
import { seconds, milliseconds, TimeError } from "cdt.time"

func check_now() -> Int {
    ts := time.now()?
    s := ts.unix_seconds()?
    ms := ts.unix_milliseconds()?
    if ms < s * 1000 { return -1 }
    return s
}

func check_dur_ms(v: Int) -> Int {
    d := milliseconds(v)?
    return d.seconds()? * 1000 + d.milliseconds()?
}

func check_elapsed() -> Int {
    t0 := time.monotonic()
    acc := 0
    for i in 0..100 { acc = acc + i }
    d := time.elapsed(t0)?
    return d.nanoseconds()? + (acc * 0)
}

func check_neg() -> String {
    switch seconds(-3) {
        case TimeError(e): return e.message
        case _: return "unexpected"
    }
    return "fallthrough"
}
"#;
    let module = compile_source(src).expect("cdt.time program should compile");
    let mut vm = Vm::new();

    let s = vm.call_scalar(&module, module.func_id("check_now").unwrap(), &[], &[]).unwrap();
    assert!(s > 1_700_000_000, "unexpected unix seconds {}", s);

    let d_res = vm
        .call_scalar(&module, module.func_id("check_dur_ms").unwrap(), &[2500], &[])
        .unwrap();
    assert_eq!(d_res, 2 * 1000 + 2500);

    let ns = vm
        .call_scalar(&module, module.func_id("check_elapsed").unwrap(), &[], &[])
        .unwrap();
    assert!((ns as i64) >= 0);

    let neg_obj = vm
        .call_obj(&module, module.func_id("check_neg").unwrap(), &[], &[])
        .unwrap();
    match neg_obj {
        Obj::Str(msg) => assert!(msg.contains("negative"), "got '{}'", msg),
        other => panic!("expected Str error message, got {:?}", other),
    }
}

#[test]
fn cdt_time_errors_and_opaque_ctors_rejected() {
    assert!(check_source("import \"cdt.time\" as time\nfunc f() -> Int { _ := time.Timestamp(); return 0 }").is_err());
    assert!(check_source("import \"cdt.time\" as time\nfunc f() -> Int { _ := time.Instant(); return 0 }").is_err());
    assert!(check_source("import \"cdt.time\" as time\nfunc f() -> Int { _ := time.Duration(); return 0 }").is_err());
    assert!(check_source("import { Timestamp } from \"cdt.time\"\nfunc f() -> Int { _ := Timestamp(); return 0 }").is_err());
    assert!(check_source("import { bogus } from \"cdt.time\"\nfunc f() -> Int { return 0 }").is_err());
    assert!(check_source("import \"cdt.time\" as time\nfunc f() -> Int { _ := time.seconds(true)?; return 0 }").is_err());
}

#[test]
fn cdt_validation_contracts_and_cross_module_privacy() {
    assert!(check_source("[#validator]\nfunc bad0() -> Void { }").is_err());
    assert!(check_source("import \"cdt.validation\" as v\n[#validator]\nfunc bad1(x: Int) -> Int { return x }").is_err());
    assert!(check_source("[#validator]\nfunc plain(x: Int) -> Int { return x }\nstruct S { x: Int @plain }").is_err());
    assert!(check_source("func plain(x: Int) -> Int { return x }\nstruct S { x: Int @plain }").is_err());
    assert!(check_source("import \"cdt.validation\" as v\nstruct S { x: Int @v.email }").is_err());
    assert!(check_source("import \"cdt.validation\" as v\nstruct S { x: Int @v.min(1.5) }").is_err());

    let mut modules = HashMap::new();
    modules.insert(
        "./rules",
        "import \"cdt.validation\" as v\n\
         [#Validator]\n\
         func hidden_pos(x: Int) -> Void | v.Issue {\n\
             if x <= 0 { return v.Issue(\"pos\", \"must be positive\") }\n\
         }\n\
         export [#validator]\n\
         func in_range(x: Int, lo: Int, hi: Int) -> Void | v.Issue {\n\
             if x < lo || x > hi { return v.Issue(\"range\", \"out of range\") }\n\
         }",
    );

    let bad_priv = "import * as r from \"./rules\"\n\
                    struct Item { x: Int @r.hidden_pos }\n\
                    func f(i: Item) -> Int { return i.x }";
    assert!(compile_with_modules(bad_priv, &modules).is_err());

    let bad_named = "import { hidden_pos } from \"./rules\"\nfunc f() -> Int { return 0 }";
    assert!(compile_with_modules(bad_named, &modules).is_err());

    let good = "import * as r from \"./rules\"\n\
                import \"cdt.validation\" as val\n\
                struct Box { n: Int @r.in_range(-10, 10) }\n\
                export func check_box(n: Int) -> Int {\n\
                    switch val.check(Box(n)) {\n\
                        case Box(b): return b.n\n\
                        case val.ValidationError(_): return -999\n\
                    }\n\
                    return -1000\n\
                }";
    let module = compile_with_modules(good, &modules).expect("exported cross-module validator should compile");
    let fid = module.func_id("check_box").unwrap();
    let mut vm = Vm::new();
    assert_eq!(vm.call_scalar(&module, fid, &[5], &[]).unwrap(), 5);
    assert_eq!(vm.call_scalar(&module, fid, &[99], &[]).unwrap(), (-999i64) as u64);
}

#[test]
fn cdt_validation_builtins_nested_optional_and_first_class() {
    let src = r#"
import "cdt.validation" as val

[#validator]
func non_blank(s: String) -> Void | val.Issue {
    if s.len() == 0 { return val.Issue("blank", "cannot be empty") }
}

[#Validator]
func step_mult(x: Int, k: Int) -> Void | val.Issue {
    if x % k != 0 { return val.Issue("mult", "not a multiple") }
}

struct Tag {
    name: String @non_blank
}

struct Address {
    city: String @val.min(2) @val.max(20)
}

struct User {
    email: String @val.email
    age: Int @val.min(18) @step_mult(2)
    score: Float @val.min(-2.5)
    nickname: String? @val.min(3)
    opt_num: Int? @val.min(5)
    address: Address
    tags: [Tag]
    meta: Map<String, Int> @val.min(1)
}

struct OptScalarBox {
    x: Int? @val.min(10)
    b: Bool?
}

func check_opt_ctor(n: Int) -> Int {
    box1 := OptScalarBox(x: nil, b: true)
    checked1 := val.check(box1)?
    box2 := OptScalarBox(x: n, b: nil)
    switch val.check(box2) {
        case OptScalarBox(_): return 1
        case val.ValidationError(_): return 0
    }
    return -1
}

func validate_user(u: User) -> String {
    switch val.check(u) {
        case User(ok): return "ok:" + ok.email
        case val.ValidationError(err):
            out := ""
            for v in err.issues {
                out = out + v.path + "=" + v.code + ";"
            }
            return out
    }
    return ""
}

func run_first_class(s: String) -> String {
    f := val.email
    return check_with_fn(f, s)
}

func check_with_fn(cb: (String) -> Void | val.Issue, s: String) -> String {
    switch cb(s) {
        case val.Issue(iss): return iss.code
        case _: return "valid"
    }
    return "valid"
}
"#;
    let module = compile_source(src).expect("validation suite should compile");
    let mut vm = Vm::new();

    let opt_fid = module.func_id("check_opt_ctor").unwrap();
    assert_eq!(vm.call_scalar(&module, opt_fid, &[12], &[]).unwrap(), 1);
    assert_eq!(vm.call_scalar(&module, opt_fid, &[5], &[]).unwrap(), 0);

    let val_fid = module.func_id("validate_user").unwrap();
    let u_sid = module.struct_schemas.iter().find(|s| s.type_name == "User").unwrap().schema_id;
    let a_sid = module.struct_schemas.iter().find(|s| s.type_name == "Address").unwrap().schema_id;
    let t_sid = module.struct_schemas.iter().find(|s| s.type_name == "Tag").unwrap().schema_id;

    let mut meta = OrderedMap::new();
    meta.set(MapKey::Str(Arc::new("k".into())), MapVal::Scalar(1));
    let meta_obj = Obj::Map(Arc::new(meta));

    let make_user = |email: &str, age: i64, score: f64, nick: Obj, opt_n: Obj, city: &str, t1: &str| {
        let addr = {
            let mut d = StructData::new(a_sid, 0, 1);
            d.inline_objs[0] = Obj::str(city);
            Obj::Record(Arc::new(d))
        };
        let tag0 = {
            let mut d = StructData::new(t_sid, 0, 1);
            d.inline_objs[0] = Obj::str("ok");
            Obj::Record(Arc::new(d))
        };
        let tag1 = {
            let mut d = StructData::new(t_sid, 0, 1);
            d.inline_objs[0] = Obj::str(t1);
            Obj::Record(Arc::new(d))
        };
        let tags = Obj::List(ListStorage::Obj(Arc::new(vec![tag0, tag1])));
        let mut u = StructData::new(u_sid, 2, 6);
        u.set_scalar(0, age as u64);
        u.set_scalar(1, score.to_bits());
        u.set_obj(0, Obj::str(email));
        u.set_obj(1, nick);
        u.set_obj(2, opt_n);
        u.set_obj(3, addr);
        u.set_obj(4, tags);
        u.set_obj(5, meta_obj.clone());
        Obj::Record(Arc::new(u))
    };

    let valid_u = make_user("a@b.co", 20, -2.5, Obj::Empty, Obj::Empty, "Kyiv", "rust");
    let res_ok = vm.call_obj(&module, val_fid, &[], &[valid_u]).unwrap();
    assert_eq!(res_ok, Obj::str("ok:a@b.co"));

    let bad_u = make_user("bad", 15, -3.0, Obj::str("hi"), Obj::boxed_int(1), "X", "");
    let res_err = vm.call_obj(&module, val_fid, &[], &[bad_u]).unwrap();
    let Obj::Str(s) = res_err else { panic!("expected Str") };
    assert!(s.contains("email=email;"), "got {}", s);
    assert!(s.contains("age=min;age=mult;"), "got {}", s);
    assert!(s.contains("score=min;"), "got {}", s);
    assert!(s.contains("nickname=min;"), "got {}", s);
    assert!(s.contains("opt_num=min;"), "got {}", s);
    assert!(s.contains("address.city=min;"), "got {}", s);
    assert!(s.contains("tags[1].name=blank;"), "got {}", s);

    let fc_fid = module.func_id("run_first_class").unwrap();
    assert_eq!(vm.call_obj(&module, fc_fid, &[], &[Obj::str("x@y.zz")]).unwrap(), Obj::str("valid"));
    assert_eq!(vm.call_obj(&module, fc_fid, &[], &[Obj::str("nope")]).unwrap(), Obj::str("email"));
}

#[test]
fn cdt_validation_runtime_error_and_tree_shaking() {
    let src = r#"
import { check, Issue } from "cdt.validation"

func helper_keep(x: Int) -> Int {
    if x == 0 { return 10 / x }
    return x
}

[#validator]
func val_keep(x: Int) -> Void | Issue {
    keep_res := helper_keep(x)
    if keep_res < -999999 { return Issue("neg", "bad") }
}

[#validator]
func val_unused(x: Int) -> Void | Issue {
    if x < 0 { return Issue("neg", "bad") }
}

struct Checked {
    v: Int @val_keep
}

struct Unchecked {
    v: Int @val_unused
}

export func run(v: Int) -> Int {
    unused_uc := Unchecked(v)
    c := check(Checked(v))?
    return c.v
}
"#;
    let module = compile_source(src).expect("should compile");
    assert!(module.func_id("val_keep").is_some(), "used validator kept");
    assert!(module.func_id("helper_keep").is_some(), "validator helper kept");
    assert!(module.func_id("val_unused").is_none(), "unused validator tree-shaken");

    let mut vm = Vm::new();
    let fid = module.func_id("run").unwrap();
    assert_eq!(vm.call_scalar(&module, fid, &[7], &[]).unwrap(), 7);
    let err = vm.call_scalar(&module, fid, &[0], &[]).unwrap_err();
    assert!(err.message.contains("zero"), "runtime error halted check: {}", err);
}

#[test]
fn switch_types_and_try_early_propagation() {
    let src = r#"
import { seconds, TimeError } from "cdt.time"
import { min, Issue, ValidationError } from "cdt.validation"

func step1(n: Int) -> Int | TimeError {
    d := seconds(n)?
    ms := d.milliseconds()?
    return ms
}

func step2(n: Int) -> Int | TimeError {
    x := step1(n)?
    return x + 1
}

func check_try_void(n: Int) -> String | Issue {
    min(n, 10)?
    return "passed"
}

func classify_int_err(n: Int) -> Int {
    v := step2(n)
    switch v {
        case Int(x): return x
        case TimeError(_): return -1
    }
    return -2
}

func strict_trap(n: Int) -> Int {
    return step2(n)?
}
"#;
    let module = compile_source(src).expect("switch & try test should compile");
    let mut vm = Vm::new();

    let s2 = module.func_id("step2").unwrap();
    assert!(matches!(vm.call_obj(&module, s2, &[2], &[]).unwrap(), Obj::Record(r) if r.inline_scalars[0] == 2001));
    assert!(matches!(vm.call_obj(&module, s2, &[(-1i64) as u64], &[]).unwrap(), Obj::Record(r) if r.schema_id == SCHEMA_TIME_ERROR));

    let cv = module.func_id("check_try_void").unwrap();
    assert_eq!(vm.call_obj(&module, cv, &[12], &[]).unwrap(), Obj::str("passed"));
    assert!(matches!(vm.call_obj(&module, cv, &[3], &[]).unwrap(), Obj::Record(_)));

    let cl = module.func_id("classify_int_err").unwrap();
    assert_eq!(vm.call_scalar(&module, cl, &[3], &[]).unwrap(), 3001);
    assert_eq!(vm.call_scalar(&module, cl, &[(-2i64) as u64], &[]).unwrap(), (-1i64) as u64);

    let st = module.func_id("strict_trap").unwrap();
    assert_eq!(vm.call_scalar(&module, st, &[4], &[]).unwrap(), 4001);
    assert!(vm.call_scalar(&module, st, &[(-5i64) as u64], &[]).is_err());

    assert_ne!(SCHEMA_TIME_ERROR, SCHEMA_VALIDATION_ERROR);
}
