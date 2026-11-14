use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, Checker};
use crate::frontend::check_native::{resolve_type_expr, type_to_json_type};
use crate::frontend::monomorph_subst::{
    format_mangled_name, resolve_type_expr_with_scope, subst_block, subst_type_expr,
    substitute_type, substitute_type_partial, type_to_type_expr, unify_type,
};
use crate::frontend::types::{FuncSig, ModuleId, ResolvedField, ResolvedStruct, Type, TypeId};
use crate::native::StructSchema;

#[derive(Debug, Clone)]
pub struct GenericStructTemplate {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone)]
pub struct GenericFuncTemplate {
    pub type_params: Vec<String>,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub item: Item,
}

pub(crate) fn ensure_monomorphized_struct(
    checked: &mut CheckedProgram,
    base_name: &str,
    type_args: &[Type],
) -> Result<String, String> {
    let mono_name = format_mangled_name(base_name, type_args);
    if checked.struct_by_name.contains_key(&mono_name) {
        return Ok(mono_name);
    }
    let tmpl = checked
        .generic_structs
        .get(base_name)
        .cloned()
        .ok_or_else(|| format!("Unknown generic struct '{}'", base_name))?;
    if type_args.len() != tmpl.type_params.len() {
        return Err(format!(
            "Generic type '{}' expects {} type argument(s), got {}",
            base_name,
            tmpl.type_params.len(),
            type_args.len()
        ));
    }

    let sid = checked.structs.len();
    checked.struct_by_name.insert(mono_name.clone(), sid);
    checked
        .mono_struct_args
        .insert(mono_name.clone(), (base_name.to_string(), type_args.to_vec()));
    checked.structs.push(ResolvedStruct {
        schema_id: sid as u32,
        name: mono_name.clone(),
        fields: Vec::new(),
        scalar_len: 0,
        obj_len: 0,
    });
    checked
        .struct_schemas
        .push(StructSchema::new(sid as u32, &mono_name, Vec::new()));

    let mut subst_tes = HashMap::new();
    for (tp, arg_ty) in tmpl.type_params.iter().zip(type_args.iter()) {
        subst_tes.insert(tp.clone(), type_to_type_expr(arg_ty));
    }

    let mut resolved_fields = Vec::with_capacity(tmpl.fields.len());
    let mut subst_fields = Vec::with_capacity(tmpl.fields.len());
    let mut scalar_len: u16 = 0;
    let mut obj_len: u16 = 0;
    let mut schema_fields = Vec::with_capacity(tmpl.fields.len());

    let no_tp = HashSet::new();
    for f in &tmpl.fields {
        let mut f_clone = f.clone();
        subst_type_expr(&mut f_clone.ty, &subst_tes);
        let fty = resolve_type_expr_with_scope(&f_clone.ty, checked, &no_tp)?;
        let is_scalar = fty.is_scalar();
        let slot_idx = if is_scalar {
            let s = scalar_len;
            scalar_len += 1;
            s
        } else {
            let o = obj_len;
            obj_len += 1;
            o
        };
        let json_name = f.json_name.clone().unwrap_or_else(|| f.name.clone());
        if let Some(jty) = type_to_json_type(&fty, &checked.struct_by_name) {
            schema_fields.push(crate::native::StructFieldDef {
                name: f.name.clone(),
                json_name: json_name.clone(),
                ty: jty,
            });
        }
        resolved_fields.push(ResolvedField {
            name: f.name.clone(),
            json_name,
            ty: fty,
            is_scalar,
            slot_idx,
        });
        subst_fields.push(f_clone);
    }

    checked.structs[sid] = ResolvedStruct {
        schema_id: sid as u32,
        name: mono_name.clone(),
        fields: resolved_fields,
        scalar_len,
        obj_len,
    };
    checked.struct_schemas[sid] = StructSchema::new(sid as u32, &mono_name, schema_fields);
    checked.pending_mono_items.push(Item {
        is_exported: false,
        is_imported: false,
        kind: ItemKind::Struct {
            name: mono_name.clone(),
            type_params: Vec::new(),
            fields: subst_fields,
        },
        span: crate::frontend::lexer::Span::new(0, 0),
    });
    Ok(mono_name)
}

