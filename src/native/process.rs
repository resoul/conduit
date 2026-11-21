use std::env;
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;

use crate::vm::value::{ListStorage, Obj, SCHEMA_PROCESS_ERROR, SCHEMA_PROCESS_OUTPUT, StructData};

pub const PROC_RUN: u8 = 0;
pub const PROC_STATUS: u8 = 1;
pub const PROC_CWD: u8 = 2;
pub const PROC_SET_CWD: u8 = 3;
pub const PROC_PID: u8 = 4;
pub const PROC_EXIT: u8 = 5;
pub const PROC_PIPE: u8 = 6;

#[inline]
pub fn make_process_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_PROCESS_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_process_output(code: i64, stdout: String, stderr: String) -> Obj {
    let mut s = StructData::new(SCHEMA_PROCESS_OUTPUT, 1, 2);
    s.inline_scalars[0] = code as u64;
    s.inline_objs[0] = Obj::str(stdout);
    s.inline_objs[1] = Obj::str(stderr);
    Obj::Record(Arc::new(s))
}

fn extract_str_args(args: &[Obj]) -> Result<Vec<&str>, Obj> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        match a {
            Obj::Str(s) => out.push(s.as_str()),
            _ => return Err(make_process_error("invalid_input", "Process args must be [String]")),
        }
    }
    Ok(out)
}

fn io_err_kind(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        _ => "spawn_failed",
    }
}

pub fn proc_run(cmd: &str, args: &[Obj]) -> Obj {
    let str_args = match extract_str_args(args) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match Command::new(cmd).args(&str_args).output() {
        Ok(out) => {
            let code = i64::from(out.status.code().unwrap_or(-1));
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            make_process_output(code, stdout, stderr)
        }
        Err(e) => make_process_error(io_err_kind(&e), format!("Failed to run '{}': {}", cmd, e)),
    }
}

pub fn proc_status(cmd: &str, args: &[Obj]) -> Obj {
    let str_args = match extract_str_args(args) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match Command::new(cmd).args(&str_args).status() {
        Ok(st) => Obj::boxed_int(i64::from(st.code().unwrap_or(-1))),
        Err(e) => make_process_error(io_err_kind(&e), format!("Failed to run '{}': {}", cmd, e)),
    }
}

pub fn proc_pipe(cmds: &[Obj]) -> Obj {
    if cmds.is_empty() {
        return make_process_error(
            "invalid_input",
            "process.pipe requires at least one command",
        );
    }

    let mut parsed: Vec<Vec<&str>> = Vec::with_capacity(cmds.len());
    for (idx, item) in cmds.iter().enumerate() {
        let Obj::List(ListStorage::Obj(argv)) = item else {
            return make_process_error(
                "invalid_input",
                "process.pipe commands must be [[String]]",
            );
        };
        if argv.is_empty() {
            return make_process_error(
                "invalid_input",
                format!("Command #{} in process.pipe cannot be empty", idx + 1),
            );
        }
        match extract_str_args(argv) {
            Ok(strs) => parsed.push(strs),
            Err(e) => return e,
        }
    }

    let mut children: Vec<Child> = Vec::with_capacity(parsed.len());
    let mut prev_stdout: Option<ChildStdout> = None;

    for (idx, argv) in parsed.iter().enumerate() {
        let is_last = idx + 1 == parsed.len();
        let mut cmd = Command::new(argv[0]);
        cmd.args(&argv[1..]);
        if let Some(prev) = prev_stdout.take() {
            cmd.stdin(Stdio::from(prev));
        } else {
            cmd.stdin(Stdio::null());
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(if is_last {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        match cmd.spawn() {
            Ok(mut child) => {
                prev_stdout = child.stdout.take();
                children.push(child);
            }
            Err(e) => {
                drop(prev_stdout);
                for mut c in children {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return make_process_error(
                    io_err_kind(&e),
                    format!("Failed to run '{}': {}", argv[0], e),
                );
            }
        }
    }

    let mut last_stdout = prev_stdout;
    let mut last_stderr = children.last_mut().and_then(|c| c.stderr.take());
    let mut out_bytes = Vec::new();
    let mut err_bytes = Vec::new();

    let read_res: std::io::Result<()> = std::thread::scope(|s| {
        let h_out = s.spawn(|| -> std::io::Result<usize> {
            if let Some(ref mut r) = last_stdout {
                r.read_to_end(&mut out_bytes)
            } else {
                Ok(0)
            }
        });
        let h_err = s.spawn(|| -> std::io::Result<usize> {
            if let Some(ref mut r) = last_stderr {
                r.read_to_end(&mut err_bytes)
            } else {
                Ok(0)
            }
        });
        h_out
            .join()
            .map_err(|_| std::io::Error::other("stdout reader panicked"))??;
        h_err
            .join()
            .map_err(|_| std::io::Error::other("stderr reader panicked"))??;
        Ok(())
    });

    if let Err(e) = read_res {
        for mut c in children {
            let _ = c.kill();
            let _ = c.wait();
        }
        return make_process_error("io", format!("Failed reading pipeline output: {}", e));
    }

    let count = children.len();
    let mut last_code = -1i64;
    for (idx, mut child) in children.into_iter().enumerate() {
        match child.wait() {
            Ok(st) => {
                if idx + 1 == count {
                    last_code = i64::from(st.code().unwrap_or(-1));
                }
            }
            Err(e) => {
                return make_process_error("io", format!("Failed waiting for process: {}", e));
            }
        }
    }

    let stdout = String::from_utf8_lossy(&out_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&err_bytes).into_owned();
    make_process_output(last_code, stdout, stderr)
}

pub fn proc_cwd() -> Obj {
    match env::current_dir() {
        Ok(p) => Obj::str(p.to_string_lossy().into_owned()),
        Err(e) => make_process_error("io", e.to_string()),
    }
}

pub fn proc_set_cwd(dir: &str) -> Obj {
    match env::set_current_dir(dir) {
        Ok(()) => Obj::Empty,
        Err(e) => make_process_error("io", format!("Failed to set cwd '{}': {}", dir, e)),
    }
}
