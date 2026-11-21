use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::parse_source;

fn is_native_module(path: &str) -> bool {
    crate::frontend::check_native::is_native_module(path)
}

struct LinkState<'a> {
    modules: &'a HashMap<&'a str, &'a str>,
    module_paths: &'a HashMap<&'a str, String>,
    loaded: HashMap<String, (HashMap<String, String>, HashSet<String>)>,
    visiting: HashSet<String>,
    used_names: HashSet<String>,
    preferred: HashMap<(String, String), String>,
    merged_items: Vec<Item>,
    next_mod_id: usize,
}

struct RewriteEnv {
    sym_map: HashMap<String, String>,
    ns_map: HashMap<String, HashMap<String, String>>,
    ns_all_defs: HashMap<String, HashSet<String>>,
    nat_ns_rename: HashMap<String, String>,
}

fn resolve_module<'a>(
    modules: &'a HashMap<&'a str, &'a str>,
    path: &str,
) -> Option<(String, &'a str)> {
    let trimmed = path.trim_start_matches("./");
    if let Some(&src) = modules.get(path) {
        return Some((trimmed.to_string(), src));
    }
    if let Some(&src) = modules.get(trimmed) {
        return Some((trimmed.to_string(), src));
    }
    let dotted = format!("./{}", trimmed);
    modules
        .get(dotted.as_str())
        .map(|&src| (trimmed.to_string(), src))
}

fn collect_local_defs(items: &[Item]) -> HashSet<String> {
    let mut defs = HashSet::new();
    for item in items {
        match &item.kind {
            ItemKind::Func { name, .. }
            | ItemKind::Struct { name, .. }
            | ItemKind::TypeAlias { name, .. }
            | ItemKind::Const { name, .. }
            | ItemKind::Enum { name, .. }
            | ItemKind::Error { name, .. } => {
                defs.insert(name.clone());
            }
            ItemKind::Query(q) => {
                defs.insert(q.name.clone());
            }
            ItemKind::Job(j) => {
                defs.insert(j.name.clone());
            }
            ItemKind::Import { .. }
            | ItemKind::ExportList { .. }
            | ItemKind::Migration { .. }
            | ItemKind::Test { .. } => {}
        }
    }
    defs
}

fn rename_item_decl(item: &mut Item, rename: &HashMap<String, String>) {
    match &mut item.kind {
        ItemKind::Func { name, .. }
        | ItemKind::Struct { name, .. }
        | ItemKind::TypeAlias { name, .. }
        | ItemKind::Const { name, .. }
        | ItemKind::Enum { name, .. }
        | ItemKind::Error { name, .. } => {
            if let Some(new_name) = rename.get(name) {
                *name = new_name.clone();
            }
        }
        ItemKind::Query(q) => {
            if let Some(new_name) = rename.get(&q.name) {
                q.name = new_name.clone();
            }
        }
        ItemKind::Job(j) => {
            if let Some(new_name) = rename.get(&j.name) {
                j.name = new_name.clone();
            }
        }
        ItemKind::Import { .. }
        | ItemKind::ExportList { .. }
        | ItemKind::Migration { .. }
        | ItemKind::Test { .. } => {}
    }
}

/// Link `import` items (other than built-in `cdt.json`) from `modules` into `program`.
pub fn link_imports(
    program: &mut Program,
    modules: &HashMap<&str, &str>,
) -> Result<(), String> {
    let empty_paths = HashMap::new();
    link_imports_with_paths(program, modules, &empty_paths)
}

