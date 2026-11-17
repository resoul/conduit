use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::resolve_type_expr;
use crate::frontend::monomorph::ensure_monomorphized_struct;
use crate::frontend::parser_stmt::format_type_expr;
use crate::frontend::types::{ModuleId, Type, TypeId};

#[inline]
pub(crate) fn has_type_param(ty: &Type) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::List(t) | Type::Optional(t) => has_type_param(t),
        Type::Map(k, v) => has_type_param(k) || has_type_param(v),
        Type::Generic { type_args, .. } => type_args.iter().any(has_type_param),
        Type::Tuple(ts) | Type::Union(ts) => ts.iter().any(has_type_param),
        Type::Function { params, return_type } => {
            params.iter().any(has_type_param) || has_type_param(return_type)
        }
        _ => false,
    }
}

pub(crate) fn format_mangled_name(base: &str, type_args: &[Type]) -> String {
    let args: Vec<String> = type_args.iter().map(|t| t.to_string()).collect();
    format!("{}<{}>", base, args.join(", "))
}

pub(crate) fn resolve_type_expr_with_scope(
    te: &TypeExpr,
    checked: &mut CheckedProgram,
    type_params: &HashSet<String>,
) -> Result<Type, String> {
    match te {
        TypeExpr::Named(name) => {
            if type_params.contains(name) {
                return Ok(Type::TypeParam(name.clone()));
            }
            if let Some(tmpl) = checked.generic_structs.get(name) {
                return Err(format!(
                    "Generic type '{}' requires {} type argument(s)",
                    name,
                    tmpl.type_params.len()
                ));
            }
            resolve_type_expr(te, checked)
        }
        TypeExpr::Generic { name, type_args } => {
            if name == "Map" {
                if type_args.len() != 2 {
                    return Err(format!(
                        "Generic type 'Map' expects 2 type argument(s), got {}",
                        type_args.len()
                    ));
                }
                let k = resolve_type_expr_with_scope(&type_args[0], checked, type_params)?;
                crate::frontend::check_native::validate_map_key_type(&k)?;
                let v = resolve_type_expr_with_scope(&type_args[1], checked, type_params)?;
                return Ok(Type::Map(Box::new(k), Box::new(v)));
            }
            let expected_arity = checked
                .generic_structs
                .get(name)
                .map(|t| t.type_params.len())
                .ok_or_else(|| format!("Type '{}' is not generic", name))?;
            if type_args.len() != expected_arity {
                return Err(format!(
                    "Generic type '{}' expects {} type argument(s), got {}",
                    name,
                    expected_arity,
                    type_args.len()
                ));
            }
            let mut resolved = Vec::with_capacity(type_args.len());
            for a in type_args {
                resolved.push(resolve_type_expr_with_scope(a, checked, type_params)?);
            }
            if resolved.iter().any(has_type_param) {
                Ok(Type::Generic {
                    type_id: TypeId::program(name),
                    type_args: resolved,
                })
            } else {
                let mono = ensure_monomorphized_struct(checked, name, &resolved)?;
                Ok(Type::Custom(TypeId::program(mono)))
            }
        }
        TypeExpr::List(inner) => Ok(Type::List(Box::new(resolve_type_expr_with_scope(
            inner,
            checked,
            type_params,
        )?))),
        TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(resolve_type_expr_with_scope(
            inner,
            checked,
            type_params,
        )?))),
        TypeExpr::Union(vars) => {
            let mut out = Vec::with_capacity(vars.len());
            for v in vars {
                out.push(resolve_type_expr_with_scope(v, checked, type_params)?);
            }
            Ok(Type::union(out))
        }
        TypeExpr::Tuple(elems) => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(resolve_type_expr_with_scope(e, checked, type_params)?);
            }
            Ok(Type::Tuple(out))
        }
        TypeExpr::Func { params, return_type } => {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(resolve_type_expr_with_scope(p, checked, type_params)?);
            }
            let r = resolve_type_expr_with_scope(return_type, checked, type_params)?;
            Ok(Type::Function {
                params: ps,
                return_type: Box::new(r),
            })
        }
    }
}

