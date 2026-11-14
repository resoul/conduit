use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, Checker};
use crate::frontend::lexer::{ESCAPED_CLOSE_BRACE, ESCAPED_OPEN_BRACE};
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::native::JsonType;

pub(crate) fn is_valid_native_type(mod_name: &str, ty_name: &str) -> bool {
    match mod_name {
        "cdt.json" => matches!(ty_name, "DecodeError" | "EncodeError"),
        "cdt.time" => matches!(ty_name, "Timestamp" | "Instant" | "Duration" | "TimeError"),
        "cdt.validation" => matches!(ty_name, "Issue" | "Violation" | "ValidationError"),
        _ => false,
    }
}

pub(crate) fn register_native_imports(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    for item in &program.items {
        let ItemKind::Import { path, alias, names } = &item.kind else {
            continue;
        };
        match path.as_str() {
            "cdt.json" => {
                if let Some(a) = alias {
                    checked.json_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "decode" | "encode" => insert_unique(
                            &mut checked.json_direct_fns,
                            &spec.alias,
                            &spec.name,
                        )?,
                        "DecodeError" | "EncodeError" => {
                            checked.json_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.json' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.time" => {
                if let Some(a) = alias {
                    checked.time_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "now" | "monotonic" | "elapsed" | "seconds" | "milliseconds" => {
                            insert_unique(&mut checked.time_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "Timestamp" | "Instant" | "Duration" | "TimeError" => {
                            checked.time_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.time' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.validation" => {
                if let Some(a) = alias {
                    checked.val_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "min" | "max" | "email" | "check" => {
                            insert_unique(&mut checked.val_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "Issue" | "Violation" | "ValidationError" => {
                            checked.val_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.validation' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            other => return Err(format!("Unknown or unlinked module '{}'", other)),
        }
    }
    Ok(())
}

fn insert_unique(
    map: &mut HashMap<String, String>,
    alias: &str,
    target: &str,
) -> Result<(), String> {
    if let Some(prev) = map.insert(alias.to_string(), target.to_string())
        && prev != target
    {
        return Err(format!("Duplicate imported identifier '{}'", alias));
    }
    Ok(())
}

pub fn resolve_type_expr(te: &TypeExpr, checked: &CheckedProgram) -> Result<Type, String> {
    match te {
        TypeExpr::Named(name) => match name.as_str() {
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Bool" => Ok(Type::Bool),
            "Nil" => Ok(Type::Nil),
            "Void" => Ok(Type::Void),
            "Any" => Ok(Type::Any),
            "IndexError" | "KeyError" | "ParseError" => Ok(Type::Custom(TypeId {
                module: ModuleId::Builtin,
                name: name.to_string(),
            })),
            _ if checked.generic_structs.contains_key(name) => Err(format!(
                "Generic type '{}' requires {} type argument(s)",
                name,
                checked.generic_structs[name].type_params.len()
            )),
            _ if checked.struct_by_name.contains_key(name) => {
                Ok(Type::Custom(TypeId::program(name)))
            }
            _ if checked.json_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.json",
                &checked.json_direct_types[name],
            ))),
            _ if checked.time_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.time",
                &checked.time_direct_types[name],
            ))),
            _ if checked.val_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.validation",
                &checked.val_direct_types[name],
            ))),
            _ if name.contains('.') => {
                let mut parts = name.splitn(2, '.');
                let alias = parts.next().unwrap_or("");
                let sub = parts.next().unwrap_or("");
                let mod_name = if checked.json_aliases.contains(alias) {
                    "cdt.json"
                } else if checked.time_aliases.contains(alias) {
                    "cdt.time"
                } else if checked.val_aliases.contains(alias) {
                    "cdt.validation"
                } else {
                    return Err(format!("Unknown type '{}'", name));
                };
                if !is_valid_native_type(mod_name, sub) {
                    return Err(format!("Unknown type '{}'", name));
                }
                Ok(Type::Custom(TypeId::native(mod_name, sub)))
            }
            other => Err(format!("Unknown type '{}'", other)),
        },
        TypeExpr::Generic { name, type_args } => {
            if name == "Map" && type_args.len() == 2 {
                let k = resolve_type_expr(&type_args[0], checked)?;
                validate_map_key_type(&k)?;
                let v = resolve_type_expr(&type_args[1], checked)?;
                Ok(Type::Map(Box::new(k), Box::new(v)))
            } else if checked.generic_structs.contains_key(name) {
                let mut resolved = Vec::with_capacity(type_args.len());
                for ta in type_args {
                    resolved.push(resolve_type_expr(ta, checked)?);
                }
                let mono = crate::frontend::monomorph_subst::format_mangled_name(name, &resolved);
                if checked.struct_by_name.contains_key(&mono) {
                    Ok(Type::Custom(TypeId::program(mono)))
                } else {
                    Err(format!("Unknown generic instantiation '{}'", mono))
                }
            } else {
                Err(format!("Unsupported generic type '{}'", name))
            }
        }
        TypeExpr::List(inner) => Ok(Type::List(Box::new(resolve_type_expr(inner, checked)?))),
        TypeExpr::Optional(inner) => {
            Ok(Type::Optional(Box::new(resolve_type_expr(inner, checked)?)))
        }
        TypeExpr::Union(vars) => {
            let mut out = Vec::with_capacity(vars.len());
            for v in vars {
                out.push(resolve_type_expr(v, checked)?);
            }
            Ok(Type::union(out))
        }
        TypeExpr::Tuple(elems) => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(resolve_type_expr(e, checked)?);
            }
            Ok(Type::Tuple(out))
        }
        TypeExpr::Func {
            params,
            return_type,
        } => {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(resolve_type_expr(p, checked)?);
            }
            let r = resolve_type_expr(return_type, checked)?;
            Ok(Type::Function {
                params: ps,
                return_type: Box::new(r),
            })
        }
    }
}