pub fn link_imports_with_paths(
    program: &mut Program,
    modules: &HashMap<&str, &str>,
    module_paths: &HashMap<&str, String>,
) -> Result<(), String> {
    let root_defs = collect_local_defs(&program.items);
    let mut preferred: HashMap<(String, String), String> = HashMap::new();
    let mut claimed: HashSet<String> = root_defs.clone();

    for item in &program.items {
        if let ItemKind::Import { path, names, .. } = &item.kind
            && !is_native_module(path)
        {
            let norm = path.trim_start_matches("./").to_string();
            for spec in names {
                if !claimed.contains(&spec.alias) {
                    preferred.insert((norm.clone(), spec.name.clone()), spec.alias.clone());
                    claimed.insert(spec.alias.clone());
                }
            }
        }
    }

    let mut state = LinkState {
        modules,
        module_paths,
        loaded: HashMap::new(),
        visiting: HashSet::new(),
        used_names: root_defs.clone(),
        preferred,
        merged_items: Vec::new(),
        next_mod_id: 0,
    };

    let env = process_module_imports(&program.items, &root_defs, &mut state, None)?;
    rewrite_items(&mut program.items, &env)?;

    let mut remaining = Vec::with_capacity(program.items.len() + state.merged_items.len());
    for item in std::mem::take(&mut program.items) {
        if let ItemKind::Import { path, .. } = &item.kind
            && !is_native_module(path)
        {
            continue;
        }
        remaining.push(item);
    }
    let mut final_items = state.merged_items;
    final_items.extend(remaining);
    program.items = final_items;
    Ok(())
}

fn process_module_imports(
    items: &[Item],
    local_defs: &HashSet<String>,
    state: &mut LinkState<'_>,
    scope_mod_id: Option<usize>,
) -> Result<RewriteEnv, String> {
    let mut env = RewriteEnv {
        sym_map: HashMap::new(),
        ns_map: HashMap::new(),
        ns_all_defs: HashMap::new(),
        nat_ns_rename: HashMap::new(),
    };

    for item in items {
        let ItemKind::Import { path, alias, names } = &item.kind else {
            continue;
        };
        if is_native_module(path) {
            if let Some(mid) = scope_mod_id {
                if let Some(ns) = alias {
                    let scoped = format!("__m{}_nat_{}", mid, ns);
                    env.nat_ns_rename.insert(ns.clone(), scoped);
                }
                for spec in names {
                    let scoped = format!("__m{}_nat_{}", mid, spec.alias);
                    env.sym_map.insert(spec.alias.clone(), scoped);
                }
            }
            continue;
        }
        let (exports, all_defs) = load_module(path, state)?;
        if let Some(ns) = alias {
            if local_defs.contains(ns) || env.ns_map.contains_key(ns) || env.sym_map.contains_key(ns) {
                return Err(format!("Duplicate import alias '{}'", ns));
            }
            env.ns_map.insert(ns.clone(), exports.clone());
            env.ns_all_defs.insert(ns.clone(), all_defs.clone());
        }
        for spec in names {
            let Some(global_name) = exports.get(&spec.name) else {
                if all_defs.contains(&spec.name) {
                    return Err(format!("Symbol '{}' is private in module '{}'", spec.name, path));
                }
                return Err(format!(
                    "Module '{}' has no exported member '{}'",
                    path, spec.name
                ));
            };
            if local_defs.contains(&spec.alias) || env.ns_map.contains_key(&spec.alias) {
                return Err(format!("Duplicate identifier '{}'", spec.alias));
            }
            if let Some(prev) = env.sym_map.insert(spec.alias.clone(), global_name.clone())
                && prev != *global_name
            {
                return Err(format!("Duplicate imported identifier '{}'", spec.alias));
            }
        }
    }
    Ok(env)
}

