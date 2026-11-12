use super::*;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::loader::ModuleLoader;
use std::path::PathBuf;

fn check_source(source: &str) -> Result<(), TypeError> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize_all().unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut checker = TypeChecker::new();
    checker.check_program(&program)
}

#[test]
fn test_guard_valid_return() {
    let src = "func f(x: Int) -> Int {\n    guard x > 0 else {\n        return 0\n    }\n    return x\n}\n";
    assert!(check_source(src).is_ok());
}

#[test]
fn test_guard_missing_return_error() {
    let src = "func f(x: Int) -> Int {\n    guard x > 0 else {\n        var a = 1\n    }\n    return x\n}\n";
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("must terminate with a 'return'"));
}

#[test]
fn test_coalesce_valid() {
    let src = "func f(x: Int?) -> Int {\n    return x ?? 10\n}\n";
    assert!(check_source(src).is_ok());
}

#[test]
fn test_coalesce_type_mismatch() {
    let src = "func f(x: Int?) -> Int {\n    return x ?? \"wrong\"\n}\n";
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Coalesce operator '??' default value must match optional inner type"));
}

#[test]
fn test_break_outside_loop_error() {
    let src = "func f() {\n    break\n}\n";
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("'break' outside of loop"));
}

#[test]
fn test_continue_outside_loop_error() {
    let src = "func f() {\n    continue\n}\n";
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("'continue' outside of loop"));
}

#[test]
fn test_pipe_valid() {
    let src = "func double(x: Int) -> Int { return x * 2 }\nfunc f() -> Int { return 5 |> double() }\n";
    assert!(check_source(src).is_ok());
}

#[test]
fn test_pipe_type_mismatch() {
    let src = "func double(x: Int) -> Int { return x * 2 }\nfunc f() -> Int { return \"str\" |> double() }\n";
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("expects Int"));
}

#[test]
fn test_error_and_union_typecheck() {
    let src = r#"
        type User { name: String }
        error InvalidUserName { message: String }

        func create_user(name: String) -> User | InvalidUserName {
            guard name.trim().len() > 0 else {
                return InvalidUserName(message: "Name is required")
            }
            return User(name: name)
        }

        func register(name: String) -> User | InvalidUserName {
            user := create_user(name)?
            return user
        }

        func main() {
            res := register("Alice")
            switch res {
                case User(u):
                    print(u.name)
                case InvalidUserName(e):
                    print(e.message)
            }
        }
    "#;
    assert!(check_source(src).is_ok());
}

