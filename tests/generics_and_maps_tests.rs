use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::parser::Parser;
use conduit::reachability::ReachabilityAnalyzer;

fn load_and_check(files: &[(&str, &str)]) -> Result<(ModuleLoader, PathBuf), String> {
    let mut loader = ModuleLoader::new();
    for (path, src) in files {
        loader.with_virtual_file(&PathBuf::from(path), *src);
    }
    let root = loader
        .load_project(&PathBuf::from(files[0].0))
        .map_err(|e| e.message)?;
    let mut checker = TypeChecker::new();
    checker
        .check_project(&root, &loader)
        .map_err(|e| e.message)?;
    Ok((loader, root))
}

fn check_project(files: &[(&str, &str)]) -> Result<(), String> {
    load_and_check(files).map(|_| ())
}

fn run_project(files: &[(&str, &str)]) -> Result<(), String> {
    let (loader, root) = load_and_check(files)?;
    let mut interp = Interpreter::new();
    interp.run_project(&root, &loader).map_err(|e| e.message)
}

fn check_single(src: &str) -> Result<(), String> {
    check_project(&[("/virtual/main.cdt", src)])
}

fn run_single(src: &str) -> Result<(), String> {
    run_project(&[("/virtual/main.cdt", src)])
}

#[test]
fn test_criterion_1_generic_structs_nesting_and_switch() {
    run_single(
        r#"
        type Box<T> {
            value: T
        }

        type Pair<A, B> {
            first: A,
            second: B
        }

        type Page<T> {
            items: [T],
            total: Int
        }

        type User {
            name: String
        }

        error FetchError {
            reason: String
        }

        func fetch_users(ok: Bool) -> Page<User> | FetchError {
            if !ok {
                return FetchError(reason: "offline")
            }
            return Page(items: [User(name: "Alice"), User(name: "Bob")], total: 2)
        }

        func assert_eq(a: String, b: String) {
            if a != b {
                boom := 1 / 0
            }
        }

        func main() {
            var int_box: Box<Int> = Box(value: 42)
            assert_eq(int_box.value.to_string(), "42")

            var nested: Box<[Pair<String, Int>]> = Box(value: [
                Pair(first: "apples", second: 3),
                Pair(first: "oranges", second: 5)
            ])
            first_pair := nested.value[0]
            switch first_pair {
                case Pair(p):
                    assert_eq(p.first, "apples")
                    assert_eq(p.second.to_string(), "3")
                case IndexError(e):
                    boom := 1 / 0
            }

            switch fetch_users(true) {
                case Page(p):
                    assert_eq(p.total.to_string(), "2")
                    first := p.items.get(0)
                    switch first {
                        case User(u): assert_eq(u.name, "Alice")
                        case nil: boom := 1 / 0
                    }
                case FetchError(e):
                    boom := 1 / 0
            }

            switch fetch_users(false) {
                case Page(p): boom := 1 / 0
                case FetchError(e): assert_eq(e.reason, "offline")
            }
        }
    "#,
    )
    .expect("generic structs, nested type args, and switch narrowing should run");

    let incompatible = r#"
        type Box<T> { value: T }
        func take_int_box(b: Box<Int>) -> Int { return b.value }
        func main() {
            s := Box(value: "hi")
            x := take_int_box(s)
        }
    "#;
    let err = check_single(incompatible).unwrap_err();
    assert!(err.contains("Box<String>") && err.contains("Box<Int>"));

    let wrong_arity = r#"
        type Box<T> { value: T }
        func main() {
            var x: Box<Int, String> = Box(value: 1)
        }
    "#;
    assert!(check_single(wrong_arity).is_err());

    let missing_type_args = r#"
        type Box<T> { value: T }
        func id(b: Box) -> Int { return 0 }
        func main() {}
    "#;
    assert!(check_single(missing_type_args).is_err());
}

