use std::env;
use std::fs;
use std::process;

use conduit::ast::Program;
use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::parser::Parser;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "--version" | "-v" => {
            println!("Conduit v{}", conduit::VERSION);
        }
        "--help" | "-h" => {
            print_usage();
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'tokens' command");
                eprintln!("Usage: conduit tokens <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let content = read_file(file_path);

            let mut lexer = Lexer::new(&content);
            match lexer.tokenize_all() {
                Ok(tokens) => {
                    for tok in tokens {
                        println!("[{:3}:{:3}] {:?}", tok.span.line, tok.span.col, tok.kind);
                    }
                }
                Err(err) => {
                    eprintln!("{}", err);
                    process::exit(1);
                }
            }
        }
        "ast" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'ast' command");
                eprintln!("Usage: conduit ast <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let content = read_file(file_path);

            let mut lexer = Lexer::new(&content);
            let tokens = match lexer.tokenize_all() {
                Ok(t) => t,
                Err(err) => {
                    eprintln!("{}", err);
                    process::exit(1);
                }
            };

            let mut parser = Parser::new(tokens);
            match parser.parse_program() {
                Ok(program) => {
                    println!("{:#?}", program);
                }
                Err(err) => {
                    eprintln!("{}", err);
                    process::exit(1);
                }
            }
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'check' command");
                eprintln!("Usage: conduit check <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let content = read_file(file_path);

            let _ = parse_and_check(file_path, &content);
            println!("Check passed: '{}' is valid.", file_path);
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'run' command");
                eprintln!("Usage: conduit run <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let content = read_file(file_path);

            let program = parse_and_check(file_path, &content);

            let mut interpreter = Interpreter::new();
            if let Err(err) = interpreter.run_program(&program) {
                eprintln!("{}", err);
                process::exit(1);
            }
        }
        cmd => {
            eprintln!("Unknown command: '{}'", cmd);
            print_usage();
            process::exit(1);
        }
    }
}

fn parse_and_check(_path: &str, content: &str) -> Program {
    let mut lexer = Lexer::new(content);
    let tokens = match lexer.tokenize_all() {
        Ok(t) => t,
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    };

    let mut parser = Parser::new(tokens);
    let program = match parser.parse_program() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    };

    let mut checker = TypeChecker::new();
    if let Err(err) = checker.check_program(&program) {
        eprintln!("{}", err);
        process::exit(1);
    }

    program
}

fn read_file(path: &str) -> String {
    match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Error reading '{}': {}", path, err);
            process::exit(1);
        }
    }
}

fn print_usage() {
    println!("Conduit Programming Language v{}", conduit::VERSION);
    println!();
    println!("Usage:");
    println!("  conduit run <file.cdt>       Type-check and execute a source file");
    println!("  conduit check <file.cdt>     Type-check a source file");
    println!("  conduit ast <file.cdt>       Parse and display the AST of a source file");
    println!("  conduit tokens <file.cdt>    Print tokens of a source file");
    println!("  conduit --version            Show version");
    println!("  conduit --help               Show this help message");
}