#[test]
fn test_switch_non_exhaustive_error() {
    let src = r#"
        type User { name: String }
        error InvalidUserName { message: String }

        func main() {
            var res: User | InvalidUserName = User(name: "Alice")
            switch res {
                case User(u):
                    print(u.name)
            }
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Non-exhaustive switch: missing cases for InvalidUserName"));
}

#[test]
fn test_switch_redundant_case_error() {
    let src = r#"
        type User { name: String }

        func main() {
            var u = User(name: "Alice")
            switch u {
                case User(a):
                    print(a.name)
                case nil:
                    print("nil")
            }
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Redundant or impossible case 'nil'"));
}

#[test]
fn test_union_field_access_disallowed_without_narrowing() {
    let src = r#"
        type User { name: String }
        error InvalidUserName { message: String }

        func test_access(res: User | InvalidUserName) {
            print(res.name)
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("without narrowing"));
}

#[test]
fn test_try_operator_unhandled_error_type_mismatch() {
    let src = r#"
        type User { name: String }
        error InvalidUserName { message: String }
        error AnotherError { code: Int }

        func get_user() -> User | InvalidUserName {
            return User(name: "Alice")
        }

        func caller() -> User | AnotherError {
            u := get_user()?
            return u
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("propagated by '?' is not assignable to function return type"));
}

#[test]
fn test_try_on_pure_optional_rejected() {
    let src = r#"
        type User { name: String }

        func get_opt() -> User? {
            return nil
        }

        func caller() -> User {
            u := get_opt()?
            return u
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Operator '?' is only allowed on types containing error variants"));
}

#[test]
fn test_try_preserves_nil() {
    let src = r#"
        type User { name: String }
        error Err { msg: String }

        func get_user_opt() -> (User?) | Err {
            return nil
        }

        func caller() -> User? | Err {
            u := get_user_opt()?
            return u
        }
    "#;
    assert!(check_source(src).is_ok());
}

#[test]
fn test_tuples_creation_destructuring_and_switch() {
    let src = r#"
        error DivisionByZero {}

        func divide(a: Int, b: Int) -> (Int, Int) | DivisionByZero {
            guard b != 0 else {
                return DivisionByZero()
            }
            return (a / b, a % b)
        }

        func quotient(a: Int, b: Int) -> Int | DivisionByZero {
            (val, _) := divide(a, b)?
            return val
        }

        func main() {
            switch divide(17, 5) {
                case (q, r):
                    print("{q}, {r}")
                case DivisionByZero(e):
                    print("Cannot divide by zero")
            }
        }
    "#;
    if let Err(e) = check_source(src) {
        panic!("Test failed with error: {}", e);
    }
}

#[test]
fn test_tuple_wildcard_cannot_discard_error() {
    let src = r#"
        error Bad { id: Int }

        func get_pair() -> (Int, Bad) {
            return (1, Bad(id: 10))
        }

        func main() {
            (num, _) := get_pair()
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Cannot discard element at index 1 with error type 'Bad' using wildcard '_'"));
}

#[test]
fn test_discarded_error_statement() {
    let src = r#"
        error Bad { id: Int }

        func produce_error() -> Bad {
            return Bad(id: 1)
        }

        func main() {
            produce_error()
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("contains an error and must not be discarded"));
}

#[test]
fn test_map_and_filter_compatibility() {
    let src = r#"
        type User { name: String }
        error InvalidName { msg: String }

        func make_user(n: String) -> User | InvalidName {
            return User(name: n)
        }

        func is_long(n: String) -> Bool {
            return n.len() > 3
        }

        func main() {
            names := ["Alice", "Bob"]
            users := map(names, make_user)
            filtered := filter(names, is_long)
        }
    "#;
    assert!(check_source(src).is_ok());
}

#[test]
fn test_filter_rejects_error_callback() {
    let src = r#"
        error CheckErr { code: Int }

        func bad_check(n: String) -> Bool | CheckErr {
            return true
        }

        func main() {
            names := ["Alice", "Bob"]
            filtered := filter(names, bad_check)
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("must return 'Bool'"));
}

#[test]
fn test_unused_error_variable_diagnosed() {
    let src = r#"
        error MyError { msg: String }

        func main() {
            err := MyError(msg: "oops")
        }
    "#;
    let err = check_source(src).unwrap_err();
    assert!(err.message.contains("Unused variable 'err' of error type 'MyError'"));
}

#[test]
fn test_private_type_leakage_in_exported_function() {
    let mut loader = ModuleLoader::new();
    let root = PathBuf::from("/virtual/main.cdt");
    loader.with_virtual_file(&root, r#"
        type SecretConfig { key: String }

        export func get_config() -> SecretConfig {
            return SecretConfig(key: "secret")
        }

        func main() {}
    "#);

    let root_canon = loader.load_project(&root).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root_canon, &loader).unwrap_err();

    assert!(err.to_string().contains("Exported function 'get_config' exposes private type 'SecretConfig'"));
}

#[test]
fn test_cannot_access_private_item_from_imported_module() {
    let mut loader = ModuleLoader::new();
    let helper = PathBuf::from("/virtual/helper.cdt");
    let root = PathBuf::from("/virtual/main.cdt");

    loader.with_virtual_file(&helper, r#"
        func secret() -> Int {
            return 42
        }

        export func public_fn() -> Int {
            return secret()
        }
    "#);

    loader.with_virtual_file(&root, r#"
        import "./helper" as helper

        func main() {
            x := helper.secret()
        }
    "#);

    let root_canon = loader.load_project(&root).unwrap();
    let mut checker = TypeChecker::new();
    let err = checker.check_project(&root_canon, &loader).unwrap_err();

    assert!(err.to_string().contains("is private in module 'helper'"));
}

#[test]
fn test_cyclic_import_detection() {
    let mut loader = ModuleLoader::new();
    let a = PathBuf::from("/virtual/a.cdt");
    let b = PathBuf::from("/virtual/b.cdt");

    loader.with_virtual_file(&a, r#"
        import "./b" as b
        export func fa() {}
    "#);

    loader.with_virtual_file(&b, r#"
        import "./a" as a
        export func fb() {}
    "#);

    let err = loader.load_project(&a).unwrap_err();
    assert!(err.to_string().contains("Cyclic import detected"));
}
