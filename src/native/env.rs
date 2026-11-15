use std::sync::Arc;

use crate::vm::value::{Obj, SCHEMA_ENV_ERROR, StructData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvFieldKind {
    String,
    Int,
    Float,
    Bool,
}

#[derive(Debug, Clone)]
pub enum EnvDefault {
    Int(i64),
    Float(u64),
    Bool(bool),
    Str(String),
    Nil,
}

#[derive(Debug, Clone)]
pub struct EnvFieldPlan {
    pub env_name: String,
    pub kind: EnvFieldKind,
    pub is_optional: bool,
    pub is_scalar: bool,
    pub slot_idx: u16,
    pub default: Option<EnvDefault>,
}

#[derive(Debug, Clone)]
pub struct EnvLoadPlan {
    pub schema_id: u32,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub fields: Box<[EnvFieldPlan]>,
}

pub fn make_env_error(name: impl Into<String>, message: impl Into<String>) -> Obj {
    let mut rec = StructData::new(SCHEMA_ENV_ERROR, 0, 2);
    rec.set_obj(0, Obj::str(name));
    rec.set_obj(1, Obj::str(message));
    Obj::Record(Arc::new(rec))
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn exec_env_load(plan: &EnvLoadPlan) -> Obj {
    let mut rec = StructData::new(plan.schema_id, plan.scalar_len, plan.obj_len);

    for f in plan.fields.iter() {
        let slot = f.slot_idx as usize;
        match std::env::var(&f.env_name) {
            Ok(raw) if !raw.is_empty() => {
                if f.is_optional {
                    let boxed = match f.kind {
                        EnvFieldKind::String => Obj::str(raw),
                        EnvFieldKind::Int => match raw.trim().parse::<i64>() {
                            Ok(v) => Obj::boxed_int(v),
                            Err(_) => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Int in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                        EnvFieldKind::Float => match raw.trim().parse::<f64>() {
                            Ok(v) if v.is_finite() => Obj::boxed_float(v.to_bits()),
                            _ => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Float in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                        EnvFieldKind::Bool => match parse_env_bool(&raw) {
                            Some(b) => Obj::boxed_bool(b),
                            None => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Bool in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                    };
                    rec.set_obj(slot, boxed);
                } else if f.is_scalar {
                    let scalar_val = match f.kind {
                        EnvFieldKind::Int => match raw.trim().parse::<i64>() {
                            Ok(v) => v as u64,
                            Err(_) => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Int in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                        EnvFieldKind::Float => match raw.trim().parse::<f64>() {
                            Ok(v) if v.is_finite() => v.to_bits(),
                            _ => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Float in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                        EnvFieldKind::Bool => match parse_env_bool(&raw) {
                            Some(b) => u64::from(b),
                            None => {
                                return make_env_error(
                                    &f.env_name,
                                    format!("Invalid Bool in '{}': '{}'", f.env_name, raw),
                                );
                            }
                        },
                        EnvFieldKind::String => unreachable!(),
                    };
                    rec.set_scalar(slot, scalar_val);
                } else {
                    rec.set_obj(slot, Obj::str(raw));
                }
            }
            _ => {
                if let Some(def) = &f.default {
                    match def {
                        EnvDefault::Int(v) => {
                            if f.is_optional {
                                rec.set_obj(slot, Obj::boxed_int(*v));
                            } else {
                                rec.set_scalar(slot, *v as u64);
                            }
                        }
                        EnvDefault::Float(bits) => {
                            if f.is_optional {
                                rec.set_obj(slot, Obj::boxed_float(*bits));
                            } else {
                                rec.set_scalar(slot, *bits);
                            }
                        }
                        EnvDefault::Bool(b) => {
                            if f.is_optional {
                                rec.set_obj(slot, Obj::boxed_bool(*b));
                            } else {
                                rec.set_scalar(slot, u64::from(*b));
                            }
                        }
                        EnvDefault::Str(s) => {
                            rec.set_obj(slot, Obj::str(s.clone()));
                        }
                        EnvDefault::Nil => {
                            rec.set_obj(slot, Obj::Empty);
                        }
                    }
                } else if f.is_optional {
                    rec.set_obj(slot, Obj::Empty);
                } else {
                    return make_env_error(
                        &f.env_name,
                        format!("Missing required environment variable '{}'", f.env_name),
                    );
                }
            }
        }
    }

    Obj::Record(Arc::new(rec))
}
