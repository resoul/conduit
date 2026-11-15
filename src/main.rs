use std::env;
use std::fs;
use std::process;

use conduit::benchmarks;
use conduit::frontend;
use conduit::frontend::types::Type;
use conduit::ir::lower::compile_program;
use conduit::vm::{ExecResult, Vm};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("bench") | None => {
            let mut filter: Option<&str> = None;
            let mut strict = false;
            let mut i = if args.first().map(|s| s.as_str()) == Some("bench") {
                1
            } else {
                0
            };
            while i < args.len() {
                match args[i].as_str() {
                    "--filter" => {
                        i += 1;
                        if i < args.len() {
                            filter = Some(args[i].as_str());
                        } else {
                            eprintln!("Missing pattern after --filter");
                            process::exit(2);
                        }
                    }
                    "--strict" => strict = true,
                    other => {
                        eprintln!("Unknown argument: {}", other);
                        process::exit(2);
                    }
                }
                i += 1;
            }
            match benchmarks::run_all(filter) {
                Ok(all_passed) => {
                    if !all_passed {
                        eprintln!("\nOne or more benchmark target gates FAILED.");
                        if strict {
                            process::exit(1);
                        }
                    } else {
                        println!("\nAll benchmark target gates PASSED.");
                    }
                }
                Err(err) => {
                    eprintln!("Benchmark error: {}", err);
                    process::exit(1);
                }
            }
        }
        Some("check") => run_check_cmd(args.get(1)),
        Some("disasm") => run_disasm_cmd(args.get(1)),
        Some("run") => run_exec_cmd(args.get(1)),
        Some("--version" | "-V" | "version") => {
            println!("Conduit v{}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help" | "-h" | "help") => {
            println!("Conduit v{}\n\nUsage:\n  conduit bench [--filter <pat>] [--strict]\n  conduit check <file.cdt>\n  conduit disasm <file.cdt>\n  conduit run <file.cdt>\n  conduit --version, -V\n  conduit --help, -h", env!("CARGO_PKG_VERSION"));
        }
        Some(cmd) => {
            eprintln!("Unknown command '{}'. Usage: conduit <bench|check|disasm|run> ...", cmd);
            process::exit(2);
        }
    }
}

fn read_source_arg(arg: Option<&String>, cmd: &str) -> String {
    let Some(path) = arg else {
        eprintln!("Usage: conduit {} <file.cdt>", cmd);
        process::exit(2);
    };
    match fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("Failed to read '{}': {}", path, e);
            process::exit(1);
        }
    }
}

fn run_check_cmd(arg: Option<&String>) {
    let src = read_source_arg(arg, "check");
    match frontend::check_source(&src) {
        Ok((_, checked)) => {
            println!(
                "OK: {} type(s), {} function(s)",
                checked.structs.len(),
                checked.funcs.len()
            );
        }
        Err(e) => {
            eprintln!("Check error: {}", e);
            process::exit(1);
        }
    }
}

fn run_disasm_cmd(arg: Option<&String>) {
    let src = read_source_arg(arg, "disasm");
    match frontend::compile_source(&src) {
        Ok(module) => print!("{}", module.disassemble()),
        Err(e) => {
            eprintln!("Compile error: {}", e);
            process::exit(1);
        }
    }
}

fn run_exec_cmd(arg: Option<&String>) {
    let src = read_source_arg(arg, "run");
    let (program, checked) = match frontend::check_source(&src) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Check error: {}", e);
            process::exit(1);
        }
    };
    let module = match compile_program(&program, &checked) {
        Ok(m) => {
            if let Err(e) = m.validate() {
                eprintln!("Validation error: {}", e);
                process::exit(1);
            }
            m
        }
        Err(e) => {
            eprintln!("Compile error: {}", e);
            process::exit(1);
        }
    };
    let Some(fid) = checked.func_by_name.get("main").copied() else {
        eprintln!("No 'main' function found");
        process::exit(1);
    };
    let sig = &checked.funcs[fid as usize];
    if !sig.params.is_empty() {
        eprintln!("'main' must take 0 parameters for CLI run");
        process::exit(1);
    }
    let mut vm = Vm::new();
    match vm.call(&module, fid, &[], &[]) {
        Ok(ExecResult::Void) => {}
        Ok(ExecResult::Scalar(v)) => match sig.return_type {
            Type::Float => println!("{}", f64::from_bits(v)),
            Type::Bool => println!("{}", v != 0),
            _ => println!("{}", v as i64),
        },
        Ok(ExecResult::Obj(obj)) => println!("{:?}", obj),
        Err(e) => {
            eprintln!("Runtime error: {}", e);
            process::exit(1);
        }
    }
}
