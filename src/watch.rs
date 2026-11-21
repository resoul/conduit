use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command};
use std::thread;
use std::time::{Duration, SystemTime};

pub type FileSnapshot = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

fn walk_cdt_files(dir: &Path, out: &mut FileSnapshot) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
            continue;
        }
        if path.is_dir() {
            walk_cdt_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("cdt")
            && let Ok(meta) = fs::metadata(&path)
        {
            out.insert(path, (meta.len(), meta.modified().ok()));
        }
    }
}

/// Collects size and modification timestamp for `root_file` and all `.cdt` files under its directory.
pub fn collect_cdt_snapshots(root_file: &Path) -> FileSnapshot {
    let mut out = BTreeMap::new();
    let watch_dir = root_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    walk_cdt_files(watch_dir, &mut out);
    if let Ok(meta) = fs::metadata(root_file) {
        out.insert(root_file.to_path_buf(), (meta.len(), meta.modified().ok()));
    }
    out
}

fn spawn_run(
    exe: &Path,
    perm_flags: &[String],
    file: &Path,
    extra_args: &[String],
) -> Option<Child> {
    let mut cmd = Command::new(exe);
    cmd.arg("run");
    for flag in perm_flags {
        cmd.arg(flag);
    }
    cmd.arg(file);
    for arg in extra_args {
        cmd.arg(arg);
    }
    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            eprintln!("[conduit watch] failed to start process: {}", e);
            None
        }
    }
}

fn stop_child(child: &mut Option<Child>) {
    if let Some(mut c) = child.take() {
        if let Ok(None) = c.try_wait() {
            let _ = c.kill();
        }
        let _ = c.wait();
    }
}

pub fn run_watch_cmd(args: &[String]) {
    let mut perms_builder = conduit::vm::PermissionsBuilder::new();
    let mut perm_flags = Vec::new();
    let mut file_idx = None;
    for (idx, arg) in args.iter().enumerate() {
        match perms_builder.apply_flag(arg) {
            Ok(true) => perm_flags.push(arg.clone()),
            Ok(false) => {
                if arg.starts_with('-') {
                    eprintln!(
                        "Unknown option '{}' in conduit watch.\nUsage: conduit watch [perm-flags...] <file.cdt> [args...]",
                        arg
                    );
                    process::exit(2);
                }
                file_idx = Some(idx);
                break;
            }
            Err(e) => {
                eprintln!("Permission flag error: {}", e);
                process::exit(2);
            }
        }
    }
    let Some(idx) = file_idx else {
        eprintln!("Usage: conduit watch [perm-flags...] <file.cdt> [args...]");
        process::exit(2);
    };
    let file = Path::new(&args[idx]);
    if !file.exists() {
        eprintln!("File not found: {}", file.display());
        process::exit(1);
    }
    let extra_args = &args[idx + 1..];
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to locate conduit executable: {}", e);
            process::exit(1);
        }
    };

    println!("[conduit watch] watching '{}' (Ctrl+C to stop)...", file.display());
    let mut prev = collect_cdt_snapshots(file);
    let mut child = spawn_run(&exe, &perm_flags, file, extra_args);

    loop {
        if let Some(c) = child.as_mut()
            && let Ok(Some(status)) = c.try_wait()
        {
            println!("[conduit watch] exited ({}), waiting for changes...", status);
            child = None;
        }
        thread::sleep(Duration::from_millis(200));
        let next = collect_cdt_snapshots(file);
        if next != prev {
            prev = next;
            stop_child(&mut child);
            println!("[conduit watch] change detected, reloading '{}'...", file.display());
            child = spawn_run(&exe, &perm_flags, file, extra_args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn snapshot_detects_cdt_file_changes() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("conduit_watch_test_{}", nanos));
        fs::create_dir_all(&dir).unwrap();
        let main_cdt = dir.join("main.cdt");
        fs::write(&main_cdt, "func main() -> Int { return 1 }").unwrap();

        let snap1 = collect_cdt_snapshots(&main_cdt);
        assert!(snap1.contains_key(&main_cdt));

        fs::write(&main_cdt, "func main() -> Int { return 123 }").unwrap();
        let snap2 = collect_cdt_snapshots(&main_cdt);
        assert_ne!(snap1, snap2);

        let _ = fs::remove_dir_all(&dir);
    }
}