fn load_module(
    path: &str,
    state: &mut LinkState<'_>,
) -> Result<(HashMap<String, String>, HashSet<String>), String> {
    let (norm, src) = resolve_module(state.modules, path)
        .ok_or_else(|| format!("Cannot find imported module '{}'", path))?;

    if let Some(cached) = state.loaded.get(&norm) {
        return Ok(cached.clone());
    }
    if !state.visiting.insert(norm.clone()) {
        return Err(format!("Cyclic module import involving '{}'", path));
    }

    let mod_id = state.next_mod_id;
    state.next_mod_id += 1;

    let display_path = state
        .module_paths
        .get(norm.as_str())
        .or_else(|| state.module_paths.get(path))
        .cloned()
        .unwrap_or_else(|| path.to_string());
    let display_path_ref = std::path::Path::new(&display_path);

    let mut mod_prog =
        parse_source(src).map_err(|e| crate::diagnostics::tag_file_error(display_path_ref, e))?;
    let local_defs = collect_local_defs(&mod_prog.items);
    let mut env = process_module_imports(&mod_prog.items, &local_defs, state, Some(mod_id))?;

    let mut local_to_global: HashMap<String, String> = HashMap::new();
    for def in &local_defs {
        let preferred = state.preferred.get(&(norm.clone(), def.clone())).cloned();
        let mut gname = match preferred {
            Some(p) if !state.used_names.contains(&p) => p,
            _ => format!("__m{}_{}", mod_id, def),
        };
        while state.used_names.contains(&gname) {
            gname = format!("__m{}_{}_{}", mod_id, def, state.used_names.len());
        }
        state.used_names.insert(gname.clone());
        local_to_global.insert(def.clone(), gname);
    }

    let mut exports: HashMap<String, String> = HashMap::new();
    for item in &mod_prog.items {
        if item.is_exported {
            match &item.kind {
                ItemKind::Func { name, .. }
                | ItemKind::Struct { name, .. }
                | ItemKind::TypeAlias { name, .. }
                | ItemKind::Const { name, .. }
                | ItemKind::Enum { name, .. }
                | ItemKind::Error { name, .. } => {
                    exports.insert(name.clone(), local_to_global[name].clone());
                }
                ItemKind::Query(q) => {
                    exports.insert(q.name.clone(), local_to_global[&q.name].clone());
                }
                ItemKind::Job(j) => {
                    exports.insert(j.name.clone(), local_to_global[&j.name].clone());
                }
                ItemKind::ExportList { names } => {
                    for spec in names {
                        let resolved = local_to_global
                            .get(&spec.name)
                            .or_else(|| env.sym_map.get(&spec.name))
                            .ok_or_else(|| {
                                format!(
                                    "Cannot export unknown symbol '{}' from '{}'",
                                    spec.name, path
                                )
                            })?;
                        exports.insert(spec.alias.clone(), resolved.clone());
                    }
                }
                ItemKind::Import { .. }
                | ItemKind::Migration { .. }
                | ItemKind::Test { .. } => {}
            }
        }
    }

    mod_prog
        .items
        .retain(|it| !matches!(it.kind, ItemKind::Test { .. }));
    env.sym_map.extend(local_to_global.clone());
    rewrite_items(&mut mod_prog.items, &env)
        .map_err(|e| crate::diagnostics::tag_file_error(display_path_ref, e))?;

    for mut item in mod_prog.items {
        if matches!(item.kind, ItemKind::ExportList { .. }) {
            continue;
        }
        if let ItemKind::Import {
            path: imp_path,
            alias,
            names,
        } = &mut item.kind
        {
            if !is_native_module(imp_path) {
                continue;
            }
            if let Some(ns) = alias
                && let Some(scoped) = env.nat_ns_rename.get(ns)
            {
                *ns = scoped.clone();
            }
            for spec in names {
                if let Some(scoped) = env.sym_map.get(&spec.alias) {
                    spec.alias = scoped.clone();
                }
            }
        }
        rename_item_decl(&mut item, &local_to_global);
        item.is_imported = true;
        item.is_exported = false;
        if item.source_file.is_none() {
            item.source_file = Some(display_path.clone());
        }
        state.merged_items.push(item);
    }

    state.visiting.remove(&norm);
    state.loaded.insert(norm, (exports.clone(), local_defs.clone()));
    Ok((exports, local_defs))
}

