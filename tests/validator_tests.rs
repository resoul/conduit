use std::path::PathBuf;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::reachability::ReachabilityAnalyzer;

// =========================================================================
// Criterion 1: Marker syntax, position, case-insensitivity, unknown/duplicates
// =========================================================================
#[test]
fn test_criterion_1_marker_syntax_and_case_insensitivity() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit1/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        [#validator]
        func check_lower(val: String) -> Void | validation.Issue {
            return
        }

        [#VALIDATOR]
        export func check_upper(val: String) -> Void | validation.Issue {
            return
        }

        [#Validator]
        func check_mixed(val: String) -> Void | validation.Issue {
            return
        }

        type User {
            a: String @check_lower
            b: String @check_upper
            c: String @check_mixed
        }

        func main() {}
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();
}

#[test]
fn test_criterion_1_unknown_and_duplicate_marker_rejected() {
    // Unknown marker [#unknown]
    let mut loader1 = ModuleLoader::new();
    let main1 = PathBuf::from("/crit1_err/main1.cdt");
    loader1.with_virtual_file(
        &main1,
        r#"
        import cdt.validation as validation

        [#unknown]
        func bad(val: String) -> Void | validation.Issue {
            return
        }
    "#,
    );
    assert!(loader1.load_project(&main1).is_err());

    // Duplicate marker
    let mut loader2 = ModuleLoader::new();
    let main2 = PathBuf::from("/crit1_err/main2.cdt");
    loader2.with_virtual_file(
        &main2,
        r#"
        import cdt.validation as validation

        [#validator]
        [#validator]
        func bad(val: String) -> Void | validation.Issue {
            return
        }
    "#,
    );
    assert!(loader2.load_project(&main2).is_err());

    // Marker on type
    let mut loader3 = ModuleLoader::new();
    let main3 = PathBuf::from("/crit1_err/main3.cdt");
    loader3.with_virtual_file(
        &main3,
        r#"
        [#validator]
        type Bad {
            name: String
        }
    "#,
    );
    assert!(loader3.load_project(&main3).is_err());

    // Marker on error
    let mut loader4 = ModuleLoader::new();
    let main4 = PathBuf::from("/crit1_err/main4.cdt");
    loader4.with_virtual_file(
        &main4,
        r#"
        [#validator]
        error BadErr {
            msg: String
        }
    "#,
    );
    assert!(loader4.load_project(&main4).is_err());
}

// =========================================================================
// Criterion 2: Local vs exported validators & privacy
// =========================================================================
#[test]
fn test_criterion_2_validator_privacy_and_visibility() {
    let mut loader = ModuleLoader::new();
    let rules = PathBuf::from("/crit2/rules.cdt");
    let main = PathBuf::from("/crit2/main.cdt");

    loader.with_virtual_file(
        &rules,
        r#"
        import cdt.validation as validation

        [#validator]
        func private_rule(val: String) -> Void | validation.Issue {
            return
        }

        [#validator]
        export func public_rule(val: String) -> Void | validation.Issue {
            return
        }

        // Local use of private rule works within the defining module
        type LocalUser {
            name: String @private_rule
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./rules" as rules

        type User {
            name: String @rules.public_rule
        }

        func main() {}
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    // Now try to access private_rule from main -> should fail
    let mut loader_err = ModuleLoader::new();
    loader_err.with_virtual_file(
        &rules,
        r#"
        import cdt.validation as validation

        [#validator]
        func private_rule(val: String) -> Void | validation.Issue {
            return
        }
    "#,
    );
    loader_err.with_virtual_file(
        &main,
        r#"
        import "./rules" as rules

        type User {
            name: String @rules.private_rule
        }
    "#,
    );
    let root_err = loader_err.load_project(&main).unwrap();
    let mut checker_err = TypeChecker::new();
    let err = checker_err.check_project(&root_err, &loader_err).unwrap_err();
    assert!(err.message.contains("private validator"));
}

// =========================================================================
// Criterion 3: Unmarked function cannot be used as an annotation
// =========================================================================
#[test]
fn test_criterion_3_unmarked_function_rejected_as_annotation() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit3/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        // Ordinary function without [#validator]
        func helper(val: String) -> Void | validation.Issue {
            return
        }

        type User {
            name: String @helper
        }

        func main() {}
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root, &loader).unwrap_err();
    assert!(err.message.contains("not marked with '[#validator]'"));
}

// =========================================================================
// Criterion 4: Validator contract and field compatibility
// =========================================================================
#[test]
fn test_criterion_4_validator_contract_and_field_compatibility() {
    // 0 params rejected
    let mut loader1 = ModuleLoader::new();
    let m1 = PathBuf::from("/crit4/m1.cdt");
    loader1.with_virtual_file(
        &m1,
        r#"
        import cdt.validation as validation
        [#validator]
        func bad() -> Void | validation.Issue {
            return
        }
    "#,
    );
    let r1 = loader1.load_project(&m1).unwrap();
    let mut c1 = TypeChecker::new();
    let err1 = c1.check_project(&r1, &loader1).unwrap_err();
    assert!(err1.message.contains("at least one parameter"));

    // Non-Issue return type rejected (Bool)
    let mut loader2 = ModuleLoader::new();
    let m2 = PathBuf::from("/crit4/m2.cdt");
    loader2.with_virtual_file(
        &m2,
        r#"
        [#validator]
        func bad(v: String) -> Bool {
            return true
        }
    "#,
    );
    let r2 = loader2.load_project(&m2).unwrap();
    let mut c2 = TypeChecker::new();
    let err2 = c2.check_project(&r2, &loader2).unwrap_err();
    assert!(err2.message.contains("must return 'Void | validation.Issue'"));

    // Field type incompatibility
    let mut loader3 = ModuleLoader::new();
    let m3 = PathBuf::from("/crit4/m3.cdt");
    loader3.with_virtual_file(
        &m3,
        r#"
        import cdt.validation as validation
        [#validator]
        func check_str(v: String) -> Void | validation.Issue {
            return
        }
        type User {
            age: Int @check_str
        }
    "#,
    );
    let r3 = loader3.load_project(&m3).unwrap();
    let mut c3 = TypeChecker::new();
    let err3 = c3.check_project(&r3, &loader3).unwrap_err();
    assert!(err3.message.contains("not compatible with validator parameter type"));

    // Parameterized validator with negative integer literal
    let mut loader4 = ModuleLoader::new();
    let m4 = PathBuf::from("/crit4/m4.cdt");
    loader4.with_virtual_file(
        &m4,
        r#"
        import cdt.validation as validation
        [#validator]
        func check_range(v: Int, min_val: Int, max_val: Int) -> Void | validation.Issue {
            return
        }
        type Account {
            balance: Int @check_range(-100, 1000)
        }
        func main() {}
    "#,
    );
    let r4 = loader4.load_project(&m4).unwrap();
    let mut c4 = TypeChecker::new();
    c4.check_project(&r4, &loader4).unwrap();
}

// =========================================================================
// Criterion 5: Optional field validation (T? validated against T, nil skipped)
// =========================================================================
#[test]
fn test_criterion_5_optional_field_validation() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit5/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        type User {
            email: String? @validation.email
        }

        func main() {
            // Case 1: nil is skipped
            u1 := User(email: nil)
            res1 := validation.check(u1)
            switch res1 {
                case User(u):
                case validation.ValidationError(e):
                    err := 1 / 0
            }

            // Case 2: present and valid
            u2 := User(email: "test@domain.com")
            res2 := validation.check(u2)
            switch res2 {
                case User(u):
                case validation.ValidationError(e):
                    err := 1 / 0
            }

            // Case 3: present and invalid
            u3 := User(email: "bad-email")
            res3 := validation.check(u3)
            switch res3 {
                case User(u):
                    err := 1 / 0
                case validation.ValidationError(e):
                    if e.issues.len() != 1 {
                        err := 1 / 0
                    }
                    p := ""
                    for iss in e.issues {
                        p = iss.path
                    }
                    if p != "email" {
                        err := 1 / 0
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

// =========================================================================
// Criterion 6: validation.check success, violations, and order preservation
// =========================================================================
#[test]
fn test_criterion_6_validation_check_flow_and_order() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit6/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        [#validator]
        func rule_first(v: String) -> Void | validation.Issue {
            return validation.Issue(code: "err.first", message: "First failed")
        }

        [#validator]
        func rule_second(v: String) -> Void | validation.Issue {
            return validation.Issue(code: "err.second", message: "Second failed")
        }

        type Product {
            title: String @rule_first @rule_second
            count: Int @validation.min(1)
        }

        func main() {
            // Normal constructor creates product without validating
            p := Product(title: "book", count: 0)
            if p.title != "book" {
                err := 1 / 0
            }
            if p.count != 0 {
                err := 1 / 0
            }

            // validation.check collects violations in order
            res := validation.check(p)
            switch res {
                case Product(ok):
                    err := 1 / 0
                case validation.ValidationError(err):
                    if err.issues.len() != 3 {
                        div := 1 / 0
                    }
                    idx := 0
                    c0 := ""
                    p0 := ""
                    c1 := ""
                    p1 := ""
                    c2 := ""
                    p2 := ""
                    for iss in err.issues {
                        if idx == 0 {
                            c0 = iss.code
                            p0 = iss.path
                        }
                        if idx == 1 {
                            c1 = iss.code
                            p1 = iss.path
                        }
                        if idx == 2 {
                            c2 = iss.code
                            p2 = iss.path
                        }
                        idx = idx + 1
                    }
                    if c0 != "err.first" {
                        div := 1 / 0
                    }
                    if p0 != "title" {
                        div := 1 / 0
                    }
                    if c1 != "err.second" {
                        div := 1 / 0
                    }
                    if p1 != "title" {
                        div := 1 / 0
                    }
                    if c2 != "min" {
                        div := 1 / 0
                    }
                    if p2 != "count" {
                        div := 1 / 0
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

// =========================================================================
// Criterion 7: Nested struct paths, lists, and deep path formatting
// =========================================================================
#[test]
fn test_criterion_7_nested_paths_and_lists() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit7/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        type Tag {
            name: String @validation.min(2)
        }

        type Address {
            city: String @validation.min(3)
        }

        type Profile {
            address: Address
            tags: [Tag]
        }

        func main() {
            bad_tag1 := Tag(name: "x")
            bad_tag2 := Tag(name: "y")
            bad_addr := Address(city: "a")
            prof := Profile(address: bad_addr, tags: [bad_tag1, bad_tag2])

            res := validation.check(prof)
            switch res {
                case Profile(p):
                    err := 1 / 0
                case validation.ValidationError(err):
                    if err.issues.len() != 3 {
                        div := 1 / 0
                    }
                    idx := 0
                    p0 := ""
                    p1 := ""
                    p2 := ""
                    for iss in err.issues {
                        if idx == 0 {
                            p0 = iss.path
                        }
                        if idx == 1 {
                            p1 = iss.path
                        }
                        if idx == 2 {
                            p2 = iss.path
                        }
                        idx = idx + 1
                    }
                    if p0 != "address.city" {
                        div := 1 / 0
                    }
                    if p1 != "tags[0].name" {
                        div := 1 / 0
                    }
                    if p2 != "tags[1].name" {
                        div := 1 / 0
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

// =========================================================================
// Criterion 8: Runtime error in validator halts validation immediately
// =========================================================================
#[test]
fn test_criterion_8_runtime_error_halts_validation() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit8/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        [#validator]
        func faulty_rule(v: Int) -> Void | validation.Issue {
            div := 10 / 0
            return
        }

        type Item {
            val: Int @faulty_rule
        }

        func main() {
            i := Item(val: 5)
            res := validation.check(i)
            switch res {
                case Item(it):
                case validation.ValidationError(e):
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let mut interp = Interpreter::new();
    let err = interp.run_project(&root, &loader).unwrap_err();
    assert!(err.message.contains("division by zero") || err.message.contains("Division by zero"));
}

// =========================================================================
// Criterion 9: Standard rules min, max, email
// =========================================================================
#[test]
fn test_criterion_9_min_max_and_email_rules() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit9/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        type Form {
            score: Int @validation.min(0) @validation.max(100)
            username: String @validation.min(3) @validation.max(10)
            email: String @validation.email
            tags: [String] @validation.min(1) @validation.max(3)
        }

        func main() {
            // Valid form with Unicode scalar counting
            // "café" has 4 Unicode scalar values
            f_valid := Form(
                score: 100,
                username: "café",
                email: "dev.user@example.co.uk",
                tags: ["rust", "cdt"]
            )
            res1 := validation.check(f_valid)
            switch res1 {
                case Form(f):
                case validation.ValidationError(err):
                    div := 1 / 0
            }

            // Boundary failures: score out of range, empty tags, bad email
            f_invalid := Form(
                score: -1,
                username: "ab",
                email: "invalid..dots@test.com",
                tags: []
            )
            res2 := validation.check(f_invalid)
            switch res2 {
                case Form(f):
                    div := 1 / 0
                case validation.ValidationError(err):
                    if err.issues.len() != 4 {
                        div := 1 / 0
                    }
                    idx := 0
                    c0 := ""
                    p0 := ""
                    c1 := ""
                    p1 := ""
                    c2 := ""
                    p2 := ""
                    c3 := ""
                    p3 := ""
                    for iss in err.issues {
                        if idx == 0 {
                            c0 = iss.code
                            p0 = iss.path
                        }
                        if idx == 1 {
                            c1 = iss.code
                            p1 = iss.path
                        }
                        if idx == 2 {
                            c2 = iss.code
                            p2 = iss.path
                        }
                        if idx == 3 {
                            c3 = iss.code
                            p3 = iss.path
                        }
                        idx = idx + 1
                    }
                    if c0 != "min" || p0 != "score" {
                        div := 1 / 0
                    }
                    if c1 != "min" || p1 != "username" {
                        div := 1 / 0
                    }
                    if c2 != "email" || p2 != "email" {
                        div := 1 / 0
                    }
                    if c3 != "min" || p3 != "tags" {
                        div := 1 / 0
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

// =========================================================================
// Criterion 10: First-class validator calls
// =========================================================================
#[test]
fn test_criterion_10_first_class_validator_calls() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit10/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        [#validator]
        func my_rule(val: String) -> Void | validation.Issue {
            if val == "secret" {
                return validation.Issue(code: "forbidden", message: "Cannot be secret")
            }
            return
        }

        func run_check(rule: (String) -> Void | validation.Issue, input: String) -> String {
            res := rule(input)
            switch res {
                case Void(v):
                    return "OK"
                case validation.Issue(iss):
                    return iss.code
            }
        }

        func main() {
            if run_check(my_rule, "admin") != "OK" {
                div := 1 / 0
            }
            if run_check(my_rule, "secret") != "forbidden" {
                div := 1 / 0
            }
            if run_check(validation.email, "test@test.com") != "OK" {
                div := 1 / 0
            }
            if run_check(validation.email, "bad_email") != "email" {
                div := 1 / 0
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

// =========================================================================
// Criterion 11: Reachability traces validators via struct annotations
// =========================================================================
#[test]
fn test_criterion_11_reachability_traces_validators() {
    let mut loader = ModuleLoader::new();
    let rules = PathBuf::from("/crit11/rules.cdt");
    let main = PathBuf::from("/crit11/main.cdt");

    loader.with_virtual_file(
        &rules,
        r#"
        import cdt.validation as validation

        [#validator]
        export func reachable_rule(v: String) -> Void | validation.Issue {
            return
        }

        [#validator]
        export func unused_rule(v: String) -> Void | validation.Issue {
            return
        }
    "#,
    );

    loader.with_virtual_file(
        &main,
        r#"
        import "./rules" as rules
        import cdt.validation as validation

        type User {
            name: String @rules.reachable_rule
        }

        type UnusedType {
            other: String @rules.unused_rule
        }

        func main() {
            u := User(name: "test")
            res := validation.check(u)
            switch res {
                case User(usr):
                case validation.ValidationError(e):
            }
        }
    "#,
    );

    let root = loader.load_project(&main).unwrap();
    let mut checker = TypeChecker::new();
    checker.check_project(&root, &loader).unwrap();

    let registry = NativeRegistry::new();
    let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &registry);
    let report = analyzer.analyze(&root, &[]);

    assert!(report.is_function_reachable(&rules, "reachable_rule"));
    assert!(!report.is_function_reachable(&rules, "unused_rule"));
    assert!(report.is_native_function_reachable("cdt.validation", "check"));
}

// =========================================================================
// Criterion 12: Try operator and error handling with validation
// =========================================================================
#[test]
fn test_criterion_12_try_operator_and_error_propagation() {
    let mut loader = ModuleLoader::new();
    let main = PathBuf::from("/crit12/main.cdt");

    loader.with_virtual_file(
        &main,
        r#"
        import cdt.validation as validation

        type Person {
            age: Int @validation.min(18)
        }

        func create_person(age: Int) -> Person | validation.ValidationError {
            candidate := Person(age: age)
            return validation.check(candidate)?
        }

        func main() {
            // Case 1: valid age returns Person
            p1 := create_person(21)
            switch p1 {
                case Person(p):
                    if p.age != 21 {
                        div := 1 / 0
                    }
                case validation.ValidationError(e):
                    div := 1 / 0
            }

            // Case 2: invalid age returns ValidationError via ?
            p2 := create_person(12)
            switch p2 {
                case Person(p):
                    div := 1 / 0
                case validation.ValidationError(e):
                    if e.issues.len() != 1 {
                        div := 1 / 0
                    }
                    c := ""
                    for iss in e.issues {
                        c = iss.code
                    }
                    if c != "min" {
                        div := 1 / 0
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
