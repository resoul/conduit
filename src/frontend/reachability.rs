use std::collections::{HashMap, HashSet, VecDeque};

use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::linker::{bind_pat_names, is_shadowed};
use crate::frontend::types::{ModuleId, Type};

/// Remove functions unreachable from entry points (`export`, `main`, or explicit roots).
pub fn shake_unreachable_funcs(
    program: &mut Program,
    checked: &mut CheckedProgram,
    explicit_roots: Option<&[&str]>,
) -> Result<(), String> {
    let mut export_list_funcs = HashSet::new();
    for item in &program.items {
        if let ItemKind::ExportList { names } = &item.kind {
            for spec in names {
                if !checked.func_by_name.contains_key(&spec.name)
                    && !checked.struct_by_name.contains_key(&spec.name)
                    && !checked.generic_structs.contains_key(&spec.name)
                    && !checked.generic_funcs.contains_key(&spec.name)
                {
                    return Err(format!("Cannot export unknown symbol '{}'", spec.name));
                }
                if checked.func_by_name.contains_key(&spec.name) {
                    export_list_funcs.insert(spec.name.clone());
                }
            }
        }
    }

    let mut seeds = Vec::new();
    if let Some(roots) = explicit_roots {
        for &r in roots {
            if !checked.func_by_name.contains_key(r) {
                return Err(format!("Unknown entry function '{}'", r));
            }
            seeds.push(r.to_string());
        }
    } else {
        let mut entry_roots = Vec::new();
        let mut root_file_funcs = Vec::new();
        for item in &program.items {
            if let ItemKind::Func { name, .. } = &item.kind {
                if (!item.is_imported && (item.is_exported || name == "main"))
                    || export_list_funcs.contains(name)
                {
                    entry_roots.push(name.clone());
                }
                if !item.is_imported {
                    root_file_funcs.push(name.clone());
                }
            }
        }
        seeds = if entry_roots.is_empty() {
            root_file_funcs
        } else {
            entry_roots
        };
    }

    let mut func_bodies: HashMap<&str, (&[Param], &Block)> = HashMap::new();
    let mut struct_fields: HashMap<&str, &[FieldDef]> = HashMap::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Func {
                name, params, body, ..
            } => {
                func_bodies.insert(name.as_str(), (params.as_slice(), body));
            }
            ItemKind::Struct { name, fields, .. } => {
                struct_fields.insert(name.as_str(), fields.as_slice());
            }
            _ => {}
        }
    }

    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for seed in seeds {
        if reachable.insert(seed.clone()) {
            queue.push_back(seed);
        }
    }
    for (_, routes) in &checked.cli_dispatches {
        for r in routes {
            if reachable.insert(r.handler_name.clone()) {
                queue.push_back(r.handler_name.clone());
            }
        }
    }
    let mut extra_validators: Vec<String> = Vec::new();
    for (_, entries) in &checked.http_router_plans {
        for entry in entries {
            if !entry.handler_name.is_empty() && reachable.insert(entry.handler_name.clone()) {
                queue.push_back(entry.handler_name.clone());
            }
            if let Some(sname) = &entry.body_struct {
                let body_ty = crate::frontend::types::Type::Custom(
                    crate::frontend::types::TypeId::program(sname),
                );
                let mut visited: HashSet<&str> = HashSet::new();
                collect_validated_type_refs(
                    &body_ty,
                    checked,
                    &struct_fields,
                    &mut visited,
                    &mut extra_validators,
                );
            }
        }
    }
    for vname in extra_validators {
        if reachable.insert(vname.clone()) {
            queue.push_back(vname);
        }
    }
    for (_, entries) in &checked.cron_schedule_plans {
        for entry in entries {
            if reachable.insert(entry.handler_name.clone()) {
                queue.push_back(entry.handler_name.clone());
            }
        }
    }
    for item in &program.items {
        if let ItemKind::Migration { up, down, .. } = &item.kind {
            let mut up_scopes = vec![HashSet::from(["db".to_string()])];
            let mut refs = Vec::new();
            collect_block_refs(up, checked, &struct_fields, &mut up_scopes, &mut refs);
            let mut down_scopes = vec![HashSet::from(["db".to_string()])];
            collect_block_refs(down, checked, &struct_fields, &mut down_scopes, &mut refs);
            for target in refs {
                if reachable.insert(target.clone()) {
                    queue.push_back(target);
                }
            }
        }
    }

    while let Some(fname) = queue.pop_front() {
        let Some(&(params, body)) = func_bodies.get(fname.as_str()) else {
            continue;
        };
        let mut scopes = vec![params.iter().map(|p| p.name.clone()).collect()];
        let mut refs = Vec::new();
        collect_block_refs(body, checked, &struct_fields, &mut scopes, &mut refs);
        for target in refs {
            if reachable.insert(target.clone()) {
                queue.push_back(target);
            }
        }
    }

    program.items.retain(|item| match &item.kind {
        ItemKind::Func { name, .. } => reachable.contains(name),
        _ => true,
    });

    let old_funcs = std::mem::take(&mut checked.funcs);
    let old_by_name = std::mem::take(&mut checked.func_by_name);
    for item in &program.items {
        if let ItemKind::Func { name, .. } = &item.kind
            && let Some(&old_idx) = old_by_name.get(name)
        {
            let new_idx = checked.funcs.len() as u32;
            checked.funcs.push(old_funcs[old_idx as usize].clone());
            checked.func_by_name.insert(name.clone(), new_idx);
        }
    }
    Ok(())
}

