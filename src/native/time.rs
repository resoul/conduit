use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant as StdInstant, SystemTime, UNIX_EPOCH};

use super::{NativeFuncDef, NativeMethodDef, NativeModule, NativeTypeDef, NativeValue};
use crate::types::{Type, TypeId};
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp(pub SystemTime);

impl NativeValue for Timestamp {
    fn type_id(&self) -> TypeId {
        TypeId::native("cdt.time", "Timestamp")
    }

    fn type_name(&self) -> &'static str {
        "time.Timestamp"
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "time.Timestamp({:?})", self.0)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn dyn_eq(&self, other: &dyn NativeValue) -> bool {
        if let Some(other_ts) = other.as_any().downcast_ref::<Timestamp>() {
            self.0 == other_ts.0
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instant(pub StdInstant);

impl NativeValue for Instant {
    fn type_id(&self) -> TypeId {
        TypeId::native("cdt.time", "Instant")
    }

    fn type_name(&self) -> &'static str {
        "time.Instant"
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "time.Instant({:?})", self.0)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn dyn_eq(&self, other: &dyn NativeValue) -> bool {
        if let Some(other_inst) = other.as_any().downcast_ref::<Instant>() {
            self.0 == other_inst.0
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(pub StdDuration);

impl NativeValue for Duration {
    fn type_id(&self) -> TypeId {
        TypeId::native("cdt.time", "Duration")
    }

    fn type_name(&self) -> &'static str {
        "time.Duration"
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "time.Duration({:?})", self.0)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn dyn_eq(&self, other: &dyn NativeValue) -> bool {
        if let Some(other_dur) = other.as_any().downcast_ref::<Duration>() {
            self.0 == other_dur.0
        } else {
            false
        }
    }
}

pub fn make_time_error(message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.time", "TimeError"),
        fields,
    }
}

pub fn create_time_module() -> NativeModule {
    let mut module = NativeModule::new("cdt.time", "time", "Standard library for system and monotonic time operations");

    let timestamp_ty = Type::Custom(TypeId::native("cdt.time", "Timestamp"));
    let instant_ty = Type::Custom(TypeId::native("cdt.time", "Instant"));
    let duration_ty = Type::Custom(TypeId::native("cdt.time", "Duration"));
    let time_error_ty = Type::Custom(TypeId::native("cdt.time", "TimeError"));

    module.add_type(NativeTypeDef {
        name: "Timestamp",
        qualified_name: "time.Timestamp",
        doc: "Moment of system time measured from the system clock",
        is_error: false,
        fields: vec![],
    });

    module.add_type(NativeTypeDef {
        name: "Instant",
        qualified_name: "time.Instant",
        doc: "Monotonically increasing clock measurement point for benchmarks and elapsed timing",
        is_error: false,
        fields: vec![],
    });

    module.add_type(NativeTypeDef {
        name: "Duration",
        qualified_name: "time.Duration",
        doc: "Non-negative time span representation",
        is_error: false,
        fields: vec![],
    });

    module.add_type(NativeTypeDef {
        name: "TimeError",
        qualified_name: "time.TimeError",
        doc: "Error occurring during time operations (overflow, negative duration, invalid order)",
        is_error: true,
        fields: vec![("message", Type::String)],
    });

    module.add_function(NativeFuncDef {
        name: "now",
        doc: "Reads the current system clock time",
        params: vec![],
        return_type: Type::union(vec![timestamp_ty.clone(), time_error_ty.clone()]),
        is_validator: false,
        implementation: |_args| {
            Ok(Value::Native(Arc::new(Timestamp(SystemTime::now()))))
        },
    });

    module.add_function(NativeFuncDef {
        name: "monotonic",
        doc: "Reads the current monotonic clock instant",
        params: vec![],
        return_type: instant_ty.clone(),
        is_validator: false,
        implementation: |_args| {
            Ok(Value::Native(Arc::new(Instant(StdInstant::now()))))
        },
    });

    module.add_function(NativeFuncDef {
        name: "elapsed",
        doc: "Calculates the duration elapsed since the given monotonic instant",
        params: vec![("start", instant_ty.clone())],
        return_type: Type::union(vec![duration_ty.clone(), time_error_ty.clone()]),
        is_validator: false,
        implementation: |args| {
            let start_val = args.into_iter().next().unwrap();
            if let Value::Native(ref nat) = start_val {
                if let Some(inst) = nat.as_any().downcast_ref::<Instant>() {
                    let now = StdInstant::now();
                    if now < inst.0 {
                        return Ok(make_time_error("Monotonic clock went backwards or start is in the future"));
                    }
                    let dur = now.duration_since(inst.0);
                    return Ok(Value::Native(Arc::new(Duration(dur))));
                }
            }
            Ok(make_time_error("Expected time.Instant argument"))
        },
    });

    module.add_function(NativeFuncDef {
        name: "seconds",
        doc: "Creates a duration from the given number of seconds",
        params: vec![("value", Type::Int)],
        return_type: Type::union(vec![duration_ty.clone(), time_error_ty.clone()]),
        is_validator: false,
        implementation: |args| {
            let val = match args.into_iter().next().unwrap() {
                Value::Int(n) => n,
                _ => return Ok(make_time_error("Expected Int")),
            };
            if val < 0 {
                return Ok(make_time_error("Duration cannot be negative"));
            }
            Ok(Value::Native(Arc::new(Duration(StdDuration::from_secs(val as u64)))))
        },
    });

    module.add_function(NativeFuncDef {
        name: "milliseconds",
        doc: "Creates a duration from the given number of milliseconds",
        params: vec![("value", Type::Int)],
        return_type: Type::union(vec![duration_ty.clone(), time_error_ty.clone()]),
        is_validator: false,
        implementation: |args| {
            let val = match args.into_iter().next().unwrap() {
                Value::Int(n) => n,
                _ => return Ok(make_time_error("Expected Int")),
            };
            if val < 0 {
                return Ok(make_time_error("Duration cannot be negative"));
            }
            Ok(Value::Native(Arc::new(Duration(StdDuration::from_millis(val as u64)))))
        },
    });

    module.add_method(
        "Timestamp",
        NativeMethodDef {
            name: "unix_seconds",
            doc: "Returns Unix timestamp in seconds (UTC). Rounded down for values before epoch.",
            params: vec![],
            return_type: Type::union(vec![Type::Int, time_error_ty.clone()]),
            implementation: |receiver, _args| {
                if let Some(ts) = receiver.as_any().downcast_ref::<Timestamp>() {
                    match ts.0.duration_since(UNIX_EPOCH) {
                        Ok(dur) => {
                            let secs = dur.as_secs();
                            if secs > i64::MAX as u64 {
                                return Ok(make_time_error("Timestamp seconds overflow i64"));
                            }
                            Ok(Value::Int(secs as i64))
                        }
                        Err(before) => {
                            let dur = before.duration();
                            let secs = dur.as_secs();
                            let extra = if dur.subsec_nanos() > 0 { 1 } else { 0 };
                            let total = secs.checked_add(extra).ok_or_else(|| {
                                make_time_error("Timestamp seconds overflow i64")
                            });
                            match total {
                                Ok(t) => {
                                    if t > (i64::MAX as u64) + 1 {
                                        return Ok(make_time_error("Timestamp seconds overflow i64"));
                                    }
                                    let res = -(t as i64);
                                    Ok(Value::Int(res))
                                }
                                Err(err) => Ok(err),
                            }
                        }
                    }
                } else {
                    Ok(make_time_error("Invalid receiver for unix_seconds"))
                }
            },
        },
    );

    module.add_method(
        "Timestamp",
        NativeMethodDef {
            name: "unix_milliseconds",
            doc: "Returns Unix timestamp in milliseconds (UTC). Rounded down for values before epoch.",
            params: vec![],
            return_type: Type::union(vec![Type::Int, time_error_ty.clone()]),
            implementation: |receiver, _args| {
                if let Some(ts) = receiver.as_any().downcast_ref::<Timestamp>() {
                    match ts.0.duration_since(UNIX_EPOCH) {
                        Ok(dur) => {
                            let ms = dur.as_millis();
                            if ms > i64::MAX as u128 {
                                return Ok(make_time_error("Timestamp milliseconds overflow i64"));
                            }
                            Ok(Value::Int(ms as i64))
                        }
                        Err(before) => {
                            let dur = before.duration();
                            let ms = dur.as_millis();
                            let extra = if (dur.subsec_nanos() % 1_000_000) > 0 { 1 } else { 0 };
                            let total = ms.checked_add(extra).ok_or_else(|| {
                                make_time_error("Timestamp milliseconds overflow i64")
                            });
                            match total {
                                Ok(t) => {
                                    if t > (i64::MAX as u128) + 1 {
                                        return Ok(make_time_error("Timestamp milliseconds overflow i64"));
                                    }
                                    let res = -(t as i64);
                                    Ok(Value::Int(res))
                                }
                                Err(err) => Ok(err),
                            }
                        }
                    }
                } else {
                    Ok(make_time_error("Invalid receiver for unix_milliseconds"))
                }
            },
        },
    );

    module.add_method(
        "Duration",
        NativeMethodDef {
            name: "seconds",
            doc: "Returns the number of full seconds in the duration",
            params: vec![],
            return_type: Type::union(vec![Type::Int, time_error_ty.clone()]),
            implementation: |receiver, _args| {
                if let Some(dur) = receiver.as_any().downcast_ref::<Duration>() {
                    let s = dur.0.as_secs();
                    if s > i64::MAX as u64 {
                        return Ok(make_time_error("Duration seconds overflow i64"));
                    }
                    Ok(Value::Int(s as i64))
                } else {
                    Ok(make_time_error("Invalid receiver for seconds"))
                }
            },
        },
    );

    module.add_method(
        "Duration",
        NativeMethodDef {
            name: "milliseconds",
            doc: "Returns the number of full milliseconds in the duration",
            params: vec![],
            return_type: Type::union(vec![Type::Int, time_error_ty.clone()]),
            implementation: |receiver, _args| {
                if let Some(dur) = receiver.as_any().downcast_ref::<Duration>() {
                    let ms = dur.0.as_millis();
                    if ms > i64::MAX as u128 {
                        return Ok(make_time_error("Duration milliseconds overflow i64"));
                    }
                    Ok(Value::Int(ms as i64))
                } else {
                    Ok(make_time_error("Invalid receiver for milliseconds"))
                }
            },
        },
    );

    module.add_method(
        "Duration",
        NativeMethodDef {
            name: "nanoseconds",
            doc: "Returns the number of full nanoseconds in the duration",
            params: vec![],
            return_type: Type::union(vec![Type::Int, time_error_ty.clone()]),
            implementation: |receiver, _args| {
                if let Some(dur) = receiver.as_any().downcast_ref::<Duration>() {
                    let ns = dur.0.as_nanos();
                    if ns > i64::MAX as u128 {
                        return Ok(make_time_error("Duration nanoseconds overflow i64"));
                    }
                    Ok(Value::Int(ns as i64))
                } else {
                    Ok(make_time_error("Invalid receiver for nanoseconds"))
                }
            },
        },
    );

    module
}

