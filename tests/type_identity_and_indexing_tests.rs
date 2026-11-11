use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::parser::Parser;
use conduit::reachability::ReachabilityAnalyzer;

#[test]
fn test_criterion_1_same_named_types_across_modules_are_distinct() {
    let mut loader = ModuleLoader::new();
    let mod_a = PathBuf::from("/virtual/a.cdt");
    let mod_b = PathBuf::from("/virtual/b.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &mod_a,
        r#"
        export type User {
            name: String
        }

        export error DomainErr {
            code: Int
        }
    "#,
    );

    loader.with_virtual_file(
        &mod_b,
        r#"
        export type User {
            title: String
        }

        export error DomainErr {
            reason: String
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./a" as a
        import "./b" as b

        func take_b(u: b.User) -> String {
            return u.title
        }

        func main() {
            ua := a.User(name: "Alice")
            res := take_b(ua)
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root, &loader).unwrap_err();
    assert!(err.message.contains("expects"));

    // Also verify assignment mismatch between same-named types and switch mismatch
    let mut loader2 = ModuleLoader::new();
    loader2.with_virtual_file(
        &mod_a,
        r#"
        export type User { name: String }
        export error DomainErr { code: Int }
    "#,
    );
    loader2.with_virtual_file(
        &mod_b,
        r#"
        export type User { name: String }
        export error DomainErr { code: Int }
    "#,
    );
    loader2.with_virtual_file(
        &main,
        r#"
        import "./a" as a
        import "./b" as b

        func bad_return(u: a.User) -> b.User {
            return u
        }

        func bad_switch(res: a.User | a.DomainErr) {
            switch res {
                case b.User(u):
                case a.DomainErr(e):
            }
        }

        func main() {}
    "#,
    );

    let root2 = loader2.load_project(&main).unwrap();
    let mut checker2 = TypeChecker::new();
    assert!(checker2.check_project(&root2, &loader2).is_err());
}

#[test]
fn test_criterion_2_runtime_switch_distinguishes_same_named_module_types() {
    let mut loader = ModuleLoader::new();
    let mod_a = PathBuf::from("/virtual/a.cdt");
    let mod_b = PathBuf::from("/virtual/b.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &mod_a,
        r#"
        export type User {
            tag: String
        }

        export error ItemErr {
            msg: String
        }
    "#,
    );

    loader.with_virtual_file(
        &mod_b,
        r#"
        export type User {
            tag: String
        }

        export error ItemErr {
            msg: String
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./a" as a
        import "./b" as b

        func check_user(v: a.User | b.User) -> String {
            switch v {
                case b.User(u):
                    return "b:" + u.tag
                case a.User(u):
                    return "a:" + u.tag
            }
        }

        func check_err(v: a.ItemErr | b.ItemErr) -> String {
            switch v {
                case a.ItemErr(e):
                    return "a_err:" + e.msg
                case b.ItemErr(e):
                    return "b_err:" + e.msg
            }
        }

        func main() {
            ua := a.User(tag: "one")
            ub := b.User(tag: "two")
            if check_user(ua) != "a:one" {
                fail := 1 / 0
            }
            if check_user(ub) != "b:two" {
                fail := 1 / 0
            }

            eb := b.ItemErr(msg: "boom")
            if check_err(eb) != "b_err:boom" {
                fail := 1 / 0
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

#[test]
fn test_criterion_3_import_aliases_resolve_to_identical_canonical_type() {
    let mut loader = ModuleLoader::new();
    let models = PathBuf::from("/virtual/models.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &models,
        r#"
        export type Account {
            id: Int
        }

        export error AuthErr {
            reason: String
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./models" as m1
        import "./models" as m2
        import cdt.validation as v

        [#validator]
        func non_negative(val: Int) -> Void | v.Issue {
            guard val >= 0 else {
                return v.Issue(code: "neg", message: "must be >= 0")
            }
        }

        type Order {
            amount: Int @non_negative
        }

        func accept_m2(a: m2.Account) -> m1.Account {
            return a
        }

        func handle(res: m1.Account | m2.AuthErr) -> Int {
            switch res {
                case m2.Account(acc):
                    return acc.id
                case m1.AuthErr(e):
                    return -1
            }
        }

        func main() {
            a1 := m1.Account(id: 7)
            a2 := accept_m2(a1)
            if handle(a2) != 7 {
                fail := 1 / 0
            }

            err_res := m1.AuthErr(reason: "denied")
            if handle(err_res) != -1 {
                fail := 1 / 0
            }

            ord := Order(amount: -5)
            switch v.check(ord) {
                case Order(ok):
                    fail := 1 / 0
                case v.ValidationError(verr):
                    first := verr.issues.get(0)
                    switch first {
                        case v.Violation(item):
                            if item.code != "neg" {
                                fail := 1 / 0
                            }
                        case nil:
                            fail := 1 / 0
                    }
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

#[test]
fn test_criterion_4_validation_uses_canonical_struct_schema_and_module_imports() {
    let mut loader = ModuleLoader::new();
    let rules_a = PathBuf::from("/virtual/rules_a.cdt");
    let rules_b = PathBuf::from("/virtual/rules_b.cdt");
    let mod_a = PathBuf::from("/virtual/mod_a.cdt");
    let mod_b = PathBuf::from("/virtual/mod_b.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &rules_a,
        r#"
        import cdt.validation as v

        [#validator]
        export func rule(s: String) -> Void | v.Issue {
            if s == "bad_a" {
                return v.Issue(code: "err_a", message: "Rejected by A")
            }
        }
    "#,
    );

    loader.with_virtual_file(
        &rules_b,
        r#"
        import cdt.validation as v

        [#validator]
        export func rule(s: String) -> Void | v.Issue {
            if s == "bad_b" {
                return v.Issue(code: "err_b", message: "Rejected by B")
            }
        }
    "#,
    );

    loader.with_virtual_file(
        &mod_a,
        r#"
        import "./rules_a" as r

        export type User {
            name: String @r.rule
        }
    "#,
    );

    loader.with_virtual_file(
        &mod_b,
        r#"
        import "./rules_b" as r

        export type User {
            name: String @r.rule
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation
        import "./mod_a" as a
        import "./mod_b" as b

        func main() {
            // b.User accepts "bad_a" (rejected only by a's rule) and rejects "bad_b"
            ub_ok := b.User(name: "bad_a")
            switch validation.check(ub_ok) {
                case b.User(u):
                case validation.ValidationError(e):
                    fail := 1 / 0
            }

            ub_bad := b.User(name: "bad_b")
            switch validation.check(ub_bad) {
                case b.User(u):
                    fail := 1 / 0
                case validation.ValidationError(e):
                    switch e.issues[0] {
                        case validation.Violation(v):
                            if v.code != "err_b" {
                                fail := 1 / 0
                            }
                        case IndexError(ie):
                            fail := 1 / 0
                    }
            }

            ua_bad := a.User(name: "bad_a")
            switch validation.check(ua_bad) {
                case a.User(u):
                    fail := 1 / 0
                case validation.ValidationError(e):
                    switch e.issues[0] {
                        case validation.Violation(v):
                            if v.code != "err_a" {
                                fail := 1 / 0
                            }
                        case IndexError(ie):
                            fail := 1 / 0
                    }
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

fn check_single(src: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize_all().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().map_err(|e| e.to_string())?;
    let mut checker = TypeChecker::new();
    checker.check_program(&program).map_err(|e| e.to_string())?;
    let mut interp = Interpreter::new();
    interp.run_program(&program).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn test_criterion_5_safe_list_indexing_behavior_and_type_checks() {
    let ok_src = r#"
        func get_item(items: [Int], idx: Int) -> Int | IndexError {
            val := items[idx]?
            return val
        }

        func get_matrix(grid: [[String]], r: Int, c: Int) -> String | IndexError {
            return grid[r]?[c]?.upper()
        }

        func main() {
            nums := [10, 20, 30]
            if get_item(nums, 0) != 10 {
                fail := 1 / 0
            }
            if get_item(nums, 2) != 30 {
                fail := 1 / 0
            }

            // Negative index must return IndexError, not element from end
            switch get_item(nums, -1) {
                case Int(v):
                    fail := 1 / 0
                case IndexError(e):
                    if e.index != -1 || e.length != 3 {
                        fail := 1 / 0
                    }
            }

            // Index >= length returns IndexError
            switch nums[3] {
                case Int(v):
                    fail := 1 / 0
                case IndexError(e):
                    if e.index != 3 || e.length != 3 {
                        fail := 1 / 0
                    }
            }

            // Empty list indexing
            var empty: [Int] = []
            switch empty[0] {
                case Int(v):
                    fail := 1 / 0
                case IndexError(e):
                    if e.length != 0 {
                        fail := 1 / 0
                    }
            }

            grid := [["hi", "there"], ["conduit"]]
            if get_matrix(grid, 0, 1) != "THERE" {
                fail := 1 / 0
            }
            switch get_matrix(grid, 1, 5) {
                case String(s):
                    fail := 1 / 0
                case IndexError(e):
                    if e.index != 5 || e.length != 1 {
                        fail := 1 / 0
                    }
            }
        }
    "#;
    check_single(ok_src).unwrap();

    // Non-Int index must fail type checking
    let bad_index = r#"
        func main() {
            arr := [1, 2]
            x := arr["0"]
        }
    "#;
    assert!(check_single(bad_index).unwrap_err().contains("List index must be Int"));

    // Indexing non-list must fail
    let non_list = r#"
        func main() {
            s := "hello"
            x := s[0]
        }
    "#;
    assert!(check_single(non_list).unwrap_err().contains("Cannot index into non-list type"));

    // Missing IndexError case in switch must fail exhaustiveness check
    let missing_case = r#"
        func main() {
            arr := [1, 2]
            switch arr[0] {
                case Int(v):
            }
        }
    "#;
    assert!(check_single(missing_case).unwrap_err().contains("IndexError"));

    // Unhandled '?' propagation inside main() -> Void must fail
    let unhandled_try = r#"
        func main() {
            arr := [1, 2]
            x := arr[0]?
        }
    "#;
    assert!(check_single(unhandled_try).unwrap_err().contains("IndexError"));

    // Index assignment list[i] = val must fail cleanly
    let index_assign = r#"
        func main() {
            arr := [1, 2]
            arr[0] = 5
        }
    "#;
    assert!(check_single(index_assign).unwrap_err().contains("not supported"));
}

#[test]
fn test_criterion_6_list_get_method_optional_and_no_double_wrapping() {
    let src = r#"
        func main() {
            nums := [10, 20]
            first := nums.get(0) ?? -1
            if first != 10 {
                fail := 1 / 0
            }

            neg := nums.get(-1) ?? -1
            if neg != -1 {
                fail := 1 / 0
            }

            oob := nums.get(99) ?? -1
            if oob != -1 {
                fail := 1 / 0
            }

            switch nums.get(1) {
                case Int(v):
                    if v != 20 {
                        fail := 1 / 0
                    }
                case nil:
                    fail := 1 / 0
            }

            // Already optional elements [Int?] must remain Int?, not Int??
            var maybe_nums: [Int?] = []
            maybe_nums.append(nil)
            maybe_nums.append(42)
            var item: Int? = maybe_nums.get(1)
            if (item ?? 0) != 42 {
                fail := 1 / 0
            }
            var absent: Int? = maybe_nums.get(5)
            if (absent ?? 99) != 99 {
                fail := 1 / 0
            }
        }
    "#;
    check_single(src).unwrap();

    // Non-Int argument to .get() must fail
    let bad_arg = r#"
        func main() {
            nums := [1, 2]
            x := nums.get("bad")
        }
    "#;
    assert!(check_single(bad_arg).is_err());
}

#[test]
fn test_criterion_7_validation_and_safe_indexing_integration() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/virtual/crit7.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        type Account {
            email: String @validation.email
        }

        func first_violation(acc: Account) -> validation.Violation | validation.ValidationError | IndexError {
            checked := validation.check(acc)
            switch checked {
                case Account(ok):
                    return validation.Violation(path: "", code: "none", message: "ok")
                case validation.ValidationError(err):
                    first := err.issues[0]?
                    return first
            }
        }

        func main() {
            bad := Account(email: "invalid")
            switch first_violation(bad) {
                case validation.Violation(v):
                    if v.path != "email" || v.code != "email" {
                        fail := 1 / 0
                    }
                case validation.ValidationError(e):
                    fail := 1 / 0
                case IndexError(ie):
                    fail := 1 / 0
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let mut interp = Interpreter::new();
    assert!(interp.run_project(&root, &loader).is_ok());
}

#[test]
fn test_criterion_8_reachability_handles_index_expressions() {
    let mut loader = ModuleLoader::new();
    let helper = PathBuf::from("/virtual/helper.cdt");
    let main = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(
        &helper,
        r#"
        export func compute_index() -> Int {
            return 1
        }

        export func unused_helper() -> Int {
            return 99
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./helper" as h

        func main() {
            items := ["zero", "one", "two"]
            _v := items[h.compute_index()]
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let registry = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &registry);
    let report = analyzer.analyze(&root, &[]);

    assert!(report.is_function_reachable(&helper, "compute_index"));
    assert!(!report.is_function_reachable(&helper, "unused_helper"));
}
