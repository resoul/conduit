use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::parser::Parser;
use conduit::reachability::ReachabilityAnalyzer;

fn check_source(src: &str) -> Result<(), String> {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&main, src);
    let root = loader.load_project(&main).map_err(|e| e.message)?;
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).map_err(|e| e.message)
}

fn run_source(src: &str) -> Result<(), String> {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&main, src);
    let root = loader.load_project(&main).map_err(|e| e.message)?;
    let mut checker = TypeChecker::new();
    checker
        .check_project(&root, &loader)
        .map_err(|e| e.message)?;
    let mut interp = Interpreter::new();
    interp.run_project(&root, &loader).map_err(|e| e.message)
}

#[test]
fn test_criterion_1_non_capturing_lambdas_everywhere() {
    run_source(
        r#"
        func apply(x: Int, f: (Int) -> Int) -> Int {
            return f(x)
        }

        func assert_eq_str(actual: String, expected: String) {
            if actual != expected {
                fail := 1 / 0
            }
        }

        func main() {
            double := func(x: Int) -> Int { return x * 2 }
            assert_eq_str(double(5).to_string(), "10")

            direct := (func(a: Int, b: Int) -> Int { return a + b })(3, 4)
            assert_eq_str(direct.to_string(), "7")

            nums := [1, 2, 3, 4, 5]
            doubled := map(nums, func(x: Int) -> Int { return x * 10 })
            evens := filter(nums, func(x: Int) -> Bool { return x % 2 == 0 })
            piped_mapped := nums |> map(func(x: Int) -> Int { return x + 1 })
            assert_eq_str(doubled.len().to_string(), "5")
            assert_eq_str((doubled.get(4) ?? 0).to_string(), "50")
            assert_eq_str(evens.len().to_string(), "2")
            assert_eq_str((evens.get(0) ?? 0).to_string(), "2")
            assert_eq_str((evens.get(1) ?? 0).to_string(), "4")
            assert_eq_str((piped_mapped.get(0) ?? 0).to_string(), "2")

            piped := 7 |> func(x: Int) -> Int { return x + 3 }
            assert_eq_str(piped.to_string(), "10")

            via_hof := apply(9, func(n: Int) -> Int { return n - 1 })
            assert_eq_str(via_hof.to_string(), "8")

            void_fn := func(msg: String) {
                assert_eq_str(msg, "hello-lambda")
            }
            void_fn("hello-lambda")
        }
    "#,
    )
    .unwrap();
}

#[test]
fn test_criterion_2_capturing_closures_and_snapshot_isolation() {
    run_source(
        r#"
        func make_adder(base: Int) -> (Int) -> Int {
            return func(x: Int) -> Int { return base + x }
        }

        func make_prefixer(prefix: String, sep: String) -> (String) -> String {
            return func(name: String) -> String {
                return prefix + sep + name
            }
        }

        func make_nested(a: Int) -> (Int) -> (Int) -> Int {
            return func(b: Int) -> (Int) -> Int {
                return func(c: Int) -> Int {
                    return a + b + c
                }
            }
        }

        func assert_eq_str(actual: String, expected: String) {
            if actual != expected {
                fail := 1 / 0
            }
        }

        func main() {
            add10 := make_adder(10)
            add20 := make_adder(20)
            assert_eq_str(add10(5).to_string(), "15")
            assert_eq_str(add20(5).to_string(), "25")

            greet := make_prefixer("Hello", ", ")
            assert_eq_str(greet("Conduit"), "Hello, Conduit")

            nested := make_nested(1)(2)(3)
            assert_eq_str(nested.to_string(), "6")

            var factor = 3
            scale := func(x: Int) -> Int { return x * factor }
            factor = 100
            assert_eq_str(scale(4).to_string(), "12")
            assert_eq_str(factor.to_string(), "100")
        }
    "#,
    )
    .unwrap();
}

