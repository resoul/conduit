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

#[test]
fn cdt_cli_dsl_annotations_help_and_bundle_e2e() {
    let src = r#"
import cli from "cdt.cli"
import { CliError } from "cdt.cli"

cli Tool "Demo CLI" {
    flag verbose "v" "Verbose output"
    opt port: Int "p" "Listen port" = 8080
    group db "Database commands" {
        cmd migrate "Run migrations" {
            arg target: String "Migration target"
            flag dry_run "Dry run"
        }
    }
    cmd run "Run script" {
        arg entry: String "Entry file"
        rest args "Extra args"
    }
}

struct AnnRun {
    entry: String @cli.arg("Entry file")
    extra: [String] @cli.rest("Extra args") = []
}

struct AnnTool {
    verbose: Bool @cli.flag("v", "Verbose") = false
    port: Int @cli.opt("p", "Port") = 3000
    run: AnnRun? @cli.cmd("Run script")
}

export func run_dsl(raw: [String]) -> String {
    res := cli.parse_args<Tool>(raw)
    switch res {
        case CliError(e):
            return e.kind + ":" + e.message
        case Tool(app):
            v := "0"
            if app.verbose { v = "1" }
            switch app.db {
                case Tool_db(db):
                    switch db.migrate {
                        case Tool_db_migrate(m):
                            dr := "live"
                            if m.dry_run { dr = "dry" }
                            return "migrate:" + m.target + ":" + dr + ":" + v + ":" + app.port.to_string()
                    }
            }
            switch app.run {
                case Tool_run(r):
                    return "run:" + r.entry + ":" + r.args.len().to_string() + ":" + v + ":" + app.port.to_string()
            }
            return "root:" + v + ":" + app.port.to_string()
    }
    return "unreachable"
}

export func run_ann(raw: [String]) -> String {
    res := cli.parse_args<AnnTool>(raw)
    switch res {
        case CliError(e):
            return e.kind
        case AnnTool(app):
            v := "0"
            if app.verbose { v = "1" }
            switch app.run {
                case AnnRun(r):
                    return "ann:" + r.entry + ":" + r.extra.len().to_string() + ":" + v + ":" + app.port.to_string()
            }
            return "ann-root:" + v + ":" + app.port.to_string()
    }
    return "unreachable"
}

export func get_help() -> String {
    return cli.help<Tool>()
}
"#;
    let module = compile_source(src).expect("cdt.cli program should compile");

    // Roundtrip through bytecode bundle to verify cli_plans serialization.
    let dsl_fid_orig = module.func_id("run_dsl").unwrap();
    let bytes = crate::ir::bundle::encode_bundle(&module, dsl_fid_orig, 1).expect("bundle encode");
    let (decoded, _, _) = crate::ir::bundle::decode_bundle(&bytes).expect("bundle decode");

    let mut vm = Vm::new();
    let dsl_fid = decoded.func_id("run_dsl").unwrap();
    let ann_fid = decoded.func_id("run_ann").unwrap();
    let help_fid = decoded.func_id("get_help").unwrap();

    let mk_args = |items: &[&str]| -> Obj {
        Obj::List(crate::vm::value::ListStorage::Obj(std::sync::Arc::new(
            items.iter().map(|s| Obj::str(*s)).collect(),
        )))
    };

    // Subcommand inside group + global short flag + default port
    let r1 = vm
        .call_obj(&decoded, dsl_fid, &[], &[mk_args(&["-v", "db", "migrate", "prod", "--dry-run"])])
        .unwrap();
    assert_eq!(r1, Obj::str("migrate:prod:dry:1:8080"));

    // Subcommand with rest args + option=val
    let r2 = vm
        .call_obj(&decoded, dsl_fid, &[], &[mk_args(&["--port=9090", "run", "main.cdt", "--", "a", "b"])])
        .unwrap();
    assert_eq!(r2, Obj::str("run:main.cdt:2:0:9090"));

    // --help returns CliError with kind == "help"
    let r_help = vm
        .call_obj(&decoded, dsl_fid, &[], &[mk_args(&["--help"])])
        .unwrap();
    let Obj::Str(help_s) = r_help else { panic!("expected Str") };
    assert!(help_s.starts_with("help:"), "got {}", help_s);
    assert!(help_s.contains("Demo CLI"), "got {}", help_s);

    // Unknown flag returns CliError
    let r_err = vm
        .call_obj(&decoded, dsl_fid, &[], &[mk_args(&["--nope"])])
        .unwrap();
    let Obj::Str(err_s) = r_err else { panic!("expected Str") };
    assert!(err_s.starts_with("unknown:"), "got {}", err_s);

    // Struct annotation syntax
    let r_ann = vm
        .call_obj(&decoded, ann_fid, &[], &[mk_args(&["-v", "-p", "4000", "run", "app.cdt", "x", "y"])])
        .unwrap();
    assert_eq!(r_ann, Obj::str("ann:app.cdt:2:1:4000"));

    // cli.help<Tool>()
    let h = vm.call_obj(&decoded, help_fid, &[], &[]).unwrap();
    let Obj::Str(hs) = h else { panic!("expected Str") };
    assert!(hs.contains("db") && hs.contains("run"), "got {}", hs);
}