pub(crate) fn type_to_type_expr(ty: &Type) -> TypeExpr {
    match ty {
        Type::Int => TypeExpr::Named("Int".into()),
        Type::Float => TypeExpr::Named("Float".into()),
        Type::String => TypeExpr::Named("String".into()),
        Type::Bool => TypeExpr::Named("Bool".into()),
        Type::Nil => TypeExpr::Named("Nil".into()),
        Type::Void => TypeExpr::Named("Void".into()),
        Type::Any => TypeExpr::Named("Any".into()),
        Type::TypeParam(n) | Type::Enum(n) => TypeExpr::Named(n.clone()),
        Type::List(t) => TypeExpr::List(Box::new(type_to_type_expr(t))),
        Type::Optional(t) => TypeExpr::Optional(Box::new(type_to_type_expr(t))),
        Type::Map(k, v) => TypeExpr::Generic {
            name: "Map".into(),
            type_args: vec![type_to_type_expr(k), type_to_type_expr(v)],
        },
        Type::Tuple(ts) => TypeExpr::Tuple(ts.iter().map(type_to_type_expr).collect()),
        Type::Union(vs) => TypeExpr::Union(vs.iter().map(type_to_type_expr).collect()),
        Type::Function { params, return_type } => TypeExpr::Func {
            params: params.iter().map(type_to_type_expr).collect(),
            return_type: Box::new(type_to_type_expr(return_type)),
        },
        Type::Generic { type_id, type_args } => TypeExpr::Generic {
            name: type_id.name.clone(),
            type_args: type_args.iter().map(type_to_type_expr).collect(),
        },
        Type::Custom(tid) => match &tid.module {
            ModuleId::Program | ModuleId::Builtin | ModuleId::Local(_) => {
                TypeExpr::Named(tid.name.clone())
            }
            ModuleId::Native(m) => {
                let short = m.strip_prefix("cdt.").unwrap_or(m);
                TypeExpr::Named(format!("{}.{}", short, tid.name))
            }
        },
    }
}

pub(crate) fn substitute_type(
    ty: &Type,
    subst: &HashMap<String, Type>,
    checked: &mut CheckedProgram,
) -> Result<Type, String> {
    Ok(match ty {
        Type::TypeParam(n) => subst.get(n).cloned().unwrap_or_else(|| ty.clone()),
        Type::List(t) => Type::List(Box::new(substitute_type(t, subst, checked)?)),
        Type::Optional(t) => Type::Optional(Box::new(substitute_type(t, subst, checked)?)),
        Type::Map(k, v) => Type::Map(
            Box::new(substitute_type(k, subst, checked)?),
            Box::new(substitute_type(v, subst, checked)?),
        ),
        Type::Tuple(ts) => {
            let mut out = Vec::with_capacity(ts.len());
            for t in ts {
                out.push(substitute_type(t, subst, checked)?);
            }
            Type::Tuple(out)
        }
        Type::Union(vs) => {
            let mut out = Vec::with_capacity(vs.len());
            for v in vs {
                out.push(substitute_type(v, subst, checked)?);
            }
            Type::union(out)
        }
        Type::Function { params, return_type } => {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(substitute_type(p, subst, checked)?);
            }
            Type::Function {
                params: ps,
                return_type: Box::new(substitute_type(return_type, subst, checked)?),
            }
        }
        Type::Generic { type_id, type_args } => {
            let mut new_args = Vec::with_capacity(type_args.len());
            for a in type_args {
                new_args.push(substitute_type(a, subst, checked)?);
            }
            if !new_args.iter().any(has_type_param) {
                let mono = ensure_monomorphized_struct(checked, &type_id.name, &new_args)?;
                Type::Custom(TypeId::program(mono))
            } else {
                Type::Generic {
                    type_id: type_id.clone(),
                    type_args: new_args,
                }
            }
        }
        _ => ty.clone(),
    })
}