#[test]
fn test_criterion_3_cross_module_closure_uses_defining_module_helpers_and_locals() {
    let mut loader = ModuleLoader::new();
    let mod_a = PathBuf::from("/virtual/a.cdt");
    let mod_b = PathBuf::from("/virtual/b.cdt");

    loader.with_virtual_file(
        &mod_a,
        r#"
        func private_format(tag: String, val: String) -> String {
            return "[" + tag + "] " + val
        }

        export func make_formatter(tag: String) -> (String) -> String {
            secret := "ok:"
            return func(msg: String) -> String {
                return private_format(tag, secret + msg)
            }
        }
    "#,
    );

    loader.with_virtual_file(
        &mod_b,
        r#"
        import "./a" as a

        func private_format(tag: String, val: String) -> String {
            return "WRONG:" + tag + ":" + val
        }

        func main() {
            fmt := a.make_formatter("AUTH")
            if fmt("signed-in") != "[AUTH] ok:signed-in" {
                fail := 1 / 0
            }
        }
    "#,
    );

    let root = loader.load_project(&mod_b).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();
    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

#[test]
fn test_criterion_4_closure_typechecker_rules() {
    let err = check_source(
        r#"
        func main() {
            f := func(x: Int) -> String { return x }
            print(f(1))
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("String") || err.contains("Int"));

    let err = check_source(
        r#"
        func main() {
            f := func(x: Int) -> Int {
                if x > 0 {
                    return x
                }
            }
            print(f(1).to_string())
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("return"));

    let err = check_source(
        r#"
        error Boom { msg: String }

        func fail_if(flag: Bool) -> Int | Boom {
            if flag {
                return Boom(msg: "bad")
            }
            return 1
        }

        func main() -> Int | Boom {
            f := func(b: Bool) -> Int {
                v := fail_if(b)?
                return v
            }
            return f(true)
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("Boom") || err.contains("propagate"));

    let err = check_source(
        r#"
        func main() {
            for i in 0..3 {
                f := func() {
                    break
                }
                f()
            }
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("break") || err.contains("loop"));

    let err = check_source(
        r#"
        func main() {
            for i in 0..3 {
                f := func() {
                    continue
                }
                f()
            }
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("continue") || err.contains("loop"));

    let err = check_source(
        r#"
        error Oops { code: Int }

        func fallible() -> Int | Oops {
            return Oops(code: 1)
        }

        func main() {
            f := func() -> Int {
                res := fallible()
                return 0
            }
            print(f().to_string())
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("Unused") || err.contains("handled") || err.contains("res"));

    run_source(
        r#"
        error Oops { code: Int }

        func fallible(flag: Bool) -> Int | Oops {
            if flag {
                return Oops(code: 42)
            }
            return 7
        }

        func main() {
            safe := func(flag: Bool) -> Int | Oops {
                v := fallible(flag)?
                return v + 1
            }
            switch safe(true) {
                case Int(n):
                    fail := 1 / 0
                case Oops(e):
                    if e.code != 42 {
                        fail := 1 / 0
                    }
            }
            switch safe(false) {
                case Int(n):
                    if n != 8 {
                        fail := 1 / 0
                    }
                case Oops(e):
                    fail := 1 / 0
            }
        }
    "#,
    )
    .unwrap();
}

#[test]
fn test_criterion_5_reachability_scans_closures() {
    let mut loader = ModuleLoader::new();
    let helper = PathBuf::from("/virtual/helper.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &helper,
        r#"
        export type Payload {
            value: Int
        }

        export func inside_closure(p: Payload) -> Int {
            return p.value + 1
        }

        export func never_used() -> Int {
            return 999
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./helper" as h

        func main() {
            fn := func(p: h.Payload) -> Int {
                return h.inside_closure(p)
            }
            print(fn(h.Payload(value: 41)).to_string())
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let registry = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &registry);
    let report = analyzer.analyze(&root, &[]);

    assert!(report.is_function_reachable(&helper, "inside_closure"));
    assert!(!report.is_function_reachable(&helper, "never_used"));

    let mut lexer = Lexer::new("func main() { f := func(x: Int) -> Int { return x } }");
    let tokens = lexer.tokenize_all().unwrap();
    let mut parser = Parser::new(tokens);
    let parsed = parser.parse_program();
    assert!(parsed.is_ok());
}

#[test]
fn test_criterion_6_string_methods_and_len_vs_char_count() {
    run_source(
        r#"
        func join_pipe(parts: [String]) -> String {
            var acc = ""
            for i in 0..parts.len() {
                part := parts.get(i) ?? ""
                if i > 0 {
                    acc = acc + "|"
                }
                acc = acc + part
            }
            return acc
        }

        func assert_eq_str(actual: String, expected: String) {
            if actual != expected {
                fail := 1 / 0
            }
        }

        func main() {
            assert_eq_str(join_pipe("a,b,c".split(",")), "a|b|c")
            assert_eq_str(join_pipe("abc".split(",")), "abc")
            assert_eq_str(join_pipe("".split(",")), "")
            assert_eq_str(join_pipe("ab".split("")), "a|b")

            assert_eq_str("hello world".starts_with("hello").to_string(), "true")
            assert_eq_str("hello world".starts_with("").to_string(), "true")
            assert_eq_str("hello world".starts_with("world").to_string(), "false")
            assert_eq_str("hello world".ends_with("world").to_string(), "true")
            assert_eq_str("hello world".ends_with("").to_string(), "true")
            assert_eq_str("hello world".ends_with("hello").to_string(), "false")

            assert_eq_str("foo bar foo".replace("foo", "baz"), "baz bar baz")
            assert_eq_str("hello".replace("", "x"), "hello")

            assert_eq_str("  \t hello \n ".trim(), "hello")
            assert_eq_str("conduit".contains("du").to_string(), "true")
            assert_eq_str("conduit".contains("xyz").to_string(), "false")

            ascii := "abc"
            accent := "café"
            cyrillic := "привет"
            assert_eq_str(ascii.len().to_string() + ":" + ascii.char_count().to_string(), "3:3")
            assert_eq_str(accent.len().to_string() + ":" + accent.char_count().to_string(), "5:4")
            assert_eq_str(cyrillic.len().to_string() + ":" + cyrillic.char_count().to_string(), "12:6")
        }
    "#,
    )
    .unwrap();
}

#[test]
fn test_criterion_7_explicit_to_string_and_no_implicit_coercion() {
    run_source(
        r#"
        func assert_eq_str(actual: String, expected: String) {
            if actual != expected {
                fail := 1 / 0
            }
        }

        func main() {
            assert_eq_str(0.to_string(), "0")
            assert_eq_str(42.to_string(), "42")
            assert_eq_str((-5).to_string(), "-5")
            assert_eq_str(9223372036854775807.to_string(), "9223372036854775807")
            assert_eq_str(true.to_string(), "true")
            assert_eq_str(false.to_string(), "false")
            assert_eq_str("text".to_string(), "text")

            n := 7
            flag := true
            assert_eq_str("n=" + n.to_string() + ", flag=" + flag.to_string(), "n=7, flag=true")
        }
    "#,
    )
    .unwrap();

    let err = check_source(
        r#"
        func main() {
            s := "count=" + 42
            print(s)
        }
    "#,
    )
    .unwrap_err();
    assert!(err.contains("+") || err.contains("String") || err.contains("Int"));
}