#[test]
fn cdt_fs_basic_and_streaming() {
    let dir = std::env::temp_dir().join(format!("conduit_fs_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_str = dir.to_string_lossy().replace('\\', "/");

    let src = r#"
        import * as fs from "cdt.fs"

        export func test_basic(base: String) -> String | fs.FsError {
            fs.mkdir_all(base + "/sub")?
            p := base + "/sub/hello.txt"
            fs.write_text(p, "hello")?
            fs.append_text(p, " world")?
            txt := fs.read_text(p)?
            sz := fs.size(p)?

            ap := base + "/atomic.txt"
            fs.write_atomic(ap, "line1\nline2\n")?
            lines := fs.read_lines(ap)?

            cp := base + "/copy.txt"
            fs.copy(ap, cp)?
            rn := base + "/renamed.txt"
            fs.rename(cp, rn)?

            entries := fs.list_dir(base)?
            ex := fs.exists(rn)
            isf := fs.is_file(rn)
            isd := fs.is_dir(base + "/sub")

            each_out := base + "/each_out.txt"
            ew := fs.open_write(each_out)?
            n := fs.each_line(rn, (ln: String) => {
                ew.write(ln + ";")
            })?
            ew.close()
            each_res := fs.read_text(each_out)?
            fs.remove(each_out)?

            fs.remove(rn)?
            after_rm := fs.exists(rn)

            return txt + ":" + sz.to_string() + ":" + lines.len().to_string() + ":" + entries.len().to_string() + ":" + ex.to_string() + ":" + isf.to_string() + ":" + isd.to_string() + ":" + n.to_string() + ":" + each_res + ":" + after_rm.to_string()
        }

        export func test_streaming(base: String) -> String | fs.FsError {
            sp := base + "/stream.txt"
            w := fs.open_write(sp)?
            w.write("a")?
            w.write_line("b")?
            w.write_line("cd")?
            w.flush()?
            w.close()

            wa := fs.open_append(sp)?
            wa.write_line("ef")?
            wa.close()

            r := fs.open_read(sp)?
            out := ""
            while true {
                maybe := r.read_line()
                switch maybe {
                    case nil:
                        break
                    case fs.FsError(e):
                        r.close()
                        return e
                    case String(s):
                        out = out + s + "|"
                }
            }
            r.close()

            r2 := fs.open_read(sp)?
            c1 := r2.read_chunk(3)
            c2 := r2.read_chunk(100)
            c3 := r2.read_chunk(10)
            r2.close()

            s1 := ""
            switch c1 {
                case nil:
                    s1 = "NIL"
                case fs.FsError(e):
                    return e
                case String(s):
                    s1 = s
            }
            s2 := ""
            switch c2 {
                case nil:
                    s2 = "NIL"
                case fs.FsError(e):
                    return e
                case String(s):
                    s2 = s
            }
            s3 := ""
            switch c3 {
                case nil:
                    s3 = "NIL"
                case fs.FsError(e):
                    return e
                case String(s):
                    s3 = s
            }

            return out + "::" + s1.len().to_string() + ":" + s2.len().to_string() + ":" + s3
        }

        export func test_utf8_chunks(base: String) -> String | fs.FsError {
            up := base + "/utf8.txt"
            fs.write_text(up, "Привет🦀мир")?
            r := fs.open_read(up)?
            acc := ""
            while true {
                ch := r.read_chunk(3)
                switch ch {
                    case nil:
                        break
                    case fs.FsError(e):
                        r.close()
                        return e
                    case String(s):
                        acc = acc + s + "|"
                }
            }
            r.close()
            return acc
        }

        export func test_error(base: String) -> String {
            res := fs.read_text(base + "/does_not_exist.txt")
            switch res {
                case String(s):
                    return "unexpected:" + s
                case fs.FsError(e):
                    return e.kind
            }
        }
        "#;

    let module = crate::frontend::compile_source(src).expect("compile cdt.fs test");
    let f_basic_orig = module.func_id("test_basic").unwrap();
    let bytes = crate::ir::bundle::encode_bundle(&module, f_basic_orig, 1).expect("encode");
    let (decoded, _, _) = crate::ir::bundle::decode_bundle(&bytes).expect("decode");

    let mut vm = Vm::new();
    let f_basic = decoded.func_id("test_basic").unwrap();
    let f_stream = decoded.func_id("test_streaming").unwrap();
    let f_utf8 = decoded.func_id("test_utf8_chunks").unwrap();
    let f_err = decoded.func_id("test_error").unwrap();

    let r_basic = vm.call_obj(&decoded, f_basic, &[], &[Obj::str(&dir_str)]).unwrap();
    assert_eq!(
        r_basic,
        Obj::str("hello world:11:2:3:true:true:true:2:line1;line2;:false")
    );

    let r_stream = vm.call_obj(&decoded, f_stream, &[], &[Obj::str(&dir_str)]).unwrap();
    assert_eq!(r_stream, Obj::str("ab|cd|ef|::3:6:NIL"));

    let r_utf8 = vm.call_obj(&decoded, f_utf8, &[], &[Obj::str(&dir_str)]).unwrap();
    assert_eq!(r_utf8, Obj::str("П|р|и|в|е|т|🦀|м|и|р|"));

    let r_err = vm.call_obj(&decoded, f_err, &[], &[Obj::str(&dir_str)]).unwrap();
    assert_eq!(r_err, Obj::str("not_found"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cdt_fs_write_lines_roundtrip_and_closed_writer() {
    let path = std::env::temp_dir().join(format!(
        "conduit_fs_write_lines_test_{}.txt",
        std::process::id()
    ));
    let src = r#"
        import * as fs from "cdt.fs"

        export func write(path: String, lines: [String]) -> Void | fs.FsError {
            w := fs.open_write(path)?
            w.write("prefix:")?
            w.write_lines([])?
            w.write_lines(lines)?
            w.write_line("tail")?
            w.flush()?
            w.close()
        }

        export func append(path: String) -> Void | fs.FsError {
            w := fs.open_append(path)?
            w.write_lines(["appended", ""])?
            w.flush()?
            w.close()
        }

        export func closed(path: String, lines: [String]) -> String | fs.FsError {
            w := fs.open_append(path)?
            w.close()
            switch w.write_lines(lines) {
                case Void(_): return "unexpected success"
                case fs.FsError(e): return e.kind
            }
        }

        export func write_existing(w: fs.Writer, lines: [String]) -> Void | fs.FsError {
            w.write_lines(lines)?
        }

        export func propagate(path: String) -> Void | fs.FsError {
            w := fs.open_append(path)?
            w.close()
            w.write_lines(["must not be written"])?
        }
    "#;
    let module = compile_source(src).expect("compile write_lines");
    let bytes = crate::ir::bundle::encode_bundle(
        &module,
        module.func_id("write").unwrap(),
        crate::ir::bundle::RET_OBJ,
    )
    .unwrap();
    let (module, _, _) = crate::ir::bundle::decode_bundle(&bytes).unwrap();
    let path_obj = Obj::str(path.to_string_lossy().into_owned());
    let lines = vec![
        "hello".to_string(),
        String::new(),
        "Привет 🌍".to_string(),
        "embedded\nnewline\r\n".to_string(),
        "long".repeat(32 * 1024),
    ];
    let values: Vec<Obj> = lines.iter().map(Obj::str).collect();
    let shared = Arc::new(values);
    let lines_obj = Obj::List(ListStorage::Obj(Arc::clone(&shared)));
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(
            &module,
            module.func_id("write").unwrap(),
            &[],
            &[path_obj.clone(), lines_obj.clone()],
        )
        .unwrap(),
        Obj::Empty
    );
    let mut expected = format!("prefix:{}\ntail\n", lines.join("\n"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(shared.as_ref(), &lines.iter().map(Obj::str).collect::<Vec<_>>());

    assert_eq!(
        vm.call_obj(&module, module.func_id("append").unwrap(), &[], &[path_obj.clone()])
            .unwrap(),
        Obj::Empty
    );
    expected.push_str("appended\n\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    for input in [lines_obj, Obj::List(ListStorage::Obj(Arc::new(Vec::new())))] {
        assert_eq!(
            vm.call_obj(
                &module,
                module.func_id("closed").unwrap(),
                &[],
                &[path_obj.clone(), input],
            )
            .unwrap(),
            Obj::str("io")
        );
    }
    let error = vm
        .call_obj(&module, module.func_id("propagate").unwrap(), &[], &[path_obj])
        .unwrap();
    assert!(matches!(error, Obj::Record(ref rec) if rec.schema_id == crate::vm::value::SCHEMA_FS_ERROR));

    // A read-only file descriptor makes the batch hit a real I/O error, even
    // though the writer handle is open. A tiny buffer forces the error now.
    let id = vm.fs_writers.len();
    vm.fs_writers.push(Some(std::io::BufWriter::with_capacity(
        1,
        std::fs::File::open(&path).unwrap(),
    )));
    let error = vm
        .call_obj(
            &module,
            module.func_id("write_existing").unwrap(),
            &[],
            &[
                crate::native::fs::make_writer_handle(id),
                Obj::List(ListStorage::Obj(shared)),
            ],
        )
        .unwrap();
    assert!(matches!(error, Obj::Record(ref rec) if rec.schema_id == crate::vm::value::SCHEMA_FS_ERROR));
    vm.fs_writers[id] = None;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cdt_fs_write_lines_rejects_invalid_arguments() {
    for args in ["", "\"text\"", "[1, 2]", "[\"ok\", 1]", "[\"a\"], [\"b\"]"] {
        let src = format!(
            "import * as fs from \"cdt.fs\"\n\
             func f(w: fs.Writer) -> Void | fs.FsError {{ w.write_lines({args})? }}"
        );
        assert!(check_source(&src).is_err(), "accepted write_lines({args})");
    }
}

#[test]
fn cdt_fs_shared_lines_string_map_json_and_bundle_interop() {
    let path = std::env::temp_dir().join(format!("conduit_shared_lines_{}.txt", std::process::id()));
    std::fs::write(&path, "beta\r\nalpha\n42\n").unwrap();
    let lines = crate::native::fs::read_lines(path.to_str().unwrap());
    std::fs::remove_file(&path).unwrap();
    let src = r#"
        import * as json from "cdt.json"

        export func exercise(lines: [String]) -> String {
            first := lines[0]?
            second := lines[1]?
            m := Map<String, Int>()
            m.set(first, 7)
            m.set("beta", 9)
            m.set("alpha", 11)
            n := m[second]?
            sorted := lines.sort()
            encoded := json.encode(lines)?
            decoded := json.decode<[String]>(encoded)?
            number_text := lines[2]?
            number := number_text.to_int()?
            same := first == "beta"
            joined := sorted.join(",")
            replaced := first.replace("b", "B")
            map_len := m.len()
            value := m[first]?
            roundtrip := decoded == lines
            combined := first + second
            return "{map_len}:{n}:{value}:{same}:{joined}:{roundtrip}:{number}:{replaced}:{combined}"
        }

        export func constant() -> String { return "placeholder" }
    "#;
    let mut module = compile_source(src).unwrap();
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, module.func_id("exercise").unwrap(), &[], &[lines.clone()]).unwrap(),
        Obj::str("2:11:9:true:42,alpha,beta:true:42:Beta:betaalpha")
    );
    let Obj::List(ListStorage::Obj(items)) = &lines else { panic!("expected lines") };
    let retained = items[0].clone();
    drop(lines);
    let constant = module.obj_consts.iter_mut().find(|obj| **obj == Obj::str("placeholder")).unwrap();
    *constant = retained;
    let fid = module.func_id("constant").unwrap();
    let encoded = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&encoded).unwrap();
    assert_eq!(vm.call_obj(&decoded, entry, &[], &[]).unwrap(), Obj::str("beta"));
}
