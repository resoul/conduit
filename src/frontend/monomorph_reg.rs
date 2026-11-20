use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, DataEnumVariant, type_to_json_type};
use crate::frontend::monomorph::{GenericFuncTemplate, GenericStructTemplate};
use crate::frontend::monomorph_subst::resolve_type_expr_with_scope;
use crate::frontend::reachability::expand_program_aliases;
use crate::frontend::types::{FuncSig, ResolvedField, ResolvedStruct, Type};
use crate::native::{StructFieldDef, StructSchema};

pub(crate) fn check_unique_type_params(type_params: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for tp in type_params {
        if !seen.insert(tp.as_str()) {
            return Err(format!("Duplicate type parameter '{}'", tp));
        }
    }
    Ok(())
}

pub(crate) fn register_program_types(
    program: &mut Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    let mut struct_defs: Vec<(usize, String, Vec<FieldDef>)> = Vec::new();
    let mut enum_tags: HashMap<usize, String> = HashMap::new();
    let mut aliases: HashMap<String, TypeExpr> = HashMap::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Struct { name, type_params, fields } => {
                if let Some(doc) = &item.doc {
                    checked.struct_abouts.insert(name.clone(), doc.clone());
                }
                if !type_params.is_empty() {
                    check_unique_type_params(type_params)?;
                    if checked.struct_by_name.contains_key(name)
                        || checked
                            .generic_structs
                            .insert(
                                name.clone(),
                                GenericStructTemplate {
                                    name: name.clone(),
                                    type_params: type_params.clone(),
                                    fields: fields.clone(),
                                },
                            )
                            .is_some()
                    {
                        return Err(format!("Duplicate struct '{}'", name));
                    }
                } else {
                    if checked.generic_structs.contains_key(name) {
                        return Err(format!("Duplicate struct '{}'", name));
                    }
                    let id = struct_defs.len();
                    if checked.struct_by_name.insert(name.clone(), id).is_some() {
                        return Err(format!("Duplicate struct '{}'", name));
                    }
                    struct_defs.push((id, name.clone(), fields.clone()));
                }
            }
            ItemKind::Error { name, fields } => {
                if checked.generic_structs.contains_key(name) {
                    return Err(format!("Duplicate struct '{}'", name));
                }
                let id = struct_defs.len();
                if checked.struct_by_name.insert(name.clone(), id).is_some() {
                    return Err(format!("Duplicate error type '{}'", name));
                }
                checked.error_structs.insert(name.clone());
                struct_defs.push((id, name.clone(), fields.clone()));
            }
            ItemKind::TypeAlias { name, target }
                if aliases.insert(name.clone(), target.clone()).is_some() =>
            {
                return Err(format!("Duplicate type alias '{}'", name));
            }
            ItemKind::Enum { name, variants } => {
                if checked.enums.contains_key(name)
                    || checked.data_enums.contains_key(name)
                    || checked.struct_by_name.contains_key(name)
                    || checked.generic_structs.contains_key(name)
                {
                    return Err(format!("Duplicate type '{}'", name));
                }
                let mut seen = HashSet::new();
                for v in variants {
                    if !seen.insert(v.name.clone()) {
                        return Err(format!("Duplicate variant '{}' in enum '{}'", v.name, name));
                    }
                    for f in &v.fields {
                        let eff = f.json_name.as_deref().unwrap_or(&f.name);
                        if f.name == "type" || eff == "type" {
                            return Err(format!(
                                "Field '{}' in enum variant '{}.{}' cannot use reserved name 'type'",
                                f.name, name, v.name
                            ));
                        }
                    }
                }
                if variants.iter().all(|v| v.fields.is_empty()) {
                    checked.enums.insert(
                        name.clone(),
                        variants.iter().map(|v| v.name.clone()).collect(),
                    );
                } else {
                    let mut dvars = Vec::with_capacity(variants.len());
                    for v in variants {
                        let sname = format!("__enum_{}__{}", name, v.name);
                        let id = struct_defs.len();
                        if checked.struct_by_name.insert(sname.clone(), id).is_some() {
                            return Err(format!("Duplicate synthetic enum variant '{}'", sname));
                        }
                        enum_tags.insert(id, v.name.clone());
                        struct_defs.push((id, sname.clone(), v.fields.clone()));
                        dvars.push(DataEnumVariant {
                            name: v.name.clone(),
                            struct_name: sname,
                            schema_id: id as u32,
                        });
                    }
                    checked.data_enums.insert(name.clone(), dvars);
                }
            }
            _ => {}
        }
    }

    for alias_name in aliases.keys() {
        if checked.struct_by_name.contains_key(alias_name)
            || checked.generic_structs.contains_key(alias_name)
        {
            return Err(format!("Duplicate type '{}'", alias_name));
        }
    }
    if !aliases.is_empty() {
        expand_program_aliases(program, &aliases)?;
        struct_defs.clear();
        for item in &program.items {
            match &item.kind {
                ItemKind::Struct { name, type_params, fields } => {
                    if type_params.is_empty() {
                        let id = checked.struct_by_name[name];
                        struct_defs.push((id, name.clone(), fields.clone()));
                    } else if let Some(tmpl) = checked.generic_structs.get_mut(name) {
                        tmpl.fields = fields.clone();
                    }
                }
                ItemKind::Error { name, fields } => {
                    let id = checked.struct_by_name[name];
                    struct_defs.push((id, name.clone(), fields.clone()));
                }
                ItemKind::Enum { name, variants } => {
                    if let Some(dvars) = checked.data_enums.get(name) {
                        for (v, dv) in variants.iter().zip(dvars.iter()) {
                            let id = checked.struct_by_name[&dv.struct_name];
                            struct_defs.push((id, dv.struct_name.clone(), v.fields.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    for (id, name, _) in &struct_defs {
        checked.structs.push(ResolvedStruct {
            schema_id: *id as u32,
            name: name.clone(),
            db_table: None,
            fields: Vec::new(),
            scalar_len: 0,
            obj_len: 0,
        });
        checked
            .struct_schemas
            .push(StructSchema::new(*id as u32, name, Vec::new()));
    }
    for (id, name, fields) in &struct_defs {
        let mut resolved_fields = Vec::with_capacity(fields.len());
        let mut scalar_len: u16 = 0;
        let mut obj_len: u16 = 0;
        let mut schema_fields = Vec::with_capacity(fields.len());
        for f in fields {
            let ty = checked.resolve_type_mut(&f.ty)?;
            let is_scalar = ty.is_scalar();
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
            if let Some(jty) = type_to_json_type(&ty, checked) {
                schema_fields.push(StructFieldDef {
                    name: f.name.clone(),
                    json_name: json_name.clone(),
                    ty: jty,
                });
            }
            let mut env_name = None;
            for ann in &f.annotations {
                if ann.target != "env" {
                    continue;
                }
                if env_name.is_some() {
                    return Err(format!("Duplicate @env annotation on '{}.{}'", name, f.name));
                }
                env_name = Some(match ann.args.as_slice() {
                    [] => f.name.to_ascii_uppercase(),
                    [crate::frontend::ast::Expr {
                        kind: crate::frontend::ast::ExprKind::String(s),
                        ..
                    }] if !s.is_empty() => s.clone(),
                    _ => {
                        return Err(format!(
                            "@env on '{}.{}' expects 0 arguments or 1 non-empty String literal",
                            name, f.name
                        ));
                    }
                });
            }
            let cli_ann = f.annotations.iter().find(|ann| {
                (ann.target.starts_with("cli.") && ann.target != "cli.handler")
                    || matches!(ann.target.as_str(), "flag" | "opt" | "arg" | "rest" | "cmd" | "group")
            }).cloned();
            let cli_handler = f.annotations.iter().find_map(|ann| {
                if ann.target == "cli.handler"
                    && let Some(Expr { kind: ExprKind::Ident(h), .. }) = ann.args.first()
                {
                    return Some(h.clone());
                }
                None
            });
            let router_ann = f
                .annotations
                .iter()
                .find(|ann| ann.target.starts_with("router.") || ann.target.starts_with("schedule."))
                .cloned();
            let db_anns: Vec<FieldAnnotation> = f
                .annotations
                .iter()
                .filter(|ann| ann.target.starts_with("db."))
                .cloned()
                .collect();
            resolved_fields.push(ResolvedField {
                name: f.name.clone(),
                json_name,
                ty,
                is_scalar,
                slot_idx,
                env_name,
                cli_ann,
                cli_handler,
                router_ann,
                db_anns,
                default: None,
            });
        }
        let mut db_table = None;
        let mut is_http_query = false;
        let mut http_status: Option<u16> = None;
        for it in &program.items {
            let matched_item = match &it.kind {
                ItemKind::Struct { name: sname, .. } if sname == name => true,
                ItemKind::Error { name: ename, .. } if ename == name => true,
                _ => false,
            };
            if !matched_item {
                continue;
            }
            for ann in &it.annotations {
                match ann.target.as_str() {
                    "db.table" => {
                        if let Some(Expr { kind: ExprKind::String(t), .. }) = ann.args.first() {
                            db_table = Some(t.clone());
                        }
                    }
                    "http.query" => {
                        if matches!(it.kind, ItemKind::Error { .. }) {
                            return Err(format!("@http.query cannot be applied to error '{}'", name));
                        }
                        if !ann.args.is_empty() {
                            return Err(format!("@http.query on '{}' takes no arguments", name));
                        }
                        is_http_query = true;
                    }
                    "http.status" => {
                        if ann.args.len() != 1 {
                            return Err(format!(
                                "@http.status on '{}' expects 1 integer status code (100..=599)",
                                name
                            ));
                        }
                        let Some(Expr { kind: ExprKind::Int(code), .. }) = ann.args.first() else {
                            return Err(format!(
                                "@http.status on '{}' expects an integer literal status code",
                                name
                            ));
                        };
                        if !(100..=599).contains(code) {
                            return Err(format!(
                                "@http.status({}) on '{}' must be between 100 and 599",
                                code, name
                            ));
                        }
                        http_status = Some(*code as u16);
                    }
                    _ => {}
                }
            }
            break;
        }
        if is_http_query {
            checked.http_query_structs.insert(name.clone());
        }
        if let Some(code) = http_status {
            checked.http_status_by_struct.insert(name.clone(), code);
        }
        checked.structs[*id] = ResolvedStruct {
            schema_id: *id as u32,
            name: name.clone(),
            db_table,
            fields: resolved_fields,
            scalar_len,
            obj_len,
        };
        let mut schema = StructSchema::with_flags(
            *id as u32,
            name,
            schema_fields,
            checked.error_structs.contains(name),
            enum_tags.get(id).cloned(),
        );
        schema.http_status = http_status;
        checked.struct_schemas[*id] = schema;
    }

    let tmpl_list: Vec<(Vec<String>, Vec<FieldDef>)> = checked
        .generic_structs
        .values()
        .map(|t| (t.type_params.clone(), t.fields.clone()))
        .collect();
    for (tps, fields) in tmpl_list {
        let tp_set: HashSet<String> = tps.into_iter().collect();
        for f in fields {
            resolve_type_expr_with_scope(&f.ty, checked, &tp_set)?;
        }
    }

    for item in &program.items {
        if let ItemKind::Func { name, type_params, params, return_type, .. } = &item.kind {
            if checked.native_direct_fns.contains_key(name) {
                return Err(format!("Function '{}' conflicts with imported identifier", name));
            }
            if !type_params.is_empty() {
                check_unique_type_params(type_params)?;
                let tp_set: HashSet<String> = type_params.iter().cloned().collect();
                let mut p_tys = Vec::with_capacity(params.len());
                for p in params {
                    p_tys.push(resolve_type_expr_with_scope(&p.ty, checked, &tp_set)?);
                }
                let ret_ty = match return_type {
                    Some(rt) => resolve_type_expr_with_scope(rt, checked, &tp_set)?,
                    None => Type::Void,
                };
                if checked.func_by_name.contains_key(name)
                    || checked
                        .generic_funcs
                        .insert(
                            name.clone(),
                            GenericFuncTemplate {
                                type_params: type_params.clone(),
                                params: p_tys,
                                return_type: ret_ty,
                                item: item.clone(),
                            },
                        )
                        .is_some()
                {
                    return Err(format!("Duplicate function '{}'", name));
                }
                continue;
            }
            if checked.generic_funcs.contains_key(name) {
                return Err(format!("Duplicate function '{}'", name));
            }
            let fid = checked.funcs.len() as u32;
            if checked.func_by_name.insert(name.clone(), fid).is_some() {
                return Err(format!("Duplicate function '{}'", name));
            }
            let mut p_tys = Vec::with_capacity(params.len());
            for p in params {
                p_tys.push(checked.resolve_type_mut(&p.ty)?);
            }
            let ret_ty = match return_type {
                Some(rt) => checked.resolve_type_mut(rt)?,
                None => Type::Void,
            };
            let defaults = vec![None; p_tys.len()];
            checked.funcs.push(FuncSig {
                name: name.clone(),
                params: p_tys,
                defaults,
                return_type: ret_ty,
            });
        } else if let ItemKind::Job(j) = &item.kind {
            if checked.generic_funcs.contains_key(&j.name) {
                return Err(format!("Duplicate function '{}'", j.name));
            }
            let fid = checked.funcs.len() as u32;
            if checked.func_by_name.insert(j.name.clone(), fid).is_some() {
                return Err(format!("Duplicate function '{}'", j.name));
            }
            let ret_ty = match &j.return_type {
                Some(rt) => checked.resolve_type(rt)?,
                None => Type::Void,
            };
            checked.funcs.push(FuncSig {
                name: j.name.clone(),
                params: Vec::new(),
                defaults: Vec::new(),
                return_type: ret_ty,
            });
        } else if let ItemKind::Query(q) = &item.kind {
            let name = &q.name;
            let params = &q.params;
            let return_type = &q.return_type;
            let model_name = &q.model_name;
            let mode = q.mode;
            if checked.generic_funcs.contains_key(name) {
                return Err(format!("Duplicate function '{}'", name));
            }
            let fid = checked.funcs.len() as u32;
            if checked.func_by_name.insert(name.clone(), fid).is_some() {
                return Err(format!("Duplicate function '{}'", name));
            }
            let mut p_tys = Vec::with_capacity(params.len());
            for p in params {
                p_tys.push(checked.resolve_type_mut(&p.ty)?);
            }
            let (base_ret, full_ret) = query_return_types(model_name, mode);
            let ret_ty = match return_type {
                Some(rt) => {
                    let resolved = checked.resolve_type_mut(rt)?;
                    if resolved == base_ret {
                        full_ret
                    } else {
                        resolved
                    }
                }
                None => full_ret,
            };
            let defaults = vec![None; p_tys.len()];
            checked.funcs.push(FuncSig {
                name: name.clone(),
                params: p_tys,
                defaults,
                return_type: ret_ty,
            });
        }
    }
    Ok(())
}

pub(crate) fn query_return_types(model_name: &str, mode: QueryModeAst) -> (Type, Type) {
    use crate::frontend::types::TypeId;
    let model_ty = Type::Custom(TypeId::program(model_name));
    let base = match mode {
        QueryModeAst::All => Type::List(Box::new(model_ty)),
        QueryModeAst::First => Type::Optional(Box::new(model_ty)),
        QueryModeAst::Count | QueryModeAst::Delete => Type::Int,
        QueryModeAst::Exists => Type::Bool,
    };
    let db_err = Type::Custom(TypeId::native("cdt.db", "DbError"));
    let full = Type::union(vec![base.clone(), db_err]);
    (base, full)
}