#[test]
fn test_criterion_2_generic_functions_inference_and_combinators() {
    run_project(&[
        (
            "/virtual/main.cdt",
            r#"
            import "./collections" as col

            type User {
                name: String,
                age: Int
            }

            func make_pair<A, B>(a: A, b: B) -> (A, B) {
                return (a, b)
            }

            func assert_eq(a: String, b: String) {
                if a != b {
                    boom := 1 / 0
                }
            }

            func main() {
                n := col.first_or_default([10, 20, 30], 0)
                assert_eq(n.to_string(), "10")

                empty_fallback := col.first_or_default<Int>([], 99)
                assert_eq(empty_fallback.to_string(), "99")

                pair := make_pair("age", 30)
                (k, v) := pair
                assert_eq(k, "age")
                assert_eq(v.to_string(), "30")

                users := [
                    User(name: "Alice", age: 30),
                    User(name: "Bob", age: 19),
                    User(name: "Carol", age: 25)
                ]

                names := map(users, func(u: User) -> String { return u.name })
                assert_eq(names.len().to_string(), "3")

                adults := filter(users, func(u: User) -> Bool { return u.age >= 21 })
                assert_eq(adults.len().to_string(), "2")

                found := col.find(users, func(u: User) -> Bool { return u.name == "Bob" })
                switch found {
                    case User(u): assert_eq(u.age.to_string(), "19")
                    case nil: boom := 1 / 0
                }

                has_teen := col.any(users, func(u: User) -> Bool { return u.age < 20 })
                all_adults := col.all(users, func(u: User) -> Bool { return u.age >= 21 })
                assert_eq(has_teen.to_string(), "true")
                assert_eq(all_adults.to_string(), "false")

                total_age := col.fold(users, 0, func(acc: Int, u: User) -> Int { return acc + u.age })
                assert_eq(total_age.to_string(), "74")
            }
        "#,
        ),
        (
            "/virtual/collections.cdt",
            r#"
            export func first_or_default<T>(items: [T], fallback: T) -> T {
                v := items.get(0)
                return v ?? fallback
            }

            export func find<T>(items: [T], pred: (T) -> Bool) -> T? {
                for item in items {
                    if pred(item) {
                        return item
                    }
                }
                return nil
            }

            export func any<T>(items: [T], pred: (T) -> Bool) -> Bool {
                for item in items {
                    if pred(item) {
                        return true
                    }
                }
                return false
            }

            export func all<T>(items: [T], pred: (T) -> Bool) -> Bool {
                for item in items {
                    if !pred(item) {
                        return false
                    }
                }
                return true
            }

            export func fold<T, Acc>(items: [T], init: Acc, f: (Acc, T) -> Acc) -> Acc {
                var acc = init
                for item in items {
                    acc = f(acc, item)
                }
                return acc
            }
        "#,
        ),
    ])
    .expect("cross-module generic functions and combinators should run");

    let conflict = r#"
        func same<T>(a: T, b: T) -> T {
            return a
        }
        func main() {
            x := same(1, "two")
        }
    "#;
    let err = check_single(conflict).unwrap_err();
    assert!(err.contains("Conflicting types inferred for type parameter 'T'"));
}

#[test]
fn test_criterion_3_generics_privacy_and_parser_rejections() {
    let leak_in_return = r#"
        type Secret { code: Int }
        export type Box<T> { value: T }
        export func leak() -> Box<Secret> {
            return Box(value: Secret(code: 7))
        }
        func main() {}
    "#;
    assert!(check_single(leak_in_return)
        .unwrap_err()
        .contains("private"));

    let leak_in_nested_list = r#"
        type Secret { code: Int }
        export type Box<T> { value: T }
        export type Pair<A, B> { first: A, second: B }
        export func leak(x: Box<[Pair<String, Secret>]>) -> Int {
            return 1
        }
        func main() {}
    "#;
    assert!(check_single(leak_in_nested_list)
        .unwrap_err()
        .contains("private"));
    
    let mut lex = Lexer::new("func bad<>() {}");
    let tokens = lex.tokenize_all().unwrap();
    let mut parser = Parser::new(tokens);
    assert!(parser.parse_program().is_err());
    
    let mut lex = Lexer::new("type Dup<T, T> { x: T }");
    let tokens = lex.tokenize_all().unwrap();
    let mut parser = Parser::new(tokens);
    assert!(parser.parse_program().is_err());
}