fn rewrite_items(items: &mut [Item], env: &RewriteEnv) -> Result<(), String> {
    if env.sym_map.is_empty() && env.ns_map.is_empty() && env.nat_ns_rename.is_empty() {
        return Ok(());
    }
    for item in items {
        match &mut item.kind {
            ItemKind::Func {
                params,
                return_type,
                body,
                ..
            } => {
                for p in params.iter_mut() {
                    rewrite_type_expr(&mut p.ty, env)?;
                    if let Some(def) = &mut p.default {
                        let mut scopes = Vec::new();
                        rewrite_expr(def, env, &mut scopes)?;
                    }
                }
                if let Some(rt) = return_type {
                    rewrite_type_expr(rt, env)?;
                }
                let mut scopes = vec![params.iter().map(|p| p.name.clone()).collect()];
                rewrite_block_stmts(body, env, &mut scopes)?;
            }
            ItemKind::Struct { fields, .. } | ItemKind::Error { fields, .. } => {
                for f in fields {
                    rewrite_type_expr(&mut f.ty, env)?;
                    if let Some(def) = &mut f.default {
                        let mut scopes = Vec::new();
                        rewrite_expr(def, env, &mut scopes)?;
                    }
                    for ann in &mut f.annotations {
                        let mut empty_scopes = Vec::new();
                        for a in &mut ann.args {
                            rewrite_expr(a, env, &mut empty_scopes)?;
                        }
                        if matches!(
                            ann.target.as_str(),
                            "json" | "env" | "flag" | "opt" | "arg" | "rest" | "cmd" | "group"
                        ) || ann.target.starts_with("db.")
                            || ann.target.starts_with("cli.")
                            || ann.target.starts_with("router.")
                            || ann.target.starts_with("schedule.")
                            || ann.target.starts_with("app.")
                        {
                            continue;
                        }
                        if let Some((ns, member)) = ann.target.split_once('.') {
                            if let Some(exports) = env.ns_map.get(ns) {
                                if let Some(resolved) = exports.get(member) {
                                    ann.target = resolved.clone();
                                } else if env
                                    .ns_all_defs
                                    .get(ns)
                                    .is_some_and(|defs| defs.contains(member))
                                {
                                    return Err(format!(
                                        "Cannot use private validator '{}' from module '{}'",
                                        member, ns
                                    ));
                                } else {
                                    return Err(format!(
                                        "Module alias '{}' has no exported validator '{}'",
                                        ns, member
                                    ));
                                }
                            } else if let Some(scoped) = env.nat_ns_rename.get(ns) {
                                ann.target = format!("{}.{}", scoped, member);
                            }
                        } else if let Some(mapped) = env.sym_map.get(&ann.target) {
                            ann.target = mapped.clone();
                        }
                    }
                }
            }
            ItemKind::TypeAlias { target, .. } => {
                rewrite_type_expr(target, env)?;
            }
            ItemKind::Const { ty, value, .. } => {
                if let Some(t) = ty {
                    rewrite_type_expr(t, env)?;
                }
                let mut scopes = Vec::new();
                rewrite_expr(value, env, &mut scopes)?;
            }
            ItemKind::ExportList { names } => {
                for spec in names {
                    if let Some(mapped) = env.sym_map.get(&spec.name) {
                        spec.name = mapped.clone();
                    }
                }
            }
            ItemKind::Migration { up, down, .. } => {
                let mut up_scopes = vec![HashSet::from(["db".to_string()])];
                rewrite_block_stmts(up, env, &mut up_scopes)?;
                let mut down_scopes = vec![HashSet::from(["db".to_string()])];
                rewrite_block_stmts(down, env, &mut down_scopes)?;
            }
            ItemKind::Test { body, .. } => {
                let mut scopes = Vec::new();
                rewrite_block_stmts(body, env, &mut scopes)?;
            }
            ItemKind::Job(j) => {
                if let Some(rt) = &mut j.return_type {
                    rewrite_type_expr(rt, env)?;
                }
                let mut scopes = Vec::new();
                rewrite_block_stmts(&mut j.body, env, &mut scopes)?;
            }
            ItemKind::Query(q) => {
                let mut scopes = vec![HashSet::new()];
                for p in &mut q.params {
                    rewrite_type_expr(&mut p.ty, env)?;
                    if let Some(def) = &mut p.default {
                        let mut empty = Vec::new();
                        rewrite_expr(def, env, &mut empty)?;
                    }
                    scopes[0].insert(p.name.clone());
                }
                if let Some(rt) = &mut q.return_type {
                    rewrite_type_expr(rt, env)?;
                }
                if let Some(mapped) = env.sym_map.get(q.model_name.as_str()) {
                    q.model_name = mapped.clone();
                }
                if let Some(pred) = &mut q.where_pred {
                    rewrite_query_pred(pred, env, &mut scopes)?;
                }
                if let Some(lim) = &mut q.limit {
                    rewrite_expr(lim, env, &mut scopes)?;
                }
                if let Some(off) = &mut q.offset {
                    rewrite_expr(off, env, &mut scopes)?;
                }
            }
            ItemKind::Enum { variants, .. } => {
                for v in variants {
                    for f in &mut v.fields {
                        rewrite_type_expr(&mut f.ty, env)?;
                        if let Some(def) = &mut f.default {
                            let mut empty_scopes = Vec::new();
                            rewrite_expr(def, env, &mut empty_scopes)?;
                        }
                        for ann in &mut f.annotations {
                            let mut empty_scopes = Vec::new();
                            for a in &mut ann.args {
                                rewrite_expr(a, env, &mut empty_scopes)?;
                            }
                            if matches!(
                                ann.target.as_str(),
                                "json" | "env" | "flag" | "opt" | "arg" | "rest" | "cmd" | "group"
                            ) || ann.target.starts_with("db.")
                                || ann.target.starts_with("cli.")
                                || ann.target.starts_with("router.")
                                || ann.target.starts_with("schedule.")
                            {
                                continue;
                            }
                            if let Some((ns, member)) = ann.target.split_once('.') {
                                if let Some(exports) = env.ns_map.get(ns) {
                                    let resolved = exports.get(member).ok_or_else(|| {
                                        format!(
                                            "Module alias '{}' has no exported member '{}'",
                                            ns, member
                                        )
                                    })?;
                                    ann.target = resolved.clone();
                                } else if let Some(scoped) = env.nat_ns_rename.get(ns) {
                                    ann.target = format!("{}.{}", scoped, member);
                                }
                            } else if let Some(mapped) = env.sym_map.get(&ann.target) {
                                ann.target = mapped.clone();
                            }
                        }
                    }
                }
            }
            ItemKind::Import { .. } => {}
        }
    }
    Ok(())
}

