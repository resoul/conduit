use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    Obj, SCHEMA_TIME_DURATION, SCHEMA_TIME_ERROR, SCHEMA_TIME_INSTANT, SCHEMA_TIME_TIMESTAMP,
    StructData,
};
use crate::vm::{RuntimeError, Vm};

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
    let mut rec = StructData::new(SCHEMA_TIME_TIMESTAMP, 2, 0);
    rec.inline_scalars[0] = secs as u64;
    rec.inline_scalars[1] = nanos;
    Obj::Record(Arc::new(rec))
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
        };
        self.objs[o_base + dst_obj as usize] = out;
        Ok(())
    }
}