pub(crate) fn ensure_monomorphized_func(
    checked: &mut CheckedProgram,
    base_name: &str,
    type_args: &[Type],
) -> Result<String, String> {
    let mono_name = format_mangled_name(base_name, type_args);
    if checked.func_by_name.contains_key(&mono_name) {
        return Ok(mono_name);
    }
    let tmpl = checked
        .generic_funcs
        .get(base_name)
        .cloned()
        .ok_or_else(|| format!("Unknown generic function '{}'", base_name))?;
    let mut subst_types = HashMap::new();
    let mut subst_tes = HashMap::new();
    for (tp, ty) in tmpl.type_params.iter().zip(type_args.iter()) {
        subst_types.insert(tp.clone(), ty.clone());
        subst_tes.insert(tp.clone(), type_to_type_expr(ty));
    }

    let mut concrete_params = Vec::with_capacity(tmpl.params.len());
    for pty in &tmpl.params {
        concrete_params.push(substitute_type(pty, &subst_types, checked)?);
    }
    let concrete_ret = substitute_type(&tmpl.return_type, &subst_types, checked)?;

    let mut item = tmpl.item;
    if let ItemKind::Func {
        name,
        type_params,
        params,
        return_type,
        body,
        ..
    } = &mut item.kind
    {
        *name = mono_name.clone();
        type_params.clear();
        for p in params {
            subst_type_expr(&mut p.ty, &subst_tes);
        }
        if let Some(rt) = return_type {
            subst_type_expr(rt, &subst_tes);
        }
        subst_block(body, &subst_tes);
    }
    let fid = checked.funcs.len() as u32;
    checked.func_by_name.insert(mono_name.clone(), fid);
    checked.funcs.push(FuncSig {
        name: mono_name.clone(),
        params: concrete_params,
        return_type: concrete_ret,
    });
    checked.pending_mono_items.push(item);
    Ok(mono_name)
}

pub(crate) fn check_generic_struct_ctor(
    tc: &mut Checker<'_>,
    base_name: &str,
    type_args: &mut Vec<TypeExpr>,
    args: &mut [Expr],
    named_args: &mut [NamedArg],
) -> Result<String, String> {
    let tmpl = tc
        .checked
        .generic_structs
        .get(base_name)
        .cloned()
        .ok_or_else(|| format!("Unknown generic struct '{}'", base_name))?;
    if !type_args.is_empty() {
        if type_args.len() != tmpl.type_params.len() {
            return Err(format!(
                "Generic type '{}' expects {} type argument(s), got {}",
                base_name,
                tmpl.type_params.len(),
                type_args.len()
            ));
        }
        let mut resolved = Vec::with_capacity(type_args.len());
        for ta in type_args.iter() {
            resolved.push(tc.checked.resolve_type_mut(ta)?);
        }
        type_args.clear();
        return ensure_monomorphized_struct(tc.checked, base_name, &resolved);
    }

    let tp_set: HashSet<String> = tmpl.type_params.iter().cloned().collect();
    let mut field_pats = Vec::with_capacity(tmpl.fields.len());
    for f in &tmpl.fields {
        let fty = resolve_type_expr_with_scope(&f.ty, tc.checked, &tp_set)?;
        field_pats.push((f.name.clone(), fty));
    }
    let mut inferred: HashMap<String, Type> = HashMap::new();
    let self_pat = Type::Generic {
        type_id: TypeId::program(base_name),
        type_args: tmpl.type_params.iter().cloned().map(Type::TypeParam).collect(),
    };
    if !named_args.is_empty() {
        if !args.is_empty() {
            return Err(format!("Cannot mix positional and named args in '{}'", base_name));
        }
        let rest = if named_args[0].name == ".." {
            tc.check_expr(&mut named_args[0].value, None)?;
            unify_type(
                &self_pat,
                &named_args[0].value.ty,
                &tmpl.type_params,
                &mut inferred,
                &tc.checked.mono_struct_args,
            )?;
            &mut named_args[1..]
        } else {
            &mut named_args[..]
        };
        for na in rest.iter_mut() {
            let Some((_, fpat)) = field_pats.iter().find(|(n, _)| n == &na.name) else {
                return Err(format!("Type '{}' has no field '{}'", base_name, na.name));
            };
            let exp = substitute_type_partial(fpat, &inferred, tc.checked);
            let exp_opt = if exp == Type::Any { None } else { Some(&exp) };
            tc.check_expr(&mut na.value, exp_opt)?;
            unify_type(
                fpat,
                &na.value.ty,
                &tmpl.type_params,
                &mut inferred,
                &tc.checked.mono_struct_args,
            )?;
        }
    } else {
        if args.len() != field_pats.len() {
            return Err(format!(
                "'{}' expects {} arguments, got {}",
                base_name,
                field_pats.len(),
                args.len()
            ));
        }
        for (arg, (_, fpat)) in args.iter_mut().zip(field_pats.iter()) {
            let exp = substitute_type_partial(fpat, &inferred, tc.checked);
            let exp_opt = if exp == Type::Any { None } else { Some(&exp) };
            tc.check_expr(arg, exp_opt)?;
            unify_type(
                fpat,
                &arg.ty,
                &tmpl.type_params,
                &mut inferred,
                &tc.checked.mono_struct_args,
            )?;
        }
    }
    let mut concrete = Vec::with_capacity(tmpl.type_params.len());
    for tp in &tmpl.type_params {
        let Some(ty) = inferred.get(tp) else {
            return Err(format!("Cannot infer type parameter '{}' for struct '{}'", tp, base_name));
        };
        concrete.push(ty.clone());
    }
    ensure_monomorphized_struct(tc.checked, base_name, &concrete)
}

