use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Block, Expr, ExprKind, ItemKind, PatternKind, Stmt, StmtKind, TypeExpr};
use crate::loader::{normalize_path, LoadedModule, ModuleTarget};
use crate::native::NativeRegistry;

#[derive(Debug, Clone, PartialEq)]
pub struct ReachabilityReport {
    pub root_module: PathBuf,
    pub reachable_functions: HashSet<(PathBuf, String)>,
    pub reachable_native_functions: HashSet<(String, String)>,
    pub reachable_native_methods: HashSet<(String, String)>,
    pub total_functions: usize,
    pub reachable_functions_count: usize,
    pub unused_functions_count: usize,
}

fn paths_match(candidate: &Path, target: &Path, norm_target: &Path) -> bool {
    candidate == target
        || candidate == norm_target
        || normalize_path(candidate) == norm_target
        || candidate.ends_with(target)
        || target.ends_with(candidate)
}

impl ReachabilityReport {
    pub fn is_function_reachable(&self, module: &Path, func_name: &str) -> bool {
        if self.reachable_functions.contains(&(module.to_path_buf(), func_name.to_string())) {
            return true;
        }
        let norm_module = normalize_path(module);
        self.reachable_functions
            .iter()
            .any(|(p, name)| name == func_name && paths_match(p, module, &norm_module))
    }

    pub fn is_native_function_reachable(&self, module_name: &str, func_name: &str) -> bool {
        self.reachable_native_functions.contains(&(module_name.to_string(), func_name.to_string()))
            || self.reachable_native_functions.iter().any(|(m, f)| {
                f == func_name && (m == module_name || m.ends_with(module_name) || module_name.ends_with(m))
            })
    }
}

pub struct ReachabilityAnalyzer<'a> {
    modules: &'a HashMap<PathBuf, LoadedModule>,
    _native_registry: &'a NativeRegistry,
    reachable_functions: HashSet<(PathBuf, String)>,
    reachable_native_functions: HashSet<(String, String)>,
    reachable_native_methods: HashSet<(String, String)>,
    reachable_structs: HashSet<(PathBuf, String)>,
}

fn lookup_module<'m>(modules: &'m HashMap<PathBuf, LoadedModule>, path: &Path) -> Option<&'m LoadedModule> {
    if let Some(m) = modules.get(path) {
        return Some(m);
    }
    let norm = normalize_path(path);
    if let Some(m) = modules.get(&norm) {
        return Some(m);
    }
    for (p, m) in modules {
        if paths_match(p, path, &norm) {
            return Some(m);
        }
    }
    None
}

impl<'a> ReachabilityAnalyzer<'a> {
    pub fn new(
        modules: &'a HashMap<PathBuf, LoadedModule>,
        native_registry: &'a NativeRegistry,
    ) -> Self {
        Self {
            modules,
            _native_registry: native_registry,
            reachable_functions: HashSet::new(),
            reachable_native_functions: HashSet::new(),
            reachable_native_methods: HashSet::new(),
            reachable_structs: HashSet::new(),
        }
    }

    pub fn analyze(&mut self, root_path: &Path, extra_entry_points: &[(&Path, &str)]) -> ReachabilityReport {
        let mut worklist: Vec<(PathBuf, String)> = Vec::new();

        let canonical_root = lookup_module(self.modules, root_path)
            .map(|m| m.canonical_path.clone())
            .unwrap_or_else(|| root_path.to_path_buf());
        worklist.push((canonical_root.clone(), "main".to_string()));
        if canonical_root != root_path {
            worklist.push((root_path.to_path_buf(), "main".to_string()));
        }

        for &(mod_path, fn_name) in extra_entry_points {
            let canon = lookup_module(self.modules, mod_path)
                .map(|m| m.canonical_path.clone())
                .unwrap_or_else(|| mod_path.to_path_buf());
            worklist.push((canon, fn_name.to_string()));
        }

        let mut total_functions: usize = 0;
        for (mod_path, loaded_mod) in self.modules {
            for item in &loaded_mod.program.items {
                if let ItemKind::Func { ref name, .. } = item.kind {
                    total_functions += 1;
                    let _ = mod_path;
                    let _ = name;
                }
            }
        }

        while let Some((mod_path, func_name)) = worklist.pop() {
            if !self.reachable_functions.insert((mod_path.clone(), func_name.clone())) {
                continue;
            }

            if let Some(loaded_mod) = lookup_module(self.modules, &mod_path) {
                self.reachable_functions.insert((loaded_mod.canonical_path.clone(), func_name.clone()));

                for item in &loaded_mod.program.items {
                    if let ItemKind::Func { ref name, ref body, ref params, ref return_type, .. } = item.kind {
                        if name == &func_name {
                            for p in params {
                                self.scan_type_expr(&p.ty, &loaded_mod.canonical_path, loaded_mod, &mut worklist);
                            }
                            if let Some(ret) = return_type {
                                self.scan_type_expr(ret, &loaded_mod.canonical_path, loaded_mod, &mut worklist);
                            }
                            self.scan_block(body, &loaded_mod.canonical_path, loaded_mod, &mut worklist);
                            break;
                        }
                    }
                }
            }
        }

        let mut unique_reachable = HashSet::new();
        for (p, f) in &self.reachable_functions {
            let file_key = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string());
            unique_reachable.insert((file_key, f.clone()));
        }
        let reachable_count = unique_reachable.len().min(total_functions);
        let unused_count = total_functions.saturating_sub(reachable_count);

