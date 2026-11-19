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
fn cdt_cli_dispatch_e2e() {
    let src = r#"
import cli from "cdt.cli"
import { CliError } from "cdt.cli"

cli App "Dispatch demo" {
    flag verbose "v" "Verbose output"
    cmd greet "Greet someone" {
        arg name: String "Target name"
    } => on_greet
    cmd ping "Ping" => on_ping
}

func on_greet(app: App, c: App_greet) -> String {
    if app.verbose {
        return "HELLO:" + c.name
    }
    return "hello:" + c.name
}

func on_ping(_c: App_ping) -> String {
    return "pong"
}

export func run_dispatch(args: [String]) -> String | CliError {
    res := cli.dispatch_args<App>(args)?
    switch res {
        case String(s):
            return s
        case _:
            return "none"
    }
}
"#;
    let module = compile_source(src).expect("compile cli dispatch");
    let fid = module.func_id("run_dispatch").unwrap();
    let mk_args = |items: &[&str]| -> Obj {
        Obj::List(crate::vm::value::ListStorage::Obj(std::sync::Arc::new(
            items.iter().map(|s| Obj::str(*s)).collect(),
        )))
    };
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, fid, &[], &[mk_args(&["-v", "greet", "Ada"])]).unwrap(),
        Obj::str("HELLO:Ada")
    );
    assert_eq!(
        vm.call_obj(&module, fid, &[], &[mk_args(&["ping"])]).unwrap(),
        Obj::str("pong")
    );
    let help = vm.call_obj(&module, fid, &[], &[mk_args(&["--help"])]).unwrap();
    let Obj::Record(rec) = help else { panic!("expected CliError") };
    assert_eq!(rec.schema_id, crate::vm::value::SCHEMA_CLI_ERROR);
    assert_eq!(rec.get_obj(0), &Obj::str("help"));
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
    let lines = [
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
        vm.call_obj(&module, module.func_id("append").unwrap(), &[], std::slice::from_ref(&path_obj))
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
    let id = vm.host.fs_writers.len();
    vm.host.fs_writers.push(Some(std::io::BufWriter::with_capacity(
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
    vm.host.fs_writers[id] = None;
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
        vm.call_obj(&module, module.func_id("exercise").unwrap(), &[], std::slice::from_ref(&lines)).unwrap(),
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

#[test]
fn cdt_path_process_and_crypto_e2e() {
    let src = r#"
        import * as path from "cdt.path"
        import * as process from "cdt.process"
        import * as crypto from "cdt.crypto"

        export func test_path() -> String {
            p := path.join("/var/log", "app/server.tar.gz")
            p2 := path.join_all(["a", "b", "..", "c.txt"])
            norm := path.normalize(p2)
            dn := path.dirname(p)
            bn := path.basename(p)
            ex := path.ext(p)
            st := path.stem(p)
            we := path.with_ext("/tmp/data.txt", "json")
            abs_pos := path.is_abs(if_abs_sample())
            abs2 := path.is_abs("rel/x")
            return norm + "|" + dn + "|" + bn + "|" + ex + "|" + st + "|" + we + "|" + abs_pos.to_string() + "|" + abs2.to_string()
        }

        func if_abs_sample() -> String {
            return "__ABS_SAMPLE__"
        }

        export func test_process() -> String | process.ProcessError {
            pid := process.pid()
            cwd := process.cwd()?
            out := process.run("__PROC_CMD__", __PROC_ARGS__)?
            return (pid > 0).to_string() + ":" + (cwd.len() > 0).to_string() + ":" + out.code.to_string() + ":" + out.stdout.trim()
        }

        export func test_crypto() -> String | crypto.CryptoError {
            h := crypto.sha256("abc")
            hm := crypto.hmac_sha256("key", "The quick brown fox jumps over the lazy dog")
            b64 := crypto.base64_encode("Hello, Conduit!")
            b64d := crypto.base64_decode(b64)?
            hx := crypto.hex_encode("AZ")
            hxd := crypto.hex_decode(hx)?
            u := crypto.uuid_v4()
            rh := crypto.random_bytes_hex(8)?
            ri := crypto.random_int(10, 20)?
            return h + "|" + hm + "|" + b64 + "|" + b64d + "|" + hx + "|" + hxd + "|" + u.len().to_string() + "|" + rh.len().to_string() + "|" + (ri >= 10 && ri < 20).to_string()
        }
    "#;

    let src = src
        .replace(
            "__ABS_SAMPLE__",
            if cfg!(windows) { r"C:\\tmp\\x" } else { "/tmp/x" },
        )
        .replace("__PROC_CMD__", if cfg!(windows) { "cmd" } else { "echo" })
        .replace(
            "__PROC_ARGS__",
            if cfg!(windows) {
                r#"["/c", "echo", "hello-proc"]"#
            } else {
                r#"["hello-proc"]"#
            },
        );

    let module = crate::frontend::compile_source(&src).expect("compile path/process/crypto");
    let f_path = module.func_id("test_path").unwrap();
    let f_proc = module.func_id("test_process").unwrap();
    let f_cryp = module.func_id("test_crypto").unwrap();

    let expected_path = if cfg!(windows) {
        r"a\c.txt|/var/log\app|server.tar.gz|gz|server.tar|/tmp/data.json|true|false"
    } else {
        "a/c.txt|/var/log/app|server.tar.gz|gz|server.tar|/tmp/data.json|true|false"
    };

    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, f_path, &[], &[]).unwrap(),
        Obj::str(expected_path)
    );
    assert_eq!(
        vm.call_obj(&module, f_proc, &[], &[]).unwrap(),
        Obj::str("true:true:0:hello-proc")
    );
    assert_eq!(
        vm.call_obj(&module, f_cryp, &[], &[]).unwrap(),
        Obj::str(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad|f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8|SGVsbG8sIENvbmR1aXQh|Hello, Conduit!|415a|AZ|36|16|true"
        )
    );
}

#[test]
fn cdt_url_e2e() {
    let src = r#"
        import * as url from "cdt.url"

        export func test_url() -> String | url.UrlError {
            u := url.parse("https://api.example.com:8443/v1/search?q=hello+world&tag=rust%2Bcdt#top")?
            q := url.decode_query(u.query)?
            enc := url.encode("a b/c")
            dec := url.decode(enc)?
            qs := url.encode_query(Map.from([("b", "2"), ("a", "hello world")]))
            qv := q.get("q") ?? ""
            tv := q.get("tag") ?? ""
            return u.scheme + "|" + u.host + "|" + u.port.to_string() + "|" + u.path + "|" + u.fragment + "|" + qv + "|" + tv + "|" + enc + "|" + dec + "|" + qs
        }
    "#;
    let module = crate::frontend::compile_source(src).expect("compile url");
    let fid = module.func_id("test_url").unwrap();
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, fid, &[], &[]).unwrap(),
        Obj::str("https|api.example.com|8443|/v1/search|top|hello world|rust+cdt|a%20b%2Fc|a b/c|a=hello%20world&b=2")
    );
}

