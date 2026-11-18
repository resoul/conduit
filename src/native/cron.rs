use std::sync::{
    Arc,
    atomic::AtomicBool,
};
use std::thread::JoinHandle;

use crate::vm::value::{Obj, SCHEMA_CRON_ERROR, SCHEMA_CRON_SCHEDULER, StructData};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    pub minutes: [bool; 60],
    pub hours: [bool; 24],
    pub dom: [bool; 32],    // 1..=31
    pub months: [bool; 13], // 1..=12
    pub dow: [bool; 7],     // 0..=6 (0 = Sunday)
    pub dom_wildcard: bool,
    pub dow_wildcard: bool,
}

fn parse_cron_field(
    field: &str,
    min: usize,
    max: usize,
    allow_sunday_7: bool,
    out: &mut [bool],
) -> Result<(), String> {
    if field.is_empty() {
        return Err("Empty cron field".to_string());
    }
    for part in field.split(',') {
        let (range_part, step) = if let Some((lhs, rhs)) = part.split_once('/') {
            let s: usize = rhs
                .parse()
                .map_err(|_| format!("Invalid cron step '{}'", rhs))?;
            if s == 0 {
                return Err("Cron step must be > 0".to_string());
            }
            (lhs, s)
        } else {
            (part, 1)
        };

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((a_s, b_s)) = range_part.split_once('-') {
            let a: usize = a_s
                .parse()
                .map_err(|_| format!("Invalid cron range start '{}'", a_s))?;
            let b: usize = b_s
                .parse()
                .map_err(|_| format!("Invalid cron range end '{}'", b_s))?;
            if a < min || b > max || a > b {
                return Err(format!("Cron range {}-{} out of bounds ({}..={})", a, b, min, max));
            }
            (a, b)
        } else {
            let v: usize = range_part
                .parse()
                .map_err(|_| format!("Invalid cron value '{}'", range_part))?;
            if v < min || v > max {
                return Err(format!("Cron value {} out of bounds ({}..={})", v, min, max));
            }
            if step == 1 {
                let idx = if allow_sunday_7 && v == 7 { 0 } else { v };
                out[idx] = true;
                continue;
            }
            (v, max)
        };

        let mut v = start;
        while v <= end {
            let idx = if allow_sunday_7 && v == 7 { 0 } else { v };
            out[idx] = true;
            v += step;
        }
    }
    Ok(())
}

pub fn parse_cron_expr(raw: &str) -> Result<CronExpr, String> {
    let trimmed = raw.trim();
    let expanded = match trimmed {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        other => other,
    };
    let parts: Vec<&str> = expanded.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!(
            "Expected 5 cron fields (min hour dom mon dow), got {} in '{}'",
            parts.len(),
            raw
        ));
    }
    let mut expr = CronExpr {
        minutes: [false; 60],
        hours: [false; 24],
        dom: [false; 32],
        months: [false; 13],
        dow: [false; 7],
        dom_wildcard: parts[2] == "*",
        dow_wildcard: parts[4] == "*",
    };
    parse_cron_field(parts[0], 0, 59, false, &mut expr.minutes)?;
    parse_cron_field(parts[1], 0, 23, false, &mut expr.hours)?;
    parse_cron_field(parts[2], 1, 31, false, &mut expr.dom)?;
    parse_cron_field(parts[3], 1, 12, false, &mut expr.months)?;
    parse_cron_field(parts[4], 0, 7, true, &mut expr.dow)?;
    Ok(expr)
}

/// Converts Unix timestamp (seconds, >= 0) to UTC `(month 1..=12, day 1..=31, hour 0..=23, min 0..=59, dow 0..=6)`.
pub fn utc_parts_from_unix(ts: u64) -> (u8, u8, u8, u8, u8) {
    let secs_of_day = (ts % 86_400) as u32;
    let hour = (secs_of_day / 3_600) as u8;
    let min = ((secs_of_day % 3_600) / 60) as u8;

    let days = (ts / 86_400) as i64;
    // 1970-01-01 was Thursday (4).
    let dow = (((days + 4) % 7 + 7) % 7) as u8;

    // Howard Hinnant civil-from-days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (m as u8, d as u8, hour, min, dow)
}

impl CronExpr {
    pub fn matches_unix(&self, ts: u64) -> bool {
        let (month, day, hour, min, dow) = utc_parts_from_unix(ts);
        if !self.minutes[min as usize]
            || !self.hours[hour as usize]
            || !self.months[month as usize]
        {
            return false;
        }
        let dom_ok = self.dom[day as usize];
        let dow_ok = self.dow[dow as usize];
        if !self.dom_wildcard && !self.dow_wildcard {
            dom_ok || dow_ok
        } else {
            dom_ok && dow_ok
        }
    }

    pub fn next_tick_after(&self, now_unix: u64) -> Option<u64> {
        let mut t = (now_unix / 60 + 1) * 60;
        // Search up to ~4 years (handles leap-day Feb 29).
        let max_iters = 4 * 366 * 24 * 60;
        for _ in 0..max_iters {
            if self.matches_unix(t) {
                return Some(t);
            }
            t += 60;
        }
        None
    }
}

pub struct CronSchedulerState {
    pub stop: Arc<AtomicBool>,
    pub running: Arc<AtomicBool>,
    pub handle: Option<JoinHandle<()>>,
}

pub fn make_cron_error(kind: &str, message: &str) -> Obj {
    let mut s = StructData::new(SCHEMA_CRON_ERROR, 0, 2);
    s.set_obj(0, Obj::str(kind));
    s.set_obj(1, Obj::str(message));
    Obj::Record(Arc::new(s))
}

pub fn make_cron_scheduler(id: i64) -> Obj {
    let mut s = StructData::new(SCHEMA_CRON_SCHEDULER, 1, 0);
    s.set_scalar(0, id as u64);
    Obj::Record(Arc::new(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_match_cron_expressions() {
        let every_5 = parse_cron_expr("*/5 * * * *").unwrap();
        assert!(every_5.matches_unix(0));
        assert!(!every_5.matches_unix(60));
        assert!(every_5.matches_unix(300));
        assert_eq!(every_5.next_tick_after(61), Some(300));

        let daily = parse_cron_expr("@daily").unwrap();
        assert!(daily.matches_unix(0));
        assert_eq!(daily.next_tick_after(0), Some(86_400));

        assert!(parse_cron_expr("* * *").is_err());
        assert!(parse_cron_expr("60 * * * *").is_err());
        assert!(parse_cron_expr("*/0 * * * *").is_err());
    }
}