        ReachabilityReport {
            root_module: canonical_root,
            reachable_functions: self.reachable_functions.clone(),
            reachable_native_functions: self.reachable_native_functions.clone(),
            reachable_native_methods: self.reachable_native_methods.clone(),
            total_functions,
            reachable_functions_count: reachable_count,
            unused_functions_count: unused_count,
        }
    }

    fn scan_block(
        &mut self,
        block: &Block,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        for stmt in &block.stmts {
            self.scan_stmt(stmt, current_mod_path, loaded_mod, worklist);
        }
    }

    fn scan_stmt(
        &mut self,
        stmt: &Stmt,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        match &stmt.kind {
            StmtKind::Define { value, .. } | StmtKind::DestructureDefine { value, .. } => {
                self.scan_expr(value, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::VarDecl { ty, init, .. } => {
                if let Some(t) = ty {
                    self.scan_type_expr(t, current_mod_path, loaded_mod, worklist);
                }
                if let Some(expr) = init {
                    self.scan_expr(expr, current_mod_path, loaded_mod, worklist);
                }
            }
            StmtKind::Assign { target, value } => {
                self.scan_expr(target, current_mod_path, loaded_mod, worklist);
                self.scan_expr(value, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::Return(Some(expr)) | StmtKind::Expr(expr) => {
                self.scan_expr(expr, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::If { condition, then_block, else_block } => {
                self.scan_expr(condition, current_mod_path, loaded_mod, worklist);
                self.scan_block(then_block, current_mod_path, loaded_mod, worklist);
                if let Some(eb) = else_block {
                    self.scan_block(eb, current_mod_path, loaded_mod, worklist);
                }
            }
            StmtKind::Guard { condition, else_block } => {
                self.scan_expr(condition, current_mod_path, loaded_mod, worklist);
                self.scan_block(else_block, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::ForIn { iterable, body, .. } | StmtKind::While { condition: iterable, body } => {
                self.scan_expr(iterable, current_mod_path, loaded_mod, worklist);
                self.scan_block(body, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::Switch { expr, cases } => {
                self.scan_expr(expr, current_mod_path, loaded_mod, worklist);
                for case in cases {
                    self.scan_pattern(&case.pattern, current_mod_path, loaded_mod, worklist);
                    self.scan_block(&case.body, current_mod_path, loaded_mod, worklist);
                }
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn scan_expr(
        &mut self,
        expr: &Expr,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if self.is_func_in_module(current_mod_path, name) {
                    worklist.push((current_mod_path.to_path_buf(), name.clone()));
                }
                self.mark_struct_reachable(current_mod_path, name, worklist);
            }
            ExprKind::Call { callee, type_args, args, named_args } => {
                for ta in type_args {
                    self.scan_type_expr(ta, current_mod_path, loaded_mod, worklist);
                }
                if let ExprKind::Ident(ref name) = callee.kind {
                    if self.is_func_in_module(current_mod_path, name) {
                        worklist.push((current_mod_path.to_path_buf(), name.clone()));
                    }
                    self.mark_struct_reachable(current_mod_path, name, worklist);
                } else if let ExprKind::Member { target, field } | ExprKind::OptionalMember { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        if let Some((mod_target, _)) = loaded_mod.imports.get(alias) {
                            match mod_target {
                                ModuleTarget::Local(target_path) => {
                                    worklist.push((target_path.clone(), field.clone()));
                                    self.mark_struct_reachable(target_path, field, worklist);
                                }
                                ModuleTarget::Native(native_name) => {
                                    self.reachable_native_functions.insert((native_name.clone(), field.clone()));
                                }
                            }
                        } else {
                            self.scan_expr(target, current_mod_path, loaded_mod, worklist);
                        }
                    } else {
                        self.reachable_native_methods.insert((String::new(), field.clone()));
                        self.scan_expr(target, current_mod_path, loaded_mod, worklist);
                    }
                } else {
                    self.scan_expr(callee, current_mod_path, loaded_mod, worklist);
                }

                for a in args {
                    self.scan_expr(a, current_mod_path, loaded_mod, worklist);
                }
                for na in named_args {
                    self.scan_expr(&na.value, current_mod_path, loaded_mod, worklist);
                }
            }
            ExprKind::OptionalMember { target, field } => {
                self.reachable_native_methods.insert((String::new(), field.clone()));
                self.scan_expr(target, current_mod_path, loaded_mod, worklist);
            }
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(ref alias) = target.kind {
                    if let Some((mod_target, _)) = loaded_mod.imports.get(alias) {
                        match mod_target {
                            ModuleTarget::Local(target_path) => {
                                worklist.push((target_path.clone(), field.clone()));
                                self.mark_struct_reachable(target_path, field, worklist);
                            }
                            ModuleTarget::Native(native_name) => {
                                self.reachable_native_functions.insert((native_name.clone(), field.clone()));
                            }
                        }
                    }
                } else {
                    self.reachable_native_methods.insert((String::new(), field.clone()));
                    self.scan_expr(target, current_mod_path, loaded_mod, worklist);
                }
            }
            ExprKind::Binary { left, right, .. } | ExprKind::Index { target: left, index: right } => {
                self.scan_expr(left, current_mod_path, loaded_mod, worklist);
                self.scan_expr(right, current_mod_path, loaded_mod, worklist);
            }
            ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => {
                self.scan_expr(inner, current_mod_path, loaded_mod, worklist);
            }
            ExprKind::List(elems) | ExprKind::Tuple(elems) => {
                for e in elems {
                    self.scan_expr(e, current_mod_path, loaded_mod, worklist);
                }
            }
            ExprKind::Closure { params, return_type, body } => {
                for p in params {
                    self.scan_type_expr(&p.ty, current_mod_path, loaded_mod, worklist);
                }
                if let Some(rt) = return_type {
                    self.scan_type_expr(rt, current_mod_path, loaded_mod, worklist);
                }
                self.scan_block(body, current_mod_path, loaded_mod, worklist);
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Nil => {}
        }
    }

    fn is_func_in_module(&self, mod_path: &Path, name: &str) -> bool {
        if let Some(m) = lookup_module(self.modules, mod_path) {
            m.program.items.iter().any(|item| {
                if let ItemKind::Func { name: ref fn_name, .. } = item.kind {
                    fn_name == name
                } else {
                    false
                }
            })
        } else {
            false
        }
    }

    fn mark_struct_reachable(
        &mut self,
        mod_path: &Path,
        type_name: &str,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        let canon = lookup_module(self.modules, mod_path)
            .map(|m| m.canonical_path.clone())
            .unwrap_or_else(|| mod_path.to_path_buf());

        if !self.reachable_structs.insert((canon.clone(), type_name.to_string())) {
            return;
        }

        let loaded_mod = match lookup_module(self.modules, &canon) {
            Some(m) => m,
            None => return,
        };

        for item in &loaded_mod.program.items {
            if let ItemKind::Type { name, fields, .. } = &item.kind {
                if name == type_name {
                    for f in fields {
                        self.scan_type_expr(&f.ty, &canon, loaded_mod, worklist);
                        for ann in &f.annotations {
                            if let Some((alias, func_name)) = ann.target.split_once('.') {
                                if let Some((mod_target, _)) = loaded_mod.imports.get(alias) {
                                    match mod_target {
                                        ModuleTarget::Local(target_path) => {
                                            worklist.push((target_path.clone(), func_name.to_string()));
                                        }
                                        ModuleTarget::Native(nat_name) => {
                                            self.reachable_native_functions.insert((nat_name.clone(), func_name.to_string()));
                                        }
                                    }
                                }
                            } else {
                                worklist.push((canon.clone(), ann.target.clone()));
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    fn mark_named_type_reachable(
        &mut self,
        name: &str,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        if let Some((alias, type_name)) = name.split_once('.') {
            if let Some((ModuleTarget::Local(target_path), _)) = loaded_mod.imports.get(alias) {
                self.mark_struct_reachable(target_path, type_name, worklist);
            }
        } else {
            self.mark_struct_reachable(current_mod_path, name, worklist);
        }
    }

    fn scan_type_expr(
        &mut self,
        ty: &TypeExpr,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        match ty {
            TypeExpr::Named(name) => {
                self.mark_named_type_reachable(name, current_mod_path, loaded_mod, worklist);
            }
            TypeExpr::Generic { name, type_args } => {
                self.mark_named_type_reachable(name, current_mod_path, loaded_mod, worklist);
                for ta in type_args {
                    self.scan_type_expr(ta, current_mod_path, loaded_mod, worklist);
                }
            }
            TypeExpr::List(inner) | TypeExpr::Optional(inner) => {
                self.scan_type_expr(inner, current_mod_path, loaded_mod, worklist);
            }
            TypeExpr::Union(variants) | TypeExpr::Tuple(variants) => {
                for v in variants {
                    self.scan_type_expr(v, current_mod_path, loaded_mod, worklist);
                }
            }
            TypeExpr::Func { params, return_type } => {
                for p in params {
                    self.scan_type_expr(p, current_mod_path, loaded_mod, worklist);
                }
                self.scan_type_expr(return_type, current_mod_path, loaded_mod, worklist);
            }
        }
    }

    fn scan_pattern(
        &mut self,
        pattern: &crate::ast::Pattern,
        current_mod_path: &Path,
        loaded_mod: &LoadedModule,
        worklist: &mut Vec<(PathBuf, String)>,
    ) {
        match &pattern.kind {
            PatternKind::Type { name, .. } => {
                self.mark_named_type_reachable(name, current_mod_path, loaded_mod, worklist);
            }
            PatternKind::Tuple(pats) => {
                for p in pats {
                    self.scan_pattern(p, current_mod_path, loaded_mod, worklist);
                }
            }
            _ => {}
        }
    }
}
