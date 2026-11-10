use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use conduit::check::TypeChecker;
use conduit::interp::Interpreter;
use conduit::lexer::Lexer;
use conduit::loader::ModuleLoader;
use conduit::native::NativeRegistry;
use conduit::parser::Parser;
use conduit::reachability::ReachabilityAnalyzer;

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
            let _ = load_and_check(file_path);
            println!("Check passed: '{}' is valid.", file_path);
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'run' command");
                eprintln!("Usage: conduit run <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let (root, loader) = load_and_check(file_path);

            let mut interpreter = Interpreter::new();
            if let Err(err) = interpreter.run_project(&root, &loader) {
                eprintln!("{}", err);
                process::exit(1);
            }
        }
        "reachability" => {
            if args.len() < 3 {
                eprintln!("Error: missing file path for 'reachability' command");
                eprintln!("Usage: conduit reachability <file.cdt>");
                process::exit(1);
            }
            let file_path = &args[2];
            let (root, loader) = load_and_check(file_path);
            let native_reg = NativeRegistry::new();
            let mut analyzer = ReachabilityAnalyzer::new(&loader.modules, &native_reg);
            let report = analyzer.analyze(&root, &[]);
            println!("=== Reachability Analysis Report ===");
            println!("Root module: {}", report.root_module.display());
            println!(
                "Reachable functions: {} / {} total ({} unused)",
                report.reachable_functions_count, report.total_functions, report.unused_functions_count
            );
            println!("Reachable native functions ({}):", report.reachable_native_functions.len());
            for (m, f) in &report.reachable_native_functions {
                println!("  - {}.{}()", m, f);
            }
        }
        "lsp" => {
            if let Err(err) = conduit::lsp::run_lsp_server() {
                eprintln!("LSP server error: {}", err);
                process::exit(1);
            }
        }
        "doc" => {
            let native_reg = NativeRegistry::new();
            if args.len() < 3 {
                for module in native_reg.modules() {
                    print!("{}", module.generate_markdown_docs());
                }
            } else {
                let mod_name = &args[2];
                if let Some(module) = native_reg.get_module(mod_name) {
                    print!("{}", module.generate_markdown_docs());
                } else {
                    eprintln!("Error: native module '{}' not found in registry", mod_name);
                    process::exit(1);
                }
            }
        }
        cmd => {
            eprintln!("Unknown command: '{}'", cmd);
            print_usage();
            process::exit(1);
        }
    }
}

fn load_and_check(file_path: &str) -> (PathBuf, ModuleLoader) {
    let path = Path::new(file_path);
    let mut loader = ModuleLoader::new();
    let root = match loader.load_project(path) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    };

    let mut checker = TypeChecker::new();
    if let Err(err) = checker.check_project(&root, &loader) {
        eprintln!("{}", err);
        process::exit(1);
    }

    (root, loader)
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
    println!("  conduit run <file.cdt>          Type-check and execute a source file");
    println!("  conduit check <file.cdt>        Type-check a source file and its imports");
    println!("  conduit reachability <file.cdt> Analyze reachable functions and native dependencies");
    println!("  conduit doc [module]            Generate Markdown API documentation from NativeRegistry");
    println!("  conduit ast <file.cdt>          Parse and display the AST of a source file");
    println!("  conduit tokens <file.cdt>       Print tokens of a source file");
    println!("  conduit lsp                     Start the Language Server Protocol (LSP) server");
    println!("  conduit --version               Show version");
    println!("  conduit --help                  Show this help message");
}