fn rewrite_type_expr(te: &mut TypeExpr, env: &RewriteEnv) -> Result<(), String> {
    match te {
        TypeExpr::Named(name) => {
            if let Some(dot) = name.find('.') {
                let (ns, member) = (&name[..dot], &name[dot + 1..]);
                if let Some(exports) = env.ns_map.get(ns) {
                    let resolved = exports.get(member).ok_or_else(|| {
                        if env.ns_all_defs.get(ns).is_some_and(|d| d.contains(member)) {
                            format!("Type '{}' is private in module '{}'", member, ns)
                        } else {
                            format!("Module alias '{}' has no exported member '{}'", ns, member)
                        }
                    })?;
                    *name = resolved.clone();
                } else if let Some(scoped) = env.nat_ns_rename.get(ns) {
                    *name = format!("{}.{}", scoped, member);
                }
            } else if let Some(mapped) = env.sym_map.get(name) {
                *name = mapped.clone();
            }
        }
        TypeExpr::Generic { name, type_args } => {
            if let Some(dot) = name.find('.') {
                let (ns, member) = (&name[..dot], &name[dot + 1..]);
                if let Some(exports) = env.ns_map.get(ns) {
                    if let Some(resolved) = exports.get(member) {
                        *name = resolved.clone();
                    }
                } else if let Some(scoped) = env.nat_ns_rename.get(ns) {
                    *name = format!("{}.{}", scoped, member);
                }
            } else if let Some(mapped) = env.sym_map.get(name) {
                *name = mapped.clone();
            }
            for a in type_args {
                rewrite_type_expr(a, env)?;
            }
        }
        TypeExpr::List(inner) | TypeExpr::Optional(inner) => {
            rewrite_type_expr(inner, env)?;
        }
        TypeExpr::Union(vars) | TypeExpr::Tuple(vars) => {
            for v in vars {
                rewrite_type_expr(v, env)?;
            }
        }
        TypeExpr::Func {
            params,
            return_type,
        } => {
            for p in params {
                rewrite_type_expr(p, env)?;
            }
            rewrite_type_expr(return_type, env)?;
        }
    }
    Ok(())
}

