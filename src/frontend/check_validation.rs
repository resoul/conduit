use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::scalar_kind;
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::native::validation::{
    CompiledValidationRule, NestedValidation, ValidationFieldPlan, ValidationStructPlan,
};
use crate::vm::value::Obj;

pub(crate) fn check_validator_contracts(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    let issue_ty = Type::Custom(TypeId::native("cdt.validation", "Issue"));
    let expected_ret = Type::union(vec![Type::Void, issue_ty]);
    for item in &program.items {
        if let ItemKind::Func {
            name,
            is_validator: true,
            ..
        } = &item.kind
        {
            let fid = checked.func_by_name[name];
            let sig = &checked.funcs[fid as usize];
            if sig.params.is_empty() {
                return Err(format!(
                    "Validator function '{}' must have at least one parameter",
                    name
                ));
            }
            if sig.return_type != expected_ret {
                return Err(format!(
                    "Validator function '{}' must return 'Void | validation.Issue', got '{}'",
                    name, sig.return_type
                ));
            }
            checked.validator_funcs.insert(name.clone());
        }
    }
    Ok(())
}

pub(crate) fn build_validation_plans(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    let mut plans = Vec::with_capacity(checked.structs.len());
    for item in &program.items {
        let fields_ast = match &item.kind {
            ItemKind::Struct { name, fields, .. } => Some((name.as_str(), fields)),
            _ => None,
        };
        let Some((struct_name, fields)) = fields_ast else {
            continue;
        };
        let sid = checked.struct_by_name[struct_name];
        let resolved_struct = &checked.structs[sid];
        let mut fplans = Vec::with_capacity(fields.len());

        for (f_ast, f_res) in fields.iter().zip(resolved_struct.fields.iter()) {
            let (is_opt, inner_ty) = match &f_res.ty {
                Type::Optional(inner) => (true, (**inner).clone()),
                other => (false, other.clone()),
            };
            let nested = build_nested_val(&inner_ty, &checked.struct_by_name);
            let mut rules = Vec::new();
            for ann in &f_ast.annotations {
                if ann.target == "json" || ann.target == "env" {
                    continue;
                }
                rules.push(compile_field_rule(ann, &inner_ty, checked)?);
            }
            fplans.push(ValidationFieldPlan {
                name: f_res.name.clone(),
                is_scalar: f_res.is_scalar,
                scalar_kind: scalar_kind(&inner_ty),
                is_optional: is_opt,
                slot_idx: f_res.slot_idx,
                rules,
                nested,
            });
        }
        plans.push(ValidationStructPlan {
            schema_id: resolved_struct.schema_id,
            fields: fplans,
        });
    }
    checked.validation_plans = plans;
    Ok(())
}

pub(crate) fn finalize_validation_plan_func_ids(checked: &mut CheckedProgram) {
    for plan in &mut checked.validation_plans {
        for f in &mut plan.fields {
            for rule in &mut f.rules {
                if let CompiledValidationRule::UserFunc {
                    func_name,
                    func_id,
                    ..
                } = rule
                    && let Some(&new_id) = checked.func_by_name.get(func_name)
                {
                    *func_id = new_id;
                }
            }
        }
    }
}

fn build_nested_val(ty: &Type, structs: &HashMap<String, usize>) -> NestedValidation {
    match ty {
        Type::Optional(inner) => build_nested_val(inner, structs),
        Type::Custom(tid) if tid.module == ModuleId::Program => structs
            .get(&tid.name)
            .map(|&id| NestedValidation::Struct(id as u32))
            .unwrap_or(NestedValidation::None),
        Type::List(inner) => {
            let child = build_nested_val(inner, structs);
            if matches!(child, NestedValidation::None) {
                NestedValidation::None
            } else {
                NestedValidation::List(Box::new(child))
            }
        }
        _ => NestedValidation::None,
    }
}