pub fn type_to_json_type(ty: &Type, structs: &HashMap<String, usize>) -> Option<JsonType> {
    match ty {
        Type::Int => Some(JsonType::Int),
        Type::Float => Some(JsonType::Float),
        Type::Bool => Some(JsonType::Bool),
        Type::String => Some(JsonType::String),
        Type::Optional(inner) => {
            Some(JsonType::Optional(Box::new(type_to_json_type(inner, structs)?)))
        }
        Type::List(inner) => Some(JsonType::List(Box::new(type_to_json_type(inner, structs)?))),
        Type::Map(k, v) if **k == Type::String => {
            Some(JsonType::Map(Box::new(type_to_json_type(v, structs)?)))
        }
        Type::Custom(tid) if tid.module == ModuleId::Program => {
            structs.get(&tid.name).map(|&idx| JsonType::Struct(idx as u32))
        }
        _ => None,
    }
}

pub(crate) fn is_error_type(ty: &Type, checked: &CheckedProgram) -> bool {
    match ty {
        Type::Custom(TypeId {
            module: ModuleId::Builtin,
            name,
        }) => matches!(name.as_str(), "IndexError" | "KeyError" | "ParseError"),
        Type::Custom(TypeId {
            module: ModuleId::Native(m),
            name,
        }) => match m.as_str() {
            "cdt.json" => matches!(name.as_str(), "DecodeError" | "EncodeError"),
            "cdt.time" => name == "TimeError",
            "cdt.validation" => matches!(name.as_str(), "Issue" | "ValidationError"),
            _ => false,
        },
        Type::Custom(TypeId {
            module: ModuleId::Program,
            name,
        }) => checked.error_structs.contains(name) || name.ends_with("Error"),
        _ => false,
    }
}

pub(crate) fn can_propagate_try(src_ty: &Type, fn_ret_ty: &Type, checked: &CheckedProgram) -> bool {
    let is_ret_compatible = |err_or_nil: &Type| err_or_nil.is_assignable_to(fn_ret_ty);
    match src_ty {
        Type::Optional(_) => matches!(fn_ret_ty, Type::Optional(_) | Type::Union(_)) && is_ret_compatible(&Type::Nil),
        Type::Union(vars) => {
            let errs: Vec<&Type> = vars.iter().filter(|v| is_error_type(v, checked)).collect();
            !errs.is_empty() && errs.iter().all(|e| is_ret_compatible(e))
        }
        _ => false,
    }
}