#[test]
fn cdt_csv_e2e() {
    let src = r#"
        import * as csv from "cdt.csv"

        export func test_csv() -> String | csv.CsvError | IndexError {
            raw := "name,note\r\n\"Ada, Lovelace\",\"says \"\"hi\"\"\"\r\nBob,ok\n"
            rows := csv.parse(raw)?
            recs := csv.parse_records(raw)?
            semi := csv.parse_with("a;b\n1;2", ";")?
            out := csv.stringify(rows)
            out_semi := csv.stringify_with(semi, ";")?
            rec0 := recs[0]?
            row1 := rows[1]?
            semi1 := semi[1]?
            ada_note := rec0.get("note") ?? ""
            return rows.len().to_string() + "|" + row1[0]? + "|" + ada_note + "|" + semi1[1]? + "|" + out_semi + "|" + out
        }

        export func test_csv_err() -> String {
            switch csv.parse("\"unclosed") {
                case csv.CsvError(e):
                    return e.kind + ":" + e.line.to_string()
                case _:
                    return "ok"
            }
        }
    "#;
    let module = crate::frontend::compile_source(src).expect("compile csv");
    let fid = module.func_id("test_csv").unwrap();
    let ferr = module.func_id("test_csv_err").unwrap();
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, fid, &[], &[]).unwrap(),
        Obj::str("3|Ada, Lovelace|says \"hi\"|2|a;b\n1;2|name,note\n\"Ada, Lovelace\",\"says \"\"hi\"\"\"\nBob,ok")
    );
    assert_eq!(
        vm.call_obj(&module, ferr, &[], &[]).unwrap(),
        Obj::str("InvalidCsv:1")
    );
}

