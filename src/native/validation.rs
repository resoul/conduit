use std::sync::Arc;

use crate::ir::Module;
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    ListStorage, Obj, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT, SCHEMA_EMPTY_TAG,
    SCHEMA_LIST_TAG, SCHEMA_MAP_TAG, SCHEMA_STR_TAG, SCHEMA_VALIDATION_ERROR,
    SCHEMA_VALIDATION_ISSUE, SCHEMA_VALIDATION_VIOLATION, StructData, is_builtin_error_schema,
};
use crate::vm::{ExecResult, RuntimeError, Vm};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValOpKind {
    Email = 0,
    MinInt = 1,
    MaxInt = 2,
    MinFloat = 3,
    MaxFloat = 4,
    MinObj = 5,
    MaxObj = 6,
    MinDyn = 7,
    MaxDyn = 8,
}

impl ValOpKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Email),
            1 => Some(Self::MinInt),
            2 => Some(Self::MaxInt),
            3 => Some(Self::MinFloat),
            4 => Some(Self::MaxFloat),
            5 => Some(Self::MinObj),
            6 => Some(Self::MaxObj),
            7 => Some(Self::MinDyn),
            8 => Some(Self::MaxDyn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompiledValidationRule {
    MinInt(i64),
    MaxInt(i64),
    MinFloat(f64),
    MaxFloat(f64),
    MinObj(i64),
    MaxObj(i64),
    Email,
    UserFunc {
        func_name: String,
        func_id: u32,
        first_param_is_scalar: bool,
        first_param_kind: u8,
        extra_scalars: Vec<u64>,
        extra_objs: Vec<Obj>,
    },
}

#[derive(Debug, Clone)]
pub enum NestedValidation {
    None,
    Struct(u32),
    List(Box<NestedValidation>),
}

#[derive(Debug, Clone)]
pub struct ValidationFieldPlan {
    pub name: String,
    pub is_scalar: bool,
    pub scalar_kind: u8,
    pub is_optional: bool,
    pub slot_idx: u16,
    pub rules: Vec<CompiledValidationRule>,
    pub nested: NestedValidation,
}

#[derive(Debug, Clone)]
pub struct ValidationStructPlan {
    pub schema_id: u32,
    pub fields: Vec<ValidationFieldPlan>,
}

pub fn make_issue(code: impl Into<String>, message: impl Into<String>) -> Obj {
    let mut rec = StructData::new(SCHEMA_VALIDATION_ISSUE, 0, 2);
    rec.inline_objs[0] = Obj::str(code);
    rec.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(rec))
}

pub fn make_violation(path: impl Into<String>, code: Obj, message: Obj) -> Obj {
    let mut rec = StructData::new(SCHEMA_VALIDATION_VIOLATION, 0, 3);
    rec.inline_objs[0] = Obj::str(path);
    rec.inline_objs[1] = code;
    rec.set_obj(2, message);
    Obj::Record(Arc::new(rec))
}

pub fn make_validation_error(violations: Vec<Obj>) -> Obj {
    let mut rec = StructData::new(SCHEMA_VALIDATION_ERROR, 0, 1);
    rec.inline_objs[0] = Obj::List(ListStorage::Obj(Arc::new(violations)));
    Obj::Record(Arc::new(rec))
}

pub fn validate_email_str(s: &str) -> bool {
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    if domain.contains('@') || local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    true
}

fn check_int_bound(val: i64, bound: i64, is_min: bool) -> Obj {
    if is_min && val < bound {
        make_issue(
            "min",
            format!("Value {} is less than minimum {}", val, bound),
        )
    } else if !is_min && val > bound {
        make_issue("max", format!("Value {} exceeds maximum {}", val, bound))
    } else {
        Obj::Empty
    }
}

