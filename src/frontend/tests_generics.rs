use std::collections::HashMap;

use crate::frontend::{check_source, compile_source, compile_with_modules};
use crate::vm::Vm;
use crate::vm::value::Obj;

#[test]
fn string_interpolation_fields_escapes_closures_and_errors() {
    let src = r#"
struct Row {
    name: String,
    count: Int,
}

func greet(name: String, row: Row, ok: Bool, ratio: Float) -> String {
    head := "hello {name}!"
    mid := "row={row.name}:{row.count},ok={ok},r={ratio}"
    escaped := "lit:\{name\} \{bad {123} {a+b} {unclosed"
    fn := () => "cap:{head}"
    cap := fn()
    return "{cap}|{mid}|{escaped}"
}

func run_greet() -> String {
    return greet("world", Row("alpha", 7), true, 1.5)
}
"#;
    let module = compile_source(src).expect("interpolation program should compile");
    let mut vm = Vm::new();
    let res = vm
        .call_obj(&module, module.func_id("run_greet").unwrap(), &[], &[])
        .unwrap();
    let Obj::Str(s) = res else {
        panic!("expected string, got {:?}", res);
    };
    assert_eq!(
        s.as_ref(),
        "cap:hello world!|row=alpha:7,ok=true,r=1.5|lit:{name} {bad {123} {a+b} {unclosed"
    );

    assert!(check_source("func f() -> String { return \"hi {missing}\" }").is_err());
    assert!(
        check_source("struct R { a: Int }\nfunc f(r: R) -> String { return \"{r.nope}\" }").is_err()
    );
}

#[test]
fn generics_structs_funcs_inference_nested_spread_fold_and_errors() {
    let src = r#"
struct Box<T> {
    value: T,
}

struct Pair<A, B> {
    first: A,
    second: B,
}

func id<T>(x: T) -> T {
    return x
}

func make_pair<A, B>(a: A, b: B) -> Pair<A, B> {
    return Pair(a, b)
}

func fold<T, Acc>(items: [T], init: Acc, f: (Acc, T) -> Acc) -> Acc {
    var acc = init
    for item in items {
        acc = f(acc, item)
    }
    return acc
}

func test_all() -> String {
    b1 := Box<Int>(10)
    b2 := Box(..b1, value: 43)
    b_str := Box("ok")
    p := make_pair(id<Int>(b2.value), id(b_str.value))
    nested := Box([Pair("k1", 2), Pair("k2", 5)])
    sum := fold(nested.value, 0, (acc: Int, item: Pair<String, Int>) => acc + item.second)
    return "{p.first}:{p.second}:{sum}"
}
"#;
    let module = compile_source(src).expect("generics program should compile");
    let mut vm = Vm::new();
    let res = vm
        .call_obj(&module, module.func_id("test_all").unwrap(), &[], &[])
        .unwrap();
    match res {
        Obj::Str(s) => assert_eq!(s.as_ref(), "43:ok:7"),
        other => panic!("expected Str, got {:?}", other),
    }

    // Wrong generic arity
    assert!(
        check_source("struct Box<T> { v: T }\nfunc f() -> Box<Int, String> { return Box(1) }")
            .is_err()
    );
    // Conflicting inferred T
    assert!(
        check_source(
            "func same<T>(a: T, b: T) -> T { return a }\nfunc f() -> Int { return same(1, \"x\") }"
        )
        .is_err()
    );
    // Cannot infer unmentioned T
    assert!(
        check_source("func empty<T>() -> [T] { return [] }\nfunc f() -> [Int] { return empty() }")
            .is_err()
    );
    // First-class reference to uninstantiated generic func
    assert!(
        check_source("func id<T>(x: T) -> T { return x }\nfunc f() -> Int {\n g := id\n return 0\n}")
            .is_err()
    );
}