#[test]
fn cdt_math_and_log_e2e() {
    let src = r#"
        import * as math from "cdt.math"
        import * as log from "cdt.log"

        export func test_math_log() -> String {
            log.set_level("warn")
            log.set_json(true)
            log.debug("suppressed")
            log.warn("active")
            lvl := log.get_level()
            log.set_level("info")
            log.set_json(false)
            h := math.hypot(3.0, 4.0)
            p := math.pow(2.0, 3.0)
            r := math.round(math.pi())
            return h.to_string() + "|" + p.to_string() + "|" + r.to_string() + "|" + lvl
        }
    "#;
    let module = crate::frontend::compile_source(src).expect("compile math/log");
    let fid = module.func_id("test_math_log").unwrap();
    let mut vm = Vm::new();
    assert_eq!(
        vm.call_obj(&module, fid, &[], &[]).unwrap(),
        Obj::str("5|8|3|warn")
    );
}

#[test]
fn cdt_http_router_e2e() {
    let src = r#"
        import * as http from "cdt.http"

        export func test_router() -> String {
            r := http.router()
            prefix := "hi"
            r.get("/users/:id", (req: http.Request) -> http.Response => {
                id := req.params.get("id") ?? ""
                return http.text(prefix + ":" + id)
            })
            r.post("/items", (req: http.Request) -> http.Response => {
                return http.json(req.body)
            })
            r.get("/files/*rest", (req: http.Request) -> http.Response => {
                rest := req.params.get("rest") ?? ""
                return http.response(201, rest)
            })
            r.patch("/items/:id", (req: http.Request) -> http.Response => {
                id := req.params.get("id") ?? ""
                return http.text("patched:" + id)
            })
            r.options("/items/:id", (req: http.Request) -> http.Response => {
                return http.response(204, "")
            })
            r.any("/health", (req: http.Request) -> http.Response => {
                return http.text("ok:" + req.method)
            })
            r.before((req: http.Request) -> http.Response? => {
                if req.path == "/blocked" {
                    return http.response(401, "nope")
                }
                return nil
            })
            r.after((req: http.Request, resp: http.Response) -> http.Response => {
                hdrs := resp.headers
                hdrs.set("x-mw", "1")
                return http.response_with(resp.status, resp.body, hdrs)
            })

            empty := Map.from([("x", "1")])
            empty.remove("x")
            r1 := r.dispatch(http.Request(method: "GET", path: "/users/42", body: "", headers: empty, query: empty, params: empty))
            r2 := r.dispatch(http.Request(method: "POST", path: "/items", body: "{\"ok\":true}", headers: empty, query: empty, params: empty))
            r3 := r.dispatch(http.Request(method: "GET", path: "/files/a/b.txt", body: "", headers: empty, query: empty, params: empty))
            r405 := r.dispatch(http.Request(method: "DELETE", path: "/users/42", body: "", headers: empty, query: empty, params: empty))
            r404 := r.dispatch(http.Request(method: "GET", path: "/missing", body: "", headers: empty, query: empty, params: empty))
            rp := r.dispatch(http.Request(method: "PATCH", path: "/items/9", body: "", headers: empty, query: empty, params: empty))
            ro := r.dispatch(http.Request(method: "OPTIONS", path: "/items/9", body: "", headers: empty, query: empty, params: empty))
            ra := r.dispatch(http.Request(method: "PUT", path: "/health", body: "", headers: empty, query: empty, params: empty))
            rb := r.dispatch(http.Request(method: "GET", path: "/blocked", body: "", headers: empty, query: empty, params: empty))
            mw := rb.headers.get("x-mw") ?? ""
            return r1.status.to_string() + ":" + r1.body + "|" + r2.status.to_string() + ":" + r2.body + "|" + r3.status.to_string() + ":" + r3.body + "|" + r405.status.to_string() + "|" + r404.status.to_string() + "|" + rp.body + "|" + ro.status.to_string() + "|" + ra.body + "|" + rb.status.to_string() + ":" + mw
        }

        func decl_before(req: http.Request) -> http.Response? {
            if req.path == "/deny" {
                return http.response(403, "denied")
            }
            return nil
        }

        func decl_after(_req: http.Request, resp: http.Response) -> http.Response {
            hdrs := resp.headers
            hdrs.set("x-decl", "yes")
            return http.response_with(resp.status, resp.body, hdrs)
        }

        func decl_hello(req: http.Request) -> http.Response {
            name := req.params.get("name") ?? ""
            return http.text("hi:" + name)
        }

        func decl_echo(req: http.Request) -> http.Response {
            return http.text("post:" + req.body)
        }

        router Api "Declarative router" {
            before decl_before
            after decl_after
            get "/hello/:name" => decl_hello
            post "/echo" => decl_echo
        }

        export func test_decl_router() -> String {
            dr := http.build<Api>()
            empty := Map.from([("x", "1")])
            empty.remove("x")
            d1 := dr.dispatch(http.Request(method: "GET", path: "/hello/ada", body: "", headers: empty, query: empty, params: empty))
            d2 := dr.dispatch(http.Request(method: "POST", path: "/echo", body: "42", headers: empty, query: empty, params: empty))
            d3 := dr.dispatch(http.Request(method: "GET", path: "/deny", body: "", headers: empty, query: empty, params: empty))
            tag := d1.headers.get("x-decl") ?? ""
            return d1.body + "|" + d2.body + "|" + d3.status.to_string() + ":" + d3.body + "|" + tag
        }
    "#;
    let module = crate::frontend::compile_source(src).expect("compile http");
    let fid = module.func_id("test_router").unwrap();
    let decl_fid = module.func_id("test_decl_router").unwrap();
    let mut vm = Vm::new();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("200:hi:42|200:{\"ok\":true}|201:a/b.txt|405|404|patched:9|204|ok:PUT|401:1"));
    let decl_res = vm.call_obj(&module, decl_fid, &[], &[]).unwrap();
    assert_eq!(decl_res, Obj::str("hi:ada|post:42|403:denied|yes"));
}

