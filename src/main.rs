use std::env;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process;

mod test_runner;
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
        Some("routes") => run_routes_cmd(&args[1..]),
        Some("openapi") => run_openapi_cmd(&args[1..]),
        Some("test") => test_runner::run_test_cmd(&args[1..]),
        Some("watch") => watch::run_watch_cmd(&args[1..]),
        Some("fmt") => run_fmt_cmd(&args[1..]),
        Some("db") => run_db_cmd(&args[1..]),
        Some("env") => run_env_cmd(&args[1..]),
        Some("cron") => run_cron_cmd(&args[1..]),
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
                "Conduit v{}\n\nUsage:\n  conduit bench [--filter <pat>] [--strict]\n  conduit build <file.cdt> [-o <output>]\n  conduit check <file.cdt>\n  conduit cron <list|print|run> <file.cdt> [job]\n  conduit db <migrate|rollback|status> <file.cdt> [--url <dsn>] [--steps <n>] [--dry-run]\n  conduit disasm <file.cdt>\n  conduit env example <file.cdt> [-o <file>]\n  conduit fmt <file_or_dir> [--check]\n  conduit openapi <file.cdt> [--router <name>] [--title <str>] [--version <str>] [--server <url>] [-o <file>]\n  conduit routes <file.cdt> [--router <name>]\n  conduit run <file.cdt> [-- args...]\n  conduit test [file_or_dir] [--filter <pat>] [--fail-fast]\n  conduit watch <file.cdt> [-- args...]\n  conduit lsp\n  conduit --version, -V\n  conduit --help, -h",
                env!("CARGO_PKG_VERSION")
            );
        }
        Some(cmd) => {
            eprintln!("Unknown command '{}'. Usage: conduit <bench|build|check|cron|db|disasm|env|fmt|openapi|routes|run|test|watch|lsp> ...", cmd);
            process::exit(2);
        }
    }
}

fn run_fmt_cmd(args: &[String]) {
    let mut check_only = false;
    let mut target: Option<&String> = None;
    for arg in args {
        if arg == "--check" {
            check_only = true;
        } else if target.is_none() {
            target = Some(arg);
        }
    }
    let path = target.map(|s| Path::new(s.as_str())).unwrap_or_else(|| Path::new("."));
    match conduit::fmt::format_path(path, check_only) {
        Ok(changed) => {
            if check_only && changed > 0 {
                eprintln!("{} file(s) need formatting", changed);
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Format error: {}", e);
            process::exit(1);
        }
    }
}

fn run_db_cmd(args: &[String]) {
    use conduit::vm::exec_db::{
        EitherMigErr, db_migration_status_at, rollback_db_migrations_at, run_db_migrations_at,
    };
    let Some(sub) = args.first().map(|s| s.as_str()) else {
        eprintln!("Usage: conduit db <migrate|rollback|status> <file.cdt> [--url <dsn>] [--steps <n>] [--dry-run]");
        process::exit(2);
    };
    if !matches!(sub, "migrate" | "rollback" | "status") {
        eprintln!("Unknown db subcommand '{}'. Expected: migrate, rollback, or status", sub);
        process::exit(2);
    }
    let mut file: Option<String> = None;
    let mut url: Option<String> = None;
    let mut steps: usize = 1;
    let mut dry_run = false;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--url" => {
                i += 1;
                if i < args.len() {
                    url = Some(args[i].clone());
                } else {
                    eprintln!("Missing DSN after --url");
                    process::exit(2);
                }
            }
            "--steps" => {
                i += 1;
                if i < args.len() {
                    steps = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid --steps '{}'", args[i]);
                        process::exit(2);
                    });
                } else {
                    eprintln!("Missing number after --steps");
                    process::exit(2);
                }
            }
            "--dry-run" => dry_run = true,
            arg if !arg.starts_with('-') && file.is_none() => file = Some(arg.to_string()),
            other => {
                eprintln!("Unknown argument '{}' in conduit db {}", other, sub);
                process::exit(2);
            }
        }
        i += 1;
    }
    let Some(file_str) = file else {
        eprintln!("Usage: conduit db {} <file.cdt> [--url <dsn>] [--steps <n>] [--dry-run]", sub);
        process::exit(2);
    };
    let dsn = url.or_else(|| env::var("DATABASE_URL").ok()).unwrap_or_else(|| {
        eprintln!("Missing database URL: pass --url <dsn> or set DATABASE_URL");
        process::exit(2);
    });
    let (program, checked) = match frontend::check_file(Path::new(&file_str)) {
        Ok(res) => res,
        Err(e) => {
            print_check_error(Path::new(&file_str), &e);
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
    let mut vm = Vm::new();
    let cid = match vm.open_db(&dsn) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Database connection error: {}", e.message);
            process::exit(1);
        }
    };
    let format_mig_err = |e: EitherMigErr| -> String {
        match e {
            EitherMigErr::Db(d) => d.message,
            EitherMigErr::Vm(v) => v.message,
        }
    };
    match sub {
        "migrate" => match run_db_migrations_at(&mut vm, &module, cid, 0, 0, dry_run) {
            Ok(list) if list.is_empty() => println!("No pending migrations."),
            Ok(list) => {
                for (ver, name) in &list {
                    if dry_run {
                        println!("[dry-run] would apply {} '{}'", ver, name);
                    } else {
                        println!("Applied {} '{}'", ver, name);
                    }
                }
            }
            Err(e) => {
                eprintln!("Migration error: {}", format_mig_err(e));
                process::exit(1);
            }
        },
        "rollback" => match rollback_db_migrations_at(&mut vm, &module, cid, steps, dry_run) {
            Ok(list) if list.is_empty() => println!("No applied migrations to rollback."),
            Ok(list) => {
                for (ver, name) in &list {
                    if dry_run {
                        println!("[dry-run] would rollback {} '{}'", ver, name);
                    } else {
                        println!("Rolled back {} '{}'", ver, name);
                    }
                }
            }
            Err(e) => {
                eprintln!("Rollback error: {}", format_mig_err(e));
                process::exit(1);
            }
        },
        "status" => match db_migration_status_at(&vm, &module, cid) {
            Ok(rows) if rows.is_empty() => println!("No migrations defined."),
            Ok(rows) => {
                for (ver, name, applied_at) in rows {
                    match applied_at {
                        Some(ts) => println!("[applied @ {}] {} '{}'", ts, ver, name),
                        None => println!("[pending] {} '{}'", ver, name),
                    }
                }
            }
            Err(e) => {
                eprintln!("Status error: {}", e.message);
                process::exit(1);
            }
        },
        _ => unreachable!(),
    }
}

