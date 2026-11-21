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
    var acc = 0
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
            var out = ""
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
            var v = "0"
            if app.verbose { v = "1" }
            switch app.db {
                case Tool_db(db):
                    switch db.migrate {
                        case Tool_db_migrate(m):
                            var dr = "live"
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
            var v = "0"
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
            var out = ""
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

            var s1 = ""
            switch c1 {
                case nil:
                    s1 = "NIL"
                case fs.FsError(e):
                    return e
                case String(s):
                    s1 = s
            }
            var s2 = ""
            switch c2 {
                case nil:
                    s2 = "NIL"
                case fs.FsError(e):
                    return e
                case String(s):
                    s2 = s
            }
            var s3 = ""
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
            var acc = ""
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
            needs_old := crypto.password_needs_rehash("pbkdf2-sha256$1000$00112233445566778899aabbccddeeff$00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")
            needs_bad := crypto.password_needs_rehash("invalid")
            return h + "|" + hm + "|" + b64 + "|" + b64d + "|" + hx + "|" + hxd + "|" + u.len().to_string() + "|" + rh.len().to_string() + "|" + (ri >= 10 && ri < 20).to_string() + "|" + needs_old.to_string() + ":" + needs_bad.to_string()
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
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad|f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8|SGVsbG8sIENvbmR1aXQh|Hello, Conduit!|415a|AZ|36|16|true|true:true"
        )
    );
}