pub(crate) fn substitute_type_partial(
    ty: &Type,
    subst: &HashMap<String, Type>,
    checked: &mut CheckedProgram,
) -> Type {
    match ty {
        Type::TypeParam(n) => subst.get(n).cloned().unwrap_or(Type::Any),
        Type::List(t) => Type::List(Box::new(substitute_type_partial(t, subst, checked))),
        Type::Optional(t) => Type::Optional(Box::new(substitute_type_partial(t, subst, checked))),
        Type::Tuple(ts) => Type::Tuple(
            ts.iter()
                .map(|t| substitute_type_partial(t, subst, checked))
                .collect(),
        ),
        Type::Function { params, return_type } => Type::Function {
            params: params
                .iter()
                .map(|p| substitute_type_partial(p, subst, checked))
                .collect(),
            return_type: Box::new(substitute_type_partial(return_type, subst, checked)),
        },
        Type::Generic { type_id, type_args } => {
            let resolved: Vec<Type> = type_args
                .iter()
                .map(|a| substitute_type_partial(a, subst, checked))
                .collect();
            if resolved.iter().all(|a| *a != Type::Any && !has_type_param(a))
                && let Ok(mono) = ensure_monomorphized_struct(checked, &type_id.name, &resolved)
            {
                return Type::Custom(TypeId::program(mono));
            }
            Type::Generic {
                type_id: type_id.clone(),
                type_args: resolved,
            }
        }
        _ => ty.clone(),
    }
}