fn print_check_error(path: &Path, err: &str) {
    let rendered = conduit::diagnostics::render_diagnostics(
        err,
        Some(path),
        std::io::stderr().is_terminal(),
    );
    eprintln!("{}", rendered);
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
            print_check_error(path, &e);
            process::exit(1);
        }
    }
}

fn run_disasm_cmd(arg: Option<&String>) {
    let path = require_path_arg(arg, "disasm");
    match frontend::compile_file(path) {
        Ok(module) => print!("{}", module.disassemble()),
        Err(e) => {
            print_check_error(path, &e);
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
            print_check_error(path, &e);
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
            print_check_error(in_path, &e);
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

fn run_cron_cmd(args: &[String]) {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!("Usage: conduit cron <list|print|run> <file.cdt> [job_name]");
        process::exit(2);
    };
    let Some(file) = args.get(1) else {
        eprintln!("Usage: conduit cron {} <file.cdt> [job_name]", sub);
        process::exit(2);
    };
    let (program, checked) = match frontend::check_file(Path::new(file)) {
        Ok(res) => res,
        Err(e) => {
            print_check_error(Path::new(file), &e);
            process::exit(1);
        }
    };
    let module = match compile_program(&program, &checked) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Compile error: {}", e);
            process::exit(1);
        }
    };
    match sub {
        "list" => {
            for sched in &module.schedules {
                println!("schedule {}:", sched.name);
                for job in &sched.jobs {
                    if let Some(expr) = &job.cron_expr {
                        println!("  {} -> every \"{}\"", job.name, expr);
                    } else if let Some(ms) = job.interval_ms {
                        println!("  {} -> interval {}ms", job.name, ms);
                    }
                }
            }
        }
        "print" => {
            let abs = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));
            for sched in &module.schedules {
                for job in &sched.jobs {
                    if let Some(expr) = &job.cron_expr {
                        println!("{} conduit cron run {} {}", expr, abs.display(), job.name);
                    }
                }
            }
        }
        "run" => {
            let Some(job_name) = args.get(2) else {
                eprintln!("Usage: conduit cron run <file.cdt> <job_name>");
                process::exit(2);
            };
            let Some(job) = module
                .schedules
                .iter()
                .flat_map(|s| s.jobs.iter())
                .find(|j| j.name == *job_name)
            else {
                eprintln!("Unknown cron job '{}'", job_name);
                process::exit(1);
            };
            let mut vm = Vm::new();
            match vm.call(&module, job.func_id, &[], &[]) {
                Ok(ExecResult::Obj(obj))
                    if conduit::native::validation::is_err_or_nil_obj(&module, &obj)
                        && !matches!(obj, conduit::vm::value::Obj::Empty) =>
                {
                    eprintln!("Job '{}' failed: {}", job_name, obj.format_obj());
                    process::exit(1);
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            }
        }
        other => {
            eprintln!("Unknown cron subcommand '{}'. Expected list, print, or run", other);
            process::exit(2);
        }
    }
}