fn check_float_bound(val: f64, bound: f64, is_min: bool) -> Obj {
    let (code, bound_word, phrase) = if is_min {
        ("min", "Minimum", "is less than minimum")
    } else {
        ("max", "Maximum", "exceeds maximum")
    };
    if !val.is_finite() || !bound.is_finite() {
        return make_issue(
            format!("{}.non_finite", code),
            format!("{} validator requires finite Float value and bound", bound_word),
        );
    }
    if (is_min && val < bound) || (!is_min && val > bound) {
        make_issue(code, format!("Value {} {} {}", val, phrase, bound))
    } else {
        Obj::Empty
    }
}

fn check_obj_len_bound(obj: &Obj, bound: i64, is_min: bool) -> Obj {
    let (code, bound_word, phrase) = if is_min {
        ("min", "Minimum", "is less than minimum")
    } else {
        ("max", "Maximum", "exceeds maximum")
    };
    let (label, count) = match obj {
        Obj::Str(s) => ("String", s.chars().count()),
        Obj::List(l) => ("List", l.len()),
        Obj::Map(m) => ("Map", m.len()),
        _ => {
            return make_issue(
                format!("{}.type", code),
                format!("Unsupported type for {} validator", code),
            );
        }
    };
    if bound < 0 {
        return make_issue(
            format!("{}.invalid_bound", code),
            format!("{} length bound cannot be negative", bound_word),
        );
    }
    let n = count as i64;
    if (is_min && n < bound) || (!is_min && n > bound) {
        make_issue(code, format!("{} length {} {} {}", label, n, phrase, bound))
    } else {
        Obj::Empty
    }
}

fn check_email_obj(obj: &Obj) -> Obj {
    match obj {
        Obj::Str(s) => {
            if !validate_email_str(s) {
                make_issue("email", "Invalid email address format")
            } else {
                Obj::Empty
            }
        }
        _ => make_issue("email.type", "Expected String argument for email validator"),
    }
}

fn check_dyn_bound(val: &Obj, bound: &Obj, is_min: bool) -> Obj {
    let code = if is_min { "min" } else { "max" };
    let bound_int = match bound {
        Obj::Record(r) if r.schema_id == SCHEMA_BOXED_INT => Some(r.inline_scalars[0] as i64),
        _ => None,
    };
    let bound_float = match bound {
        Obj::Record(r) if r.schema_id == SCHEMA_BOXED_FLOAT => {
            Some(f64::from_bits(r.inline_scalars[0]))
        }
        _ => None,
    };
    match val {
        Obj::Record(r) if r.schema_id == SCHEMA_BOXED_INT => match bound_int {
            Some(b) => check_int_bound(r.inline_scalars[0] as i64, b, is_min),
            None => make_issue(format!("{}.args", code), "Bound must be an Int"),
        },
        Obj::Record(r) if r.schema_id == SCHEMA_BOXED_FLOAT => match bound_float {
            Some(b) => check_float_bound(f64::from_bits(r.inline_scalars[0]), b, is_min),
            None => make_issue(format!("{}.args", code), "Bound for Float must be a Float"),
        },
        Obj::Str(_) | Obj::List(_) | Obj::Map(_) => match bound_int {
            Some(b) => check_obj_len_bound(val, b, is_min),
            None => make_issue(format!("{}.args", code), "Bound must be an Int"),
        },
        _ => make_issue(format!("{}.type", code), "Unsupported type for validator"),
    }
}