pub(crate) fn check_generic_func_call(
    tc: &mut Checker<'_>,
    callee: &mut Expr,
    fn_name: &str,
    type_args: &mut Vec<TypeExpr>,
    args: &mut [Expr],
    named_args: &[NamedArg],
) -> Result<Type, String> {
    if !named_args.is_empty() {
        return Err("Named arguments are only supported on type constructors".to_string());
    }
    let tmpl = tc
        .checked
        .generic_funcs
        .get(fn_name)
        .cloned()
        .ok_or_else(|| format!("Unknown generic function '{}'", fn_name))?;
    if args.len() != tmpl.params.len() {
        return Err(format!(
            "Expected {} arguments, got {}",
            tmpl.params.len(),
            args.len()
        ));
    }
    let mut inferred: HashMap<String, Type> = HashMap::new();
    if !type_args.is_empty() {
        if type_args.len() != tmpl.type_params.len() {
            return Err(format!(
                "Generic function '{}' expects {} type argument(s), got {}",
                fn_name,
                tmpl.type_params.len(),
                type_args.len()
            ));
        }
        for (tp, ta) in tmpl.type_params.iter().zip(type_args.iter()) {
            inferred.insert(tp.clone(), tc.checked.resolve_type_mut(ta)?);
        }
    }

    let is_unannotated_closure = |e: &Expr| {
        matches!(&e.kind, ExprKind::Closure { params, .. } if params.iter().any(|p| matches!(&p.ty, TypeExpr::Named(n) if n == "_")))
    };
    let mut deferred = Vec::new();
    for (i, (arg, ppat)) in args.iter_mut().zip(tmpl.params.iter()).enumerate() {
        if is_unannotated_closure(arg) {
            deferred.push(i);
            continue;
        }
        let exp = substitute_type_partial(ppat, &inferred, tc.checked);
        let exp_opt = if exp == Type::Any { None } else { Some(&exp) };
        tc.check_expr(arg, exp_opt)?;
        unify_type(
            ppat,
            &arg.ty,
            &tmpl.type_params,
            &mut inferred,
            &tc.checked.mono_struct_args,
        )?;
    }
    for i in deferred {
        let ppat = &tmpl.params[i];
        let exp = substitute_type_partial(ppat, &inferred, tc.checked);
        let exp_opt = if exp == Type::Any { None } else { Some(&exp) };
        tc.check_expr(&mut args[i], exp_opt)?;
        unify_type(
            ppat,
            &args[i].ty,
            &tmpl.type_params,
            &mut inferred,
            &tc.checked.mono_struct_args,
        )?;
    }

    let mut concrete_args = Vec::with_capacity(tmpl.type_params.len());
    for tp in &tmpl.type_params {
        let Some(ty) = inferred.get(tp) else {
            return Err(format!(
                "Cannot infer type parameter '{}' for function '{}'",
                tp, fn_name
            ));
        };
        concrete_args.push(ty.clone());
    }
    let mut concrete_params = Vec::with_capacity(tmpl.params.len());
    for (arg, ppat) in args.iter_mut().zip(tmpl.params.iter()) {
        let cty = substitute_type(ppat, &inferred, tc.checked)?;
        if matches!(&arg.kind, ExprKind::List(es) if es.is_empty()) {
            arg.ty = cty.clone();
        }
        if !arg.ty.is_assignable_to(&cty) {
            return Err(format!(
                "Argument type mismatch: expected {}, got {}",
                cty, arg.ty
            ));
        }
        concrete_params.push(cty);
    }
    let ret_ty = substitute_type(&tmpl.return_type, &inferred, tc.checked)?;
    let mono_name = ensure_monomorphized_func(tc.checked, fn_name, &concrete_args)?;
    callee.kind = ExprKind::Ident(mono_name);
    callee.ty = Type::Function {
        params: concrete_params,
        return_type: Box::new(ret_ty.clone()),
    };
    type_args.clear();
    Ok(ret_ty)
}

pub(crate) fn resolve_switch_pattern_type(
    name: &str,
    scrutinee_ty: &Type,
    checked: &CheckedProgram,
) -> Result<Type, String> {
    if let Ok(resolved) = resolve_type_expr(&TypeExpr::Named(name.to_string()), checked) {
        return Ok(resolved);
    }
    if !name.contains('<') {
        let prefix = format!("{}<", name);
        let variants: &[Type] = match scrutinee_ty {
            Type::Union(vs) => vs.as_slice(),
            other => std::slice::from_ref(other),
        };
        let mut matches: Vec<Type> = Vec::new();
        for v in variants {
            if let Type::Custom(tid) = v
                && tid.module == ModuleId::Program
                && tid.name.starts_with(&prefix)
                && !matches.contains(v)
            {
                matches.push(v.clone());
            }
        }
        if matches.len() == 1 {
            return Ok(matches.into_iter().next().unwrap());
        }
        if matches.len() > 1 {
            return Err(format!(
                "Ambiguous generic type pattern '{}' in switch; specify type arguments",
                name
            ));
        }
    }
    Err(format!("Unknown type '{}'", name))
}