pub(crate) fn is_shadowed(scopes: &[HashSet<String>], name: &str) -> bool {
    scopes.iter().rev().any(|s| s.contains(name))
}

pub(crate) fn bind_pat_names(pat: &Pattern, scope: &mut HashSet<String>) {
    match &pat.kind {
        PatternKind::Ident(n) | PatternKind::Type { binding: n, .. } => {
            scope.insert(n.clone());
        }
        PatternKind::EnumVariant { bindings, .. } => {
            for (_, v) in bindings {
                if v != "_" {
                    scope.insert(v.clone());
                }
            }
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                bind_pat_names(p, scope);
            }
        }
        PatternKind::Nil | PatternKind::Wildcard => {}
    }
}

fn rewrite_block_stmts(
    block: &mut Block,
    env: &RewriteEnv,
    scopes: &mut Vec<HashSet<String>>,
) -> Result<(), String> {
    scopes.push(HashSet::new());
    for s in &mut block.stmts {
        match &mut s.kind {
            StmtKind::Define { name, value } => {
                rewrite_expr(value, env, scopes)?;
                if let Some(top) = scopes.last_mut() {
                    top.insert(name.clone());
                }
            }
            StmtKind::DestructureDefine { pattern, value } => {
                rewrite_expr(value, env, scopes)?;
                if let Some(top) = scopes.last_mut() {
                    bind_pat_names(pattern, top);
                }
            }
            StmtKind::VarDecl { name, ty, init } => {
                if let Some(te) = ty {
                    rewrite_type_expr(te, env)?;
                }
                if let Some(e) = init {
                    rewrite_expr(e, env, scopes)?;
                }
                if let Some(top) = scopes.last_mut() {
                    top.insert(name.clone());
                }
            }
            StmtKind::Assign { target, value } => {
                rewrite_expr(target, env, scopes)?;
                rewrite_expr(value, env, scopes)?;
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => {
                rewrite_expr(e, env, scopes)?;
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                rewrite_expr(condition, env, scopes)?;
                rewrite_block_stmts(then_block, env, scopes)?;
                if let Some(eb) = else_block {
                    rewrite_block_stmts(eb, env, scopes)?;
                }
            }
            StmtKind::Guard {
                condition,
                else_block,
            } => {
                rewrite_expr(condition, env, scopes)?;
                rewrite_block_stmts(else_block, env, scopes)?;
            }
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            } => {
                rewrite_expr(iterable, env, scopes)?;
                let mut loop_scope = HashSet::new();
                bind_pat_names(pattern, &mut loop_scope);
                scopes.push(loop_scope);
                rewrite_block_stmts(body, env, scopes)?;
                scopes.pop();
            }
            StmtKind::While { condition, body } => {
                rewrite_expr(condition, env, scopes)?;
                rewrite_block_stmts(body, env, scopes)?;
            }
            StmtKind::Defer(block) | StmtKind::Spawn(block) => {
                rewrite_block_stmts(block, env, scopes)?;
            }
            StmtKind::Switch { expr, cases } => {
                rewrite_expr(expr, env, scopes)?;
                for case in cases {
                    if let PatternKind::Ident(name) = &mut case.pattern.kind
                        && let Some(dot) = name.find('.')
                    {
                        let first = &name[..dot];
                        let rest = &name[dot + 1..];
                        if let Some(exports) = env.ns_map.get(first)
                            && let Some(dot2) = rest.find('.')
                        {
                            let (ename, vname) = (&rest[..dot2], &rest[dot2 + 1..]);
                            if let Some(resolved) = exports.get(ename) {
                                *name = format!("{}.{}", resolved, vname);
                            }
                        } else if let Some(resolved) = env.sym_map.get(first) {
                            *name = format!("{}.{}", resolved, rest);
                        }
                    }
                    if let PatternKind::Type { name, .. } = &mut case.pattern.kind {
                        if let Some(dot) = name.find('.') {
                            let (ns, member) = (&name[..dot], &name[dot + 1..]);
                            if let Some(exports) = env.ns_map.get(ns) {
                                if let Some((ename, vname)) = member.split_once('.') {
                                    if let Some(resolved) = exports.get(ename) {
                                        *name = format!("{}.{}", resolved, vname);
                                    } else {
                                        return Err(format!(
                                            "Module alias '{}' has no exported enum '{}'",
                                            ns, ename
                                        ));
                                    }
                                } else if let Some(resolved) = exports.get(member) {
                                    *name = resolved.clone();
                                } else {
                                    return Err(format!(
                                        "Module alias '{}' has no exported type '{}'",
                                        ns, member
                                    ));
                                }
                            } else if let Some(scoped) = env.nat_ns_rename.get(ns) {
                                *name = format!("{}.{}", scoped, member);
                            } else if let Some(mapped) = env.sym_map.get(ns) {
                                *name = format!("{}.{}", mapped, member);
                            }
                        } else if let Some(mapped) = env.sym_map.get(name) {
                            *name = mapped.clone();
                        }
                    }
                    if let PatternKind::EnumVariant { enum_name, .. } = &mut case.pattern.kind {
                        if let Some((ns, ename)) = enum_name.split_once('.') {
                            if let Some(exports) = env.ns_map.get(ns) {
                                if let Some(resolved) = exports.get(ename) {
                                    *enum_name = resolved.clone();
                                } else {
                                    return Err(format!(
                                        "Module alias '{}' has no exported enum '{}'",
                                        ns, ename
                                    ));
                                }
                            }
                        } else if let Some(mapped) = env.sym_map.get(enum_name) {
                            *enum_name = mapped.clone();
                        }
                    }
                    let mut case_scope = HashSet::new();
                    bind_pat_names(&case.pattern, &mut case_scope);
                    scopes.push(case_scope);
                    rewrite_block_stmts(&mut case.body, env, scopes)?;
                    scopes.pop();
                }
            }
        }
    }
    scopes.pop();
    Ok(())
}

