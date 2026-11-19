use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use conduit::frontend;
use conduit::frontend::ast::ItemKind;
use conduit::vm::Vm;

pub fn run_test_cmd(args: &[String]) {
    let mut target: Option<String> = None;
    let mut filter: Option<String> = None;
    let mut fail_fast = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" | "-f" => {
                i += 1;
                if i < args.len() {
                    filter = Some(args[i].clone());
                } else {
                    eprintln!("Missing pattern after --filter");
                    process::exit(2);
                }
            }
            "--fail-fast" => fail_fast = true,
            arg if !arg.starts_with('-') && target.is_none() => {
                target = Some(arg.to_string());
            }
            other => {
                eprintln!(
                    "Usage: conduit test [file_or_dir] [--filter <pat>] [--fail-fast] (unexpected '{}')",
                    other
                );
                process::exit(2);
            }
        }
        i += 1;
    }

    let target_path = PathBuf::from(target.as_deref().unwrap_or("."));
    let files = match discover_test_files(&target_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Test discovery error: {}", e);
            process::exit(2);
        }
    };

    if files.is_empty() {
        println!("No .cdt test files found in '{}'.", target_path.display());
        println!("test result: ok. 0 passed; 0 failed; 0 filtered out (0.00 ms)");
        return;
    }

    let total_start = Instant::now();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut filtered_out = 0usize;

    'files: for file in &files {
        println!("running {}", file.display());
        let module = match frontend::compile_file(file) {
            Ok(m) => m,
            Err(e) => {
                println!("  FAIL  {} (compile error)", file.display());
                for line in e.lines() {
                    println!("        {}", line);
                }
                failed += 1;
                if fail_fast {
                    break;
                }
                continue;
            }
        };

        for spec in &module.tests {
            if let Some(pat) = filter.as_deref()
                && !spec.name.contains(pat)
            {
                filtered_out += 1;
                continue;
            }
            let t0 = Instant::now();
            let mut vm = Vm::new();
            let res = vm.call(&module, spec.func_id, &[], &[]);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match res {
                Ok(_) => {
                    println!("  PASS  {} ({:.2} ms)", spec.name, ms);
                    passed += 1;
                }
                Err(e) => {
                    println!("  FAIL  {} ({:.2} ms)", spec.name, ms);
                    for line in e.to_string().lines() {
                        println!("        {}", line);
                    }
                    failed += 1;
                    if fail_fast {
                        break 'files;
                    }
                }
            }
        }
    }

    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
    let status = if failed == 0 { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {}. {} passed; {} failed; {} filtered out ({:.2} ms)",
        status, passed, failed, filtered_out, total_ms
    );
    if failed > 0 {
        process::exit(1);
    }
}

fn discover_test_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.exists() {
        return Err(format!("Path does not exist: '{}'", path.display()));
    }
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut candidates = Vec::new();
    collect_cdt_recursive(path, &mut candidates)?;
    candidates.sort();

    let mut selected = Vec::new();
    for file in candidates {
        let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let is_named_test = name.ends_with("_test.cdt") || name.starts_with("test_");
        let Ok(src) = fs::read_to_string(&file) else {
            if is_named_test {
                selected.push(file);
            }
            continue;
        };
        match frontend::parse_source(&src) {
            Ok(prog) => {
                if prog
                    .items
                    .iter()
                    .any(|it| matches!(it.kind, ItemKind::Test { .. }))
                {
                    selected.push(file);
                }
            }
            Err(_) if is_named_test => selected.push(file),
            Err(_) => {}
        }
    }
    Ok(selected)
}

fn collect_cdt_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory '{}': {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Directory entry error: {}", e))?;
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with('.')
                || matches!(name, "target" | "node_modules" | "dist" | "book")
            {
                continue;
            }
            collect_cdt_recursive(&p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("cdt") {
            out.push(p);
        }
    }
    Ok(())
}
