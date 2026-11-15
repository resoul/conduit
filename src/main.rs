use std::env;
use std::path::{Path, PathBuf};
use std::process;

mod watch;

use conduit::benchmarks;
use conduit::frontend;
use conduit::frontend::types::Type;
use conduit::ir::bundle::{
    RET_BOOL, RET_FLOAT, RET_INT, RET_OBJ, RET_VOID, decode_bundle, encode_bundle,
    extract_bundle_from_exe, write_standalone_binary,
};
use conduit::ir::lower::compile_program;
use conduit::vm::{ExecResult, Vm};

fn print_exec_result(res: ExecResult, ret_tag: u8) {
    match res {
        ExecResult::Void => {}
        ExecResult::Scalar(v) => match ret_tag {
            RET_FLOAT => println!("{}", f64::from_bits(v)),
            RET_BOOL => println!("{}", v != 0),
            _ => println!("{}", v as i64),
        },
        ExecResult::Obj(obj) => println!("{:?}", obj),
    }
}

fn type_to_ret_tag(ty: &Type) -> u8 {
    match ty {
        Type::Void => RET_VOID,
        Type::Float => RET_FLOAT,
        Type::Bool => RET_BOOL,
        Type::Int => RET_INT,
        _ => RET_OBJ,
    }
}

fn main() {
    if let Ok(exe) = env::current_exe()
        && let Ok(Some(bundle)) = extract_bundle_from_exe(&exe)
    {
        let (module, main_fid, main_ret_tag) = match decode_bundle(&bundle) {
            Ok(res) => res,
            Err(e) => {
                eprintln!("Bundle error: {}", e);
                process::exit(1);
            }
        };
        let mut vm = Vm::new();
        match vm.call(&module, main_fid, &[], &[]) {
            Ok(res) => print_exec_result(res, main_ret_tag),
            Err(e) => {
                eprintln!("Runtime error: {}", e);
                process::exit(1);
            }
        }
        return;
    }

    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("bench") | None => {
            let mut filter: Option<&str> = None;
            let mut strict = false;
            let mut i = usize::from(args.first().map(|s| s.as_str()) == Some("bench"));
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
        Some("build") => run_build_cmd(&args[1..]),
        Some("check") => run_check_cmd(args.get(1)),
        Some("disasm") => run_disasm_cmd(args.get(1)),
        Some("run") => run_exec_cmd(&args[1..]),
        Some("watch") => watch::run_watch_cmd(&args[1..]),
        Some("lsp") => {
            if let Err(e) = conduit::lsp::run_lsp_server() {
                eprintln!("LSP error: {}", e);
                process::exit(1);
            }
        }
        Some("--version" | "-V" | "version") => {
            println!("Conduit v{}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help" | "-h" | "help") => {
            println!(
                "Conduit v{}\n\nUsage:\n  conduit bench [--filter <pat>] [--strict]\n  conduit build <file.cdt> [-o <output>]\n  conduit check <file.cdt>\n  conduit disasm <file.cdt>\n  conduit run <file.cdt> [-- args...]\n  conduit watch <file.cdt> [-- args...]\n  conduit lsp\n  conduit --version, -V\n  conduit --help, -h",
                env!("CARGO_PKG_VERSION")
            );
        }
        Some(cmd) => {
            eprintln!("Unknown command '{}'. Usage: conduit <bench|build|check|disasm|run|watch|lsp> ...", cmd);
            process::exit(2);
        }
    }
}

fn require_path_arg<'a>(arg: Option<&'a String>, cmd: &str) -> &'a Path {
    let Some(path) = arg else {
        eprintln!("Usage: conduit {} <file.cdt>", cmd);
        process::exit(2);
    };
    Path::new(path)
}

fn run_check_cmd(arg: Option<&String>) {
    let path = require_path_arg(arg, "check");
    match frontend::check_file(path) {
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
    let path = require_path_arg(arg, "disasm");
    match frontend::compile_file(path) {
        Ok(module) => print!("{}", module.disassemble()),
        Err(e) => {
            eprintln!("Compile error: {}", e);
            process::exit(1);
        }
    }
}

fn run_exec_cmd(args: &[String]) {
    let path = require_path_arg(args.first(), "run");
    let extra = args.get(1..).unwrap_or(&[]);
    let script_args: Vec<String> = if extra.first().map(|s| s.as_str()) == Some("--") {
        extra[1..].to_vec()
    } else {
        extra.to_vec()
    };
    let (program, checked) = match frontend::check_file(path) {
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
    let ret_tag = type_to_ret_tag(&sig.return_type);
    let mut vm = Vm::new();
    vm.script_args = Some(script_args);
    match vm.call(&module, fid, &[], &[]) {
        Ok(res) => print_exec_result(res, ret_tag),
        Err(e) => {
            eprintln!("Runtime error: {}", e);
            process::exit(1);
        }
    }
}

fn run_build_cmd(args: &[String]) {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                } else {
                    eprintln!("Missing path after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            arg if !arg.starts_with('-') && input.is_none() => {
                input = Some(arg.to_string());
            }
            other => {
                eprintln!("Usage: conduit build <file.cdt> [-o <output>] (unexpected '{}')", other);
                process::exit(2);
            }
        }
        i += 1;
    }

    let Some(in_str) = input else {
        eprintln!("Usage: conduit build <file.cdt> [-o <output>]");
        process::exit(2);
    };
    let in_path = Path::new(&in_str);
    let out_path: PathBuf = match output {
        Some(o) => PathBuf::from(o),
        None => {
            let stem = in_path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("app");
            PathBuf::from(stem)
        }
    };
    let same_file = in_path == out_path
        || (in_path.canonicalize().ok().is_some()
            && in_path.canonicalize().ok() == out_path.canonicalize().ok());
    if same_file {
        eprintln!("Refusing to overwrite input source file '{}'", in_path.display());
        process::exit(2);
    }

    let (program, checked) = match frontend::check_file(in_path) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Check error: {}", e);
            process::exit(1);
        }
    };
    let Some(fid) = checked.func_by_name.get("main").copied() else {
        eprintln!("No 'main' function found");
        process::exit(1);
    };
    let sig = &checked.funcs[fid as usize];
    if !sig.params.is_empty() {
        eprintln!("'main' must take 0 parameters for conduit build");
        process::exit(1);
    }
    let ret_tag = type_to_ret_tag(&sig.return_type);

    let module = match compile_program(&program, &checked) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Compile error: {}", e);
            process::exit(1);
        }
    };
    if let Err(e) = module.validate() {
        eprintln!("Validation error: {}", e);
        process::exit(1);
    }
    let bundle = match encode_bundle(&module, fid, ret_tag) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Bundle error: {}", e);
            process::exit(1);
        }
    };
    let base_exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to locate conduit executable: {}", e);
            process::exit(1);
        }
    };
    if let Err(e) = write_standalone_binary(&base_exe, &out_path, &bundle) {
        eprintln!("Build error: {}", e);
        process::exit(1);
    }
    println!(
        "Built standalone binary '{}' ({} bytes bytecode)",
        out_path.display(),
        bundle.len()
    );
}