#[inline]
pub(crate) fn scalar_kind(ty: &Type) -> u8 {
    match ty {
        Type::Float => 1,
        Type::Bool => 2,
        _ => 0,
    }
}

pub(crate) fn validate_map_key_type(k: &Type) -> Result<(), String> {
    if matches!(
        k,
        Type::Int | Type::String | Type::Bool | Type::Any | Type::TypeParam(_)
    ) {
        Ok(())
    } else {
        Err(format!("Map key type must be Int, String, or Bool, got {}", k))
    }
}

pub(crate) fn native_member_field(tid: &TypeId, field: &str) -> Result<(Type, u16), String> {
    if tid.module == ModuleId::Builtin {
        return match (tid.name.as_str(), field) {
            ("IndexError", "index") => Ok((Type::Int, 0)),
            ("IndexError", "length") => Ok((Type::Int, 1)),
            ("IndexError", "message") => Ok((Type::String, 0)),
            ("KeyError", "key") | ("ParseError", "value") => Ok((Type::String, 0)),
            ("KeyError" | "ParseError", "message") => Ok((Type::String, 1)),
            _ => Err(format!("Type '{}' has no field '{}'", tid, field)),
        };
    }
    let TypeId {
        module: ModuleId::Native(mod_name),
        name,
    } = tid
    else {
        return Err(format!("Unknown native type '{}'", tid));
    };
    match (mod_name.as_str(), name.as_str(), field) {
        ("cdt.time", "TimeError", "message") => Ok((Type::String, 0)),
        ("cdt.validation", "Issue", "code") => Ok((Type::String, 0)),
        ("cdt.validation", "Issue", "message") => Ok((Type::String, 1)),
        ("cdt.validation", "Violation", "path") => Ok((Type::String, 0)),
        ("cdt.validation", "Violation", "code") => Ok((Type::String, 1)),
        ("cdt.validation", "Violation", "message") => Ok((Type::String, 2)),
        ("cdt.validation", "ValidationError", "issues") => Ok((
            Type::List(Box::new(Type::Custom(TypeId::native("cdt.validation", "Violation")))),
            0,
        )),
        ("cdt.json", "DecodeError", "path") => Ok((Type::String, 0)),
        ("cdt.json", "DecodeError", "message") => Ok((Type::String, 1)),
        ("cdt.json", "EncodeError", "message") => Ok((Type::String, 0)),
        _ => Err(format!("Type '{}' has no field '{}'", tid, field)),
    }
}

