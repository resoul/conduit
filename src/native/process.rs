use std::env;
use std::process::Command;
use std::sync::Arc;

use crate::vm::value::{Obj, SCHEMA_PROCESS_ERROR, SCHEMA_PROCESS_OUTPUT, StructData};

pub const PROC_RUN: u8 = 0;
pub const PROC_STATUS: u8 = 1;
pub const PROC_CWD: u8 = 2;
pub const PROC_SET_CWD: u8 = 3;
pub const PROC_PID: u8 = 4;
pub const PROC_EXIT: u8 = 5;

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
        Err(e) => {
            let kind = match e.kind() {
                std::io::ErrorKind::NotFound => "not_found",
                std::io::ErrorKind::PermissionDenied => "permission_denied",
                _ => "spawn_failed",
            };
            make_process_error(kind, format!("Failed to run '{}': {}", cmd, e))
        }
    }
}

pub fn proc_status(cmd: &str, args: &[Obj]) -> Obj {
    let str_args = match extract_str_args(args) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match Command::new(cmd).args(&str_args).status() {
        Ok(st) => Obj::boxed_int(i64::from(st.code().unwrap_or(-1))),
        Err(e) => {
            let kind = match e.kind() {
                std::io::ErrorKind::NotFound => "not_found",
                std::io::ErrorKind::PermissionDenied => "permission_denied",
                _ => "spawn_failed",
            };
            make_process_error(kind, format!("Failed to run '{}': {}", cmd, e))
        }
    }
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