#[test]
fn cdt_http_client_loopback_e2e() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for idx in 0..7 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            match idx {
                0 => {
                    assert!(req.starts_with("GET /hello HTTP/1.1\r\n"), "req0: {req}");
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Test: yes\r\n\r\nworld")
                        .unwrap();
                }
                1 => {
                    assert!(req.starts_with("POST /echo HTTP/1.1\r\n"), "req1: {req}");
                    assert!(req.ends_with("\r\n\r\nping"), "req1 body: {req}");
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\npong\r\n0\r\n\r\n",
                        )
                        .unwrap();
                }
                2 => {
                    assert!(req.starts_with("PUT /custom HTTP/1.1\r\n"), "req2: {req}");
                    assert!(req.contains("X-Auth: secret\r\n"), "req2 hdr: {req}");
                    assert!(req.ends_with("\r\n\r\ndata"), "req2 body: {req}");
                    stream
                        .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\n\r\ncreated")
                        .unwrap();
                }
                3 => {
                    assert!(req.starts_with("GET /redir HTTP/1.1\r\n"), "req3: {req}");
                    stream
                        .write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                }
                4 => {
                    assert!(req.starts_with("GET /final HTTP/1.1\r\n"), "req4: {req}");
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nfound")
                        .unwrap();
                }
                5 => {
                    assert!(req.starts_with("GET /bin HTTP/1.1\r\n"), "req5: {req}");
                    let mut resp = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n".to_vec();
                    resp.extend_from_slice(&[0u8, 255, 128, 1]);
                    stream.write_all(&resp).unwrap();
                }
                6 => {
                    assert!(req.starts_with("POST /bin-echo HTTP/1.1\r\n"), "req6: {req}");
                    assert!(buf[..n].ends_with(&[10u8, 250, 42]), "req6 bin body");
                    let mut resp = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n".to_vec();
                    resp.extend_from_slice(&[10u8, 250, 42]);
                    stream.write_all(&resp).unwrap();
                }
                _ => unreachable!(),
            }
        }
    });

    let src = format!(
        r#"
        import * as http from "cdt.http"

        export func test_client() -> String | http.HttpError | IndexError {{
            r1 := http.get("http://127.0.0.1:{port}/hello")?
            r2 := http.post("http://127.0.0.1:{port}/echo", "ping")?
            hdrs := Map.from([("X-Auth", "secret")])
            r3 := http.request("PUT", "http://127.0.0.1:{port}/custom", "data", hdrs)?
            r4 := http.request_timeout("GET", "http://127.0.0.1:{port}/redir", "", Map.from([("X-A", "1")]), 5000)?
            rw := http.response_with(202, "ok", Map.from([("x-custom", "yes")]))
            rw_hdr := rw.headers.get("x-custom") ?? ""
            xt := r1.headers.get("x-test") ?? ""
            b1 := http.get_bytes("http://127.0.0.1:{port}/bin")?
            b2 := http.request_bytes("POST", "http://127.0.0.1:{port}/bin-echo", [10, 250, 42], Map.from([("Content-Type", "application/octet-stream")]))?
            b1_1 := b1.body[1]?
            b2_1 := b2.body[1]?
            return r1.status.to_string() + ":" + r1.body + ":" + xt + "|" + r2.status.to_string() + ":" + r2.body + "|" + r3.status.to_string() + ":" + r3.body + "|" + r4.status.to_string() + ":" + r4.body + "|" + rw.status.to_string() + ":" + rw_hdr + "|" + b1.body.len().to_string() + ":" + b1_1.to_string() + "|" + b2_1.to_string()
        }}

        export func test_bad_url() -> String | http.HttpError {{
            _r := http.get("ftp://invalid")?
            return "unexpected"
        }}

        export func test_bad_timeout() -> String | http.HttpError {{
            _r := http.request_timeout("GET", "http://127.0.0.1:{port}/hello", "", Map.from([("X-A", "1")]), 0)?
            return "unexpected"
        }}
        "#
    );

    let module = crate::frontend::compile_source(&src).expect("compile http client");
    let fid = module.func_id("test_client").unwrap();
    let bad_fid = module.func_id("test_bad_url").unwrap();
    let bad_to_fid = module.func_id("test_bad_timeout").unwrap();
    let mut vm = Vm::new();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("200:world:yes|200:pong|201:created|200:found|202:yes|4:255|250"));
    handle.join().unwrap();
    let bad_res = vm.call_obj(&module, bad_fid, &[], &[]).unwrap();
    let Obj::Record(rec) = bad_res else {
        panic!("expected HttpError record, got {bad_res:?}");
    };
    assert_eq!(rec.schema_id, crate::vm::value::SCHEMA_HTTP_ERROR);
    assert_eq!(rec.get_obj(0), &Obj::str("invalid_url"));
    let bad_to = vm.call_obj(&module, bad_to_fid, &[], &[]).unwrap();
    let Obj::Record(rec_to) = bad_to else {
        panic!("expected HttpError record, got {bad_to:?}");
    };
    assert_eq!(rec_to.schema_id, crate::vm::value::SCHEMA_HTTP_ERROR);
    assert_eq!(rec_to.get_obj(0), &Obj::str("invalid_timeout"));
}