fn collect_block_refs(
    block: &Block,
    checked: &CheckedProgram,
    struct_fields: &HashMap<&str, &[FieldDef]>,
    scopes: &mut Vec<HashSet<String>>,
    out: &mut Vec<String>,
) {
    scopes.push(HashSet::new());
    for s in &block.stmts {
        match &s.kind {
            StmtKind::Define { name, value } => {
                collect_expr_refs(value, checked, struct_fields, scopes, out);
                if let Some(top) = scopes.last_mut() {
                    top.insert(name.clone());
                }
            }
            StmtKind::DestructureDefine { pattern, value } => {
                collect_expr_refs(value, checked, struct_fields, scopes, out);
                if let Some(top) = scopes.last_mut() {
                    bind_pat_names(pattern, top);
                }
            }
            StmtKind::VarDecl { name, init, .. } => {
                if let Some(e) = init {
                    collect_expr_refs(e, checked, struct_fields, scopes, out);
                }
                if let Some(top) = scopes.last_mut() {
                    top.insert(name.clone());
                }
            }
            StmtKind::Assign { target, value } => {
                collect_expr_refs(target, checked, struct_fields, scopes, out);
                collect_expr_refs(value, checked, struct_fields, scopes, out);
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => {
                collect_expr_refs(e, checked, struct_fields, scopes, out);
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                collect_expr_refs(condition, checked, struct_fields, scopes, out);
                collect_block_refs(then_block, checked, struct_fields, scopes, out);
                if let Some(eb) = else_block {
                    collect_block_refs(eb, checked, struct_fields, scopes, out);
                }
            }
            StmtKind::Guard {
                condition,
                else_block,
            } => {
                collect_expr_refs(condition, checked, struct_fields, scopes, out);
                collect_block_refs(else_block, checked, struct_fields, scopes, out);
            }
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            } => {
                collect_expr_refs(iterable, checked, struct_fields, scopes, out);
                let mut loop_scope = HashSet::new();
                bind_pat_names(pattern, &mut loop_scope);
                scopes.push(loop_scope);
                collect_block_refs(body, checked, struct_fields, scopes, out);
                scopes.pop();
            }
            StmtKind::While { condition, body } => {
                collect_expr_refs(condition, checked, struct_fields, scopes, out);
                collect_block_refs(body, checked, struct_fields, scopes, out);
            }
            StmtKind::Switch { expr, cases } => {
                collect_expr_refs(expr, checked, struct_fields, scopes, out);
                for c in cases {
                    let mut cs = HashSet::new();
                    bind_pat_names(&c.pattern, &mut cs);
                    scopes.push(cs);
                    collect_block_refs(&c.body, checked, struct_fields, scopes, out);
                    scopes.pop();
                }
            }
        }
    }
    scopes.pop();
}

