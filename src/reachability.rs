use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Block, Expr, ExprKind, ItemKind, Stmt, StmtKind};
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

impl ReachabilityReport {
    pub fn is_function_reachable(&self, module: &Path, func_name: &str) -> bool {
        if self.reachable_functions.contains(&(module.to_path_buf(), func_name.to_string())) {
            return true;
        }
        let norm_module = normalize_path(module);
        self.reachable_functions.iter().any(|(p, name)| {
            name == func_name
                && (p == module
                    || p == &norm_module
                    || normalize_path(p) == norm_module
                    || p.ends_with(module)
                    || module.ends_with(p)
                    || (p.file_name().is_some() && p.file_name() == module.file_name()))
        })
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
        if p == path
            || p == &norm
            || normalize_path(p) == norm
            || p.ends_with(path)
            || path.ends_with(p)
            || (p.file_name().is_some() && p.file_name() == path.file_name())
        {
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
        }
    }

    pub fn analyze(&mut self, root_path: &Path, extra_entry_points: &[(&Path, &str)]) -> ReachabilityReport {
        let mut worklist: Vec<(PathBuf, String)> = Vec::new();

        // 1. Root main() entry point
        let canonical_root = lookup_module(self.modules, root_path)
            .map(|m| m.canonical_path.clone())
            .unwrap_or_else(|| root_path.to_path_buf());
        worklist.push((canonical_root.clone(), "main".to_string()));
        if canonical_root != root_path {
            worklist.push((root_path.to_path_buf(), "main".to_string()));
        }

        // Extra entry points (e.g. for future HTTP / RPC handlers)
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
                // Ensure the canonical path is also marked reachable
                self.reachable_functions.insert((loaded_mod.canonical_path.clone(), func_name.clone()));

                for item in &loaded_mod.program.items {
                    if let ItemKind::Func { ref name, ref body, .. } = item.kind {
                        if name == &func_name {
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
            StmtKind::VarDecl { init, .. } => {
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
            StmtKind::ForIn { iterable, body, .. } => {
                self.scan_expr(iterable, current_mod_path, loaded_mod, worklist);
                self.scan_block(body, current_mod_path, loaded_mod, worklist);
            }
            StmtKind::Switch { expr, cases } => {
                self.scan_expr(expr, current_mod_path, loaded_mod, worklist);
                for case in cases {
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
                // Function used as a value / callback
                if self.is_func_in_module(current_mod_path, name) {
                    worklist.push((current_mod_path.to_path_buf(), name.clone()));
                }
            }
            ExprKind::Call { callee, args, named_args } => {
                // Check if direct local call `foo()`
                if let ExprKind::Ident(ref name) = callee.kind {
                    if self.is_func_in_module(current_mod_path, name) {
                        worklist.push((current_mod_path.to_path_buf(), name.clone()));
                    }
                }
                // Check if qualified call `alias.func()`
                else if let ExprKind::Member { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        if let Some((mod_target, _)) = loaded_mod.imports.get(alias) {
                            match mod_target {
                                ModuleTarget::Local(target_path) => {
                                    worklist.push((target_path.clone(), field.clone()));
                                }
                                ModuleTarget::Native(native_name) => {
                                    self.reachable_native_functions.insert((native_name.clone(), field.clone()));
                                }
                            }
                        }
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
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(ref alias) = target.kind {
                    if let Some((mod_target, _)) = loaded_mod.imports.get(alias) {
                        match mod_target {
                            ModuleTarget::Local(target_path) => {
                                worklist.push((target_path.clone(), field.clone()));
                            }
                            ModuleTarget::Native(native_name) => {
                                self.reachable_native_functions.insert((native_name.clone(), field.clone()));
                            }
                        }
                    }
                } else {
                    // Method call on value
                    self.reachable_native_methods.insert((String::new(), field.clone()));
                    self.scan_expr(target, current_mod_path, loaded_mod, worklist);
                }
            }
            ExprKind::Binary { left, right, .. } => {
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
            ExprKind::Int(_) | ExprKind::String(_) | ExprKind::Bool(_) | ExprKind::Nil => {}
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
}