#[test]
fn cdt_http_server_graceful_shutdown_e2e() {
    use std::net::TcpListener;

    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };

    let server_src = format!(
        r#"
        import * as http from "cdt.http"

        export func run_server() -> Void | http.HttpError {{
            r := http.router()
            r.get("/hello", (req: http.Request) -> http.Response => {{
                return http.text("hi")
            }})
            r.post("/stop", (req: http.Request) -> http.Response => {{
                r.stop()
                return http.text("stopping")
            }})
            return r.serve("127.0.0.1:{port}")
        }}
        "#
    );

    let client_src = format!(
        r#"
        import * as http from "cdt.http"

        export func hit_hello() -> String | http.HttpError {{
            r := http.get("http://127.0.0.1:{port}/hello")?
            return r.status.to_string() + ":" + r.body
        }}

        export func hit_stop() -> String | http.HttpError {{
            r := http.post("http://127.0.0.1:{port}/stop", "")?
            return r.status.to_string() + ":" + r.body
        }}
        "#
    );

    let srv_mod = crate::frontend::compile_source(&server_src).expect("compile server");
    let cli_mod = crate::frontend::compile_source(&client_src).expect("compile client");
    let srv_fid = srv_mod.func_id("run_server").unwrap();
    let hello_fid = cli_mod.func_id("hit_hello").unwrap();
    let stop_fid = cli_mod.func_id("hit_stop").unwrap();

    let srv_handle = std::thread::spawn(move || Vm::new().call_obj(&srv_mod, srv_fid, &[], &[]).unwrap());

    let mut client_vm = Vm::new();
    let mut hello_res = Obj::Empty;
    for _ in 0..50 {
        let res = client_vm.call_obj(&cli_mod, hello_fid, &[], &[]).unwrap();
        if matches!(res, Obj::Str(_)) {
            hello_res = res;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(hello_res, Obj::str("200:hi"));

    let stop_res = client_vm.call_obj(&cli_mod, stop_fid, &[], &[]).unwrap();
    assert_eq!(stop_res, Obj::str("200:stopping"));

    let srv_ret = srv_handle.join().unwrap();
    assert_eq!(srv_ret, Obj::Empty);
}

#[test]
fn cdt_db_migrations_bundle_and_crypto_passwords_e2e() {
    use crate::ir::bundle::{decode_bundle, encode_bundle};

    let src = r#"
    import * as db from "cdt.db"
    import * as crypto from "cdt.crypto"

    migration 1 "init" {
        up {
            db.exec("CREATE TABLE items (id SERIAL PRIMARY KEY, label TEXT NOT NULL)")?
        }
        down {
            db.exec("DROP TABLE items")?
        }
    }

    migration 2 "seed" {
        up "INSERT INTO items (id, label) VALUES (1, 'first')"
        down "DELETE FROM items WHERE id = 1"
    }

    export func main() -> String | crypto.CryptoError {
        pw_hash := crypto.hash_password("hunter2")?
        ok1 := crypto.verify_password("hunter2", pw_hash)
        ok2 := crypto.verify_password("wrong", pw_hash)
        eq := crypto.constant_time_eq("abc", "abc")
        return "{ok1}:{ok2}:{eq}"
    }

    export func bad_url() -> String | db.DbError {
        _conn := db.connect("sqlite::memory:")?
        return "unreachable"
    }
    "#;
    let (mut prog, checked) = crate::frontend::check_source(src).unwrap();
    let module = crate::ir::lower::compile_program(&prog, &checked).unwrap();
    module.validate().unwrap();
    let main_fid = checked.func_by_name["main"];
    let bytes = encode_bundle(&module, main_fid, 1).unwrap();
    let (decoded_mod, _, _) = decode_bundle(&bytes).unwrap();
    assert_eq!(decoded_mod.migrations.len(), 2);
    assert_eq!(decoded_mod.migrations[0].version, 1);
    assert_eq!(decoded_mod.migrations[1].version, 2);

    let mut vm = Vm::new();
    let out = vm
        .call_obj(&decoded_mod, decoded_mod.func_id("main").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(out, Obj::str("true:false:true"));

    let bad = vm
        .call_obj(&decoded_mod, decoded_mod.func_id("bad_url").unwrap(), &[], &[])
        .unwrap();
    let Obj::Record(rec) = bad else {
        panic!("expected DbError record, got {bad:?}");
    };
    assert_eq!(rec.schema_id, crate::vm::value::SCHEMA_DB_ERROR);
    let _ = &mut prog;
}

#[test]
fn cdt_db_model_schemas_ddl_and_bundle_e2e() {
    use crate::ir::bundle::{decode_bundle, encode_bundle};

    let src = r#"
    import * as db from "cdt.db"

    @db.table("users")
    struct User {
        id: Int @db.pk @db.auto
        email: String @db.unique @db.index
        org_id: Int @db.fk("orgs.id")
        active: Bool @db.default("true") = true
        bio: String? @db.col("bio_text")
    }

    migration 1 "create_users" {
        up { db.create_table<User>()? }
        down { db.drop_table<User>()? }
    }

    export func main() -> String {
        pg := db.ddl<User>("postgres")
        my := db.ddl<User>("mysql")
        return "{pg}|{my}"
    }
    "#;
    let (prog, checked) = crate::frontend::check_source(src).unwrap();
    let module = crate::ir::lower::compile_program(&prog, &checked).unwrap();
    module.validate().unwrap();
    let main_fid = checked.func_by_name["main"];
    let bytes = encode_bundle(&module, main_fid, 1).unwrap();
    let (decoded_mod, _, _) = decode_bundle(&bytes).unwrap();
    assert_eq!(decoded_mod.db_models.len(), 1);
    assert_eq!(decoded_mod.db_models[0].table_name, "users");

    let mut vm = Vm::new();
    let out = vm
        .call_obj(&decoded_mod, decoded_mod.func_id("main").unwrap(), &[], &[])
        .unwrap();
    let Obj::Str(s) = out else {
        panic!("expected String, got {out:?}");
    };
    let s = s.as_str();
    assert!(s.contains("CREATE TABLE IF NOT EXISTS users (id BIGSERIAL PRIMARY KEY, email TEXT NOT NULL UNIQUE, org_id BIGINT NOT NULL REFERENCES orgs(id), active BOOLEAN NOT NULL DEFAULT true, bio_text TEXT);"), "got:\n{s}");
    assert!(s.contains("CREATE INDEX IF NOT EXISTS idx_users_email ON users (email)"));
    assert!(s.contains("CREATE TABLE IF NOT EXISTS users (id BIGINT AUTO_INCREMENT PRIMARY KEY, email VARCHAR(255) NOT NULL UNIQUE,"));
}

#[test]
fn cdt_cron_schedule_interval_and_bundle_e2e() {
    use crate::ir::bundle::{decode_bundle, encode_bundle};

    let src = r#"
    import * as cron from "cdt.cron"

    schedule Jobs {
        every "*/5 * * * *" => tick_cron
        interval 20ms => tick_fast
    }

    func tick_cron() -> Void {
        return
    }

    func tick_fast() -> Void {
        return
    }

    export func main() -> String | cron.CronError {
        v1 := cron.validate("*/5 * * * *")
        v2 := cron.validate("bad")
        cron.run_once<Jobs>("tick_cron")?
        sched := cron.start<Jobs>()?
        r1 := sched.is_running()
        sched.stop()
        r2 := sched.is_running()
        return "{v1}:{v2}:{r1}:{r2}"
    }
    "#;
    let (prog, checked) = crate::frontend::check_source(src).unwrap();
    let module = crate::ir::lower::compile_program(&prog, &checked).unwrap();
    module.validate().unwrap();
    let main_fid = checked.func_by_name["main"];
    let bytes = encode_bundle(&module, main_fid, 1).unwrap();
    let (decoded_mod, _, _) = decode_bundle(&bytes).unwrap();
    assert_eq!(decoded_mod.schedules.len(), 1);
    assert_eq!(decoded_mod.schedules[0].jobs.len(), 2);

    let mut vm = Vm::new();
    let out = vm
        .call_obj(&decoded_mod, decoded_mod.func_id("main").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(out, Obj::str("true:false:true:false"));
}

#[test]
fn cdt_http_typed_contract_validation_e2e() {
    let src = r#"
    import * as http from "cdt.http"
    import * as validation from "cdt.validation"

    [#validator]
    func no_admin(v: String) -> Void | validation.Issue {
        if v == "admin" {
            return validation.Issue(code: "reserved", message: "reserved name")
        }
    }

    struct CreateUser {
        email: String @validation.email
        name: String @validation.min(2) @no_admin
    }

    func create_user(req: http.Request, body: CreateUser) -> http.Response {
        email := body.email
        name := body.name
        return http.response(201, "{name}<{email}>")
    }

    router Api {
        post "/users" => create_user
    }

    export func main() -> String {
        r := http.build<Api>()
        r_ok := r.dispatch(http.Request(method: "POST", path: "/users", body: "{\"email\":\"alice@example.com\",\"name\":\"Alice\"}", headers: Map<String, String>(), query: Map<String, String>(), params: Map<String, String>()))
        r_bad_json := r.dispatch(http.Request(method: "POST", path: "/users", body: "{broken", headers: Map<String, String>(), query: Map<String, String>(), params: Map<String, String>()))
        r_bad_val := r.dispatch(http.Request(method: "POST", path: "/users", body: "{\"email\":\"bad\",\"name\":\"admin\"}", headers: Map<String, String>(), query: Map<String, String>(), params: Map<String, String>()))

        s1 := r_ok.status
        b1 := r_ok.body
        s2 := r_bad_json.status
        b2 := r_bad_json.body.contains("invalid_json")
        s3 := r_bad_val.status
        b3 := r_bad_val.body.contains("validation_failed") && r_bad_val.body.contains("reserved")
        return "{s1}:{b1}|{s2}:{b2}|{s3}:{b3}"
    }
    "#;
    let module = compile_source(src).unwrap();
    let mut vm = Vm::new();
    let out = vm
        .call_obj(&module, module.func_id("main").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(
        out,
        Obj::str("201:Alice<alice@example.com>|400:true|422:true")
    );
}

#[test]
fn cdt_http_groups_static_cors_helpers_e2e() {
    let tmp = std::env::temp_dir().join(format!("conduit_static_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("hello.txt"), "static-hello").unwrap();
    let dir_str = tmp.to_str().unwrap().replace('\\', "/");

    let src = format!(
        r#"
    import * as http from "cdt.http"

    struct Item {{
        id: String
        q: String
    }}

    func health(_req: http.Request) -> http.Response {{
        return http.no_content()
    }}

    func require_auth(req: http.Request) -> http.Response? {{
        tok := req.header("Authorization") ?? ""
        if tok != "Bearer ok" {{
            return http.status_json(401, Item(id: "0", q: "unauth"))
        }}
        return nil
    }}

    func get_item(req: http.Request) -> http.Response {{
        id := req.param("id") ?? ""
        q := req.query_param("q") ?? ""
        return http.ok_json(Item(id: id, q: q))
    }}

    func go_new(_req: http.Request) -> http.Response {{
        return http.redirect("/new", 301)
    }}

    router Api {{
        cors "*"
        static "/assets" => "{dir_str}"
        get "/health" => health
        get "/old" => go_new
        group "/api/v1" {{
            before require_auth
            get "/items/:id" => get_item
        }}
    }}

    export func main() -> String {{
        r := http.build<Api>()
        empty := Map<String, String>()

        r_health := r.dispatch(http.Request(method: "GET", path: "/health", body: "", headers: empty, query: empty, params: empty))
        r_unauth := r.dispatch(http.Request(method: "GET", path: "/api/v1/items/42", body: "", headers: empty, query: empty, params: empty))

        auth_h := Map<String, String>()
        auth_h.set("authorization", "Bearer ok")
        q_map := Map<String, String>()
        q_map.set("q", "fast")
        r_ok := r.dispatch(http.Request(method: "GET", path: "/api/v1/items/42", body: "", headers: auth_h, query: q_map, params: empty))

        r_opt := r.dispatch(http.Request(method: "OPTIONS", path: "/api/v1/items/42", body: "", headers: empty, query: empty, params: empty))
        r_redir := r.dispatch(http.Request(method: "GET", path: "/old", body: "", headers: empty, query: empty, params: empty))
        r_static := r.dispatch(http.Request(method: "GET", path: "/assets/hello.txt", body: "", headers: empty, query: empty, params: empty))
        r_trav := r.dispatch(http.Request(method: "GET", path: "/assets/../hello.txt", body: "", headers: empty, query: empty, params: empty))

        s1 := r_health.status
        c1 := r_health.headers.get("access-control-allow-origin") ?? ""
        s2 := r_unauth.status
        s3 := r_ok.status
        b3 := r_ok.body
        s4 := r_opt.status
        s5 := r_redir.status
        loc := r_redir.headers.get("location") ?? ""
        s6 := r_static.status
        b6 := r_static.body
        s7 := r_trav.status
        return "{{s1}}:{{c1}}|{{s2}}|{{s3}}:{{b3}}|{{s4}}|{{s5}}:{{loc}}|{{s6}}:{{b6}}|{{s7}}"
    }}
    "#
    );
    let module = compile_source(&src).unwrap();
    let mut vm = Vm::new();
    let out = vm
        .call_obj(&module, module.func_id("main").unwrap(), &[], &[])
        .unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(
        out,
        Obj::str(r#"204:*|401|200:{"id":"42","q":"fast"}|204|301:/new|200:static-hello|404"#)
    );
}
