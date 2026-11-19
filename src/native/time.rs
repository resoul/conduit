use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    Obj, SCHEMA_TIME_DURATION, SCHEMA_TIME_ERROR, SCHEMA_TIME_INSTANT, SCHEMA_TIME_TIMESTAMP,
    StructData,
};
use crate::vm::{RuntimeError, Vm};

pub const TIME_ADD: u8 = 0;
pub const TIME_SUB: u8 = 1;
pub const TIME_DIFF: u8 = 2;
pub const TIME_YEAR: u8 = 3;
pub const TIME_MONTH: u8 = 4;
pub const TIME_DAY: u8 = 5;
pub const TIME_HOUR: u8 = 6;
pub const TIME_MINUTE: u8 = 7;
pub const TIME_SECOND: u8 = 8;
pub const TIME_WEEKDAY: u8 = 9;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeOpKind {
    Now = 0,
    Monotonic = 1,
    Elapsed = 2,
    Seconds = 3,
    Milliseconds = 4,
    UnixSeconds = 5,
    UnixMilliseconds = 6,
    DurSeconds = 7,
    DurMilliseconds = 8,
    DurNanoseconds = 9,
    Sleep = 10,
    FromUnix = 11,
    FromUnixMillis = 12,
    ParseIso = 13,
    FormatIso = 14,
}

impl TimeOpKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Now),
            1 => Some(Self::Monotonic),
            2 => Some(Self::Elapsed),
            3 => Some(Self::Seconds),
            4 => Some(Self::Milliseconds),
            5 => Some(Self::UnixSeconds),
            6 => Some(Self::UnixMilliseconds),
            7 => Some(Self::DurSeconds),
            8 => Some(Self::DurMilliseconds),
            9 => Some(Self::DurNanoseconds),
            10 => Some(Self::Sleep),
            11 => Some(Self::FromUnix),
            12 => Some(Self::FromUnixMillis),
            13 => Some(Self::ParseIso),
            14 => Some(Self::FormatIso),
            _ => None,
        }
    }
}

static BASE_INSTANT: OnceLock<Instant> = OnceLock::new();

#[inline]
fn base_instant() -> Instant {
    *BASE_INSTANT.get_or_init(Instant::now)
}

pub fn make_time_error(msg: impl Into<String>) -> Obj {
    let mut rec = StructData::new(SCHEMA_TIME_ERROR, 0, 1);
    rec.inline_objs[0] = Obj::str(msg);
    Obj::Record(Arc::new(rec))
}

#[inline]
pub fn make_timestamp_parts(secs: i64, nanos: u64) -> Obj {
    let mut rec = StructData::new(SCHEMA_TIME_TIMESTAMP, 2, 0);
    rec.inline_scalars[0] = secs as u64;
    rec.inline_scalars[1] = nanos;
    Obj::Record(Arc::new(rec))
}

pub fn make_timestamp(st: SystemTime) -> Obj {
    let (secs, nanos) = match st.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let s = dur.as_secs();
            if s > i64::MAX as u64 {
                return make_time_error("Timestamp seconds overflow i64");
            }
            (s as i64, dur.subsec_nanos() as u64)
        }
        Err(before) => {
            let dur = before.duration();
            let s = dur.as_secs();
            let sub = dur.subsec_nanos();
            if sub == 0 {
                if s > (i64::MAX as u64) + 1 {
                    return make_time_error("Timestamp seconds overflow i64");
                }
                ((s as i64).wrapping_neg(), 0)
            } else {
                let Some(total) = s.checked_add(1) else {
                    return make_time_error("Timestamp seconds overflow i64");
                };
                if total > (i64::MAX as u64) + 1 {
                    return make_time_error("Timestamp seconds overflow i64");
                }
                ((total as i64).wrapping_neg(), (1_000_000_000 - sub) as u64)
            }
        }
    };
    make_timestamp_parts(secs, nanos)
}

#[inline]
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year, m as u8, d as u8)
}

#[inline]
fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_u64 = u64::from(m);
    let doy = (153 * (if m_u64 > 2 { m_u64 - 3 } else { m_u64 + 9 }) + 2) / 5 + u64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

#[inline]
fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(y) => 29,
        2 => 28,
        _ => 0,
    }
}

fn format_iso_utc(secs: i64, nanos: u64) -> Result<String, String> {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return Err(format!("Year {} is outside RFC 3339 range 0000..=9999", year));
    }
    let hour = sod / 3600;
    let minute = (sod % 3600) / 60;
    let second = sod % 60;
    if nanos == 0 {
        Ok(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, hour, minute, second
        ))
    } else {
        let frac_full = format!("{:09}", nanos);
        let frac = frac_full.trim_end_matches('0');
        Ok(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{}Z",
            year, month, day, hour, minute, second, frac
        ))
    }
}