impl Vm {
    #[cold]
    #[inline(never)]
    pub(crate) fn exec_validation_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        kind: u8,
        dst_obj: u16,
        a_reg: u16,
        b_reg: u16,
    ) -> Result<(), RuntimeError> {
        let Some(op) = ValOpKind::from_u8(kind) else {
            return Err(err_msg("Invalid ValidationOp kind"));
        };
        let out = match op {
            ValOpKind::Email => check_email_obj(&self.objs[o_base + a_reg as usize]),
            ValOpKind::MinInt => check_int_bound(
                self.scalars[s_base + a_reg as usize] as i64,
                self.scalars[s_base + b_reg as usize] as i64,
                true,
            ),
            ValOpKind::MaxInt => check_int_bound(
                self.scalars[s_base + a_reg as usize] as i64,
                self.scalars[s_base + b_reg as usize] as i64,
                false,
            ),
            ValOpKind::MinFloat => check_float_bound(
                f64::from_bits(self.scalars[s_base + a_reg as usize]),
                f64::from_bits(self.scalars[s_base + b_reg as usize]),
                true,
            ),
            ValOpKind::MaxFloat => check_float_bound(
                f64::from_bits(self.scalars[s_base + a_reg as usize]),
                f64::from_bits(self.scalars[s_base + b_reg as usize]),
                false,
            ),
            ValOpKind::MinObj => check_obj_len_bound(
                &self.objs[o_base + a_reg as usize],
                self.scalars[s_base + b_reg as usize] as i64,
                true,
            ),
            ValOpKind::MaxObj => check_obj_len_bound(
                &self.objs[o_base + a_reg as usize],
                self.scalars[s_base + b_reg as usize] as i64,
                false,
            ),
            ValOpKind::MinDyn => check_dyn_bound(
                &self.objs[o_base + a_reg as usize],
                &self.objs[o_base + b_reg as usize],
                true,
            ),
            ValOpKind::MaxDyn => check_dyn_bound(
                &self.objs[o_base + a_reg as usize],
                &self.objs[o_base + b_reg as usize],
                false,
            ),
        };
        self.objs[o_base + dst_obj as usize] = out;
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn exec_validation_check(
        &mut self,
        module: &Module,
        scratch_s_base: usize,
        scratch_o_base: usize,
        o_base: usize,
        dst_obj: u16,
        src_obj: u16,
    ) -> Result<(), RuntimeError> {
        let target = self.objs[o_base + src_obj as usize].clone();
        let mut violations = Vec::new();
        self.validate_dyn_val(
            module,
            scratch_s_base,
            scratch_o_base,
            &target,
            "",
            &mut violations,
            0,
        )?;
        self.objs[o_base + dst_obj as usize] = if violations.is_empty() {
            target
        } else {
            make_validation_error(violations)
        };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_dyn_val(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        val: &Obj,
        prefix: &str,
        violations: &mut Vec<Obj>,
        depth: usize,
    ) -> Result<(), RuntimeError> {
        if depth > 256 {
            return Ok(());
        }
        match val {
            Obj::Record(rec) => {
                self.validate_struct_val(
                    module,
                    scratch_s,
                    scratch_o,
                    val,
                    rec.schema_id,
                    prefix,
                    violations,
                    depth,
                )
            }
            Obj::List(ListStorage::Obj(items)) => {
                for (i, item) in items.iter().enumerate() {
                    if !matches!(item, Obj::Empty) {
                        let item_path = format!("{}[{}]", prefix, i);
                        self.validate_dyn_val(
                            module,
                            scratch_s,
                            scratch_o,
                            item,
                            &item_path,
                            violations,
                            depth + 1,
                        )?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_struct_val(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        val: &Obj,
        schema_id: u32,
        prefix: &str,
        violations: &mut Vec<Obj>,
        depth: usize,
    ) -> Result<(), RuntimeError> {
        if depth > 256 {
            return Ok(());
        }
        let Obj::Record(rec) = val else {
            return Ok(());
        };
        let Some(plan) = module
            .validation_plans
            .get(schema_id as usize)
            .filter(|p| p.schema_id == schema_id)
            .or_else(|| module.validation_plans.iter().find(|p| p.schema_id == schema_id))
        else {
            return Ok(());
        };
        for f in &plan.fields {
            let field_path = if prefix.is_empty() {
                f.name.clone()
            } else {
                format!("{}.{}", prefix, f.name)
            };
            if f.is_scalar {
                let s_val = rec.get_scalar(f.slot_idx as usize);
                for rule in &f.rules {
                    let res = self.eval_scalar_rule(
                        module,
                        scratch_s,
                        scratch_o,
                        rule,
                        s_val,
                        f.scalar_kind,
                    )?;
                    self.collect_issue(&field_path, res, violations)?;
                }
            } else {
                let f_obj = rec.get_obj(f.slot_idx as usize);
                if matches!(f_obj, Obj::Empty) {
                    continue;
                }
                self.validate_nested(
                    module,
                    scratch_s,
                    scratch_o,
                    f_obj,
                    &f.nested,
                    &field_path,
                    violations,
                    depth + 1,
                )?;
                let unboxed_opt_scalar = match f_obj {
                    Obj::Record(b)
                        if b.schema_id == SCHEMA_BOXED_INT
                            || b.schema_id == SCHEMA_BOXED_FLOAT
                            || b.schema_id == SCHEMA_BOXED_BOOL =>
                    {
                        Some((b.inline_scalars[0], f.scalar_kind))
                    }
                    _ => None,
                };
                for rule in &f.rules {
                    let res = if let Some((s_val, skind)) = unboxed_opt_scalar {
                        self.eval_scalar_rule(module, scratch_s, scratch_o, rule, s_val, skind)?
                    } else {
                        self.eval_obj_rule(module, scratch_s, scratch_o, rule, f_obj)?
                    };
                    self.collect_issue(&field_path, res, violations)?;
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_nested(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        val: &Obj,
        nested: &NestedValidation,
        path: &str,
        violations: &mut Vec<Obj>,
        depth: usize,
    ) -> Result<(), RuntimeError> {
        match nested {
            NestedValidation::None => Ok(()),
            NestedValidation::Struct(sid) => {
                self.validate_struct_val(module, scratch_s, scratch_o, val, *sid, path, violations, depth)
            }
            NestedValidation::List(inner) => {
                if let Obj::List(ListStorage::Obj(items)) = val {
                    for (i, item) in items.iter().enumerate() {
                        if !matches!(item, Obj::Empty) {
                            let item_path = format!("{}[{}]", path, i);
                            self.validate_nested(
                                module, scratch_s, scratch_o, item, inner, &item_path, violations, depth + 1,
                            )?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn eval_scalar_rule(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        rule: &CompiledValidationRule,
        s_val: u64,
        scalar_kind: u8,
    ) -> Result<Obj, RuntimeError> {
        match rule {
            CompiledValidationRule::MinInt(b) => Ok(check_int_bound(s_val as i64, *b, true)),
            CompiledValidationRule::MaxInt(b) => Ok(check_int_bound(s_val as i64, *b, false)),
            CompiledValidationRule::MinFloat(b) => {
                Ok(check_float_bound(f64::from_bits(s_val), *b, true))
            }
            CompiledValidationRule::MaxFloat(b) => {
                Ok(check_float_bound(f64::from_bits(s_val), *b, false))
            }
            CompiledValidationRule::UserFunc {
                func_id,
                first_param_is_scalar,
                extra_scalars,
                extra_objs,
                ..
            } => {
                if *first_param_is_scalar {
                    let mut scalars = Vec::with_capacity(1 + extra_scalars.len());
                    scalars.push(s_val);
                    scalars.extend_from_slice(extra_scalars);
                    self.call_validator_fn(module, scratch_s, scratch_o, *func_id, &scalars, extra_objs)
                } else {
                    let boxed = box_scalar_val(s_val, scalar_kind);
                    let mut objs = Vec::with_capacity(1 + extra_objs.len());
                    objs.push(boxed);
                    objs.extend_from_slice(extra_objs);
                    self.call_validator_fn(module, scratch_s, scratch_o, *func_id, extra_scalars, &objs)
                }
            }
            _ => Ok(make_issue("validator.type", "Unsupported rule for scalar field")),
        }
    }

    fn eval_obj_rule(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        rule: &CompiledValidationRule,
        obj_val: &Obj,
    ) -> Result<Obj, RuntimeError> {
        match rule {
            CompiledValidationRule::Email => Ok(check_email_obj(obj_val)),
            CompiledValidationRule::MinObj(b) => Ok(check_obj_len_bound(obj_val, *b, true)),
            CompiledValidationRule::MaxObj(b) => Ok(check_obj_len_bound(obj_val, *b, false)),
            CompiledValidationRule::UserFunc {
                func_id,
                extra_scalars,
                extra_objs,
                ..
            } => {
                let mut objs = Vec::with_capacity(1 + extra_objs.len());
                objs.push(obj_val.clone());
                objs.extend_from_slice(extra_objs);
                self.call_validator_fn(module, scratch_s, scratch_o, *func_id, extra_scalars, &objs)
            }
            _ => Ok(make_issue("validator.type", "Unsupported rule for object field")),
        }
    }

    fn call_validator_fn(
        &mut self,
        module: &Module,
        scratch_s: usize,
        scratch_o: usize,
        func_id: u32,
        scalars: &[u64],
        objs: &[Obj],
    ) -> Result<Obj, RuntimeError> {
        let func = &module.functions[func_id as usize];
        self.ensure_capacity(
            scratch_s + func.num_scalars as usize,
            scratch_o + func.num_objs as usize,
        );
        if !scalars.is_empty() {
            self.scalars[scratch_s..scratch_s + scalars.len()].copy_from_slice(scalars);
        }
        for (i, obj) in objs.iter().enumerate() {
            self.objs[scratch_o + i] = obj.clone();
        }
        match self.exec_from(module, func_id, scratch_s, scratch_o, usize::MAX)? {
            ExecResult::Obj(v) => Ok(v),
            ExecResult::Void => Ok(Obj::Empty),
            ExecResult::Scalar(_) => Err(err_msg("Validator returned unexpected scalar")),
        }
    }

    fn collect_issue(
        &self,
        field_path: &str,
        res: Obj,
        violations: &mut Vec<Obj>,
    ) -> Result<(), RuntimeError> {
        match res {
            Obj::Empty => Ok(()),
            Obj::Record(rec) if rec.schema_id == SCHEMA_VALIDATION_ISSUE => {
                let code = rec.get_obj(0).clone();
                let message = rec.get_obj(1).clone();
                violations.push(make_violation(field_path, code, message));
                Ok(())
            }
            _ => Err(err_msg("Validator must return Void or validation.Issue")),
        }
    }
}

#[inline]
pub fn box_scalar_val(raw: u64, kind: u8) -> Obj {
    match kind {
        0 => Obj::boxed_int(raw as i64),
        1 => Obj::boxed_float(raw),
        _ => Obj::boxed_bool(raw != 0),
    }
}

#[inline]
pub fn is_type_tag_match(obj: &Obj, expected_tag: u32) -> bool {
    match expected_tag {
        SCHEMA_EMPTY_TAG => matches!(obj, Obj::Empty),
        SCHEMA_STR_TAG => matches!(obj, Obj::Str(_)),
        SCHEMA_LIST_TAG => matches!(obj, Obj::List(_)),
        SCHEMA_MAP_TAG => matches!(obj, Obj::Map(_)),
        id => matches!(obj, Obj::Record(r) if r.schema_id == id),
    }
}

#[inline]
pub fn is_err_or_nil_obj(module: &Module, obj: &Obj) -> bool {
    match obj {
        Obj::Empty => true,
        Obj::Record(rec) => {
            if is_builtin_error_schema(rec.schema_id) {
                return true;
            }
            module
                .struct_schemas
                .get(rec.schema_id as usize)
                .filter(|s| s.schema_id == rec.schema_id)
                .or_else(|| module.struct_schemas.iter().find(|s| s.schema_id == rec.schema_id))
                .is_some_and(|s| s.is_error)
        }
        _ => false,
    }
}
