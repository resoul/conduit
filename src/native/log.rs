use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_DEBUG: u8 = 0;
pub const LOG_INFO: u8 = 1;
pub const LOG_WARN: u8 = 2;
pub const LOG_ERROR: u8 = 3;
pub const LOG_SET_LEVEL: u8 = 4;
pub const LOG_SET_JSON: u8 = 5;
pub const LOG_GET_LEVEL: u8 = 6;

static MIN_LEVEL: AtomicU8 = AtomicU8::new(LOG_INFO);
static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_log_level(level: &str) {
    let code = match level.trim().to_ascii_lowercase().as_str() {
        "debug" => LOG_DEBUG,
        "info" => LOG_INFO,
        "warn" | "warning" => LOG_WARN,
        "error" => LOG_ERROR,
        _ => return,
    };
    MIN_LEVEL.store(code, Ordering::Relaxed);
}

pub fn get_log_level() -> &'static str {
    match MIN_LEVEL.load(Ordering::Relaxed) {
        LOG_DEBUG => "debug",
        LOG_INFO => "info",
        LOG_WARN => "warn",
        _ => "error",
    }
}

pub fn set_log_json(enabled: bool) {
    JSON_MODE.store(enabled, Ordering::Relaxed);
}

fn utc_iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let hour = sod / 3_600;
    let min = (sod % 3_600) / 60;
    let sec = sod % 60;

    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hour, min, sec)
}

pub fn format_log_line(level: u8, msg: &str) -> Option<String> {
    if level < MIN_LEVEL.load(Ordering::Relaxed) {
        return None;
    }
    let ts = utc_iso_now();
    let (lower, upper) = match level {
        LOG_DEBUG => ("debug", "DEBUG"),
        LOG_INFO => ("info", "INFO "),
        LOG_WARN => ("warn", "WARN "),
        _ => ("error", "ERROR"),
    };
    if JSON_MODE.load(Ordering::Relaxed) {
        let mut escaped = String::with_capacity(msg.len());
        for ch in msg.chars() {
            match ch {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    use std::fmt::Write;
                    let _ = write!(escaped, "\\u{:04x}", c as u32);
                }
                c => escaped.push(c),
            }
        }
        Some(format!(
            r#"{{"ts":"{}","level":"{}","msg":"{}"}}"#,
            ts, lower, escaped
        ))
    } else {
        Some(format!("[{}] {} {}", ts, upper, msg))
    }
}

pub fn emit_log(level: u8, msg: &str) {
    if let Some(line) = format_log_line(level, msg) {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{}", line);
    }
}