pub(crate) fn unify_type(
    pattern: &Type,
    actual: &Type,
    type_params: &[String],
    inferred: &mut HashMap<String, Type>,
    mono_struct_args: &HashMap<String, (String, Vec<Type>)>,
) -> Result<(), String> {
    if *actual == Type::Any {
        return Ok(());
    }
    match pattern {
        Type::TypeParam(p) if type_params.iter().any(|tp| tp == p) => {
            if let Some(prev) = inferred.get(p) {
                if prev == actual || actual.is_assignable_to(prev) {
                    return Ok(());
                }
                if prev.is_assignable_to(actual) {
                    inferred.insert(p.clone(), actual.clone());
                    return Ok(());
                }
                return Err(format!(
                    "Conflicting types inferred for type parameter '{}': {} vs {}",
                    p, prev, actual
                ));
            }
            inferred.insert(p.clone(), actual.clone());
            Ok(())
        }
        Type::List(pa) => {
            if let Type::List(aa) = actual {
                unify_type(pa, aa, type_params, inferred, mono_struct_args)?;
            }
            Ok(())
        }
        Type::Optional(pa) => {
            if let Type::Optional(aa) = actual {
                unify_type(pa, aa, type_params, inferred, mono_struct_args)?;
            } else if *actual != Type::Nil {
                unify_type(pa, actual, type_params, inferred, mono_struct_args)?;
            }
            Ok(())
        }
        Type::Map(pk, pv) => {
            if let Type::Map(ak, av) = actual {
                unify_type(pk, ak, type_params, inferred, mono_struct_args)?;
                unify_type(pv, av, type_params, inferred, mono_struct_args)?;
            }
            Ok(())
        }
        Type::Tuple(ps) => {
            if let Type::Tuple(acs) = actual
                && ps.len() == acs.len()
            {
                for (p, a) in ps.iter().zip(acs.iter()) {
                    unify_type(p, a, type_params, inferred, mono_struct_args)?;
                }
            }
            Ok(())
        }
        Type::Function { params: pp, return_type: pr } => {
            if let Type::Function { params: ap, return_type: ar } = actual
                && pp.len() == ap.len()
            {
                for (p, a) in pp.iter().zip(ap.iter()) {
                    unify_type(p, a, type_params, inferred, mono_struct_args)?;
                }
                unify_type(pr, ar, type_params, inferred, mono_struct_args)?;
            }
            Ok(())
        }
        Type::Union(pvs) => {
            if let Type::Union(avs) = actual
                && pvs.len() == avs.len()
            {
                for (p, a) in pvs.iter().zip(avs.iter()) {
                    unify_type(p, a, type_params, inferred, mono_struct_args)?;
                }
            }
            Ok(())
        }
        Type::Generic { type_id, type_args } => {
            if let Type::Generic { type_id: aid, type_args: aargs } = actual
                && aid.name == type_id.name
                && aargs.len() == type_args.len()
            {
                for (p, a) in type_args.iter().zip(aargs.iter()) {
                    unify_type(p, a, type_params, inferred, mono_struct_args)?;
                }
            } else if let Type::Custom(atid) = actual
                && let Some((base, concrete)) = mono_struct_args.get(&atid.name)
                && base == &type_id.name
                && concrete.len() == type_args.len()
            {
                for (p, a) in type_args.iter().zip(concrete.iter()) {
                    unify_type(p, a, type_params, inferred, mono_struct_args)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn subst_type_expr(te: &mut TypeExpr, subst: &HashMap<String, TypeExpr>) {
    match te {
        TypeExpr::Named(n) => {
            if let Some(rep) = subst.get(n) {
                *te = rep.clone();
            }
        }
        TypeExpr::Generic { type_args, .. } => {
            for a in type_args {
                subst_type_expr(a, subst);
            }
        }
        TypeExpr::List(i) | TypeExpr::Optional(i) => subst_type_expr(i, subst),
        TypeExpr::Union(vs) | TypeExpr::Tuple(vs) => {
            for v in vs {
                subst_type_expr(v, subst);
            }
        }
        TypeExpr::Func { params, return_type } => {
            for p in params {
                subst_type_expr(p, subst);
            }
            subst_type_expr(return_type, subst);
        }
    }
}

pub(crate) fn subst_block(block: &mut Block, subst: &HashMap<String, TypeExpr>) {
    for s in &mut block.stmts {
        match &mut s.kind {
            StmtKind::Define { value, .. }
            | StmtKind::DestructureDefine { value, .. }
            | StmtKind::Expr(value) => subst_expr(value, subst),
            StmtKind::VarDecl { ty, init, .. } => {
                if let Some(t) = ty {
                    subst_type_expr(t, subst);
                }
                if let Some(e) = init {
                    subst_expr(e, subst);
                }
            }
            StmtKind::Assign { target, value } => {
                subst_expr(target, subst);
                subst_expr(value, subst);
            }
            StmtKind::Return(Some(e)) => subst_expr(e, subst),
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::If { condition, then_block, else_block } => {
                subst_expr(condition, subst);
                subst_block(then_block, subst);
                if let Some(eb) = else_block {
                    subst_block(eb, subst);
                }
            }
            StmtKind::Guard { condition, else_block } => {
                subst_expr(condition, subst);
                subst_block(else_block, subst);
            }
            StmtKind::ForIn { iterable, body, .. } => {
                subst_expr(iterable, subst);
                subst_block(body, subst);
            }
            StmtKind::While { condition, body } => {
                subst_expr(condition, subst);
                subst_block(body, subst);
            }
            StmtKind::Switch { expr, cases } => {
                subst_expr(expr, subst);
                for c in cases {
                    if let PatternKind::Type { name, .. } = &mut c.pattern.kind {
                        *name = subst_pattern_type_name(name, subst);
                    }
                    subst_block(&mut c.body, subst);
                }
            }
        }
    }
}

fn subst_pattern_type_name(name: &str, subst: &HashMap<String, TypeExpr>) -> String {
    if let Some(rep) = subst.get(name) {
        return format_type_expr(rep);
    }
    let Some(lt) = name.find('<') else {
        return name.to_string();
    };
    let Some(gt) = name.rfind('>') else {
        return name.to_string();
    };
    let base = &name[..lt];
    let inner = &name[lt + 1..gt];
    let replaced = inner
        .split(',')
        .map(|part| subst_pattern_type_name(part.trim(), subst))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}<{}>", base, replaced)
}

fn subst_expr(expr: &mut Expr, subst: &HashMap<String, TypeExpr>) {
    match &mut expr.kind {
        ExprKind::Binary { left, right, .. } => {
            subst_expr(left, subst);
            subst_expr(right, subst);
        }
        ExprKind::Unary { expr: i, .. } | ExprKind::Try(i) => subst_expr(i, subst),
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                subst_expr(e, subst);
            }
        }
        ExprKind::Call { callee, type_args, args, named_args } => {
            subst_expr(callee, subst);
            for t in type_args {
                subst_type_expr(t, subst);
            }
            for a in args {
                subst_expr(a, subst);
            }
            for na in named_args {
                subst_expr(&mut na.value, subst);
            }
        }
        ExprKind::Member { target, .. } | ExprKind::OptionalMember { target, .. } => {
            subst_expr(target, subst);
        }
        ExprKind::Index { target, index } => {
            subst_expr(target, subst);
            subst_expr(index, subst);
        }
        ExprKind::Closure { params, return_type, body } => {
            for p in params {
                subst_type_expr(&mut p.ty, subst);
            }
            if let Some(rt) = return_type {
                subst_type_expr(rt, subst);
            }
            subst_block(body, subst);
        }
        _ => {}
    }
}