fn parse_two_digits(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    Some((bytes[0] - b'0') * 10 + (bytes[1] - b'0'))
}

fn parse_iso_utc(raw: &str) -> Result<(i64, u64), String> {
    let s = raw.trim();
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' || !b[0..4].iter().all(|c| c.is_ascii_digit()) {
        return Err(format!("Invalid ISO 8601 timestamp '{}'", raw));
    }
    let year: i64 = std::str::from_utf8(&b[0..4])
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("Invalid year in '{}'", raw))?;
    let month = parse_two_digits(&b[5..7]).ok_or_else(|| format!("Invalid month in '{}'", raw))?;
    let day = parse_two_digits(&b[8..10]).ok_or_else(|| format!("Invalid day in '{}'", raw))?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(format!("Calendar date out of range in '{}'", raw));
    }
    let days = days_from_civil(year, month, day);
    if b.len() == 10 {
        return Ok((days * 86_400, 0));
    }
    if !matches!(b[10], b'T' | b't' | b' ') {
        return Err(format!("Expected 'T' separator in ISO 8601 timestamp '{}'", raw));
    }
    if b.len() < 19 || b[13] != b':' || b[16] != b':' {
        return Err(format!("Invalid time component in '{}'", raw));
    }
    let hour = parse_two_digits(&b[11..13]).ok_or_else(|| format!("Invalid hour in '{}'", raw))?;
    let minute = parse_two_digits(&b[14..16]).ok_or_else(|| format!("Invalid minute in '{}'", raw))?;
    let second = parse_two_digits(&b[17..19]).ok_or_else(|| format!("Invalid second in '{}'", raw))?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err(format!("Time component out of range in '{}'", raw));
    }
    let mut pos = 19usize;
    let mut nanos = 0u64;
    if pos < b.len() && b[pos] == b'.' {
        pos += 1;
        let frac_start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() {
            pos += 1;
        }
        let frac_len = pos - frac_start;
        if frac_len == 0 {
            return Err(format!("Empty fractional seconds in '{}'", raw));
        }
        let mut mul = 100_000_000u64;
        for &digit in &b[frac_start..pos.min(frac_start + 9)] {
            nanos += u64::from(digit - b'0') * mul;
            mul /= 10;
        }
    }
    if pos >= b.len() {
        return Err(format!("Missing timezone designator ('Z' or '+/-HH:MM') in '{}'", raw));
    }
    let offset_secs: i64 = match b[pos] {
        b'Z' | b'z' => {
            pos += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            pos += 1;
            if pos + 5 != b.len() || b[pos + 2] != b':' {
                return Err(format!("Invalid timezone offset in '{}'", raw));
            }
            let oh = parse_two_digits(&b[pos..pos + 2])
                .ok_or_else(|| format!("Invalid timezone offset hour in '{}'", raw))?;
            let om = parse_two_digits(&b[pos + 3..pos + 5])
                .ok_or_else(|| format!("Invalid timezone offset minute in '{}'", raw))?;
            if oh > 23 || om > 59 {
                return Err(format!("Timezone offset out of range in '{}'", raw));
            }
            pos += 5;
            let mag = i64::from(oh) * 3600 + i64::from(om) * 60;
            if sign == b'-' { -mag } else { mag }
        }
        _ => return Err(format!("Invalid timezone designator in '{}'", raw)),
    };
    if pos != b.len() {
        return Err(format!("Trailing characters in ISO 8601 timestamp '{}'", raw));
    }
    let total_secs = days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second) - offset_secs;
    Ok((total_secs, nanos))
}

pub fn make_duration(d: Duration) -> Obj {
    let mut rec = StructData::new(SCHEMA_TIME_DURATION, 2, 0);
    rec.inline_scalars[0] = d.as_secs();
    rec.inline_scalars[1] = d.subsec_nanos() as u64;
    Obj::Record(Arc::new(rec))
}

pub fn make_monotonic_now() -> Obj {
    let d = Instant::now().saturating_duration_since(base_instant());
    let mut rec = StructData::new(SCHEMA_TIME_INSTANT, 2, 0);
    rec.inline_scalars[0] = d.as_secs();
    rec.inline_scalars[1] = d.subsec_nanos() as u64;
    Obj::Record(Arc::new(rec))
}