fn rewrite_expr(
    expr: &mut Expr,
    env: &RewriteEnv,
    scopes: &mut Vec<HashSet<String>>,
) -> Result<(), String> {
    match &mut expr.kind {
        ExprKind::Ident(name) => {
            if !is_shadowed(scopes, name)
                && let Some(mapped) = env.sym_map.get(name)
            {
                *name = mapped.clone();
            }
        }
        ExprKind::Member { target, field } => {
            if let ExprKind::Ident(ns) = &mut target.kind
                && !is_shadowed(scopes, ns)
            {
                if let Some(exports) = env.ns_map.get(ns.as_str()) {
                    let resolved = exports.get(field).ok_or_else(|| {
                        if env.ns_all_defs.get(ns.as_str()).is_some_and(|d| d.contains(field)) {
                            format!("Symbol '{}' is private in module '{}'", field, ns)
                        } else {
                            format!("Module alias '{}' has no exported member '{}'", ns, field)
                        }
                    })?;
                    expr.kind = ExprKind::Ident(resolved.clone());
                    return Ok(());
                }
                if let Some(scoped) = env.nat_ns_rename.get(ns.as_str()) {
                    *ns = scoped.clone();
                    return Ok(());
                }
            }
            rewrite_expr(target, env, scopes)?;
        }
        ExprKind::OptionalMember { target, .. } | ExprKind::Try(target) => {
            rewrite_expr(target, env, scopes)?;
        }
        ExprKind::Binary { left, right, .. } => {
            rewrite_expr(left, env, scopes)?;
            rewrite_expr(right, env, scopes)?;
        }
        ExprKind::Unary { expr: inner, .. } => rewrite_expr(inner, env, scopes)?,
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                rewrite_expr(e, env, scopes)?;
            }
        }
        ExprKind::Call {
            callee,
            type_args,
            args,
            named_args,
        } => {
            rewrite_expr(callee, env, scopes)?;
            for t in type_args {
                rewrite_type_expr(t, env)?;
            }
            for a in args {
                rewrite_expr(a, env, scopes)?;
            }
            for na in named_args {
                rewrite_expr(&mut na.value, env, scopes)?;
            }
        }
        ExprKind::Index { target, index } => {
            rewrite_expr(target, env, scopes)?;
            rewrite_expr(index, env, scopes)?;
        }
        ExprKind::Closure {
            params,
            return_type,
            body,
        } => {
            for p in params.iter_mut() {
                rewrite_type_expr(&mut p.ty, env)?;
            }
            if let Some(rt) = return_type {
                rewrite_type_expr(rt, env)?;
            }
            scopes.push(params.iter().map(|p| p.name.clone()).collect());
            rewrite_block_stmts(body, env, scopes)?;
            scopes.pop();
        }
        ExprKind::String(s) => {
            if s.contains('{') {
                *s = rewrite_interp_string(s, env, scopes);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil => {}
    }
    Ok(())
}