fn collect_expr_refs(
    expr: &Expr,
    checked: &CheckedProgram,
    struct_fields: &HashMap<&str, &[FieldDef]>,
    scopes: &mut Vec<HashSet<String>>,
    out: &mut Vec<String>,
) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            if !is_shadowed(scopes, name) && checked.func_by_name.contains_key(name) {
                out.push(name.clone());
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_expr_refs(left, checked, struct_fields, scopes, out);
            collect_expr_refs(right, checked, struct_fields, scopes, out);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Try(inner)
        | ExprKind::Member { target: inner, .. }
        | ExprKind::OptionalMember { target: inner, .. } => {
            collect_expr_refs(inner, checked, struct_fields, scopes, out);
        }
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                collect_expr_refs(e, checked, struct_fields, scopes, out);
            }
        }
        ExprKind::Call {
            callee,
            args,
            named_args,
            ..
        } => {
            collect_expr_refs(callee, checked, struct_fields, scopes, out);
            for a in args {
                collect_expr_refs(a, checked, struct_fields, scopes, out);
            }
            for na in named_args {
                collect_expr_refs(&na.value, checked, struct_fields, scopes, out);
            }
            if let Some(arg0) = args.first()
                && is_validation_check_callee(callee, checked, scopes)
            {
                let mut visited = HashSet::new();
                collect_validated_type_refs(
                    &arg0.ty,
                    checked,
                    struct_fields,
                    &mut visited,
                    out,
                );
            }
        }
        ExprKind::Index { target, index } => {
            collect_expr_refs(target, checked, struct_fields, scopes, out);
            collect_expr_refs(index, checked, struct_fields, scopes, out);
        }
        ExprKind::Closure { params, body, .. } => {
            scopes.push(params.iter().map(|p| p.name.clone()).collect());
            collect_block_refs(body, checked, struct_fields, scopes, out);
            scopes.pop();
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil => {}
    }
}

fn is_validation_check_callee(
    callee: &Expr,
    checked: &CheckedProgram,
    scopes: &[HashSet<String>],
) -> bool {
    match &callee.kind {
        ExprKind::Member { target, field } => {
            field == "check"
                && matches!(&target.kind, ExprKind::Ident(alias) if !is_shadowed(scopes, alias) && checked.is_native_alias(alias, "cdt.validation"))
        }
        ExprKind::Ident(name) => {
            !is_shadowed(scopes, name)
                && checked.native_direct_fn(name, "cdt.validation") == Some("check")
        }
        _ => false,
    }
}