#[test]
fn test_criterion_4_map_keys_indexing_and_deterministic_order() {
    run_single(
        r#"
        func assert_eq(a: String, b: String) {
            if a != b {
                boom := 1 / 0
            }
        }

        func lookup_upper(m: Map<String, String>, k: String) -> String | KeyError {
            v := m[k]?
            return v.upper()
        }

        func main() {
            var scores: Map<String, Int> = Map<String, Int>()
            scores.set("alice", 10)
            scores.set("bob", 20)
            scores.set("carol", 30)

            scores.set("bob", 99)

            keys := scores.keys()
            assert_eq(keys.get(0) ?? "", "alice")
            assert_eq(keys.get(1) ?? "", "bob")
            assert_eq(keys.get(2) ?? "", "carol")

            vals := scores.values()
            assert_eq((vals.get(1) ?? 0).to_string(), "99")

            removed := scores.remove("alice")
            assert_eq((removed ?? -1).to_string(), "10")
            assert_eq(scores.contains_key("alice").to_string(), "false")
            scores.set("alice", 55)

            entries := scores.entries()
            assert_eq(scores.len().to_string(), "3")
            (k0, v0) := entries.get(0) ?? ("", 0)
            (k1, v1) := entries.get(1) ?? ("", 0)
            (k2, v2) := entries.get(2) ?? ("", 0)
            assert_eq(k0, "bob")
            assert_eq(k1, "carol")
            assert_eq(k2, "alice")
            assert_eq(v2.to_string(), "55")

            by_id := Map.from([(1, "one"), (2, "two")])
            assert_eq(by_id.get(1) ?? "", "one")
            assert_eq(by_id.get(99) ?? "missing", "missing")

            flags := Map.from([(true, 100), (false, 0)])
            assert_eq((flags.get(true) ?? -1).to_string(), "100")

            empty_map := Map.from<String, Int>([])
            assert_eq(empty_map.len().to_string(), "0")

            switch scores["bob"] {
                case Int(v): assert_eq(v.to_string(), "99")
                case KeyError(e): boom := 1 / 0
            }

            switch scores["nobody"] {
                case Int(v): boom := 1 / 0
                case KeyError(e): assert_eq(e.key, "nobody")
            }

            words := Map.from([("ok", "hello")])
            switch lookup_upper(words, "ok") {
                case String(s): assert_eq(s, "HELLO")
                case KeyError(e): boom := 1 / 0
            }
            switch lookup_upper(words, "nope") {
                case String(s): boom := 1 / 0
                case KeyError(e): assert_eq(e.key, "nope")
            }
        }
    "#,
    )
    .expect("Map operations, indexing, and deterministic insertion order should pass");

    for (decl, bad_key) in [
        ("var m: Map<[Int], Int> = Map<[Int], Int>()", "[Int]"),
        ("type User { id: Int }\n func main() { var m: Map<User, Int> = Map<User, Int>() }", "User"),
        ("var m: Map<String?, Int> = Map<String?, Int>()", "String?"),
    ] {
        let src = if decl.contains("func main") {
            decl.to_string()
        } else {
            format!("func main() {{ {} }}", decl)
        };
        let err = check_single(&src).unwrap_err();
        let expected = format!(
            "Map key type must be 'String', 'Int', or 'Bool', got '{}'",
            bad_key
        );
        assert!(
            err.contains(&expected),
            "expected '{}' in error '{}'",
            expected,
            err
        );
    }
}

#[test]
fn test_criterion_5_validation_generics_maps_and_reachability() {
    run_single(
        r#"
        import cdt.validation as validation

        type User {
            email: String @validation.email
        }

        type Page<T> {
            items: [T]
        }

        type Directory {
            page: Page<User>
            users_by_id: Map<String, User> @validation.min(1)
            items_by_num: Map<Int, User> @validation.max(5)
        }

        func assert_eq(a: String, b: String) {
            if a != b {
                boom := 1 / 0
            }
        }

        func main() {
            var by_str = Map<String, User>()
            by_str.set("alice", User(email: "bad-email"))

            var by_int = Map<Int, User>()
            by_int.set(42, User(email: "also-bad"))

            dir := Directory(
                page: Page(items: [User(email: "page-bad")]),
                users_by_id: by_str,
                items_by_num: by_int
            )

            switch validation.check(dir) {
                case Directory(ok):
                    boom := 1 / 0
                case validation.ValidationError(errs):
                    assert_eq(errs.issues.len().to_string(), "3")
                    var paths: [String] = []
                    for iss in errs.issues {
                        paths.append(iss.path)
                    }
                    assert_eq(paths.get(0) ?? "", "page.items[0].email")
                    assert_eq(paths.get(1) ?? "", "users_by_id[\"alice\"].email")
                    assert_eq(paths.get(2) ?? "", "items_by_num[42].email")
            }
        }
    "#,
    )
    .expect("validation should recurse into generic structs and Map values with exact paths");
    
    run_single(
        r#"
        import cdt.validation as validation

        type Config {
            tags: Map<String, String> @validation.min(2)
        }

        func main() {
            cfg := Config(tags: Map.from([("env", "prod")]))
            switch validation.check(cfg) {
                case Config(c): boom := 1 / 0
                case validation.ValidationError(errs):
                    var path = ""
                    for iss in errs.issues {
                        path = iss.path
                    }
                    if errs.issues.len() != 1 || path != "tags" {
                        boom := 1 / 0
                    }
            }
        }
    "#,
    )
    .expect("validation.min should validate Map length");
    
    let reach_src = r#"
        import cdt.validation as validation

        [#validator]
        func check_payload(n: Int) -> Void | validation.Issue {
            return validation.min(n, 0)
        }

        type Payload { val: Int @check_payload }
        type Box<T> { inner: T }

        func inspect(b: Box<Payload>, m: Map<String, Payload>) -> Int {
            return m.len()
        }

        func never_used() -> Int {
            return 1
        }

        func main() {
            b := Box(inner: Payload(val: 1))
            m := Map.from([("a", Payload(val: 2))])
            print(inspect(b, m))
        }
    "#;
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&main, reach_src);
    let root = loader.load_project(&main).unwrap();
    let native = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &native);
    let report = analyzer.analyze(&root, &[]);
    assert!(report.is_function_reachable(&main, "inspect"));
    assert!(report.is_function_reachable(&main, "check_payload"));
    assert!(!report.is_function_reachable(&main, "never_used"));
}