fn is_interp_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn rewrite_interp_string(
    raw: &str,
    env: &RewriteEnv,
    scopes: &[HashSet<String>],
) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            out.push(ch);
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if ch != '{' {
            out.push(ch);
            continue;
        }
        let mut seg = String::new();
        let mut closed = false;
        for inner in chars.by_ref() {
            if inner == '}' {
                closed = true;
                break;
            }
            seg.push(inner);
        }
        if !closed {
            out.push('{');
            out.push_str(&seg);
            break;
        }
        let seg_parts: Vec<&str> = seg.split('.').collect();
        if !seg.is_empty()
            && seg_parts.iter().all(|p| is_interp_ident(p))
            && !is_shadowed(scopes, seg_parts[0])
        {
            if seg_parts.len() >= 2
                && let Some(ns_exports) = env.ns_map.get(seg_parts[0])
                && let Some(mapped) = ns_exports.get(seg_parts[1])
            {
                out.push('{');
                out.push_str(mapped);
                for rest in &seg_parts[2..] {
                    out.push('.');
                    out.push_str(rest);
                }
                out.push('}');
                continue;
            }
            if let Some(mapped) = env.sym_map.get(seg_parts[0]) {
                out.push('{');
                out.push_str(mapped);
                for rest in &seg_parts[1..] {
                    out.push('.');
                    out.push_str(rest);
                }
                out.push('}');
                continue;
            }
        }
        out.push('{');
        out.push_str(&seg);
        out.push('}');
    }
    out
}

fn rewrite_query_pred(
    pred: &mut QueryPredicate,
    env: &RewriteEnv,
    scopes: &mut Vec<HashSet<String>>,
) -> Result<(), String> {
    match pred {
        QueryPredicate::And(l, r) | QueryPredicate::Or(l, r) => {
            rewrite_query_pred(l, env, scopes)?;
            rewrite_query_pred(r, env, scopes)
        }
        QueryPredicate::Cmp { value, .. } => rewrite_expr(value, env, scopes),
    }
}