fn run_routes_cmd(args: &[String]) {
    let mut file: Option<String> = None;
    let mut router: Option<String> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--router" | "-r" => {
                i += 1;
                if i < args.len() {
                    router = Some(args[i].clone());
                } else {
                    eprintln!("Missing router name after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            arg if !arg.starts_with('-') && file.is_none() => {
                file = Some(arg.to_string());
            }
            other => {
                eprintln!(
                    "Usage: conduit routes <file.cdt> [--router <name>] (unexpected '{}')",
                    other
                );
                process::exit(2);
            }
        }
        i += 1;
    }
    let Some(file_str) = file else {
        eprintln!("Usage: conduit routes <file.cdt> [--router <name>]");
        process::exit(2);
    };
    let path = Path::new(&file_str);
    let (_, checked) = match frontend::check_file(path) {
        Ok(res) => res,
        Err(e) => {
            print_check_error(path, &e);
            process::exit(1);
        }
    };
    match conduit::openapi::format_routes(&checked, router.as_deref()) {
        Ok(table) => println!("{}", table),
        Err(e) => {
            eprintln!("Routes error: {}", e);
            process::exit(1);
        }
    }
}

fn run_openapi_cmd(args: &[String]) {
    let mut file: Option<String> = None;
    let mut router: Option<String> = None;
    let mut title: Option<String> = None;
    let mut version: Option<String> = None;
    let mut server: Option<String> = None;
    let mut output: Option<String> = None;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--router" | "-r" => {
                i += 1;
                if i < args.len() {
                    router = Some(args[i].clone());
                } else {
                    eprintln!("Missing router name after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            "--title" => {
                i += 1;
                if i < args.len() {
                    title = Some(args[i].clone());
                } else {
                    eprintln!("Missing title after --title");
                    process::exit(2);
                }
            }
            "--version" => {
                i += 1;
                if i < args.len() {
                    version = Some(args[i].clone());
                } else {
                    eprintln!("Missing version after --version");
                    process::exit(2);
                }
            }
            "--server" | "-s" => {
                i += 1;
                if i < args.len() {
                    server = Some(args[i].clone());
                } else {
                    eprintln!("Missing URL after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                } else {
                    eprintln!("Missing output path after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            arg if !arg.starts_with('-') && file.is_none() => {
                file = Some(arg.to_string());
            }
            other => {
                eprintln!(
                    "Usage: conduit openapi <file.cdt> [--router <name>] [--title <str>] [--version <str>] [--server <url>] [-o <file>] (unexpected '{}')",
                    other
                );
                process::exit(2);
            }
        }
        i += 1;
    }

    let Some(file_str) = file else {
        eprintln!(
            "Usage: conduit openapi <file.cdt> [--router <name>] [--title <str>] [--version <str>] [--server <url>] [-o <file>]"
        );
        process::exit(2);
    };
    let path = Path::new(&file_str);
    let (program, checked) = match frontend::check_file(path) {
        Ok(res) => res,
        Err(e) => {
            print_check_error(path, &e);
            process::exit(1);
        }
    };

    let resolved_title = title.unwrap_or_else(|| {
        if let Some(r) = &router {
            r.clone()
        } else if checked.http_router_plans.len() == 1 {
            conduit::openapi::clean_symbol_name(&checked.http_router_plans[0].0).to_string()
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("Conduit API")
                .to_string()
        }
    });
    let resolved_version = version.as_deref().unwrap_or("0.1.0");

    let spec = match conduit::openapi::generate_openapi_json(
        &program,
        &checked,
        &resolved_title,
        resolved_version,
        router.as_deref(),
        server.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("OpenAPI error: {}", e);
            process::exit(1);
        }
    };

    if let Some(out_file) = output {
        if let Err(e) = std::fs::write(&out_file, format!("{}\n", spec)) {
            eprintln!("Failed to write '{}': {}", out_file, e);
            process::exit(1);
        }
    } else {
        println!("{}", spec);
    }
}

fn run_env_cmd(args: &[String]) {
    if args.first().map(|s| s.as_str()) != Some("example") {
        eprintln!("Usage: conduit env example <file.cdt> [-o <file>]");
        process::exit(2);
    }
    let mut file: Option<String> = None;
    let mut output: Option<String> = None;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                } else {
                    eprintln!("Missing path after {}", args[i - 1]);
                    process::exit(2);
                }
            }
            arg if !arg.starts_with('-') && file.is_none() => {
                file = Some(arg.to_string());
            }
            other => {
                eprintln!("Usage: conduit env example <file.cdt> [-o <file>] (unexpected '{}')", other);
                process::exit(2);
            }
        }
        i += 1;
    }

    let Some(file_str) = file else {
        eprintln!("Usage: conduit env example <file.cdt> [-o <file>]");
        process::exit(2);
    };
    let path = Path::new(&file_str);
    let (_program, checked) = match frontend::check_file(path) {
        Ok(res) => res,
        Err(e) => {
            print_check_error(path, &e);
            process::exit(1);
        }
    };
    let content = match conduit::native::env::generate_env_example(&checked) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Env example error: {}", e);
            process::exit(1);
        }
    };
    if let Some(out_file) = output {
        if let Err(e) = std::fs::write(&out_file, format!("{}\n", content)) {
            eprintln!("Failed to write '{}': {}", out_file, e);
            process::exit(1);
        }
    } else {
        println!("{}", content);
    }
}