#[test]
fn generics_validation_json_and_switch_patterns() {
    let src = r#"
import "cdt.json" as json
import "cdt.validation" as val

struct User {
    name: String @v_ok @val.min(2),
}

[#validator]
func v_ok(s: String) -> Void | val.Issue {
    if s == "bad" { return val.Issue("bad", "reserved name") }
}

struct Page<T> {
    items: [T],
    total: Int @val.min(1),
}

func make_page_or_err(ok: Bool) -> Page<User> | val.ValidationError {
    if ok {
        return val.check(Page<User>([User("Ada")], 1))
    }
    return val.check(Page([User("bad")], 0))
}

func test_inferred_switch_inner(ok: Bool) -> String | IndexError {
    switch make_page_or_err(ok) {
        case Page(p):
            u := p.items[0]?
            return "inferred:{u.name}:{p.total}"
        case val.ValidationError(e):
            n := e.issues.len()
            return "err:{n}"
    }
    return ""
}

func test_explicit_and_inferred_switch(ok: Bool) -> String {
    switch test_inferred_switch_inner(ok) {
        case IndexError(e): return e.message
        case String(s): return s
    }
    return ""
}

func test_explicit_case_and_json_inner() -> String | json.EncodeError | json.DecodeError | IndexError {
    res := make_page_or_err(true)
    var label = ""
    switch res {
        case Page<User>(p): label = json.encode(p)?
        case val.ValidationError(_): label = "err"
    }
    decoded := json.decode<Page<User>>(label)?
    u := decoded.items[0]?
    return u.name
}

func test_explicit_case_and_json() -> String {
    switch test_explicit_case_and_json_inner() {
        case json.EncodeError(e): return "enc:{e.message}"
        case json.DecodeError(e): return "dec:{e.message}"
        case IndexError(e): return "idx:{e.message}"
        case String(s): return s
    }
    return ""
}
"#;
    let module = compile_source(src).expect("generic json + validation + switch should compile");
    let mut vm = Vm::new();

    let s1 = vm
        .call_obj(
            &module,
            module.func_id("test_explicit_and_inferred_switch").unwrap(),
            &[1],
            &[],
        )
        .unwrap();
    assert_eq!(
        match s1 {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "inferred:Ada:1"
    );

    let s2 = vm
        .call_obj(
            &module,
            module.func_id("test_explicit_and_inferred_switch").unwrap(),
            &[0],
            &[],
        )
        .unwrap();
    assert_eq!(
        match s2 {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "err:2"
    );

    let s3 = vm
        .call_obj(
            &module,
            module.func_id("test_explicit_case_and_json").unwrap(),
            &[],
            &[],
        )
        .unwrap();
    assert_eq!(
        match s3 {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "Ada"
    );

    // Ambiguous bare `case Page(p):` when two distinct instantiations exist in union
    let ambig = r#"
struct Page<T> { x: T }
func g(b: Bool) -> Page<Int> | Page<String> {
    if b { return Page(1) }
    return Page("a")
}
func f(b: Bool) -> Int {
    switch g(b) {
        case Page(p): return 1
    }
    return 0
}
"#;
    assert!(check_source(ambig).is_err());
}

#[test]
fn list_index_try_methods_float_and_tuple_destructuring() {
    let src = r#"
func try_index(xs: [Int], i: Int) -> Int | IndexError {
    v := xs[i]?
    return v * 10
}

func catch_index(xs: [Int], i: Int) -> String {
    switch xs[i] {
        case IndexError(e): return e.message
        case Int(v): return v.to_string()
    }
    return ""
}

func test_list_methods_inner() -> String | IndexError {
    xs := [10, 20, 30, 40]
    g0 := xs.get(1) ?? -1
    g_oob := xs.get(99) ?? -1
    g_neg := xs.get(-1) ?? -1

    var opts: [Int?] = [1, nil]
    o_hit := opts.get(0) ?? -1
    o_nil := opts.get(1) ?? -1
    o_oob := opts.get(5) ?? -1

    has20 := xs.contains(20)
    has99 := xs.contains(99)
    sl := xs.slice(1, 3)
    rev := sl.reverse()

    words := ["  hi ", "THERE"]
    w0 := words[0]?
    w1 := words[1]?
    trimmed := w0.trim()
    low := w1.lower()
    up := trimmed.upper()
    joined := [up, low].join("-")

    var enum_acc = ""
    for (i, w) in rev.enumerate() {
        enum_acc = "{enum_acc}{i}={w};"
    }

    (a, b) := (123, "pair")
    var sw = ""
    switch (4, 5) {
        case (x, y):
            sum_xy := x + y
            sw = "{sum_xy}"
    }

    return "{g0},{g_oob},{g_neg}|{o_hit},{o_nil},{o_oob}|{has20},{has99}|{joined}|{enum_acc}|{a}:{b}:{sw}"
}

func test_list_methods() -> String {
    switch test_list_methods_inner() {
        case IndexError(e): return e.message
        case String(s): return s
    }
    return ""
}

func test_float_lists_inner() -> String | IndexError {
    fs := [1.5, 2.5]
    fs.append(3.5)
    has := fs.contains(2.5)
    var sum = 0.0
    for f in fs {
        sum = sum + f
    }
    big := fs |> filter(x => x > 2.0) |> map(x => x * 2.0)
    blen := big.len()
    b0 := big[0]?
    b1 := big[1]?
    return "{sum}:{has}:{blen}:{b0}:{b1}"
}

func test_float_lists() -> String {
    switch test_float_lists_inner() {
        case IndexError(e): return e.message
        case String(s): return s
    }
    return ""
}

func run_oob() -> String { return catch_index([5, 6], 5) }
func run_neg() -> String { return catch_index([5, 6], -1) }
func run_ok() -> String { return catch_index([5, 6], 1) }
func run_try() -> Int {
    switch try_index([3, 4], 9) {
        case IndexError(_): return -77
        case Int(n): return n
    }
    return 0
}
"#;
    let module = compile_source(src).expect("list/tuple/float program should compile");
    let mut vm = Vm::new();

    let s = vm
        .call_obj(&module, module.func_id("test_list_methods").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(
        match s {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "20,-1,-1|1,-1,-1|true,false|HI-there|0=30;1=20;|123:pair:9"
    );

    let fs = vm
        .call_obj(&module, module.func_id("test_float_lists").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(
        match fs {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "7.5:true:2:5:7"
    );

    let oob = vm.call_obj(&module, module.func_id("run_oob").unwrap(), &[], &[]).unwrap();
    let neg = vm.call_obj(&module, module.func_id("run_neg").unwrap(), &[], &[]).unwrap();
    let ok = vm.call_obj(&module, module.func_id("run_ok").unwrap(), &[], &[]).unwrap();
    let tr = vm.call_scalar(&module, module.func_id("run_try").unwrap(), &[], &[]).unwrap();
    assert!(matches!(oob, Obj::Str(ref st) if st.contains("out of bounds")));
    assert!(matches!(neg, Obj::Str(ref st) if st.contains("out of bounds")));
    assert!(matches!(ok, Obj::Str(ref st) if st.as_ref() == "6"));
    assert_eq!(tr, (-77i64) as u64);

    // Reject index assignment arr[0] = v
    let err_idx = check_source("func f() -> Int {\n xs := [1, 2]\n xs[0] = 3\n return 0\n}")
        .expect_err("index assign should fail");
    assert!(err_idx.contains("assign") || err_idx.contains("Index") || err_idx.contains("Cannot"), "got: {}", err_idx);
    // Reject .join on non-string list
    assert!(check_source("func f() -> String { return [1, 2].join(\",\") }").is_err());
}

#[test]
fn generic_exports_across_modules() {
    let mut modules = HashMap::new();
    modules.insert(
        "./box",
        "export struct Wrap<T> { val: T }\nexport func unwrap_or<T>(x: Wrap<T>, f: (T) -> Int) -> Int { return f(x.val) }",
    );
    let main = "import { Wrap, unwrap_or } from \"./box\"\nexport func main() -> Int {\n w := Wrap(21)\n return unwrap_or(w, (v: Int) => v * 2)\n}";
    let m = compile_with_modules(main, &modules).expect("cross-module generics should compile");
    let mut vm = Vm::new();
    let out = vm.call_scalar(&m, m.func_id("main").unwrap(), &[], &[]).unwrap();
    assert_eq!(out, 42);
}

#[test]
fn print_prelude_errors_ceil_and_string_parse() {
    let src = r#"
export func test_print_and_ceil() -> String {
    print(42)
    print(3.5)
    print(true)
    print("hello")
    print([1, 2, 3])
    c1 := (3.1).ceil()
    c2 := (-2.9).ceil()
    c3 := (4.0).ceil()
    return "{c1}:{c2}:{c3}"
}

func use_ctors(flag: Int) -> String | IndexError | KeyError | ParseError {
    if flag == 0 {
        e1 := IndexError(5, 3, "pos idx")
        return e1
    }
    if flag == 1 {
        return KeyError(message: "named key", key: "k1")
    }
    if flag == 2 {
        return ParseError("xyz", "bad parse")
    }
    return "ok"
}

export func inspect_ctor(flag: Int) -> String {
    switch use_ctors(flag) {
        case String(s): return s
        case IndexError(e): return "idx:{e.index}:{e.length}:{e.message}"
        case KeyError(e): return "key:{e.key}:{e.message}"
        case ParseError(e): return "parse:{e.value}:{e.message}"
    }
    return ""
}

func parse_both(si: String, sf: String) -> Float | ParseError {
    n := si.to_int()?
    f := sf.to_float()?
    return n.to_float() + f
}

export func test_parse(si: String, sf: String) -> String {
    switch parse_both(si, sf) {
        case Float(v): return "{v}"
        case ParseError(e): return "err:{e.value}:{e.message}"
    }
    return ""
}
"#;
    let m = compile_source(src).expect("should compile");
    let mut vm = Vm::new();

    let s = vm.call_obj(&m, m.func_id("test_print_and_ceil").unwrap(), &[], &[]).unwrap();
    assert!(matches!(s, Obj::Str(ref st) if st.as_ref() == "4:-2:4"), "got {:?}", s);

    let r0 = vm.call_obj(&m, m.func_id("inspect_ctor").unwrap(), &[0], &[]).unwrap();
    let r1 = vm.call_obj(&m, m.func_id("inspect_ctor").unwrap(), &[1], &[]).unwrap();
    let r2 = vm.call_obj(&m, m.func_id("inspect_ctor").unwrap(), &[2], &[]).unwrap();
    assert!(matches!(r0, Obj::Str(ref st) if st.as_ref() == "idx:5:3:pos idx"));
    assert!(matches!(r1, Obj::Str(ref st) if st.as_ref() == "key:k1:named key"));
    assert!(matches!(r2, Obj::Str(ref st) if st.as_ref() == "parse:xyz:bad parse"));

    let p_ok = vm
        .call_obj(&m, m.func_id("test_parse").unwrap(), &[], &[Obj::str("10"), Obj::str("2.5")])
        .unwrap();
    let p_bad_int = vm
        .call_obj(&m, m.func_id("test_parse").unwrap(), &[], &[Obj::str("abc"), Obj::str("1.0")])
        .unwrap();
    let p_bad_float = vm
        .call_obj(&m, m.func_id("test_parse").unwrap(), &[], &[Obj::str("10"), Obj::str("NaN")])
        .unwrap();
    assert!(matches!(p_ok, Obj::Str(ref st) if st.as_ref() == "12.5"));
    assert!(matches!(p_bad_int, Obj::Str(ref st) if st.starts_with("err:abc:")));
    assert!(matches!(p_bad_float, Obj::Str(ref st) if st.starts_with("err:NaN:")));

    // Reject printing Void
    assert!(check_source("func f() -> Void {} func g() -> Void { print(f()) }").is_err());
}

#[test]
fn map_full_methods_indexing_iteration_and_cow() {
    let src = r#"
func lookup_or_err(m: Map<String, Int>, k: String) -> Int | KeyError {
    v := m[k]?
    return v * 2
}

export func test_string_int_map() -> String {
    m := Map.from([("b", 2), ("a", 1), ("c", 3)])
    m.set("d", 4)
    has_a := m.contains_key("a")
    has_z := m.contains_key("z")
    copy := m
    removed := copy.remove("a") ?? -1
    copy_has_a := copy.contains_key("a")
    orig_has_a := m.contains_key("a")
    ks := m.keys().join(",")

    var sum = 0
    for v in m.values() {
        sum = sum + v
    }

    var order = ""
    for (k, v) in m {
        order = "{order}{k}={v};"
    }

    var entries_acc = 0
    for pair in m.entries() {
        (ek, ev) := pair
        entries_acc = entries_acc + ek.len() + ev
    }

    g1 := match_lookup(m, "c")
    g2 := match_lookup(m, "missing")
    return "{has_a}:{has_z}:{removed}:{copy_has_a}:{orig_has_a}:{ks}:{sum}:{order}:{entries_acc}:{g1}:{g2}"
}

func match_lookup(m: Map<String, Int>, k: String) -> String {
    switch lookup_or_err(m, k) {
        case Int(n): return "{n}"
        case KeyError(e): return "err:{e.key}"
    }
    return ""
}

func idx_int_str(m: Map<Int, String>, k: Int) -> String | KeyError {
    v := m[k]?
    return v
}

export func test_bool_float_and_int_str_maps() -> String {
    bf := Map<Bool, Float>()
    bf.set(true, 1.5)
    bf.set(false, 2.5)
    got_t := bf.get(true) ?? 0.0
    rem_f := bf.remove(false) ?? 0.0
    len_bf := bf.len()

    mut_is := Map<Int, String>()
    mut_is.set(10, "ten")
    mut_is.set(20, "twenty")
    s10 := mut_is.get(10) ?? "none"
    var s20 = ""
    switch idx_int_str(mut_is, 20) {
        case String(s): s20 = s
        case KeyError(e): s20 = "err:{e.key}"
    }

    // Optional values should not double-wrap in .get / .remove
    opt_m := Map<String, Int?>()
    opt_m.set("present_nil", nil)
    opt_m.set("present_val", 7)
    has_pn := opt_m.contains_key("present_nil")
    pn_or := opt_m.get("present_nil") ?? -9
    pv_rem := opt_m.remove("present_val") ?? -1

    return "{got_t}:{rem_f}:{len_bf}:{s10}:{s20}:{has_pn}:{pn_or}:{pv_rem}"
}
"#;
    let m = compile_source(src).expect("map program should compile");
    let mut vm = Vm::new();

    let res1 = vm.call_obj(&m, m.func_id("test_string_int_map").unwrap(), &[], &[]).unwrap();
    assert_eq!(
        match res1 {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "true:false:1:false:true:b,a,c,d:10:b=2;a=1;c=3;d=4;:14:6:err:missing"
    );

    let res2 = vm
        .call_obj(&m, m.func_id("test_bool_float_and_int_str_maps").unwrap(), &[], &[])
        .unwrap();
    assert_eq!(
        match res2 {
            Obj::Str(s) => s.to_string(),
            o => panic!("{:?}", o),
        },
        "1.5:2.5:1:ten:twenty:true:-9:7"
    );

    // Reject invalid Map key types
    assert!(check_source("func f() -> Map<Float, Int> { return Map<Float, Int>() }").is_err());
    assert!(check_source("func f() { m := Map.from([(1.5, 2)]) }").is_err());
    assert!(check_source("func f() -> Map<[Int], Int> { return Map<[Int], Int>() }").is_err());
}

#[test]
fn immutable_bindings_and_typed_empty_list_ctor() {
    let src = r#"
struct Item {
    id: Int
}

func make_empty<T>() -> [T] {
    return [T]()
}

func test_empty_and_var() -> String {
    ints := [Int]()
    ints.append(7)
    ints.append(8)

    strs := List<String>()
    strs.append("ok")

    items := make_empty<Item>()
    items.append(Item(id: 3))

    var total = 0
    for n in ints {
        total = total + n
    }
    first_item := items.get(0)?.id ?? 0
    slen := strs.len()
    return "{total}:{slen}:{first_item}"
}
"#;
    let m = compile_source(src).expect("typed empty list and var should compile");
    let mut vm = Vm::new();
    let res = vm
        .call_obj(&m, m.func_id("test_empty_and_var").unwrap(), &[], &[])
        .unwrap();
    match res {
        Obj::Str(s) => assert_eq!(s.as_str(), "15:1:3"),
        other => panic!("expected String, got {:?}", other),
    }

    let imm_err = check_source("func f() -> Void {\n x := 1\n x = 2\n}")
        .expect_err("reassigning := binding must fail");
    assert!(imm_err.contains("immutable"), "got: {imm_err}");

    assert!(check_source("func f(x: Int) -> Void {\n x = 2\n}").is_err());
    assert!(check_source("func f() -> Void {\n (a, b) := (1, 2)\n a = 3\n}").is_err());
    assert!(check_source("func f() -> Void {\n for i in 3 { i = 1 }\n}").is_err());
    assert!(check_source("func f() { xs := [Void]() }").is_err());
}