impl Vm {
    #[cold]
    #[inline(never)]
    pub(crate) fn exec_time_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        kind: u8,
        dst_obj: u16,
        arg_reg: u16,
    ) -> Result<(), RuntimeError> {
        let Some(op) = TimeOpKind::from_u8(kind) else {
            return Err(err_msg("Invalid TimeOp kind"));
        };
        let out = match op {
            TimeOpKind::Now => make_timestamp(SystemTime::now()),
            TimeOpKind::Monotonic => make_monotonic_now(),
            TimeOpKind::Elapsed => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Expected time.Instant argument");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_INSTANT {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Expected time.Instant argument");
                    return Ok(());
                }
                let start_off = Duration::new(rec.inline_scalars[0], rec.inline_scalars[1] as u32);
                if let Some(start) = base_instant().checked_add(start_off) {
                    let now = Instant::now();
                    if let Some(dur) = now.checked_duration_since(start) {
                        make_duration(dur)
                    } else {
                        make_time_error("Monotonic clock went backwards or start is in the future")
                    }
                } else {
                    make_time_error("Monotonic instant overflow")
                }
            }
            TimeOpKind::Seconds => {
                let val = self.scalars[s_base + arg_reg as usize] as i64;
                if val < 0 {
                    make_time_error("Duration cannot be negative")
                } else {
                    make_duration(Duration::from_secs(val as u64))
                }
            }
            TimeOpKind::Milliseconds => {
                let val = self.scalars[s_base + arg_reg as usize] as i64;
                if val < 0 {
                    make_time_error("Duration cannot be negative")
                } else {
                    make_duration(Duration::from_millis(val as u64))
                }
            }
            TimeOpKind::UnixSeconds => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Invalid receiver for unix_seconds");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_TIMESTAMP {
                    make_time_error("Invalid receiver for unix_seconds")
                } else {
                    Obj::boxed_int(rec.inline_scalars[0] as i64)
                }
            }
            TimeOpKind::UnixMilliseconds => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Invalid receiver for unix_milliseconds");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_TIMESTAMP {
                    make_time_error("Invalid receiver for unix_milliseconds")
                } else {
                    let secs = rec.inline_scalars[0] as i64;
                    let sub_ms = (rec.inline_scalars[1] / 1_000_000) as i64;
                    match secs.checked_mul(1000).and_then(|v| v.checked_add(sub_ms)) {
                        Some(ms) => Obj::boxed_int(ms),
                        None => make_time_error("Timestamp milliseconds overflow i64"),
                    }
                }
            }
            TimeOpKind::DurSeconds => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Invalid receiver for seconds");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_DURATION {
                    make_time_error("Invalid receiver for seconds")
                } else {
                    let s = rec.inline_scalars[0];
                    if s > i64::MAX as u64 {
                        make_time_error("Duration seconds overflow i64")
                    } else {
                        Obj::boxed_int(s as i64)
                    }
                }
            }
            TimeOpKind::DurMilliseconds => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Invalid receiver for milliseconds");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_DURATION {
                    make_time_error("Invalid receiver for milliseconds")
                } else {
                    let ms = (rec.inline_scalars[0] as u128) * 1000
                        + (rec.inline_scalars[1] as u128) / 1_000_000;
                    if ms > i64::MAX as u128 {
                        make_time_error("Duration milliseconds overflow i64")
                    } else {
                        Obj::boxed_int(ms as i64)
                    }
                }
            }
            TimeOpKind::DurNanoseconds => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Invalid receiver for nanoseconds");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_DURATION {
                    make_time_error("Invalid receiver for nanoseconds")
                } else {
                    let ns = (rec.inline_scalars[0] as u128) * 1_000_000_000
                        + (rec.inline_scalars[1] as u128);
                    if ns > i64::MAX as u128 {
                        make_time_error("Duration nanoseconds overflow i64")
                    } else {
                        Obj::boxed_int(ns as i64)
                    }
                }
            }
            TimeOpKind::Sleep => {
                let ms = self.scalars[s_base + arg_reg as usize] as i64;
                if ms < 0 {
                    make_time_error("Sleep duration cannot be negative")
                } else {
                    thread::sleep(Duration::from_millis(ms as u64));
                    Obj::Empty
                }
            }
            TimeOpKind::FromUnix => {
                let secs = self.scalars[s_base + arg_reg as usize] as i64;
                make_timestamp_parts(secs, 0)
            }
            TimeOpKind::FromUnixMillis => {
                let ms = self.scalars[s_base + arg_reg as usize] as i64;
                let secs = ms.div_euclid(1000);
                let sub_ms = ms.rem_euclid(1000) as u64;
                make_timestamp_parts(secs, sub_ms * 1_000_000)
            }
            TimeOpKind::ParseIso => {
                let Obj::Str(s) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Expected String in time.parse_iso()");
                    return Ok(());
                };
                match parse_iso_utc(s.as_str()) {
                    Ok((secs, nanos)) => make_timestamp_parts(secs, nanos),
                    Err(msg) => make_time_error(msg),
                }
            }
            TimeOpKind::FormatIso => {
                let Obj::Record(rec) = &self.objs[o_base + arg_reg as usize] else {
                    self.objs[o_base + dst_obj as usize] =
                        make_time_error("Expected time.Timestamp in format_iso");
                    return Ok(());
                };
                if rec.schema_id != SCHEMA_TIME_TIMESTAMP {
                    make_time_error("Expected time.Timestamp in format_iso")
                } else {
                    match format_iso_utc(rec.inline_scalars[0] as i64, rec.inline_scalars[1]) {
                        Ok(iso) => Obj::str(iso),
                        Err(msg) => make_time_error(msg),
                    }
                }
            }
        };
        self.objs[o_base + dst_obj as usize] = out;
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn exec_time_ext_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        let Obj::Record(ts_rec) = &self.objs[o_base + b as usize] else {
            return Err(err_msg("Expected time.Timestamp receiver"));
        };
        if ts_rec.schema_id != SCHEMA_TIME_TIMESTAMP {
            return Err(err_msg("Expected time.Timestamp receiver"));
        }
        let secs = ts_rec.inline_scalars[0] as i64;
        let nanos = ts_rec.inline_scalars[1];

        match aux {
            TIME_ADD | TIME_SUB => {
                let Obj::Record(dur_rec) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected time.Duration argument"));
                };
                if dur_rec.schema_id != SCHEMA_TIME_DURATION {
                    return Err(err_msg("Expected time.Duration argument"));
                }
                let d_secs = dur_rec.inline_scalars[0];
                let d_nanos = dur_rec.inline_scalars[1];
                let total: i128 = if aux == TIME_ADD {
                    let ns = nanos + d_nanos;
                    let carry = (ns / 1_000_000_000) as i128;
                    let rem_ns = ns % 1_000_000_000;
                    let new_secs = (secs as i128) + (d_secs as i128) + carry;
                    if new_secs > i64::MAX as i128 || new_secs < i64::MIN as i128 {
                        self.objs[o_base + a as usize] = make_time_error("Timestamp overflow in .add()");
                        return Ok(());
                    }
                    self.objs[o_base + a as usize] = make_timestamp_parts(new_secs as i64, rem_ns);
                    return Ok(());
                } else {
                    let diff_ns = (nanos as i64) - (d_nanos as i64);
                    let (borrow, rem_ns) = if diff_ns < 0 {
                        (1i128, (diff_ns + 1_000_000_000) as u64)
                    } else {
                        (0i128, diff_ns as u64)
                    };
                    let new_secs = (secs as i128) - (d_secs as i128) - borrow;
                    if new_secs > i64::MAX as i128 || new_secs < i64::MIN as i128 {
                        self.objs[o_base + a as usize] = make_time_error("Timestamp underflow in .sub()");
                        return Ok(());
                    }
                    self.objs[o_base + a as usize] = make_timestamp_parts(new_secs as i64, rem_ns);
                    return Ok(());
                };
                #[allow(unreachable_code)]
                {
                    let _ = total;
                }
            }
            TIME_DIFF => {
                let Obj::Record(other_rec) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected time.Timestamp argument"));
                };
                if other_rec.schema_id != SCHEMA_TIME_TIMESTAMP {
                    return Err(err_msg("Expected time.Timestamp argument"));
                }
                let o_secs = other_rec.inline_scalars[0] as i64;
                let o_nanos = other_rec.inline_scalars[1];
                if (secs, nanos) < (o_secs, o_nanos) {
                    self.objs[o_base + a as usize] =
                        make_time_error("Cannot compute negative duration between timestamps");
                    return Ok(());
                }
                let diff_ns = (nanos as i64) - (o_nanos as i64);
                let (borrow, rem_ns) = if diff_ns < 0 {
                    (1i128, (diff_ns + 1_000_000_000) as u32)
                } else {
                    (0i128, diff_ns as u32)
                };
                let diff_secs = (secs as i128) - (o_secs as i128) - borrow;
                if diff_secs > u64::MAX as i128 {
                    self.objs[o_base + a as usize] = make_time_error("Duration overflow in .diff()");
                    return Ok(());
                }
                self.objs[o_base + a as usize] =
                    make_duration(Duration::new(diff_secs as u64, rem_ns));
            }
            TIME_YEAR..=TIME_WEEKDAY => {
                let days = secs.div_euclid(86_400);
                let sod = secs.rem_euclid(86_400);
                let (year, month, day) = civil_from_days(days);
                let val = match aux {
                    TIME_YEAR => year,
                    TIME_MONTH => i64::from(month),
                    TIME_DAY => i64::from(day),
                    TIME_HOUR => sod / 3600,
                    TIME_MINUTE => (sod % 3600) / 60,
                    TIME_SECOND => sod % 60,
                    _ => (days + 3).rem_euclid(7) + 1,
                };
                self.scalars[s_base + a as usize] = val as u64;
            }
            _ => return Err(err_msg("Invalid EXT_TIME_OP sub-op")),
        }
        Ok(())
    }
}