pub(crate) fn validator_email_fn_ty() -> Type {
    Type::Function {
        params: vec![Type::String],
        return_type: Box::new(Type::union(vec![
            Type::Void,
            Type::Custom(TypeId::native("cdt.validation", "Issue")),
        ])),
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn check_time_call(
        &mut self,
        func: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("time.{} does not accept type arguments", func));
        }
        let ts = Type::Custom(TypeId::native("cdt.time", "Timestamp"));
        let inst = Type::Custom(TypeId::native("cdt.time", "Instant"));
        let dur = Type::Custom(TypeId::native("cdt.time", "Duration"));
        let terr = Type::Custom(TypeId::native("cdt.time", "TimeError"));
        if matches!(func, "Timestamp" | "Instant" | "Duration") {
            return Err(format!("Cannot construct opaque type 'time.{}' directly", func));
        }
        if func == "TimeError" {
            return check_fields_ctor(
                self,
                "TimeError",
                &[("message", Type::String)],
                args,
                named_args,
                terr,
            );
        }
        if !named_args.is_empty() {
            return Err(format!("time.{} does not accept named arguments", func));
        }
        match func {
            "now" => {
                if !args.is_empty() {
                    return Err("time.now() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![ts, terr]))
            }
            "monotonic" => {
                if !args.is_empty() {
                    return Err("time.monotonic() takes 0 arguments".to_string());
                }
                Ok(inst)
            }
            "elapsed" => {
                if args.len() != 1 {
                    return Err("time.elapsed(start) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&inst))?;
                if args[0].ty != inst {
                    return Err(format!("time.elapsed expects time.Instant, got {}", args[0].ty));
                }
                Ok(Type::union(vec![dur, terr]))
            }
            "seconds" | "milliseconds" => {
                if args.len() != 1 {
                    return Err(format!("time.{}(value) expects 1 Int argument", func));
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if !args[0].ty.is_int() {
                    return Err(format!("time.{} expects Int, got {}", func, args[0].ty));
                }
                Ok(Type::union(vec![dur, terr]))
            }
            other => Err(format!("Unknown cdt.time symbol '{}'", other)),
        }
    }

    pub(crate) fn check_validation_call(
        &mut self,
        func: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("validation.{} does not accept type arguments", func));
        }
        let issue = Type::Custom(TypeId::native("cdt.validation", "Issue"));
        let violation = Type::Custom(TypeId::native("cdt.validation", "Violation"));
        let val_err = Type::Custom(TypeId::native("cdt.validation", "ValidationError"));
        match func {
            "Issue" => check_fields_ctor(
                self,
                "Issue",
                &[("code", Type::String), ("message", Type::String)],
                args,
                named_args,
                issue,
            ),
            "Violation" => check_fields_ctor(
                self,
                "Violation",
                &[
                    ("path", Type::String),
                    ("code", Type::String),
                    ("message", Type::String),
                ],
                args,
                named_args,
                violation,
            ),
            "email" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err("validation.email(val) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("validation.email expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "min" | "max" => {
                if !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("validation.{}(val, bound) expects 2 arguments", func));
                }
                let (v_slice, b_slice) = args.split_at_mut(1);
                self.check_expr(&mut v_slice[0], None)?;
                let exp_bound = if v_slice[0].ty.is_float() {
                    Type::Float
                } else {
                    Type::Int
                };
                self.check_expr(&mut b_slice[0], Some(&exp_bound))?;
                let ok_val = matches!(
                    v_slice[0].ty,
                    Type::Int | Type::Float | Type::String | Type::List(_) | Type::Map(_, _) | Type::Union(_) | Type::Any
                );
                if !ok_val {
                    return Err(format!("Unsupported type {} for validation.{}", v_slice[0].ty, func));
                }
                if !b_slice[0].ty.is_assignable_to(&exp_bound) && !matches!(v_slice[0].ty, Type::Union(_) | Type::Any) {
                    return Err(format!(
                        "validation.{} expected {} bound, got {}",
                        func, exp_bound, b_slice[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "check" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err("validation.check(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], None)?;
                Ok(Type::union(vec![args[0].ty.clone(), val_err]))
            }
            other => Err(format!("Unknown cdt.validation symbol '{}'", other)),
        }
    }

    pub(crate) fn check_and_desugar_string_literal(&mut self, expr: &mut Expr) -> Result<Type, String> {
        let span = expr.span;
        let raw = match &expr.kind {
            ExprKind::String(s) => s.clone(),
            _ => unreachable!(),
        };
        let mut parts: Vec<Expr> = Vec::new();
        let mut lit = String::new();
        let mut has_hole = false;
        let mut chars = raw.chars();

        let flush_lit = |buf: &mut String, out: &mut Vec<Expr>| {
            if !buf.is_empty() {
                let mut e = Expr::new(ExprKind::String(std::mem::take(buf)), span);
                e.ty = Type::String;
                out.push(e);
            }
        };

        while let Some(ch) = chars.next() {
            if ch == ESCAPED_OPEN_BRACE {
                lit.push('{');
            } else if ch == ESCAPED_CLOSE_BRACE {
                lit.push('}');
            } else if ch == '{' {
                let mut seg = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    seg.push(inner);
                }
                let seg_parts: Vec<&str> = seg.split('.').collect();
                if closed && !seg.is_empty() && seg_parts.iter().all(|p| is_interp_ident(p)) {
                    has_hole = true;
                    flush_lit(&mut lit, &mut parts);
                    if self.lookup(seg_parts[0]).is_none() {
                        return Err(format!(
                            "Unknown variable '{}' in string interpolation",
                            seg_parts[0]
                        ));
                    }
                    let mut hole = Expr::new(ExprKind::Ident(seg_parts[0].to_string()), span);
                    for f in &seg_parts[1..] {
                        hole = Expr::new(
                            ExprKind::Member {
                                target: Box::new(hole),
                                field: (*f).to_string(),
                            },
                            span,
                        );
                    }
                    self.check_expr(&mut hole, None)?;
                    if hole.ty != Type::String {
                        let callee = Expr::new(
                            ExprKind::Member {
                                target: Box::new(hole),
                                field: "to_string".to_string(),
                            },
                            span,
                        );
                        hole = Expr::new(
                            ExprKind::Call {
                                callee: Box::new(callee),
                                type_args: Vec::new(),
                                args: Vec::new(),
                                named_args: Vec::new(),
                            },
                            span,
                        );
                        hole.ty = Type::String;
                    }
                    parts.push(hole);
                } else {
                    lit.push('{');
                    for c in seg.chars() {
                        if c == ESCAPED_OPEN_BRACE {
                            lit.push('{');
                        } else if c == ESCAPED_CLOSE_BRACE {
                            lit.push('}');
                        } else {
                            lit.push(c);
                        }
                    }
                    if closed {
                        lit.push('}');
                    }
                }
            } else {
                lit.push(ch);
            }
        }

        if !has_hole {
            expr.kind = ExprKind::String(lit);
            return Ok(Type::String);
        }
        flush_lit(&mut lit, &mut parts);
        let mut iter = parts.into_iter();
        let mut acc = iter.next().unwrap();
        for rhs in iter {
            acc = Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(acc),
                    right: Box::new(rhs),
                },
                span,
            );
            acc.ty = Type::String;
        }
        *expr = acc;
        Ok(Type::String)
    }

    pub(crate) fn check_native_method(
        &self,
        tid: &TypeId,
        method: &str,
        args: &[Expr],
    ) -> Result<Type, String> {
        let terr = Type::Custom(TypeId::native("cdt.time", "TimeError"));
        match (tid.module.clone(), tid.name.as_str(), method) {
            (ModuleId::Native(ref m), "Timestamp", "unix_seconds" | "unix_milliseconds")
                if m == "cdt.time" =>
            {
                if !args.is_empty() {
                    return Err(format!("Timestamp.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Int, terr]))
            }
            (ModuleId::Native(ref m), "Duration", "seconds" | "milliseconds" | "nanoseconds")
                if m == "cdt.time" =>
            {
                if !args.is_empty() {
                    return Err(format!("Duration.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Int, terr]))
            }
            _ => Err(format!("Type '{}' has no method '.{}()'", tid, method)),
        }
    }
}

fn check_fields_ctor(
    tc: &mut Checker<'_>,
    name: &str,
    fields: &[(&str, Type)],
    args: &mut [Expr],
    named_args: &mut [NamedArg],
    ret_ty: Type,
) -> Result<Type, String> {
    if !named_args.is_empty() {
        if !args.is_empty() || named_args.len() != fields.len() {
            return Err(format!("'{}' expects {} named arguments", name, fields.len()));
        }
        let mut seen = HashSet::new();
        for na in named_args.iter_mut() {
            if !seen.insert(na.name.clone()) {
                return Err(format!("Duplicate field '{}' in '{}'", na.name, name));
            }
            let (_, fty) = fields
                .iter()
                .find(|(fnm, _)| *fnm == na.name)
                .ok_or_else(|| format!("Type '{}' has no field '{}'", name, na.name))?;
            tc.check_expr(&mut na.value, Some(fty))?;
            if !na.value.ty.is_assignable_to(fty) {
                return Err(format!("Field '{}' expected {}, got {}", na.name, fty, na.value.ty));
            }
        }
    } else {
        if args.len() != fields.len() {
            return Err(format!("'{}' expects {} arguments", name, fields.len()));
        }
        for (arg, (fname, fty)) in args.iter_mut().zip(fields.iter()) {
            tc.check_expr(arg, Some(fty))?;
            if !arg.ty.is_assignable_to(fty) {
                return Err(format!("Field '{}' expected {}, got {}", fname, fty, arg.ty));
            }
        }
    }
    Ok(ret_ty)
}

fn is_interp_ident(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}