fn compile_field_rule(
    ann: &FieldAnnotation,
    field_ty: &Type,
    checked: &CheckedProgram,
) -> Result<CompiledValidationRule, String> {
    let builtin_name = if let Some((alias, sub)) = ann.target.split_once('.') {
        if checked.val_aliases.contains(alias) {
            Some(sub)
        } else {
            return Err(format!("Unknown module '{}' in annotation '@{}'", alias, ann.target));
        }
    } else {
        checked.val_direct_fns.get(&ann.target).map(String::as_str)
    };

    if let Some(bname) = builtin_name {
        return match bname {
            "email" => {
                if !ann.args.is_empty() {
                    return Err("@validation.email takes no arguments".to_string());
                }
                if *field_ty != Type::String {
                    return Err(format!(
                        "Field type '{}' is not compatible with validator parameter type 'String'",
                        field_ty
                    ));
                }
                Ok(CompiledValidationRule::Email)
            }
            "min" | "max" => compile_bound_rule(bname, &ann.args, field_ty),
            other => Err(format!("'cdt.validation.{}' is not a validator", other)),
        };
    }

    let Some(&fid) = checked.func_by_name.get(&ann.target) else {
        return Err(format!("Unknown validator function '{}'", ann.target));
    };
    if !checked.validator_funcs.contains(&ann.target) {
        return Err(format!(
            "Function '{}' is not marked with '[#validator]'",
            ann.target
        ));
    }
    let sig = &checked.funcs[fid as usize];
    if !field_ty.is_assignable_to(&sig.params[0]) {
        return Err(format!(
            "Field type '{}' is not compatible with validator parameter type '{}'",
            field_ty, sig.params[0]
        ));
    }
    if ann.args.len() != sig.params.len() - 1 {
        return Err(format!(
            "Validator '{}' expects {} argument(s), got {}",
            ann.target,
            sig.params.len() - 1,
            ann.args.len()
        ));
    }
    let mut extra_scalars = Vec::new();
    let mut extra_objs = Vec::new();
    for (arg_expr, param_ty) in ann.args.iter().zip(sig.params[1..].iter()) {
        push_const_validator_arg(arg_expr, param_ty, &mut extra_scalars, &mut extra_objs)?;
    }
    Ok(CompiledValidationRule::UserFunc {
        func_name: ann.target.clone(),
        func_id: fid,
        first_param_is_scalar: sig.params[0].is_scalar(),
        first_param_kind: scalar_kind(&sig.params[0]),
        extra_scalars,
        extra_objs,
    })
}

fn compile_bound_rule(
    kind: &str,
    args: &[Expr],
    field_ty: &Type,
) -> Result<CompiledValidationRule, String> {
    if args.len() != 1 {
        return Err(format!("@validation.{} expects 1 bound argument", kind));
    }
    let is_min = kind == "min";
    match field_ty {
        Type::Int => match args[0].kind {
            ExprKind::Int(n) => Ok(if is_min {
                CompiledValidationRule::MinInt(n)
            } else {
                CompiledValidationRule::MaxInt(n)
            }),
            _ => Err(format!("@validation.{} on Int expects an Int literal", kind)),
        },
        Type::Float => match args[0].kind {
            ExprKind::Float(v) => Ok(if is_min {
                CompiledValidationRule::MinFloat(v)
            } else {
                CompiledValidationRule::MaxFloat(v)
            }),
            _ => Err(format!("@validation.{} on Float expects a Float literal", kind)),
        },
        Type::String | Type::List(_) | Type::Map(_, _) => match args[0].kind {
            ExprKind::Int(n) => Ok(if is_min {
                CompiledValidationRule::MinObj(n)
            } else {
                CompiledValidationRule::MaxObj(n)
            }),
            _ => Err(format!("@validation.{} length bound expects an Int literal", kind)),
        },
        other => Err(format!(
            "Field type '{}' is not compatible with validator '@validation.{}'",
            other, kind
        )),
    }
}

fn push_const_validator_arg(
    expr: &Expr,
    param_ty: &Type,
    scalars: &mut Vec<u64>,
    objs: &mut Vec<Obj>,
) -> Result<(), String> {
    let (lit_ty, scalar_val, obj_val) = match &expr.kind {
        ExprKind::Int(n) => (Type::Int, Some(*n as u64), Obj::boxed_int(*n)),
        ExprKind::Float(v) => (Type::Float, Some(v.to_bits()), Obj::boxed_float(v.to_bits())),
        ExprKind::Bool(b) => (Type::Bool, Some(*b as u64), Obj::boxed_bool(*b)),
        ExprKind::String(s) => (Type::String, None, Obj::str(s.as_str())),
        ExprKind::Nil => (Type::Nil, None, Obj::Empty),
        _ => return Err("Annotation argument must be a constant literal".to_string()),
    };
    if !lit_ty.is_assignable_to(param_ty) {
        return Err(format!(
            "Annotation argument of type {} does not match parameter type {}",
            lit_ty, param_ty
        ));
    }
    if param_ty.is_scalar() {
        scalars.push(scalar_val.unwrap());
    } else {
        objs.push(obj_val);
    }
    Ok(())
}