fn collect_validated_type_refs<'a>(
    ty: &'a Type,
    checked: &'a CheckedProgram,
    struct_fields: &HashMap<&str, &[FieldDef]>,
    visited: &mut HashSet<&'a str>,
    out: &mut Vec<String>,
) {
    match ty {
        Type::Optional(inner) | Type::List(inner) => {
            collect_validated_type_refs(inner, checked, struct_fields, visited, out);
        }
        Type::Map(_, val) => {
            collect_validated_type_refs(val, checked, struct_fields, visited, out);
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                collect_validated_type_refs(e, checked, struct_fields, visited, out);
            }
        }
        Type::Custom(tid) if tid.module == ModuleId::Program => {
            if !visited.insert(tid.name.as_str()) {
                return;
            }
            if let Some(fields) = struct_fields.get(tid.name.as_str()) {
                for f in *fields {
                    for ann in &f.annotations {
                        if checked.func_by_name.contains_key(&ann.target) {
                            out.push(ann.target.clone());
                        }
                    }
                }
            }
            if let Some(&sid) = checked.struct_by_name.get(&tid.name) {
                for rf in &checked.structs[sid].fields {
                    collect_validated_type_refs(&rf.ty, checked, struct_fields, visited, out);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn expand_program_aliases(
    program: &mut Program,
    aliases: &HashMap<String, TypeExpr>,
) -> Result<(), String> {
    for item in &mut program.items {
        match &mut item.kind {
            ItemKind::Struct { fields, .. } | ItemKind::Error { fields, .. } => {
                for f in fields {
                    expand_type_expr(&mut f.ty, aliases, 0)?;
                }
            }
            ItemKind::Func {
                params,
                return_type,
                body,
                ..
            } => {
                for p in params {
                    expand_type_expr(&mut p.ty, aliases, 0)?;
                }
                if let Some(rt) = return_type {
                    expand_type_expr(rt, aliases, 0)?;
                }
                expand_block_aliases(body, aliases)?;
            }
            ItemKind::TypeAlias { .. }
            | ItemKind::Const { .. }
            | ItemKind::Enum { .. }
            | ItemKind::Import { .. }
            | ItemKind::ExportList { .. }
            | ItemKind::Migration { .. } => {}
        }
    }
    Ok(())
}

fn expand_type_expr(
    te: &mut TypeExpr,
    aliases: &HashMap<String, TypeExpr>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("Cyclic type alias involving {:?}", te));
    }
    match te {
        TypeExpr::Named(name) => {
            if let Some(target) = aliases.get(name) {
                *te = target.clone();
                expand_type_expr(te, aliases, depth + 1)?;
            }
        }
        TypeExpr::Generic { type_args, .. } => {
            for a in type_args {
                expand_type_expr(a, aliases, depth)?;
            }
        }
        TypeExpr::List(inner) | TypeExpr::Optional(inner) => {
            expand_type_expr(inner, aliases, depth)?;
        }
        TypeExpr::Union(vars) | TypeExpr::Tuple(vars) => {
            for v in vars {
                expand_type_expr(v, aliases, depth)?;
            }
        }
        TypeExpr::Func {
            params,
            return_type,
        } => {
            for p in params {
                expand_type_expr(p, aliases, depth)?;
            }
            expand_type_expr(return_type, aliases, depth)?;
        }
    }
    Ok(())
}

fn expand_block_aliases(
    block: &mut Block,
    aliases: &HashMap<String, TypeExpr>,
) -> Result<(), String> {
    for s in &mut block.stmts {
        match &mut s.kind {
            StmtKind::Define { value, .. }
            | StmtKind::DestructureDefine { value, .. }
            | StmtKind::Expr(value) => {
                expand_expr_aliases(value, aliases)?;
            }
            StmtKind::VarDecl { ty, init, .. } => {
                if let Some(te) = ty {
                    expand_type_expr(te, aliases, 0)?;
                }
                if let Some(e) = init {
                    expand_expr_aliases(e, aliases)?;
                }
            }
            StmtKind::Assign { target, value } => {
                expand_expr_aliases(target, aliases)?;
                expand_expr_aliases(value, aliases)?;
            }
            StmtKind::Return(Some(e)) => expand_expr_aliases(e, aliases)?,
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                expand_expr_aliases(condition, aliases)?;
                expand_block_aliases(then_block, aliases)?;
                if let Some(eb) = else_block {
                    expand_block_aliases(eb, aliases)?;
                }
            }
            StmtKind::Guard {
                condition,
                else_block,
            } => {
                expand_expr_aliases(condition, aliases)?;
                expand_block_aliases(else_block, aliases)?;
            }
            StmtKind::ForIn { iterable, body, .. } => {
                expand_expr_aliases(iterable, aliases)?;
                expand_block_aliases(body, aliases)?;
            }
            StmtKind::While { condition, body } => {
                expand_expr_aliases(condition, aliases)?;
                expand_block_aliases(body, aliases)?;
            }
            StmtKind::Switch { expr, cases } => {
                expand_expr_aliases(expr, aliases)?;
                for c in cases {
                    expand_block_aliases(&mut c.body, aliases)?;
                }
            }
        }
    }
    Ok(())
}

fn expand_expr_aliases(
    expr: &mut Expr,
    aliases: &HashMap<String, TypeExpr>,
) -> Result<(), String> {
    match &mut expr.kind {
        ExprKind::Binary { left, right, .. } => {
            expand_expr_aliases(left, aliases)?;
            expand_expr_aliases(right, aliases)?;
        }
        ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => {
            expand_expr_aliases(inner, aliases)?;
        }
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                expand_expr_aliases(e, aliases)?;
            }
        }
        ExprKind::Call {
            callee,
            type_args,
            args,
            named_args,
        } => {
            expand_expr_aliases(callee, aliases)?;
            for t in type_args {
                expand_type_expr(t, aliases, 0)?;
            }
            for a in args {
                expand_expr_aliases(a, aliases)?;
            }
            for na in named_args {
                expand_expr_aliases(&mut na.value, aliases)?;
            }
        }
        ExprKind::Member { target, .. } | ExprKind::OptionalMember { target, .. } => {
            expand_expr_aliases(target, aliases)?;
        }
        ExprKind::Index { target, index } => {
            expand_expr_aliases(target, aliases)?;
            expand_expr_aliases(index, aliases)?;
        }
        ExprKind::Closure {
            params,
            return_type,
            body,
        } => {
            for p in params {
                expand_type_expr(&mut p.ty, aliases, 0)?;
            }
            if let Some(rt) = return_type {
                expand_type_expr(rt, aliases, 0)?;
            }
            expand_block_aliases(body, aliases)?;
        }
        _ => {}
    }
    Ok(())
}