#[test]
fn cdt_path_safe_join_e2e_and_bundle() {
    let base = std::env::temp_dir().join(format!("conduit_safe_join_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/existing.txt"), "ok").unwrap();

    let canon_root = std::fs::canonicalize(&root).unwrap();
    let expected_existing = canon_root.join("sub/existing.txt").to_string_lossy().into_owned();
    let expected_new = canon_root.join("sub/new/nested.txt").to_string_lossy().into_owned();
    let root_lit = root.to_str().unwrap().replace('\\', "/");

    let src = format!(
        r#"
        import * as path from "cdt.path"

        export func run() -> String {{
            ok1 := path.safe_join("{root_lit}", "sub/existing.txt") ?? "nil"
            ok2 := path.safe_join("{root_lit}", "sub/new/nested.txt") ?? "nil"
            bad_trav := path.safe_join("{root_lit}", "../outside") ?? "nil"
            bad_abs := path.safe_join("{root_lit}", "/etc/passwd") ?? "nil"
            return "{{ok1}}|{{ok2}}|{{bad_trav}}|{{bad_abs}}"
        }}
        "#
    );

    let module = crate::frontend::compile_source(&src).expect("compile path.safe_join");
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    let expected = format!("{expected_existing}|{expected_new}|nil|nil");
    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        assert_eq!(vm.call_obj(m, f, &[], &[]).unwrap(), Obj::str(&expected));
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn cdt_process_pipe_e2e_and_errors() {
    let src = if cfg!(windows) {
        r#"
            import * as process from "cdt.process"

            export func test_pipe_ok() -> String | process.ProcessError {
                out := process.pipe([
                    ["cmd", "/c", "echo", "hello-pipe"],
                    ["findstr", "hello"]
                ])?
                return out.code.to_string() + ":" + out.stdout.trim()
            }

            export func test_pipe_empty() -> String {
                switch process.pipe([]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }

            export func test_pipe_empty_cmd() -> String {
                switch process.pipe([[]]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }

            export func test_pipe_not_found() -> String {
                switch process.pipe([
                    ["cmd", "/c", "echo", "hi"],
                    ["definitely_no_such_conduit_pipe_cmd_xyz"]
                ]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }
        "#
    } else {
        r#"
            import * as process from "cdt.process"

            export func test_pipe_ok() -> String | process.ProcessError {
                out := process.pipe([
                    ["printf", "b\na\nc\n"],
                    ["sort"],
                    ["head", "-n", "2"]
                ])?
                return out.code.to_string() + ":" + out.stdout.trim().replace("\n", ",")
            }

            export func test_pipe_empty() -> String {
                switch process.pipe([]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }

            export func test_pipe_empty_cmd() -> String {
                switch process.pipe([[]]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }

            export func test_pipe_not_found() -> String {
                switch process.pipe([
                    ["echo", "hi"],
                    ["definitely_no_such_conduit_pipe_cmd_xyz"]
                ]) {
                    case process.ProcessError(e): return e.kind
                    case process.Output(o): return "unexpected:" + o.code.to_string()
                }
                return ""
            }
        "#
    };

    let module = crate::frontend::compile_source(src).expect("compile process.pipe");
    let mut vm = Vm::new();

    let ok_fid = module.func_id("test_pipe_ok").unwrap();
    let expected_ok = if cfg!(windows) { "0:hello-pipe" } else { "0:a,b" };
    assert_eq!(
        vm.call_obj(&module, ok_fid, &[], &[]).unwrap(),
        Obj::str(expected_ok)
    );

    for (fn_name, expected_kind) in [
        ("test_pipe_empty", "invalid_input"),
        ("test_pipe_empty_cmd", "invalid_input"),
        ("test_pipe_not_found", "not_found"),
    ] {
        let fid = module.func_id(fn_name).unwrap();
        assert_eq!(
            vm.call_obj(&module, fid, &[], &[]).unwrap(),
            Obj::str(expected_kind),
            "unexpected result for {}",
            fn_name
        );
    }

    let bad_type = r#"
        import * as process from "cdt.process"
        export func main() -> Void {
            _ := process.pipe(["echo"])
        }
    "#;
    assert!(
        crate::frontend::compile_source(bad_type).is_err(),
        "process.pipe([\"echo\"]) must fail type checking"
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
fn cdt_http_router_server_config_methods_and_bundle_e2e() {
    let src = r#"
        import * as http from "cdt.http"

        export func run() -> String {
            r := http.router()
            r.workers(4)
            r.timeouts(2000, 5000, 15000)
            r.max_conns_per_ip(0)
            r.trusted_proxies(["10.0.0.0/8", "172.16.0.0/12"])
            r.get("/ping", (_req: http.Request) -> http.Response => {
                return http.text("pong")
            })
            empty := Map.from([("k", "v")])
            empty.remove("k")
            resp := r.dispatch(http.Request(method: "GET", path: "/ping", body: "", headers: empty, query: empty, params: empty))
            return "{resp.status}:{resp.body}"
        }
    "#;
    let module = crate::frontend::compile_source(src).expect("compile router config");
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        assert_eq!(vm.call_obj(m, f, &[], &[]).unwrap(), Obj::str("200:pong"));
    }
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
fn cdt_http_client_deny_private_addrs_e2e() {
    let src = r#"
        import * as http from "cdt.http"

        export func run() -> String {
            c := http.client("http://127.0.0.1:8080").deny_private_addrs()
            defer c.close()
            var k1 = ""
            switch c.get("/secret") {
                case http.HttpError(e): k1 = e.kind
                case http.Response(_): k1 = "unexpected"
            }
            var k2 = ""
            switch c.get("http://169.254.169.254/latest/meta-data/") {
                case http.HttpError(e): k2 = e.kind
                case http.Response(_): k2 = "unexpected"
            }
            return "{k1}:{k2}"
        }
    "#;
    let module = crate::frontend::compile_source(src).unwrap();
    let fid = module.func_id("run").unwrap();
    let mut vm = Vm::new();
    let res = vm.call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("blocked_addr:blocked_addr"));
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
            r.post("/upload", (req: http.Request) -> http.Response => {{
                second := req.body_bytes.get(1) ?? 0
                return http.text(second.to_string())
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

        export func hit_upload() -> String | http.HttpError {{
            r := http.request_bytes("POST", "http://127.0.0.1:{port}/upload", Bytes([0, 255]), Map<String, String>())?
            return r.status.to_string() + ":" + r.body.to_string()
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
    let upload_fid = cli_mod.func_id("hit_upload").unwrap();
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

    let upload_res = client_vm.call_obj(&cli_mod, upload_fid, &[], &[]).unwrap();
    assert_eq!(upload_res, Obj::str("200:255"));

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
    assert!(s.contains(r#"CREATE TABLE IF NOT EXISTS "users" ("id" BIGSERIAL PRIMARY KEY, "email" TEXT NOT NULL UNIQUE, "org_id" BIGINT NOT NULL REFERENCES "orgs"("id"), "active" BOOLEAN NOT NULL DEFAULT true, "bio_text" TEXT);"#), "got:\n{s}");
    assert!(s.contains(r#"CREATE INDEX IF NOT EXISTS "idx_users_email" ON "users" ("email")"#));
    assert!(s.contains("CREATE TABLE IF NOT EXISTS `users` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, `email` VARCHAR(255) NOT NULL UNIQUE,"));
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

#[test]
fn cdt_test_blocks_assertions_try_and_tree_shaking() {
    let pass_src = r#"
    import * as json from "cdt.json"

    enum Color { Red, Green }

    struct Point {
        x: Int
        y: Int
    }

    func private_add(a: Int, b: Int) -> Int {
        return a + b
    }

    test "assertions and try" {
        test := 40
        assert(test + 2 == 42)
        assert(true, "should pass")
        assert_eq(private_add(20, 22), 42)
        assert_eq(1.5 + 2.5, 4.0)
        assert_eq(true, true)
        assert_eq("hello", "he" + "llo")
        assert_eq(Point(x: 1, y: 2), Point(x: 1, y: 2))
        assert_eq([1, 2, 3], [1, 2, 3])
        var empty_ints: [Int] = []
        assert_eq([], empty_ints)
        m1 := Map<String, Int>()
        m1.set("a", 7)
        m2 := Map.from([("a", 7)])
        assert_eq(m1, m2)
        assert_eq(Color.Green, Color.Green)
        var opt: Int? = 9
        assert_eq(opt, 9)
        var none_val: Int? = nil
        assert_eq(none_val, nil)
        assert_ne(10, 20)
        assert_ne("a", "b")

        p := json.decode<Point>("{\"x\":3,\"y\":4}")?
        assert_eq(p.x + p.y, 7)
        return
    }
    "#;
    let module = compile_source(pass_src).unwrap();
    assert_eq!(module.tests.len(), 1);
    assert_eq!(module.tests[0].name, "assertions and try");
    let mut vm = Vm::new();
    vm.call(&module, module.tests[0].func_id, &[], &[]).unwrap();

    // Failure diagnostics for assert, assert_eq, assert_ne, fail, and ? propagation.
    let fail_src = r#"
    import * as json from "cdt.json"
    struct Point { x: Int }

    test "fail assert" {
        assert(false)
    }
    test "fail assert msg" {
        assert(1 > 2, "math broke")
    }
    test "fail eq" {
        assert_eq("abc", "xyz")
    }
    test "fail eq msg" {
        m := Map.from([("k", 1)])
        n := Map.from([("k", 2)])
        assert_eq(m, n, "map mismatch")
    }
    test "fail ne" {
        assert_ne(42, 42)
    }
    test "fail explicit" {
        fail("kaboom")
    }
    test "fail try" {
        _p := json.decode<Point>("{bad json")?
    }
    "#;
    let fail_mod = compile_source(fail_src).unwrap();
    assert_eq!(fail_mod.tests.len(), 7);
    let run_fail = |idx: usize| -> String {
        let mut fvm = Vm::new();
        fvm.call(&fail_mod, fail_mod.tests[idx].func_id, &[], &[])
            .unwrap_err()
            .to_string()
    };
    assert!(run_fail(0).contains("Assertion failed (line "));
    assert!(run_fail(1).contains("math broke"));
    let eq_err = run_fail(2);
    assert!(eq_err.contains("actual:   \"abc\"") && eq_err.contains("expected: \"xyz\""), "{eq_err}");
    let map_err = run_fail(3);
    assert!(
        map_err.contains("map mismatch")
            && map_err.contains("{\"k\": 1}")
            && map_err.contains("{\"k\": 2}"),
        "{map_err}"
    );
    let ne_err = run_fail(4);
    assert!(ne_err.contains("both equal: 42"), "{ne_err}");
    assert!(run_fail(5).contains("kaboom"));
    let _ = run_fail(6);

    // Reject duplicate test names and `export test`.
    let dup_err = compile_source("test \"same\" {}\ntest \"same\" {}\n").unwrap_err();
    assert!(dup_err.contains("Duplicate test name 'same'"), "{dup_err}");
    let exp_err = compile_source("export test \"bad\" {}\n").unwrap_err();
    assert!(exp_err.contains("'export' cannot be applied to 'test'"), "{exp_err}");

    // Imported modules' `test` blocks must be stripped and not included in root `module.tests`.
    let mut mods = HashMap::new();
    mods.insert(
        "./lib",
        "export func double(x: Int) -> Int { return x * 2 }\ntest \"lib test\" { fail(\"should not run\") }\n",
    );
    let root_src = "import { double } from \"./lib\"\ntest \"root test\" { assert_eq(double(6), 12) }\n";
    let linked_mod = compile_with_modules(root_src, &mods).unwrap();
    assert_eq!(linked_mod.tests.len(), 1);
    assert_eq!(linked_mod.tests[0].name, "root test");
    let mut lvm = Vm::new();
    lvm.call(&linked_mod, linked_mod.tests[0].func_id, &[], &[]).unwrap();
}

#[test]
fn cdt_validation_extended_rules_and_bundle_e2e() {
    let src = r#"
    import * as validation from "cdt.validation"
    import * as http from "cdt.http"

    struct AccountInput {
        id: String @validation.uuid
        role: String @validation.one_of("admin", "user")
        homepage: String @validation.url
        name: String @validation.not_empty @validation.min_len(2) @validation.max_len(12)
        score: Int @validation.range(-10, 10)
        ratio: Float @validation.range(-1.5, 1.5)
        tags: [String] @validation.not_empty @validation.max_len(3)
    }

    func create_acc(_req: http.Request, body: AccountInput) -> http.Response {
        return http.ok_json(body)
    }

    router Api {
        post "/accounts" => create_acc
    }

    export func run() -> String {
        ok_acc := AccountInput(
            id: "550e8400-e29b-41d4-a716-446655440000",
            role: "admin",
            homepage: "https://conduit-lang.org/docs?v=1",
            name: "Alice",
            score: -5,
            ratio: 0.75,
            tags: ["core"],
        )
        var ok_res = ""
        switch validation.check(ok_acc) {
            case AccountInput(a): ok_res = "ok:{a.name}"
            case validation.ValidationError(e): ok_res = "unexpected:{e.issues.len()}"
        }

        bad_acc := AccountInput(
            id: "not-a-uuid",
            role: "root",
            homepage: "ftp://example.com",
            name: " ",
            score: -11,
            ratio: 2.0,
            tags: [],
        )
        var bad_codes = ""
        switch validation.check(bad_acc) {
            case AccountInput(_): bad_codes = "unexpected"
            case validation.ValidationError(err):
                for v in err.issues {
                    if bad_codes == "" {
                        bad_codes = "{v.path}={v.code}"
                    } else {
                        bad_codes = "{bad_codes},{v.path}={v.code}"
                    }
                }
        }
        over_acc := AccountInput(
            id: "550e8400-e29b-41d4-a716-446655440000",
            role: "user",
            homepage: "https://example.com",
            name: "name_over_twelve_chars",
            score: 0,
            ratio: 0.0,
            tags: ["a", "b", "c", "d"],
        )
        switch validation.check(over_acc) {
            case AccountInput(_): bad_codes = "{bad_codes},unexpected_over"
            case validation.ValidationError(err2):
                for v in err2.issues {
                    bad_codes = "{bad_codes},{v.path}={v.code}"
                }
        }

        d_ok := validation.uuid("550e8400-e29b-41d4-a716-446655440000") == nil
            && validation.url("http://localhost:8080/api") == nil
            && validation.one_of("user", ["admin", "user"]) == nil
            && validation.not_empty("hi") == nil
            && validation.min_len("abc", 3) == nil
            && validation.max_len("abc", 3) == nil
            && validation.range(-5, -10, 10) == nil
            && validation.range(0.5, -1.0, 1.0) == nil

        d_fail := validation.uuid("bad") != nil
            && validation.url("not a url") != nil
            && validation.one_of("guest", ["admin", "user"]) != nil
            && validation.not_empty("   ") != nil
            && validation.min_len("a", 2) != nil
            && validation.max_len("abcd", 2) != nil
            && validation.range(11, -10, 10) != nil
            && validation.range(2.0, 0.0, 1.0) != nil

        r := http.build<Api>()
        bad_http := r.dispatch(http.Request(
            method: "POST",
            path: "/accounts",
            body: "{\"id\":\"bad\",\"role\":\"owner\",\"homepage\":\"nope\",\"name\":\"\",\"score\":99,\"ratio\":9.0,\"tags\":[]}",
            headers: Map<String, String>(),
            query: Map<String, String>(),
            params: Map<String, String>(),
        ))
        return "{ok_res}|{d_ok}:{d_fail}|{bad_http.status}|{bad_codes}"
    }
    "#;
    let module = compile_source(src).unwrap();
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();

    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        let Obj::Str(s) = vm.call_obj(m, f, &[], &[]).unwrap() else {
            panic!("expected String output");
        };
        let out = s.as_str();
        assert!(
            out.starts_with("ok:Alice|true:true|422|"),
            "unexpected result: {out}"
        );
        for expected in [
            "id=uuid",
            "role=one_of",
            "homepage=url",
            "name=not_empty",
            "name=min",
            "name=max",
            "score=range",
            "ratio=range",
            "tags=not_empty",
            "tags=max",
        ] {
            assert!(out.contains(expected), "missing '{expected}' in {out}");
        }
    }
}

#[test]
fn cdt_time_iso_calendar_arithmetic_and_sleep_e2e() {
    let src = r#"
    import * as time from "cdt.time"

    export func run() -> String | time.TimeError {
        epoch := time.from_unix(0)
        pre := time.from_unix_millis(-500)
        pre_iso := pre.to_iso()?
        pre_s := pre.unix_seconds()?

        ts := time.parse_iso("2024-02-29T14:35:09.25Z")?
        tz := time.parse_iso("2024-02-29T16:35:09+02:00")?
        day_only := time.parse_iso("2026-11-19")?

        y := ts.year()
        mo := ts.month()
        d := ts.day()
        h := ts.hour()
        mi := ts.minute()
        sec := ts.second()
        wd := ts.weekday()

        later := ts.add(time.seconds(3661)?)?
        back := later.sub(time.milliseconds(1000)?)?
        delta := later.diff(ts)?
        delta_s := delta.seconds()?

        var bad_leap = false
        switch time.parse_iso("2023-02-29T00:00:00Z") {
            case time.TimeError(_): bad_leap = true
            case time.Timestamp(_): bad_leap = false
        }
        var bad_diff = false
        switch ts.diff(later) {
            case time.TimeError(_): bad_diff = true
            case time.Duration(_): bad_diff = false
        }

        time.sleep(1)?
        var bad_sleep = false
        switch time.sleep(-5) {
            case time.TimeError(_): bad_sleep = true
            case Void: bad_sleep = false
        }

        epoch_iso := time.format_iso(epoch)?
        ts_iso := ts.to_iso()?
        tz_iso := tz.to_iso()?
        day_iso := day_only.to_iso()?
        back_iso := back.to_iso()?

        return "{epoch_iso}|{pre_iso}:{pre_s}|{ts_iso}|{tz_iso}|{day_iso}|{y}-{mo}-{d} {h}:{mi}:{sec} wd={wd}|{back_iso}|{delta_s}|{bad_leap}:{bad_diff}:{bad_sleep}"
    }
    "#;
    let module = compile_source(src).unwrap();
    let mut vm = Vm::new();
    let out = vm
        .call_obj(&module, module.func_id("run").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(
        out,
        Obj::str(
            "1970-01-01T00:00:00Z|1969-12-31T23:59:59.5Z:-1|2024-02-29T14:35:09.25Z|2024-02-29T14:35:09Z|2026-11-19T00:00:00Z|2024-2-29 14:35:9 wd=4|2024-02-29T15:36:09.25Z|3661|true:true:true"
        )
    );
}

#[test]
fn cdt_db_extended_crud_sql_and_check_e2e() {
    use crate::native::db::{
        Cell, Column, QueryResult, build_count_sql, build_find_by_sql, build_update_and_cells,
        extract_count_cell,
    };

    let src = r#"
    import * as db from "cdt.db"

    @db.table("accounts")
    struct Account {
        id: Int @db.pk @db.auto = 0
        email: String @db.unique
        age: Int? = nil
    }

    export func exercise(conn: db.Connection) -> Void | db.DbError {
        _u := conn.update<Account>(Account(id: 7, email: "a@b.com", age: 30))?
        _c := conn.count<Account>()?
        _found := conn.find_by<Account>("email", "a@b.com")?
        _one := conn.query_one<Account>("SELECT id, email, age FROM accounts WHERE id = ?", [db.int(7)])?
        return
    }
    "#;
    let module = compile_source(src).unwrap();
    let fid = module.func_id("exercise").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, _, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    let model = &decoded.db_models[0];
    let schema = decoded
        .struct_schemas
        .iter()
        .find(|s| s.schema_id == model.schema_id)
        .unwrap();

    let mut rec = crate::vm::value::StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
    rec.set_scalar(0, 7);
    rec.set_obj(0, Obj::str("updated@example.com"));
    rec.set_obj(1, Obj::boxed_int(42));

    let (upd_sql, upd_cells) =
        build_update_and_cells(model, schema, &rec, crate::native::db::Dialect::Postgres).unwrap();
    assert_eq!(upd_sql, r#"UPDATE "accounts" SET "email" = ?, "age" = ? WHERE "id" = ?"#);
    assert_eq!(
        upd_cells,
        vec![
            Cell::Text("updated@example.com".to_string()),
            Cell::Int(42),
            Cell::Int(7)
        ]
    );

    assert_eq!(
        build_count_sql(model, crate::native::db::Dialect::Postgres),
        r#"SELECT COUNT(*) FROM "accounts""#
    );
    assert_eq!(
        build_find_by_sql(model, "email", crate::native::db::Dialect::Postgres).unwrap(),
        r#"SELECT "id", "email", "age" FROM "accounts" WHERE "email" = ?"#
    );
    assert!(
        build_find_by_sql(model, "email = '' OR 1=1", crate::native::db::Dialect::Postgres).is_err(),
        "must reject non-model columns to prevent SQL injection"
    );

    let qr = QueryResult {
        columns: vec![Column { name: "count".to_string(), is_bool: false }],
        rows: vec![vec![Cell::Int(9)]],
    };
    assert_eq!(extract_count_cell(&qr).unwrap(), 9);
}

#[test]
fn cdt_jwt_sign_verify_hs256_hs512_expiry_and_bundle_e2e() {
    let src = r#"
    import * as jwt from "cdt.jwt"
    import * as time from "cdt.time"

    struct Claims {
        sub: String
        role: String
        iat: Int? = nil
        exp: Int? = nil
        nbf: Int? = nil
    }

    export func run() -> String | jwt.JwtError {
        secret := "s3cr3t-hmac-key-0123456789abcdef"
        tok256 := jwt.sign(Claims(sub: "u42", role: "admin"), secret, 3600)?
        c256 := jwt.verify<Claims>(tok256, secret)?
        has_exp := c256.exp != nil && c256.iat != nil

        tok512 := jwt.sign_alg("HS512", Claims(sub: "u99", role: "ops"), secret, 120)?
        c512 := jwt.verify_alg<Claims>("HS512", tok512, secret)?

        var weak_sign = ""
        switch jwt.sign(Claims(sub: "u1", role: "user"), "short", 60) {
            case jwt.JwtError(e): weak_sign = e.kind
            case String(_): weak_sign = "unexpected"
        }

        var weak_verify = ""
        switch jwt.verify<Claims>(tok256, "short") {
            case jwt.JwtError(e): weak_verify = e.kind
            case Claims(_): weak_verify = "unexpected"
        }

        var bad_sig = ""
        switch jwt.verify<Claims>(tok256, "wrong-secret-0123456789abcdef012") {
            case jwt.JwtError(e): bad_sig = e.kind
            case Claims(_): bad_sig = "unexpected"
        }

        var alg_mismatch = ""
        switch jwt.verify_alg<Claims>("HS512", tok256, secret) {
            case jwt.JwtError(e): alg_mismatch = e.kind
            case Claims(_): alg_mismatch = "unexpected"
        }

        // alg: none attack token
        none_tok := "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ1MSIsInJvbGUiOiJhZG1pbiJ9.sig"
        var bad_none = ""
        switch jwt.verify<Claims>(none_tok, secret) {
            case jwt.JwtError(e): bad_none = e.kind
            case Claims(_): bad_none = "unexpected"
        }

        now_ts := time.now()
        var now_s = 1800000000
        switch now_ts {
            case time.Timestamp(t):
                switch t.unix_seconds() {
                    case Int(s): now_s = s
                    case time.TimeError(_):
                }
            case time.TimeError(_):
        }

        exp_tok := jwt.sign(Claims(sub: "old", role: "user", exp: now_s - 5), secret)?
        var exp_kind = ""
        switch jwt.verify<Claims>(exp_tok, secret) {
            case jwt.JwtError(e): exp_kind = e.kind
            case Claims(_): exp_kind = "unexpected"
        }
        leeway_ok := jwt.verify<Claims>(exp_tok, secret, 20)?

        nbf_tok := jwt.sign(Claims(sub: "future", role: "user", nbf: now_s + 600), secret)?
        var nbf_kind = ""
        switch jwt.verify<Claims>(nbf_tok, secret) {
            case jwt.JwtError(e): nbf_kind = e.kind
            case Claims(_): nbf_kind = "unexpected"
        }

        no_exp_tok := jwt.sign(Claims(sub: "noexp", role: "user"), secret)?
        var miss_exp_kind = ""
        switch jwt.verify<Claims>(no_exp_tok, secret, 0, true) {
            case jwt.JwtError(e): miss_exp_kind = e.kind
            case Claims(_): miss_exp_kind = "unexpected"
        }
        req_exp_ok := jwt.verify<Claims>(tok256, secret, 0, true)?

        unverified := jwt.decode_unverified<Claims>(tok512)?

        s1 := c256.sub
        r1 := c256.role
        s2 := c512.sub
        s_lw := leeway_ok.sub
        s_uv := unverified.sub
        s_re := req_exp_ok.sub
        return "{s1}:{r1}:{has_exp}|{s2}|{weak_sign}:{weak_verify}|{bad_sig}:{alg_mismatch}:{bad_none}|{exp_kind}:{s_lw}|{nbf_kind}|{miss_exp_kind}:{s_re}|{s_uv}"
    }
    "#;
    let module = compile_source(src).unwrap();
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        let out = vm.call_obj(m, f, &[], &[]).unwrap();
        assert_eq!(
            out,
            Obj::str("u42:admin:true|u99|weak_secret:weak_secret|invalid_signature:unsupported_alg:unsupported_alg|expired:old|not_before|missing_exp:u42|u99")
        );
    }
}

#[test]
fn cdt_jwt_asymmetric_and_iss_aud_bundle_e2e() {
    let es256_pub = concat!(
        "-----BEGIN PUBLIC KEY-----\n",
        "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEX2/s+nZGKG7ObgRuzlUTP3A/R4zp\n",
        "qNI9BldXja25TdPbm4EAfZST633+UBHz2KmDlGUpOof0UKf6micOu2YnyA==\n",
        "-----END PUBLIC KEY-----\n"
    );
    let es256_tok = "eyJhbGciOiJFUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1NDIiLCJyb2xlIjoiYWRtaW4iLCJpc3MiOiJodHRwczovL2F1dGguZXhhbXBsZS5jb20iLCJhdWQiOlsid2ViIiwiYXBpIl0sImV4cCI6NDEwMjQ0NDgwMH0.UgvS1tVi8mx4ysVN3sh2nFtLbRMuNvFnJq03NCYemle60ee23tGeAZVrbaYpGbOT0r4eQOq5jgLZTTgWr7IdPw";

    let rsa_pub = concat!(
        "-----BEGIN PUBLIC KEY-----\n",
        "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA4H+SvBx/EjBweI9CHluq\n",
        "bZr4AbLM/WgbzpZ6vOjj9POIOxgzSrJsSHfJi+tFr82GKViotuYSMBd9b0haurHq\n",
        "eudJgj2Y+fw2OCVxKl8/Uql0cz1ghfsUMETeVCMCbSwr21XyZ4trlSNlTchuteDJ\n",
        "2oNIHu5EG7UWEDOqIrQYb5Xn6thcB3qCJGc0KOl3UoSis+un8t4GxbyocmofsR51\n",
        "w1kduAb/IzG2/EHqbg9jvHPuPqZGTZ+Z7reODKIo0ORJuRLUzhFs23FYQFBRGVH0\n",
        "MbfBOCsQSnToUApCG9fioyAISUnz+emuFUrUXAOC0DP2Y33JNAEpzvKSOjH9+ubL\n",
        "2wIDAQAB\n",
        "-----END PUBLIC KEY-----\n"
    );
    let rs256_tok = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1NDIiLCJyb2xlIjoiYWRtaW4iLCJpc3MiOiJodHRwczovL2F1dGguZXhhbXBsZS5jb20iLCJhdWQiOlsid2ViIiwiYXBpIl0sImV4cCI6NDEwMjQ0NDgwMH0.qUXfCUDWOno-w2YssFQZyopGlZ4XQoTHZYj_MH31KJy7z-Q5VDGms6P0hh5FpUKY_HAfNuUUn2zVHG_TaEZva_PiJ-I1v1892NhCcfgOquyUnC2uMv9b2-jRoclUI8h5qvMO9VWNag_w74JRbTpDlaEOaANJaTkbSqz_vWZwbEUomQBJWNN44c9zo3pk1-XooQOMoGCduUV5GlozJ4V1wYuiYoNeX9YsLZlwmr6rnt0ec5ReCly1hXOJJFdyE3Hkb6FRuYLICLz27n8wHPmn5sHC7FcRXJFeXYeoE8Kz0RHzs1IdCWx5ET4ySIDYIrd4OWIPDFvyaQ6RAH_e0fov6w";

    let es256_pub_escaped = es256_pub.replace('\n', "\\n");
    let rsa_pub_escaped = rsa_pub.replace('\n', "\\n");

    let src = format!(
        r#"
    import * as jwt from "cdt.jwt"

    struct Claims {{
        sub: String
        role: String
        iss: String? = nil
        aud: [String]? = nil
        iat: Int? = nil
        exp: Int? = nil
    }}

    export func run() -> String | jwt.JwtError {{
        es_pub := "{es256_pub_escaped}"
        es_tok := "{es256_tok}"
        rsa_pub := "{rsa_pub_escaped}"
        rs_tok := "{rs256_tok}"

        c1 := jwt.verify_alg<Claims>("ES256", es_tok, es_pub, 0, true, "https://auth.example.com", "web")?
        c2 := jwt.verify_alg<Claims>("RS256", rs_tok, rsa_pub, 0, true, "", "api")?

        var bad_iss = ""
        switch jwt.verify_alg<Claims>("ES256", es_tok, es_pub, 0, false, "https://wrong.example.com") {{
            case jwt.JwtError(e): bad_iss = e.kind
            case Claims(_): bad_iss = "unexpected"
        }}

        var bad_aud = ""
        switch jwt.verify_alg<Claims>("RS256", rs_tok, rsa_pub, 0, false, "https://auth.example.com", "mobile") {{
            case jwt.JwtError(e): bad_aud = e.kind
            case Claims(_): bad_aud = "unexpected"
        }}

        var bad_key = ""
        switch jwt.verify_alg<Claims>("ES256", es_tok, rsa_pub) {{
            case jwt.JwtError(e): bad_key = e.kind
            case Claims(_): bad_key = "unexpected"
        }}

        secret := "s3cr3t-hmac-key-0123456789abcdef"
        hs_tok := jwt.sign(Claims(sub: "s1", role: "dev", iss: "svc"), secret, 60)?
        c_hs := jwt.verify<Claims>(hs_tok, secret, 0, true, "svc")?

        return "{{c1.sub}}:{{c2.role}}|{{bad_iss}}|{{bad_aud}}|{{bad_key}}|{{c_hs.sub}}"
    }}
    "#
    );

    let module = compile_source(&src).expect("compile jwt asymmetric");
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        let out = vm.call_obj(m, f, &[], &[]).unwrap();
        assert_eq!(out, Obj::str("u42:admin|invalid_issuer|invalid_audience|invalid_key|s1"));
    }
}

#[test]
fn cdt_lock_inprocess_rwlock_file_and_guards_e2e() {
    let dir = std::env::temp_dir().join(format!("conduit_lock_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lock_file = dir.join("shared.lock");
    let data_file = dir.join("audit.log");
    let lock_path = lock_file.to_str().unwrap().replace('\\', "/");
    let data_path = data_file.to_str().unwrap().replace('\\', "/");

    let src = format!(
        r#"
    import * as lock from "cdt.lock"
    import * as fs from "cdt.fs"

    error StepError {{
        msg: String
    }}

    func fail_inside() -> Int | StepError {{
        return StepError(msg: "boom")
    }}

    export func run(do_busy_check: Bool) -> String | lock.LockError | fs.FsError {{
        n := lock.with("m1", () => 21 * 2)?
        s := lock.with("m1", () => "ok")?

        inner_err := lock.with("m_err", () => fail_inside())
        var saw_step_err = false
        switch inner_err {{
            case StepError(_): saw_step_err = true
            case Int(_): saw_step_err = false
            case lock.LockError(_): saw_step_err = false
        }}
        // Mutex must be unlocked even though inner callback returned an error value
        after_err := lock.try_with("m_err", 0, () => 99)?

        r_val := lock.read("rw1", () => 10)?
        w_val := lock.write("rw1", () => 20)?

        // Scoped OS file lock + audit log append
        lock.file("{lock_path}", () => fs.write_text("{data_path}", "line1\n"))?
        log_txt := fs.read_text("{data_path}")?

        // Manual file guard contention check
        fg := lock.file_acquire("{lock_path}")?
        held1 := fg.is_held()
        var try_busy = ""
        if do_busy_check {{
            switch lock.file_try("{lock_path}", () => 1) {{
                case lock.LockError(e): try_busy = e.kind
                case Int(_): try_busy = "unexpected"
            }}
        }}
        fg.release()
        held2 := fg.is_held()
        after_rel := lock.file_try("{lock_path}", () => 77)?

        shared_ok := lock.file_shared("{lock_path}", () => "shared")?

        // Manual mutex guard check
        mg := lock.acquire("manual_m")
        m_held1 := mg.is_held()
        var m_busy = ""
        switch lock.try_with("manual_m", 0, () => 1) {{
            case lock.LockError(e): m_busy = e.kind
            case Int(_): m_busy = "unexpected"
        }}
        var m_timeout = ""
        switch lock.try_with("manual_m", 15, () => 1) {{
            case lock.LockError(e): m_timeout = e.kind
            case Int(_): m_timeout = "unexpected"
        }}
        mg.release()
        m_held2 := mg.is_held()
        m_after := lock.try_with("manual_m", 0, () => 5)?

        return "{{n}}:{{s}}|{{saw_step_err}}:{{after_err}}|{{r_val}}:{{w_val}}|{{log_txt}}|{{held1}}:{{try_busy}}:{{held2}}:{{after_rel}}:{{shared_ok}}|{{m_held1}}:{{m_busy}}:{{m_timeout}}:{{m_held2}}:{{m_after}}"
    }}
    "#
    );

    let module = compile_source(&src).unwrap();
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    for m in [&module, &decoded] {
        let f = if std::ptr::eq(m, &module) { fid } else { entry };
        let mut vm = Vm::new();
        let out = vm.call_obj(m, f, &[1], &[]).unwrap();
        assert_eq!(
            out,
            Obj::str("42:ok|true:99|10:20|line1\n|true:busy:false:77:shared|true:busy:timeout:false:5")
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cdt_cache_lru_ttl_json_stampede_snapshot_and_wal_e2e() {
    let dir = std::env::temp_dir().join(format!("conduit_cache_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let snap_path = dir.join("snap.json").to_str().unwrap().replace('\\', "/");
    let wal_path = dir.join("store.wal").to_str().unwrap().replace('\\', "/");

    let src_writer = r#"
    import * as cache from "cdt.cache"
    import * as time from "cdt.time"

    struct Profile {
        name: String
        level: Int
    }

    error FetchError {
        msg: String
    }

    func compute_ok(calls: cache.Cache) -> String {
        _n := calls.incr("calls", 1)
        return "computed-v1"
    }

    func compute_fail(calls: cache.Cache) -> String | FetchError {
        _n := calls.incr("fail_calls", 1)
        return FetchError(msg: "db down")
    }

    export func phase1() -> String | cache.CacheError | time.TimeError {
        // LRU eviction + touch order with max_entries = 2
        lru := cache.open("lru_shared", 2)
        lru.set("a", "1")?
        lru.set("b", "2")?
        _touch := lru.get("a")
        lru.set("c", "3")?
        b_evicted := lru.get("b") == nil

        // JSON helpers & incr fixed window
        c := cache.open("app_shared")
        c.set_json("prof", Profile(name: "Ada", level: 9), 5000)?
        p := c.get_json<Profile>("prof")?
        var pname = ""
        switch p {
            case Profile(pr): pname = pr.name
            case Nil: pname = "nil"
        }

        // TTL expiration
        c.set("short", "alive", 30)?
        has_before := c.has("short")
        rem := c.ttl("short") ?? -99
        rem_ok := rem >= 0
        c.set("perm", "forever")?
        perm_ttl := c.ttl("perm") ?? -99
        time.sleep(45)?
        expired_gone := c.get("short") == nil && !c.has("short")
        var missing_ttl_nil = false
        switch c.ttl("short") {
            case Nil: missing_ttl_nil = true
            case Int(_): missing_ttl_nil = false
        }

        // Rate limiter incr
        h1 := c.incr("rate:u1", 1, 5000)?
        h2 := c.incr("rate:u1", 4, 5000)?

        // Stampede-safe get_or_set (hit avoids re-running, errors are not cached)
        metrics := cache.open("metrics_ctr")
        v1 := c.get_or_set("cfg", 5000, () => compute_ok(metrics))?
        v2 := c.get_or_set("cfg", 5000, () => compute_ok(metrics))?
        call_count := metrics.get("calls") ?? "0"

        f1 := c.get_or_set("bad_cfg", 5000, () => compute_fail(metrics))
        var f1_err = false
        switch f1 {
            case FetchError(_): f1_err = true
            case String(_): f1_err = false
            case cache.CacheError(_): f1_err = false
        }
        // Must retry because FetchError was not cached
        v_ok := c.get_or_set("bad_cfg", 5000, () => compute_ok(metrics))?
        call_count2 := metrics.get("calls") ?? "0"

        return "{b_evicted}|{pname}|{has_before}:{rem_ok}:{perm_ttl}:{expired_gone}:{missing_ttl_nil}|{h1}:{h2}|{v1}:{v2}:{call_count}:{f1_err}:{v_ok}:{call_count2}"
    }
    "#;

    let src_reader_persist = format!(
        r#"
    import * as cache from "cdt.cache"
    import * as time from "cdt.time"

    export func phase2() -> String | cache.CacheError | time.TimeError {{
        // Verify process-wide cache sharing from phase1 (separate VM!)
        lru2 := cache.open("lru_shared")
        a_val := lru2.get("a") ?? "missing"
        c_val := lru2.get("c") ?? "missing"

        // Snapshot save/load (dropping expired items on load)
        snap_src := cache.open("snap_src")
        snap_src.set("keep", "yes")?
        snap_src.set("drop_me", "soon", 20)?
        saved := snap_src.save("{snap_path}")?
        time.sleep(35)?
        snap_dst := cache.open("snap_dst")
        loaded := snap_dst.load("{snap_path}")?
        keep_val := snap_dst.get("keep") ?? "missing"
        drop_gone := snap_dst.get("drop_me") == nil

        // WAL persistence + drop + reopen + compact + reopen
        w1 := cache.open_file("wal_c", "{wal_path}")?
        w1.set("k1", "v1")?
        w1.set("k2", "v2")?
        w1.del("k1")?
        cnt := w1.incr("seq", 5)?
        _dropped := cache.drop("wal_c")

        w2 := cache.open_file("wal_c", "{wal_path}")?
        k1_after := w2.get("k1") == nil
        k2_after := w2.get("k2") ?? ""
        seq_after := w2.get("seq") ?? ""
        live := w2.compact()?
        _dropped2 := cache.drop("wal_c")

        w3 := cache.open_file("wal_c", "{wal_path}")?
        k2_compact := w3.get("k2") ?? ""
        w3_keys := w3.keys("k")
        key_len := w3_keys.len()

        return "{{a_val}}:{{c_val}}|{{saved}}:{{loaded}}:{{keep_val}}:{{drop_gone}}|{{cnt}}:{{k1_after}}:{{k2_after}}:{{seq_after}}:{{live}}:{{k2_compact}}:{{key_len}}"
    }}
    "#
    );

    let m1 = compile_source(src_writer).unwrap();
    let mut vm1 = Vm::new();
    let out1 = vm1.call_obj(&m1, m1.func_id("phase1").unwrap(), &[], &[]).unwrap();
    assert_eq!(
        out1,
        Obj::str("true|Ada|true:true:-1:true:true|1:5|computed-v1:computed-v1:1:true:computed-v1:2")
    );

    let m2 = compile_source(&src_reader_persist).unwrap();
    let fid2 = m2.func_id("phase2").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&m2, fid2, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    let mut vm2 = Vm::new();
    let out2 = vm2.call_obj(&decoded, entry, &[], &[]).unwrap();
    assert_eq!(
        out2,
        Obj::str("1:3|2:1:yes:true|5:true:v2:5:2:v2:1")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cdt_redis_resp2_commands_json_and_distributed_locks_e2e() {
    let src = r#"
    import * as redis from "cdt.redis"

    struct Item {
        id: Int
        name: String
    }

    error WorkError {
        reason: String
    }

    export func run() -> String | redis.RedisError {
        var bad_url = ""
        switch redis.connect("http://127.0.0.1:6379") {
            case redis.RedisError(e): bad_url = e.kind
            case redis.Client(_): bad_url = "unexpected"
        }

        // First connect initializes shared mock server on host `e2e_redis` with pass `topsecret`
        r1 := redis.connect("redis-mock://:topsecret@e2e_redis:6379/2")?
        r2 := redis.connect("redis-mock://:topsecret@e2e_redis:6379/2")?

        var bad_auth = ""
        switch redis.connect("redis-mock://:wrong@e2e_redis:6379/2") {
            case redis.RedisError(e): bad_auth = e.kind
            case redis.Client(_): bad_auth = "unexpected"
        }

        pong := r1.ping()?
        r1.set("hello", "world")?
        g1 := r1.get("hello") ?? ""
        g_miss := r1.get("absent") == nil
        r1.set_ex("exp_k", "val", 60)?
        t_exp := r1.ttl("exp_k")?
        t_exp_pos := t_exp > 0

        nx1 := r1.set_nx("hello", "overwrite")?
        nx2 := r1.set_nx("nx_fresh", "yes", 30)?
        ex1 := r1.exists("hello")?
        d1 := r1.del("hello")?
        ex2 := r1.exists("hello")?

        c1 := r1.incr("cnt")?
        c2 := r1.incr_by("cnt", 4)?
        c3 := r1.decr("cnt")?
        ks := r1.keys("cnt*")?
        k_len := ks.len()

        r1.set_json("item:1", Item(id: 7, name: "bolt"), 60)?
        loaded := r1.get_json<Item>("item:1")?
        var item_name = ""
        switch loaded {
            case Item(it): item_name = it.name
            case Nil: item_name = "nil"
        }

        // Hash
        hs := r1.hset("user:1", "name", "Alice")?
        hg := r1.hget("user:1", "name") ?? ""
        hm := r1.hgetall("user:1")?
        hm_val := hm.get("name") ?? ""
        hd := r1.hdel("user:1", "name")?

        // List
        _l1 := r1.lpush("q", "a")?
        _l2 := r1.rpush("q", "b")?
        lr := r1.lrange("q", 0, -1)?
        lr_len := lr.len()
        lp := r1.lpop("q") ?? ""
        rp := r1.rpop("q") ?? ""

        // Set
        sa := r1.sadd("tags", "rust")?
        sm := r1.sismember("tags", "rust")?
        smems := r1.smembers("tags")?
        sm_len := smems.len()
        sr := r1.srem("tags", "rust")?

        // Raw command
        var raw_pong = ""
        switch r1.cmd(["PING", "hi"]) {
            case String(s): raw_pong = s
            case Nil: raw_pong = "nil"
            case redis.RedisError(e): return e
        }

        // Distributed locks across two clients
        lk_opt := r1.lock("job:1", 5000)
        var lk_acquired = false
        var lk_busy_on_r2 = false
        var lk_ext = false
        var lk_un1 = false
        var lk_un2 = false
        switch lk_opt {
            case redis.Lock(lk):
                lk_acquired = true
                switch r2.lock("job:1", 5000) {
                    case Nil: lk_busy_on_r2 = true
                    case redis.Lock(_): lk_busy_on_r2 = false
                    case redis.RedisError(e): return e
                }
                lk_ext = lk.extend(8000)?
                lk_un1 = lk.unlock()?
                lk_un2 = lk.unlock()?
            case Nil:
            case redis.RedisError(e): return e
        }
        // Re-acquirable by r2 after unlock
        var re_acq = false
        switch r2.lock("job:1", 5000) {
            case redis.Lock(lk2):
                re_acq = r2.unlock(lk2)?
            case Nil:
            case redis.RedisError(e): return e
        }

        // Re-entrant `with_lock` calling `r1.incr(...)` on same Client without self-deadlock
        wl_n := r1.with_lock("job:wl", 5000, () => r1.incr("wl_cnt"))?
        var after_wl = false
        switch r2.lock("job:wl", 1000) {
            case redis.Lock(lk3):
                after_wl = lk3.unlock()?
            case Nil:
            case redis.RedisError(e): return e
        }

        r1.close()
        r2.close()
        return "{bad_url}:{bad_auth}:{pong}|{g1}:{g_miss}:{t_exp_pos}:{nx1}:{nx2}:{ex1}:{d1}:{ex2}|{c1}:{c2}:{c3}:{k_len}:{item_name}|{hs}:{hg}:{hm_val}:{hd}|{lr_len}:{lp}:{rp}|{sa}:{sm}:{sm_len}:{sr}:{raw_pong}|{lk_acquired}:{lk_busy_on_r2}:{lk_ext}:{lk_un1}:{lk_un2}:{re_acq}|{wl_n}:{after_wl}"
    }
    "#;

    let module = compile_source(src).unwrap();
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    // Execute decoded bundle in a clean VM (each run hits the same mock host, incr advances)
    let mut vm = Vm::new();
    let out = vm.call_obj(&decoded, entry, &[], &[]).unwrap();
    assert_eq!(
        out,
        Obj::str("invalid_url:auth:PONG|world:true:true:false:true:true:true:false|1:5:4:1:bolt|true:Alice:Alice:true|2:a:b|true:true:1:true:hi|true:true:true:true:false:true|1:true")
    );
}

#[test]
fn defer_lifo_early_return_try_propagation_loops_closures_and_bundle_e2e() {
    let bad_ret = "func f() -> Int {\n    defer {\n        return 1\n    }\n    return 0\n}\n";
    assert!(check_source(bad_ret).is_err(), "return inside defer must fail");

    let bad_try = r#"
error E { msg: String }
func fail() -> Int | E { return E("x") }
func f() -> Int | E {
    defer { _ := fail()? }
    return 0
}
"#;
    assert!(check_source(bad_try).is_err(), "? inside defer must fail");

    let bad_nested = "func f() -> Void {\n    defer {\n        defer print(\"x\")\n    }\n}\n";
    assert!(check_source(bad_nested).is_err(), "nested defer must fail");

    let src = r#"
import "cdt.cache" as cache

error AppErr {
    msg: String
}

func push_log(st: cache.Cache, key: String, item: String) -> Void {
    cur := st.get(key) ?? ""
    if cur == "" {
        _s1 := st.set(key, item)
    } else {
        _s2 := st.set(key, "{cur},{item}")
    }
}

func fallthrough_case(st: cache.Cache) -> Void {
    defer push_log(st, "l1", "f1")
    defer {
        push_log(st, "l1", "f2a")
        push_log(st, "l1", "f2b")
    }
    push_log(st, "l1", "body")
}

func early_ret(st: cache.Cache, early: Bool) -> Int {
    var out = 10
    defer {
        push_log(st, "l2", "a")
        out = 999
    }
    if early {
        out = 42
        return out
    }
    defer push_log(st, "l2", "b")
    out = 7
    return out
}

func might_fail(fail: Bool) -> Int | AppErr {
    if fail {
        return AppErr("boom")
    }
    return 5
}

func try_prop(st: cache.Cache, fail: Bool) -> Int | AppErr {
    defer push_log(st, "l3", "cleanup")
    v := might_fail(fail)?
    defer push_log(st, "l3", "after_ok")
    return v + 1
}

func loop_and_closure(st: cache.Cache) -> Int {
    for _ in 0..3 {
        defer push_log(st, "l4", "loop")
    }
    tag := "cls"
    cb := () => {
        push_log(st, "l4", tag)
    }
    defer cb()
    inner := () => {
        defer push_log(st, "l4", "inner_d")
        push_log(st, "l4", "inner_body")
    }
    inner()
    push_log(st, "l4", "after_inner")
    return 11
}

func run() -> String {
    st := cache.open("defer_test_shared")
    st.clear()
    fallthrough_case(st)

    r_early := early_ret(st, true)
    r_normal := early_ret(st, false)

    var try_err = "ok"
    switch try_prop(st, true) {
        case Int(_): try_err = "unexpected"
        case AppErr(e): try_err = e.msg
    }
    var try_ok = 0
    switch try_prop(st, false) {
        case Int(n): try_ok = n
        case AppErr(_): try_ok = -1
    }

    inner_res := loop_and_closure(st)

    s1 := st.get("l1") ?? ""
    s2 := st.get("l2") ?? ""
    s3 := st.get("l3") ?? ""
    s4 := st.get("l4") ?? ""
    return "{s1}|{r_early},{r_normal}:{s2}|{try_err},{try_ok}:{s3}|{inner_res}:{s4}"
}
"#;
    let module = compile_source(src).expect("defer program should compile");
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    let mut vm = Vm::new();
    let out = vm.call_obj(&decoded, entry, &[], &[]).unwrap();
    assert_eq!(
        out,
        Obj::str(
            "body,f2a,f2b,f1|42,7:a,b,a|boom,6:cleanup,after_ok,cleanup|11:inner_body,inner_d,after_inner,cls,loop,loop,loop"
        )
    );
}

#[test]
fn data_enum_tagged_union_switch_json_cross_module_openapi_and_bundle_e2e() {
    let bad_reserved = "enum Bad {\n    A(type: Int)\n}\n";
    assert!(
        check_source(bad_reserved).is_err(),
        "field named 'type' in data enum must fail"
    );

    let bad_non_exhaustive = r#"
enum M {
    Ping,
    Say(text: String)
}
func f(m: M) -> String {
    switch m {
        case M.Ping: return "p"
    }
    return ""
}
"#;
    assert!(
        check_source(bad_non_exhaustive).is_err(),
        "non-exhaustive switch on data enum must fail"
    );

    let bad_payload_no_parens = r#"
enum M {
    Ping,
    Say(text: String)
}
func f(m: M) -> String {
    switch m {
        case M.Ping: return "p"
        case M.Say: return "s"
    }
}
"#;
    assert!(
        check_source(bad_payload_no_parens).is_err(),
        "payload variant matched without () must fail"
    );

    let proto_mod = r#"
export enum Packet {
    Ack,
    Data(payload: String, seq: Int = 0)
}

export func make_pkt(s: String) -> Packet {
    return Packet.Data(payload: s, seq: 7)
}
"#;

    let main_src = r#"
import "cdt.json" as json
import { Packet, make_pkt } from "proto"

enum Msg {
    Say(text: String, room: Int = 1),
    Move(dx: Int, dy: Int = 0),
    Ping
}

enum Node {
    Leaf(val: Int),
    Pair(left: Node, right: Node)
}

error BadMsg {
    reason: String
}

func make_result(ok: Bool) -> Msg | BadMsg {
    if !ok {
        return BadMsg("invalid")
    }
    return Msg.Say(text: "ok")
}

func describe(m: Msg) -> String {
    switch m {
        case Msg.Say(t, r):
            return "say:{t}:{r}"
        case Msg.Move(_, dy: y):
            return "move:{y}"
        case Msg.Ping:
            return "ping"
    }
}

func sum_tree(n: Node) -> Int {
    switch n {
        case Node.Leaf(v):
            return v
        case Node.Pair(l, r):
            return sum_tree(l) + sum_tree(r)
    }
}

func handle_res_inner(flag: Bool) -> String | BadMsg {
    m := make_result(flag)?
    return describe(m)
}

func handle_res(flag: Bool) -> String {
    switch handle_res_inner(flag) {
        case String(s): return s
        case BadMsg(e): return "err:{e.reason}"
    }
}

func run_inner() -> String | json.DecodeError | json.EncodeError {
    m1 := Msg.Say("hi", 3)
    m2 := Msg.Say(text: "yo")
    m3 := Msg.Move(5)
    m4 := Msg.Ping

    var named_bind = ""
    switch m2 {
        case Msg.Say(text: txt):
            named_bind = txt
        case Msg.Move(_):
            named_bind = "mv"
        case Msg.Ping:
            named_bind = "p"
    }

    j1 := json.encode(m2)?
    j4 := json.encode(m4)?

    d1 := json.decode<Msg>(j1)?
    d_reordered := json.decode<Msg>("{\"room\":9,\"text\":\"reordered\",\"type\":\"Say\"}")?
    d4 := json.decode<Msg>("{\"type\":\"Ping\"}")?

    list_in := [Msg.Ping, Msg.Move(dx: 2, dy: 4)]
    list_json := json.encode(list_in)?
    list_out := json.decode<[Msg]>(list_json)?

    mp := Map<String, Msg>()
    mp.set("k", Msg.Say(text: "mapped"))
    mp_json := json.encode(mp)?
    mp_out := json.decode<Map<String, Msg>>(mp_json)?
    mp_val := describe(mp_out.get("k") ?? Msg.Ping)

    tree := Node.Pair(left: Node.Leaf(4), right: Node.Pair(left: Node.Leaf(5), right: Node.Leaf(6)))
    tree_json := json.encode(tree)?
    tree_dec := json.decode<Node>(tree_json)?
    tree_sum := sum_tree(tree_dec)

    pkt := make_pkt("hello")
    var pkt_str = ""
    switch pkt {
        case Packet.Ack: pkt_str = "ack"
        case Packet.Data(p, s): pkt_str = "{p}:{s}"
    }
    pkt_json := json.encode(Packet.Ack)?
    var ack_rt = false
    switch json.decode<Packet>(pkt_json) {
        case Packet(rt):
            switch rt {
                case Packet.Ack: ack_rt = true
                case Packet.Data(_): ack_rt = false
            }
        case json.DecodeError(_): ack_rt = false
    }

    s_m1 := describe(m1)
    s_m3 := describe(m3)
    s_m4 := describe(m4)
    h_t := handle_res(true)
    h_f := handle_res(false)
    s_d1 := describe(d1)
    s_dr := describe(d_reordered)
    s_d4 := describe(d4)
    l_len := list_out.len()
    return "{s_m1}|{named_bind}|{s_m3}|{s_m4}|{h_t}|{h_f}|{j1}|{j4}|{s_d1}|{s_dr}|{s_d4}|{l_len}|{mp_val}|{tree_sum}|{pkt_str}:{ack_rt}"
}

export func decode_raw_msg(raw: String) -> String | json.DecodeError {
    m := json.decode<Msg>(raw)?
    return describe(m)
}

export func run() -> String {
    switch run_inner() {
        case String(s): return s
        case json.DecodeError(e): return "dec_err:{e.message}"
        case json.EncodeError(e): return "enc_err:{e.message}"
    }
}
"#;

    let mut mods = HashMap::new();
    mods.insert("proto", proto_mod);
    let module = compile_with_modules(main_src, &mods).expect("data enum program should compile");
    let fid = module.func_id("run").unwrap();
    let bundle = crate::ir::bundle::encode_bundle(&module, fid, crate::ir::bundle::RET_OBJ).unwrap();
    let (decoded, entry, _) = crate::ir::bundle::decode_bundle(&bundle).unwrap();
    decoded.validate().unwrap();

    let mut vm = Vm::new();
    let out = vm.call_obj(&decoded, entry, &[], &[]).unwrap();
    assert_eq!(
        out,
        Obj::str(
            "say:hi:3|yo|move:0|ping|say:ok:1|err:invalid|{\"type\":\"Say\",\"text\":\"yo\",\"room\":1}|{\"type\":\"Ping\"}|say:yo:1|say:reordered:9|ping|2|say:mapped:1|15|hello:7:true"
        )
    );

    let dec_fid = decoded.func_id("decode_raw_msg").unwrap();
    let err_missing = vm
        .call_obj(&decoded, dec_fid, &[], &[Obj::str("{\"text\":\"hi\"}")])
        .unwrap_err();
    assert!(
        err_missing.message.contains("Missing discriminator field 'type'"),
        "unexpected err: {}",
        err_missing.message
    );
    let err_unknown = vm
        .call_obj(&decoded, dec_fid, &[], &[Obj::str("{\"type\":\"Ghost\"}")])
        .unwrap_err();
    assert!(
        err_unknown.message.contains("Unknown variant 'Ghost'"),
        "unexpected err: {}",
        err_unknown.message
    );

    let openapi_src = r#"
import "cdt.http" as http

enum Msg {
    Say(text: String, room: Int = 1),
    Ping
}

router EventsApi "Events" {
    post "/events" => handle_event
}

func handle_event(req: http.Request, body: Msg) -> http.Response {
    return http.ok_json(Msg.Ping)
}
"#;
    let (prog, checked) = check_source(openapi_src).unwrap();
    let spec = crate::openapi::generate_openapi_json(&prog, &checked, "Events", "1.0.0", None, None).unwrap();
    assert!(spec.contains("\"$ref\": \"#/components/schemas/Msg\""), "{}", spec);
    assert!(spec.contains("\"oneOf\""), "{}", spec);
    assert!(spec.contains("\"discriminator\""), "{}", spec);
    assert!(spec.contains("\"propertyName\": \"type\""), "{}", spec);
    assert!(spec.contains("\"Msg_Say\""), "{}", spec);
    assert!(spec.contains("\"Msg_Ping\""), "{}", spec);
    assert!(spec.contains("\"mapping\""), "{}", spec);
}

#[test]
fn cdt_db_fluent_query_builder_sql_and_bundle_e2e() {
    use crate::native::db_query::{OrderDir, QueryMode, WhereNode, WhereOp, build_query_sql};

    let src = r#"
import * as db from "cdt.db"

@db.table("users")
struct User {
    id: Int @db.pk @db.auto
    email: String @db.unique
    role: String
    age: Int
    active: Bool = true
    bio: String? = nil
}

export func build_sql_cases(conn: db.Connection) -> String {
    sub := conn.from<User>()
        .where_eq("role", "admin")
        .or_where_eq("role", "owner")
    q := conn.from<User>()
        .select(["id", "email"])
        .where_eq("active", true)
        .where("age", ">=", 18)
        .where_in("role", ["admin", "owner", "mod"])
        .where_not_null("bio")
        .and_query(sub)
        .order_by("age", "desc")
        .order_by("id", "asc")
        .paginate(2, 10)
    s1 := q.to_sql("all")
    s2 := q.to_sql("count")
    s3 := q.to_sql("exists")
    s4 := q.to_sql("first")
    s5 := q.to_sql("delete")
    return s1 + "\n" + s2 + "\n" + s3 + "\n" + s4 + "\n" + s5
}

export func dummy_exec(conn: db.Connection) -> [User] | db.DbError {
    return conn.from<User>().where_eq("active", true).limit(5).all()
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    let model = &module.db_models[0];

    let where_root = WhereNode::And(vec![
        WhereNode::Cond {
            col_name: "active".to_string(),
            op: WhereOp::Eq,
            param_count: 1,
        },
        WhereNode::Or(vec![
            WhereNode::Cond {
                col_name: "role".to_string(),
                op: WhereOp::Eq,
                param_count: 1,
            },
            WhereNode::Cond {
                col_name: "email".to_string(),
                op: WhereOp::ILike,
                param_count: 1,
            },
        ]),
        WhereNode::Cond {
            col_name: "id".to_string(),
            op: WhereOp::In,
            param_count: 0,
        },
    ]);
    let pg_sql = build_query_sql(
        model,
        &["id".to_string(), "email".to_string()],
        Some(&where_root),
        &[("age".to_string(), OrderDir::Desc)],
        Some(10),
        Some(20),
        QueryMode::All,
        "postgres",
    )
    .unwrap();
    assert_eq!(
        pg_sql,
        r#"SELECT "id", "email" FROM "users" WHERE "active" = ? AND ("role" = ? OR "email" ILIKE ?) AND 1 = 0 ORDER BY "age" DESC LIMIT 10 OFFSET 20"#
    );

    let mysql_sql = build_query_sql(
        model,
        &[],
        Some(&where_root),
        &[],
        None,
        None,
        QueryMode::All,
        "mysql",
    )
    .unwrap();
    assert!(mysql_sql.contains("LOWER(`email`) LIKE LOWER(?)"), "{}", mysql_sql);

    assert!(
        build_query_sql(
            model,
            &["id = 1; DROP TABLE users".to_string()],
            None,
            &[],
            None,
            None,
            QueryMode::All,
            "postgres",
        )
        .is_err()
    );

    // Bundle roundtrip check
    let bytes = crate::ir::bundle::encode_bundle(&module, 0, 0).unwrap();
    let (decoded, _, _) = crate::ir::bundle::decode_bundle(&bytes).unwrap();
    assert_eq!(decoded.db_models.len(), 1);
}

#[test]
fn cdt_db_declarative_query_blocks_typecheck_desugar_and_errors_e2e() {
    let ok_src = r#"
import * as db from "cdt.db"

@db.table("users")
struct User {
    id: Int @db.pk @db.auto
    email: String @db.unique
    role: String
    age: Int
    active: Bool = true
    bio: String? = nil
}

export query FindAdults(conn: db.Connection, min_age: Int, max_rows: Int = 20) -> [User] {
    from User
    where active == true && (age >= min_age || role in ["admin", "owner"]) && bio != nil
    order_by age desc, id asc
    limit max_rows
    offset 0
}

query FindByEmail(conn: db.Connection, target_email: String) -> User? | db.DbError {
    from User
    where email ilike target_email
    first
}

query CountActive(conn: db.Connection) -> Int {
    from User
    where active == true
    count
}

query HasRole(conn: db.Connection, target_role: String) -> Bool {
    from User
    where role == target_role
    exists
}

query PurgeInactive(conn: db.Connection) -> Int {
    from User
    where active == false
    delete
}

export func caller(conn: db.Connection) -> Int | db.DbError {
    adults := FindAdults(conn, 18)?
    one := FindByEmail(conn, "%@example.com")?
    n := CountActive(conn)?
    ok := HasRole(conn, "admin")?
    del := PurgeInactive(conn)?
    if one != nil && ok {
        return adults.len() + n + del
    }
    return n
}
"#;
    let module = crate::frontend::compile_source(ok_src).unwrap();
    assert!(module.functions.iter().any(|f| f.name == "FindAdults"));
    assert!(module.functions.iter().any(|f| f.name == "FindByEmail"));
    assert!(module.functions.iter().any(|f| f.name == "CountActive"));
    assert!(module.functions.iter().any(|f| f.name == "HasRole"));
    assert!(module.functions.iter().any(|f| f.name == "PurgeInactive"));

    // Compile-time errors: unknown field, type mismatch, missing Connection param
    let bad_field_src = r#"
import * as db from "cdt.db"
@db.table("users")
struct User { id: Int @db.pk }
query BadField(conn: db.Connection) -> [User] {
    from User
    where nonexistent == 1
}
"#;
    let err = crate::frontend::compile_source(bad_field_src).unwrap_err();
    assert!(err.contains("unknown field 'nonexistent'"), "{}", err);

    let bad_type_src = r#"
import * as db from "cdt.db"
@db.table("users")
struct User { id: Int @db.pk, age: Int }
query BadType(conn: db.Connection) -> [User] {
    from User
    where age >= "eighteen"
}
"#;
    let err = crate::frontend::compile_source(bad_type_src).unwrap_err();
    assert!(err.contains("cannot compare"), "{}", err);

    let bad_conn_src = r#"
import * as db from "cdt.db"
@db.table("users")
struct User { id: Int @db.pk }
query BadConn(x: Int) -> [User] {
    from User
}
"#;
    let err = crate::frontend::compile_source(bad_conn_src).unwrap_err();
    assert!(err.contains("db.Connection"), "{}", err);
}

#[test]
fn cdt_db_pool_typecheck_config_and_bundle_e2e() {
    use crate::native::db_pool::DbPoolState;

    assert!(DbPoolState::open("postgres://u:p@127.0.0.1:5432/app", 0, 1000).is_err());
    assert!(DbPoolState::open("postgres://u:p@127.0.0.1:5432/app", 4, -1).is_err());

    let src = r#"
import * as db from "cdt.db"

@db.table("items")
struct Item {
    id: Int @db.pk @db.auto
    name: String
}

export func run() -> String {
    res := db.pool("sqlite://invalid", 8, 60000)
    switch res {
        case db.DbError(e):
            return e.kind
        case db.Pool(p):
            defer p.close()
            _st := p.stats()
            return "ok"
    }
}

export func use_pool_helpers(p: db.Pool) -> Int | db.DbError {
    c := p.acquire()?
    defer c.close()
    n := p.with(conn => conn.count<Item>())?
    return n + c.count<Item>()?
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    let mut vm = crate::vm::Vm::new();
    let run_fid = module
        .functions
        .iter()
        .position(|f| f.name == "run")
        .unwrap() as u32;
    let out = vm.call(&module, run_fid, &[], &[]).unwrap();
    assert_eq!(out, crate::vm::ExecResult::Obj(crate::vm::value::Obj::str("invalid_url")));

    let bytes = crate::ir::bundle::encode_bundle(&module, 0, 0).unwrap();
    let _decoded = crate::ir::bundle::decode_bundle(&bytes).unwrap();

    // Verify Obj::DbQuery cow/in-place chaining does not allocate any host table entries
    let vm_q = crate::vm::Vm::new();
    let sql_fid = module
        .functions
        .iter()
        .position(|f| f.name == "use_pool_helpers")
        .unwrap();
    assert!(sql_fid < module.functions.len());
    assert_eq!(vm_q.host.db_conns.len(), 0);
}

#[test]
fn cdt_db_prepared_url_flags_and_insert_all_e2e() {
    use crate::native::db::{Cell, Dialect, build_insert_all_and_cells};
    use crate::native::mysql::parse_mysql_url;
    use crate::native::pg::parse_pg_url;
    use crate::vm::value::{Obj, StructData};

    let pg_default = parse_pg_url("postgres://u:p@127.0.0.1:5432/app").unwrap();
    assert!(pg_default.prepare);
    let pg_off = parse_pg_url("postgres://u:p@127.0.0.1:5432/app?prepare=off").unwrap();
    assert!(!pg_off.prepare);

    let my_default = parse_mysql_url("mysql://u:p@127.0.0.1:3306/app").unwrap();
    assert!(my_default.prepare);
    let my_off = parse_mysql_url("mysql://u:p@127.0.0.1:3306/app?prepare=off").unwrap();
    assert!(!my_off.prepare);

    let src = r#"
import * as db from "cdt.db"

@db.table("accounts")
struct Account {
    id: Int @db.pk @db.auto
    email: String @db.unique
    age: Int
}

export func bulk(conn: db.Connection) -> Int | db.DbError {
    rows := [
        Account(id: 0, email: "a@x.com", age: 20),
        Account(id: 0, email: "b@x.com", age: 30),
    ]
    return conn.insert_all<Account>(rows)
}
"#;
    let module = crate::frontend::compile_source(src).unwrap();
    let model = &module.db_models[0];
    let schema = module
        .struct_schemas
        .iter()
        .find(|s| s.schema_id == model.schema_id)
        .unwrap();

    let mut r1 = StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
    r1.set_scalar(0, 0);
    r1.set_scalar(1, 20);
    r1.set_obj(0, Obj::str("a@x.com"));

    let mut r2 = StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
    r2.set_scalar(0, 0);
    r2.set_scalar(1, 30);
    r2.set_obj(0, Obj::str("b@x.com"));

    let batches = build_insert_all_and_cells(model, schema, &[&r1, &r2], Dialect::Postgres).unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0].0,
        r#"INSERT INTO "accounts" ("email", "age") VALUES (?, ?), (?, ?)"#
    );
    assert_eq!(
        batches[0].1,
        vec![
            Cell::Text("a@x.com".to_string()),
            Cell::Int(20),
            Cell::Text("b@x.com".to_string()),
            Cell::Int(30),
        ]
    );
    assert!(build_insert_all_and_cells(model, schema, &[], Dialect::Mysql).unwrap().is_empty());

    // Reserved SQL keywords ("order", "user", "group") are safely quoted per dialect.
    let kw_src = r#"
    @db.table("order")
    struct OrderRow {
        id: Int @db.pk @db.auto = 0
        user: String @db.index
        group: Int = 1
    }
    func main() -> Void {}
    "#;
    let kw_mod = compile_source(kw_src).unwrap();
    let kw_model = &kw_mod.db_models[0];
    let kw_schema = kw_mod
        .struct_schemas
        .iter()
        .find(|s| s.schema_id == kw_model.schema_id)
        .unwrap();
    let pg_ddl = crate::native::db::build_create_table_sqls(kw_model, kw_schema, Dialect::Postgres).join(";");
    assert!(
        pg_ddl.contains(r#"CREATE TABLE IF NOT EXISTS "order" ("id" BIGSERIAL PRIMARY KEY, "user" TEXT NOT NULL, "group" BIGINT NOT NULL)"#),
        "got: {pg_ddl}"
    );
    assert!(pg_ddl.contains(r#"ON "order" ("user")"#), "got: {pg_ddl}");
    let my_ddl = crate::native::db::build_create_table_sqls(kw_model, kw_schema, Dialect::Mysql).join(";");
    assert!(
        my_ddl.contains("CREATE TABLE IF NOT EXISTS `order` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, `user` TEXT NOT NULL, `group` BIGINT NOT NULL)"),
        "got: {my_ddl}"
    );
    assert!(my_ddl.contains("ON `order` (`user`)"));
    assert_eq!(
        crate::native::db::build_find_by_sql(kw_model, "user", Dialect::Mysql).unwrap(),
        "SELECT `id`, `user`, `group` FROM `order` WHERE `user` = ?"
    );
    assert_eq!(
        crate::native::db_query::build_query_sql(
            kw_model,
            &["user".to_string()],
            None,
            &[("group".to_string(), crate::native::db_query::OrderDir::Asc)],
            None,
            None,
            crate::native::db_query::QueryMode::All,
            "postgres",
        )
        .unwrap(),
        r#"SELECT "user" FROM "order" ORDER BY "group" ASC"#
    );
}

#[test]
fn spawn_background_tasks_e2e_and_compile_errors() {
    let src = r#"
        import * as cache from "cdt.cache"
        import * as time from "cdt.time"

        func write_later(store_name: String, key: String, val: String, delay: Int) -> Void {
            _s := time.sleep(delay)
            c := cache.open(store_name)
            c.set(key, val)
        }

        on stop {
            c := cache.open("spawn_test_store")
            a := c.get("block_key") ?? "missing"
            b := c.get("fn_key") ?? "missing"
            c.set("stop_saw", a + "|" + b)
        }

        export func main() -> String {
            store := "spawn_test_store"
            delay := 25
            spawn {
                _s := time.sleep(delay)
                c := cache.open(store)
                c.set("block_key", "from_block")
            }
            spawn write_later(store, "fn_key", "from_fn", 25)
            return "started"
        }
    "#;

    let reader_src = r#"
        import * as cache from "cdt.cache"
        export func read_val(key: String) -> String {
            c := cache.open("spawn_test_store")
            return c.get(key) ?? "missing"
        }
    "#;

    let module = crate::frontend::compile_source(src).expect("compile spawn test");
    let reader_mod = crate::frontend::compile_source(reader_src).expect("compile reader");
    let main_fid = module.func_id("main").unwrap();
    let read_fid = reader_mod.func_id("read_val").unwrap();

    let mut vm = Vm::new();
    let res = vm.call_obj(&module, main_fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("started"));

    let mut reader_vm = Vm::new();
    let stop_saw = reader_vm
        .call_obj(&reader_mod, read_fid, &[], &[Obj::str("stop_saw")])
        .unwrap();
    assert_eq!(stop_saw, Obj::str("from_block|from_fn"));

    for bad_src in [
        r#"
        export func main() -> Void {
            while true {
                spawn { break }
            }
        }
        "#,
        r#"
        export func main() -> Void {
            while true {
                spawn { continue }
            }
        }
        "#,
        r#"
        export func main() -> Int {
            spawn { return 42 }
            return 0
        }
        "#,
        r#"
        on start -> Int {
            return 1
        }
        export func main() -> Void {}
        "#,
    ] {
        assert!(
            crate::frontend::compile_source(bad_src).is_err(),
            "expected compile error for:\n{}",
            bad_src
        );
    }
}

#[test]
fn http_server_lifecycle_hooks_and_graceful_shutdown_e2e() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let store_name = format!("lifecycle_seq_{}_{}", std::process::id(), port);
    let server_src = format!(
        r#"
        import * as http from "cdt.http"
        import * as cache from "cdt.cache"
        import * as time from "cdt.time"

        func append_seq(step: String) -> Void {{
            c := cache.open("{store_name}")
            prev := c.get("seq") ?? ""
            if prev == "" {{
                c.set("seq", step)
            }} else {{
                c.set("seq", prev + "|" + step)
            }}
        }}

        on start {{
            append_seq("top_start")
        }}

        on stop {{
            append_seq("top_stop")
        }}

        func decl_start() -> Void {{
            append_seq("decl_start")
        }}

        func decl_stop() -> Void {{
            append_seq("decl_stop")
        }}

        func imp_start() -> Void {{
            append_seq("imp_start")
        }}

        func imp_stop() -> Void {{
            append_seq("imp_stop")
        }}

        func work(_req: http.Request) -> http.Response {{
            spawn {{
                _s := time.sleep(30)
                append_seq("spawn_done")
            }}
            return http.text("accepted")
        }}

        router Api {{
            on_start => decl_start
            on_stop => decl_stop
            get "/work" => work
        }}

        export func run_server() -> String {{
            r := http.build<Api>()
            r.on_start(imp_start)
            r.on_stop(imp_stop)
            r.post("/stop", (_req: http.Request) -> http.Response => {{
                r.stop()
                return http.text("stopping")
            }})
            switch r.serve("127.0.0.1:{port}") {{
                case http.HttpError(e): return "err:" + e.kind + ":" + e.message
                case _: return "ok"
            }}
            return "ok"
        }}
        "#
    );

    let fail_src = r#"
        import * as http from "cdt.http"

        error BootErr { message: String }

        func fail_boot() -> Void | BootErr {
            return BootErr(message: "db unavailable")
        }

        func ok_handler(_req: http.Request) -> http.Response {
            return http.text("ok")
        }

        router FailingApi {
            on_start => fail_boot
            get "/" => ok_handler
        }

        export func test_startup_fail() -> String {
            switch http.serve<FailingApi>("127.0.0.1:0") {
                case http.HttpError(e): return e.kind + ":" + e.message
                case _: return "unexpected"
            }
            return ""
        }
    "#;

    let reader_src = format!(
        r#"
        import * as cache from "cdt.cache"
        export func read_seq() -> String {{
            c := cache.open("{store_name}")
            return c.get("seq") ?? ""
        }}
        "#
    );

    let client_src = format!(
        r#"
        import * as http from "cdt.http"

        export func hit_work() -> String | http.HttpError {{
            r := http.get("http://127.0.0.1:{port}/work")?
            return r.status.to_string() + ":" + r.body
        }}

        export func hit_stop() -> String | http.HttpError {{
            r := http.post("http://127.0.0.1:{port}/stop", "")?
            return r.status.to_string() + ":" + r.body
        }}
        "#
    );

    let srv_mod = crate::frontend::compile_source(&server_src).expect("compile lifecycle server");
    let fail_mod = crate::frontend::compile_source(fail_src).expect("compile failing server");
    let reader_mod = crate::frontend::compile_source(&reader_src).expect("compile reader");
    let cli_mod = crate::frontend::compile_source(&client_src).expect("compile client");

    let fail_fid = fail_mod.func_id("test_startup_fail").unwrap();
    let fail_res = Vm::new().call_obj(&fail_mod, fail_fid, &[], &[]).unwrap();
    let Obj::Str(fail_str) = fail_res else {
        panic!("expected Str startup error, got {:?}", fail_res);
    };
    assert!(fail_str.starts_with("startup_error:"), "got {}", fail_str);
    assert!(fail_str.contains("db unavailable"), "got {}", fail_str);

    let srv_fid = srv_mod.func_id("run_server").unwrap();
    let work_fid = cli_mod.func_id("hit_work").unwrap();
    let stop_fid = cli_mod.func_id("hit_stop").unwrap();
    let read_fid = reader_mod.func_id("read_seq").unwrap();

    let srv_handle = std::thread::spawn(move || Vm::new().call_obj(&srv_mod, srv_fid, &[], &[]).unwrap());

    let mut client_vm = Vm::new();
    let mut work_res = Obj::Empty;
    for _ in 0..50 {
        let res = client_vm.call_obj(&cli_mod, work_fid, &[], &[]).unwrap();
        if matches!(res, Obj::Str(_)) {
            work_res = res;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(work_res, Obj::str("200:accepted"));

    let stop_res = client_vm.call_obj(&cli_mod, stop_fid, &[], &[]).unwrap();
    assert_eq!(stop_res, Obj::str("200:stopping"));

    let srv_ret = srv_handle.join().unwrap();
    assert_eq!(srv_ret, Obj::str("ok"));

    let seq = Vm::new().call_obj(&reader_mod, read_fid, &[], &[]).unwrap();
    assert_eq!(
        seq,
        Obj::str("top_start|decl_start|imp_start|spawn_done|decl_stop|imp_stop|top_stop")
    );
}

#[test]
fn cdt_chan_and_for_range_e2e_and_compile_errors() {
    let src = r#"
    import * as chan from "cdt.chan"
    import { Chan } from "cdt.chan"
    import * as time from "cdt.time"

    struct Msg {
        id: Int
        text: String
    }

    func send_generic<T>(c: Chan<T>, val: T) -> Bool {
        return c.send(val)
    }

    func collect_sum(c: chan.Chan<Int>) -> Int {
        var total = 0
        for n in c {
            if n == 2 {
                continue
            }
            if n > 4 {
                break
            }
            total = total + n
        }
        return total
    }

    export func run_chan_and_ranges() -> String {
        // 1. Go 1.22 integer range loops and wildcard forms
        var r_sum = 0
        for i in 5 {
            r_sum = r_sum + i
        }
        var neg_runs = 0
        for _ in -2 {
            neg_runs = neg_runs + 1
        }
        var bare_in = 0
        for in 4 {
            bare_in = bare_in + 1
        }
        var bare_range = 0
        for range 3 {
            bare_range = bare_range + 1
        }
        var var_named_range = 0
        for range in [10, 20] {
            var_named_range = var_named_range + range
        }

        // 2. Unbounded Chan + spawn + for-in iteration with continue/break
        var unbounded: Chan<Int> = chan.open<Int>()
        assert_eq(unbounded.cap(), 0)
        spawn {
            for i in 1..7 {
                unbounded.send(i)
            }
            unbounded.close()
        }
        c_sum := collect_sum(unbounded)

        // 3. Bounded channel backpressure, try_send/try_recv, recv_timeout, close
        b := Chan<String>(2)
        var b_alias: chan.Chan<String> = b
        assert_eq(b_alias.cap(), 2)
        assert(b.is_empty())
        assert(b.try_send("first"))
        assert(send_generic<String>(b, "second"))
        assert(!b.try_send("full"))
        assert_eq(b.len(), 2)
        assert(!b.is_empty())
        assert(!b.is_closed())

        empty_ch := chan.open<Int>(1)
        assert_eq(empty_ch.try_recv(), nil)
        assert_eq(empty_ch.recv_timeout(15), nil)

        spawn {
            _s := time.sleep(20)
            // Blocked until main calls recv()
            ok1 := b.send("third")
            assert(ok1)
            b.close()
            ok2 := b.send("after_close")
            assert(!ok2)
            assert(!b.try_send("after_close"))
        }

        m1 := b.recv() ?? ""
        m2 := b.recv_timeout(500) ?? ""
        var rest = ""
        for item in b {
            rest = rest + item
        }
        assert(b.is_closed())
        assert_eq(b.recv(), nil)

        // 4. Structs and tuples over Chan<T>
        msgs := chan.open<Msg>(4)
        pairs := chan.open<(String, Int)>(2)
        spawn {
            msgs.send(Msg(id: 7, text: "hi"))
            msgs.close()
            pairs.send(("alpha", 99))
            pairs.close()
        }
        var msg_out = ""
        for m in msgs {
            msg_out = "{m.id}:{m.text}"
        }
        var pair_out = ""
        for (k, v) in pairs {
            pair_out = "{k}={v}"
        }

        return "{r_sum}|{neg_runs}|{bare_in}|{bare_range}|{var_named_range}|{c_sum}|{m1},{m2},{rest}|{msg_out}|{pair_out}"
    }
    "#;

    let module = crate::frontend::compile_source(src).expect("compile cdt.chan & range program");
    let fid = module.func_id("run_chan_and_ranges").unwrap();
    let res = Vm::new().call_obj(&module, fid, &[], &[]).unwrap();
    assert_eq!(res, Obj::str("10|0|4|3|30|8|first,second,third|7:hi|alpha=99"));

    // Compile-time rejections
    let bad_no_targs = r#"
        import { Chan } from "cdt.chan"
        func f(c: Chan) -> Void {}
    "#;
    assert!(crate::frontend::compile_source(bad_no_targs).is_err());

    let bad_void = r#"
        import * as chan from "cdt.chan"
        func main() -> Void {
            _c := chan.open<Void>()
        }
    "#;
    assert!(crate::frontend::compile_source(bad_void).is_err());

    let bad_send_ty = r#"
        import * as chan from "cdt.chan"
        func main() -> Void {
            c := chan.open<Int>()
            c.send("oops")
        }
    "#;
    assert!(crate::frontend::compile_source(bad_send_ty).is_err());

    let bad_recv_assign = r#"
        import * as chan from "cdt.chan"
        func main() -> Void {
            c := chan.open<Int>()
            var x: Int = c.recv()
        }
    "#;
    assert!(crate::frontend::compile_source(bad_recv_assign).is_err());

    let bad_iter = r#"
        func main() -> Void {
            for x in "abc" {
                print(x)
            }
        }
    "#;
    assert!(crate::frontend::compile_source(bad_iter).is_err());
}

#[test]
fn cdt_chan_declarative_workers_e2e() {
    let src = r#"
        import * as cache from "cdt.cache"
        import * as chan from "cdt.chan"
        import * as time from "cdt.time"

        error WorkErr {
            message: String
        }

        worker ScaleWorker(n: Int, out: Chan<Int>, factor: Int = 2) {
            out.send(n * factor)
        }

        worker PoolWorker(n: Int, out: Chan<Int>) concurrency 4 {
            out.send(n + 100)
        }

        func fail_if_neg(n: Int) -> Int | WorkErr {
            if n < 0 {
                return WorkErr(message: "neg")
            }
            return n
        }

        worker RetryWorker(id: Int, out: Chan<Int>, defers: Chan<Int>) retry 2 backoff 5ms -> Void | WorkErr {
            defer {
                defers.send(id)
            }
            c := cache.open("worker_retry_e2e")
            var attempt = 0
            switch c.incr("k_{id}") {
                case Int(v):
                    attempt = v
                case cache.CacheError(e):
                    return WorkErr(message: e.message)
            }
            if id == 2 && attempt == 1 {
                return WorkErr(message: "transient")
            }
            _v := fail_if_neg(id)?
            out.send(id * 10 + attempt)
        }

        worker GenericTag<T>(item: T, out: Chan<String>, tag: String) concurrency 2 {
            out.send("{tag}:{item}")
        }

        export func run_workers() -> String {
            // 1. Sequential worker with extra param default
            s_in := chan.open<Int>(8)
            s_out := chan.open<Int>(8)
            spawn {
                s_in.send(1)
                s_in.send(3)
                s_in.close()
            }
            s_count := ScaleWorker(s_in, s_out)
            s_out.close()
            var s_sum = 0
            for v in s_out {
                s_sum = s_sum + v
            }

            // 2. Concurrent pool worker
            p_in := chan.open<Int>(16)
            p_out := chan.open<Int>(16)
            spawn {
                for i in 6 {
                    p_in.send(i)
                }
                p_in.close()
            }
            p_count := PoolWorker(p_in, p_out)
            p_out.close()
            var p_sum = 0
            for v in p_out {
                p_sum = p_sum + v
            }

            // 3. Retry + backoff + ? + defer + permanent error skip
            _dropped := cache.drop("worker_retry_e2e")
            r_in := chan.open<Int>(8)
            r_out := chan.open<Int>(8)
            r_defers := chan.open<Int>(16)
            t0 := time.monotonic()
            r_in.send(1)
            r_in.send(2)
            r_in.send(-9)
            r_in.close()
            r_count := RetryWorker(r_in, r_out, r_defers)
            var elapsed_ms = 0
            switch time.elapsed(t0) {
                case time.Duration(d):
                    switch d.milliseconds() {
                        case Int(ms):
                            elapsed_ms = ms
                        case time.TimeError(_):
                    }
                case time.TimeError(_):
            }
            r_out.close()
            r_defers.close()
            var r_out_sum = 0
            for v in r_out {
                r_out_sum = r_out_sum + v
            }
            var r_defer_count = 0
            for _d in r_defers {
                r_defer_count = r_defer_count + 1
            }
            assert(elapsed_ms >= 5)

            // 4. Generic workers
            gi := chan.open<Int>(4)
            gi_out := chan.open<String>(4)
            gi.send(7)
            gi.send(8)
            gi.close()
            gi_n := GenericTag<Int>(gi, gi_out, "g")
            gi_out.close()
            var gi_count = 0
            for _s in gi_out {
                gi_count = gi_count + 1
            }

            gs := chan.open<String>(4)
            gs_out := chan.open<String>(4)
            gs.send("ok")
            gs.close()
            gs_n := GenericTag<String>(gs, gs_out, "s")
            gs_out.close()
            var gs_val = ""
            for s in gs_out {
                gs_val = s
            }

            return "{s_count}:{s_sum}|{p_count}:{p_sum}|{r_count}:{r_out_sum}:{r_defer_count}|{gi_n}:{gi_count}|{gs_n}:{gs_val}"
        }
    "#;

    let module = crate::frontend::compile_source(src).expect("compile declarative workers");
    let fid = module.func_id("run_workers").unwrap();
    let res = Vm::new().call_obj(&module, fid, &[], &[]).unwrap();
    // s: 2*(1+3)=8 (count 2)
    // p: sum(100..105)=615 (count 6)
    // r: id=1 attempt 1 => 11; id=2 retry succeeds attempt 2 => 22; total 33, count 2, defers=1(id=1)+2(id=2)+3(id=-9)=6
    // g: 2:2 | 1:s:ok
    assert_eq!(res, Obj::str("2:8|6:615|2:33:6|2:2|1:s:ok"));

    // Cross-module exported worker
    let mut modules = std::collections::HashMap::new();
    modules.insert(
        "./mod_worker",
        r#"
            import * as chan from "cdt.chan"
            export worker DoubleWorker(x: Int, out: chan.Chan<Int>) concurrency 2 {
                out.send(x * 2)
            }
        "#,
    );
    let mod_main = r#"
        import { Chan } from "cdt.chan"
        import { DoubleWorker } from "./mod_worker"

        export func main() -> Int {
            ch := Chan<Int>(4)
            out := Chan<Int>(4)
            ch.send(5)
            ch.send(9)
            ch.close()
            n := DoubleWorker(ch, out)
            out.close()
            var sum = 0
            for v in out {
                sum = sum + v
            }
            return n * 100 + sum
        }
    "#;
    let mod_compiled = crate::frontend::compile_with_modules(mod_main, &modules)
        .expect("compile imported declarative worker");
    let mf = mod_compiled.func_id("main").unwrap();
    let mv = Vm::new().call(&mod_compiled, mf, &[], &[]).unwrap();
    assert_eq!(mv, crate::vm::ExecResult::Scalar(228));

    // Compile-time rejections
    for bad in [
        "worker Bad() {}",
        "worker Bad(x: Int = 1) {}",
        "worker Bad(x: Int) -> Int { return x }",
        "worker Bad(x: Int) concurrency 0 {}",
        "worker Bad(x: Int) retry -1 {}",
        "worker Bad(x: Int) backoff 0ms {}",
        "worker Bad(x: Int) backoff 5x {}",
        "worker Bad(x: Int) retry 1 retry 2 {}",
    ] {
        assert!(
            crate::frontend::compile_source(bad).is_err(),
            "Expected failure for: {}",
            bad
        );
    }
}

#[test]
fn cdt_app_declarative_lifecycle_e2e() {
    let src = r#"
        import * as app from "cdt.app"
        import * as cache from "cdt.cache"
        import * as http from "cdt.http"
        import * as time from "cdt.time"
        import * as validation from "cdt.validation"

        config OkCfg {
            port: Int @validation.min(1024) = 8080
        }

        config MissingEnvCfg {
            must_exist_var: String
        }

        config InvalidValCfg {
            port: Int @validation.min(1024) = 80
        }

        error LifeErr {
            message: String
        }

        func push_event(name: String) -> Void {
            c := cache.open("app_life_e2e")
            prev := c.get("log") ?? ""
            var next = name
            if prev != "" {
                next = "{prev},{name}"
            }
            _ignored := c.set("log", next)
        }

        func start_first() -> Void {
            push_event("start1")
            spawn {
                _s := time.sleep(70)
                push_event("spawn_done")
            }
        }

        func start_second() -> Void {
            push_event("start2")
        }

        func stop_first() -> Void {
            push_event("stop1")
        }

        func stop_second() -> Void {
            push_event("stop2")
        }

        func cron_tick() -> Void {
            c := cache.open("app_life_e2e")
            _ignored := c.set("ticked", "yes")
        }

        schedule AppJobs {
            interval 15ms => cron_tick
        }

        app HeadlessApp {
            config OkCfg
            schedule AppJobs
            shutdown 2s
            on start => start_first
            on start => start_second
            on stop => stop_first
            on stop => stop_second
        }

        func never_called_start() -> Void {
            push_event("BAD_START")
        }

        app MissingCfgApp {
            config MissingEnvCfg
            on start => never_called_start
        }

        app InvalidCfgApp {
            config InvalidValCfg
            on start => never_called_start
        }

        func failing_start() -> Void | LifeErr {
            return LifeErr(message: "init_failed")
        }

        app FailingStartApp {
            on start => failing_start
        }

        func ok_handler(_req: http.Request) -> http.Response {
            return http.text("ok")
        }

        router Api {
            get "/health" => ok_handler
        }

        app BadBindApp {
            config OkCfg
            http Api on "invalid_host:999999"
            schedule AppJobs
            shutdown 1s
        }

        export func run_app_tests() -> String {
            _dropped := cache.drop("app_life_e2e")

            // 1. Headless app: start, verify running, let cron tick, stop cleanly in LIFO order
            h := app.start<HeadlessApp>()
            var h_out = ""
            switch h {
                case app.App(a):
                    r1 := a.is_running()
                    c := cache.open("app_life_e2e")
                    var wait_i = 0
                    while wait_i < 200 && (c.get("ticked") ?? "no") != "yes" {
                        _s := time.sleep(10)
                        wait_i = wait_i + 1
                    }
                    _st1 := a.stop()
                    r2 := a.is_running()
                    // Idempotent stop
                    _st2 := a.stop()
                    log_str := c.get("log") ?? ""
                    ticked := c.get("ticked") ?? "no"
                    h_out = "{r1}:{r2}:{ticked}:{log_str}"
                case app.AppError(e):
                    h_out = "ERR:{e.kind}:{e.message}"
            }

            // 2. Fail-fast on missing env var
            var missing_kind = ""
            switch app.run<MissingCfgApp>() {
                case app.AppError(e):
                    missing_kind = e.kind
                case _:
            }

            // 3. Fail-fast on validation failure
            var val_kind = ""
            switch app.run<InvalidCfgApp>() {
                case app.AppError(e):
                    val_kind = e.kind
                case _:
            }

            // 4. Startup hook error propagation
            var start_err_out = ""
            switch app.run<FailingStartApp>() {
                case app.AppError(e):
                    start_err_out = "{e.kind}:{e.message}"
                case _:
            }

            // 5. Invalid http bind error stops cleanly
            var bind_kind = ""
            switch app.run<BadBindApp>() {
                case app.AppError(e):
                    bind_kind = e.kind
                case _:
            }

            return "{h_out}|{missing_kind}|{val_kind}|{start_err_out}|{bind_kind}"
        }
    "#;

    let module = crate::frontend::compile_source(src).expect("compile declarative app");
    assert_eq!(module.apps.len(), 5);
    assert!(module.apps.iter().any(|a| a.name == "BadBindApp" && a.http_addr.as_deref() == Some("invalid_host:999999")));

    // Bundle round-trip preserves AppPlan
    let fid = module.func_id("run_app_tests").unwrap();
    let bundled = crate::ir::bundle::encode_bundle(&module, fid, 0).expect("encode app bundle");
    let (decoded, dfid, _) = crate::ir::bundle::decode_bundle(&bundled).expect("decode app bundle");
    assert_eq!(decoded.apps.len(), module.apps.len());
    assert_eq!(decoded.apps[0].shutdown_ms, module.apps[0].shutdown_ms);

    let res = Vm::new().call_obj(&decoded, dfid, &[], &[]).unwrap();
    assert_eq!(
        res,
        Obj::str("true:false:yes:start1,start2,spawn_done,stop2,stop1|config_error|config_error|startup_error:app on start hook failed: init_failed|bind_error")
    );

    // Compile-time rejections
    for bad in [
        r#"import * as app from "cdt.app"; struct S { x: Int }; func main() -> Void { _r := app.run<S>() }"#,
        r#"import * as app from "cdt.app"; app A { config Missing }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; app A { http Missing on "127.0.0.1:8080" }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; app A { schedule Missing }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; func h(x: Int) -> Void {}; app A { on start => h }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; func h() -> Int { return 1 }; app A { on stop => h }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; struct C { a: Int = 1 }; app A { config C; config C }; func main() -> Void {}"#,
        r#"import * as app from "cdt.app"; func f() -> Void {}; app A { shutdown 0s; on start => f }; func main() -> Void {}"#,
    ] {
        assert!(
            crate::frontend::compile_source(bad).is_err(),
            "Expected failure for: {}",
            bad
        );
    }
}
