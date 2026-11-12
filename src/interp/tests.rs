use super::*;
use crate::check::TypeChecker;
use crate::lexer::Lexer;
use crate::parser::Parser;

fn run_code(src: &str) -> Result<Interpreter, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize_all().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().map_err(|e| e.to_string())?;
    let mut checker = TypeChecker::new();
    checker.check_program(&program).map_err(|e| e.to_string())?;
    let mut interp = Interpreter::new();
    interp.run_program(&program).map_err(|e| e.to_string())?;
    Ok(interp)
}

#[test]
fn test_error_return_and_switch() {
    let src = r#"
        type User { name: String }
        error InvalidName { msg: String }

        func make_user(n: String) -> User | InvalidName {
            guard n.len() > 0 else {
                return InvalidName(msg: "empty name")
            }
            return User(name: n)
        }

        func main() {
            u1 := make_user("Alice")
            var out1 = ""
            switch u1 {
                case User(u):
                    out1 = u.name
                case InvalidName(e):
                    out1 = e.msg
            }

            u2 := make_user("")
            var out2 = ""
            switch u2 {
                case User(u):
                    out2 = u.name
                case InvalidName(e):
                    out2 = e.msg
            }

            print(out1)
            print(out2)
        }
    "#;
    assert!(run_code(src).is_ok());
}

#[test]
fn test_try_operator_flow() {
    let src = r#"
        type User { name: String }
        error InvalidName { msg: String }

        func make_user(n: String) -> User | InvalidName {
            guard n.len() > 0 else {
                return InvalidName(msg: "empty name")
            }
            return User(name: n)
        }

        func register(n: String) -> User | InvalidName {
            u := make_user(n)?
            return u
        }

        func main() {
            u := register("Bob")
            switch u {
                case User(user):
                    print(user.name)
                case InvalidName(e):
                    print(e.msg)
            }
        }
    "#;
    assert!(run_code(src).is_ok());
}

#[test]
fn test_tuples_and_destructuring() {
    let src = r#"
        error DivisionByZero {}

        func divide(a: Int, b: Int) -> (Int, Int) | DivisionByZero {
            guard b != 0 else {
                return DivisionByZero()
            }
            return (a / b, a % b)
        }

        func quotient(a: Int, b: Int) -> Int | DivisionByZero {
            (q, _) := divide(a, b)?
            return q
        }

        func main() {
            switch quotient(17, 5) {
                case Int(v):
                    print(v)
                case DivisionByZero(e):
                    print("error")
            }
        }
    "#;
    assert!(run_code(src).is_ok());
}

#[test]
fn test_map_preserves_errors_as_values() {
    let src = r#"
        error Bad { msg: String }

        func validate(n: Int) -> Int | Bad {
            guard n > 0 else {
                return Bad(msg: "negative")
            }
            return n * 2
        }

        func main() {
            nums := [10, -5, 20]
            results := map(nums, validate)
            print(results)
        }
    "#;
    assert!(run_code(src).is_ok());
}
