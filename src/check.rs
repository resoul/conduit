use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::lexer::Span;
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::NativeRegistry;
use crate::types::Type;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Type error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for TypeError {}

#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub is_exported: bool,
}

#[derive(Debug, Clone)]
pub struct StructSignature {
    pub fields: HashMap<String, Type>,
    pub is_exported: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleInfo {
    pub exports: HashSet<String>,
    pub functions: HashMap<String, FuncSignature>,
    pub structs: HashMap<String, StructSignature>,
    pub errors: HashSet<String>,
    pub imports: HashMap<String, ModuleTarget>,
}

pub struct TypeChecker {
    pub native_registry: NativeRegistry,
    pub module_info: HashMap<PathBuf, ModuleInfo>,
    pub current_module_path: Option<PathBuf>,
    pub functions: HashMap<String, FuncSignature>,
    pub structs: HashMap<String, StructSignature>,
    pub errors: HashSet<String>,
    pub imports: HashMap<String, ModuleTarget>,
    scopes: Vec<HashMap<String, (Type, Span, bool)>>,
    current_return_type: Option<Type>,
    loop_depth: usize,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            native_registry: NativeRegistry::new(),
            module_info: HashMap::new(),
            current_module_path: None,
            functions: HashMap::new(),
            structs: HashMap::new(),
            errors: HashSet::new(),
            imports: HashMap::new(),
            scopes: Vec::new(),
            current_return_type: None,
            loop_depth: 0,
        };

        checker.functions.insert(
            "print".to_string(),
            FuncSignature {
                params: vec![("msg".to_string(), Type::Any)],
                return_type: Type::Void,
                is_exported: false,
            },
        );

        checker
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) -> Result<(), TypeError> {
        if let Some(scope) = self.scopes.pop() {
            for (name, (ty, span, used)) in scope {
                if !used && !name.starts_with('_') && self.contains_error(&ty) {
                    return Err(TypeError {
                        message: format!("Unused variable '{}' of error type '{}' must be handled", name, ty),
                        span,
                    });
                }
            }
        }
        Ok(())
    }

    fn define_variable(&mut self, name: String, ty: Type, span: Span) {
        if let Some(current_scope) = self.scopes.last_mut() {
            current_scope.insert(name, (ty, span, false));
        }
    }

    fn lookup_variable(&mut self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some((ty, _, used)) = scope.get_mut(name) {
                *used = true;
                return Some(ty.clone());
            }
        }
        None
    }

    pub fn is_error_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Custom(name) => {
                if self.errors.contains(name) {
                    return true;
                }
                if let Some((alias, field)) = name.split_once('.') {
                    if let Some(target) = self.imports.get(alias) {
                        match target {
                            ModuleTarget::Native(nat_name) => {
                                if let Some(m) = self.native_registry.get_module(nat_name) {
                                    if let Some(t) = m.types.get(field) {
                                        return t.is_error;
                                    }
                                }
                            }
                            ModuleTarget::Local(path) => {
                                if let Some(info) = self.module_info.get(path) {
                                    return info.errors.contains(field);
                                }
                            }
                        }
                    } else if let Some(m) = self.native_registry.get_module(alias) {
                        if let Some(t) = m.types.get(field) {
                            return t.is_error;
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub fn split_success_and_errors(&self, ty: &Type) -> (Vec<Type>, Vec<Type>) {
        match ty {
            Type::Union(variants) => {
                let mut successes = Vec::new();
                let mut errors = Vec::new();
                for v in variants {
                    if self.is_error_type(v) {
                        errors.push(v.clone());
                    } else {
                        successes.push(v.clone());
                    }
                }
                (successes, errors)
            }
            other => {
                if self.is_error_type(other) {
                    (Vec::new(), vec![other.clone()])
                } else {
                    (vec![other.clone()], Vec::new())
                }
            }
        }
    }

    pub fn contains_error(&self, ty: &Type) -> bool {
        !self.split_success_and_errors(ty).1.is_empty()
    }

    /// Checks if a type exposes any private types defined in the current module.
    fn contains_private_type(&self, ty: &Type, current_mod: Option<&Path>) -> Option<String> {
        match ty {
            Type::Custom(name) => {
                // If it's a simple name without a dot, it refers to a type in the current module
                if !name.contains('.') {
                    let (exists, is_exported) = if let Some(path) = current_mod {
                        if let Some(info) = self.module_info.get(path) {
                            (
                                info.structs.contains_key(name) || info.errors.contains(name),
                                info.exports.contains(name),
                            )
                        } else {
                            (false, false)
                        }
                    } else {
                        (
                            self.structs.contains_key(name) || self.errors.contains(name),
                            self.structs.get(name).map(|s| s.is_exported).unwrap_or(false),
                        )
                    };
                    if exists && !is_exported {
                        return Some(name.clone());
                    }
                }
                None
            }
            Type::List(inner) | Type::Optional(inner) => self.contains_private_type(inner, current_mod),
            Type::Tuple(elems) => {
                for e in elems {
                    if let Some(leak) = self.contains_private_type(e, current_mod) {
                        return Some(leak);
                    }
                }
                None
            }
            Type::Union(vars) => {
                for v in vars {
                    if let Some(leak) = self.contains_private_type(v, current_mod) {
                        return Some(leak);
                    }
                }
                None
            }
            Type::Function { params, return_type } => {
                for p in params {
                    if let Some(leak) = self.contains_private_type(p, current_mod) {
                        return Some(leak);
                    }
                }
                self.contains_private_type(return_type, current_mod)
            }
            _ => None,
        }
    }

    /// Checks all modules in a loaded project.
    pub fn check_project(&mut self, root_path: &Path, loader: &ModuleLoader) -> Result<(), TypeError> {
        // Step 1: Register top-level items for all modules (Pass 1)
        for (path, loaded_mod) in &loader.modules {
            let mut info = ModuleInfo::default();
            for (alias, (target, _)) in &loaded_mod.imports {
                info.imports.insert(alias.clone(), target.clone());
            }

            for item in &loaded_mod.program.items {
                match &item.kind {
                    ItemKind::Type { name, fields } => {
                        let mut field_map = HashMap::new();
                        for f in fields {
                            field_map.insert(f.name.clone(), Type::from_type_expr(&f.ty));
                        }
                        if info.structs.contains_key(name) || info.errors.contains(name) {
                            return Err(TypeError {
                                message: format!("Duplicate type definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.structs.insert(
                            name.clone(),
                            StructSignature {
                                fields: field_map,
                                is_exported: item.is_exported,
                            },
                        );
                    }
                    ItemKind::Error { name, fields } => {
                        let mut field_map = HashMap::new();
                        for f in fields {
                            field_map.insert(f.name.clone(), Type::from_type_expr(&f.ty));
                        }
                        if info.structs.contains_key(name) || info.errors.contains(name) {
                            return Err(TypeError {
                                message: format!("Duplicate error definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.structs.insert(
                            name.clone(),
                            StructSignature {
                                fields: field_map,
                                is_exported: item.is_exported,
                            },
                        );
                        info.errors.insert(name.clone());
                    }
                    ItemKind::Func { name, params, return_type, .. } => {
                        let param_types = params
                            .iter()
                            .map(|p| (p.name.clone(), Type::from_type_expr(&p.ty)))
                            .collect();
                        let ret_type = return_type
                            .as_ref()
                            .map(Type::from_type_expr)
                            .unwrap_or(Type::Void);

                        if info.functions.contains_key(name) {
                            return Err(TypeError {
                                message: format!("Duplicate function definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.functions.insert(
                            name.clone(),
                            FuncSignature {
                                params: param_types,
                                return_type: ret_type,
                                is_exported: item.is_exported,
                            },
                        );
                    }
                    ItemKind::Import { .. } => {}
                }
            }

            self.module_info.insert(path.clone(), info);
        }

        // Step 2: Validate private type leaks for all exported declarations
        for (path, info) in &self.module_info {
            for (fn_name, sig) in &info.functions {
                if sig.is_exported {
                    for (param_name, param_ty) in &sig.params {
                        if let Some(leak) = self.contains_private_type(param_ty, Some(path)) {
                            return Err(TypeError {
                                message: format!(
                                    "Exported function '{}' exposes private type '{}' in parameter '{}'",
                                    fn_name, leak, param_name
                                ),
                                span: Span::new(1, 1),
                            });
                        }
                    }
                    if let Some(leak) = self.contains_private_type(&sig.return_type, Some(path)) {
                        return Err(TypeError {
                            message: format!(
                                "Exported function '{}' exposes private type '{}' in return type",
                                fn_name, leak
                            ),
                            span: Span::new(1, 1),
                        });
                    }
                }
            }

            for (type_name, sig) in &info.structs {
                if sig.is_exported {
                    for (field_name, field_ty) in &sig.fields {
                        if let Some(leak) = self.contains_private_type(field_ty, Some(path)) {
                            return Err(TypeError {
                                message: format!(
                                    "Exported type '{}' exposes private type '{}' in field '{}'",
                                    type_name, leak, field_name
                                ),
                                span: Span::new(1, 1),
                            });
                        }
                    }
                }
            }
        }

        // Step 3: Type check bodies of all modules
        for (path, loaded_mod) in &loader.modules {
            self.current_module_path = Some(path.clone());
            if let Some(info) = self.module_info.get(path) {
                self.functions = info.functions.clone();
                self.structs = info.structs.clone();
                self.errors = info.errors.clone();
                self.imports = info.imports.clone();
            }

            // Always add built-in print
            self.functions.insert(
                "print".to_string(),
                FuncSignature {
                    params: vec![("msg".to_string(), Type::Any)],
                    return_type: Type::Void,
                    is_exported: false,
                },
            );

            for item in &loaded_mod.program.items {
                if let ItemKind::Func { name, body, return_type, .. } = &item.kind {
                    let sig = self.functions.get(name).unwrap().clone();
                    let ret_ty = return_type
                        .as_ref()
                        .map(Type::from_type_expr)
                        .unwrap_or(Type::Void);

                    let can_fallthrough_void = match &ret_ty {
                        Type::Void => true,
                        Type::Union(vars) => vars.contains(&Type::Void),
                        _ => false,
                    };
                    if !can_fallthrough_void && !block_always_returns(body) {
                        return Err(TypeError {
                            message: format!(
                                "Function '{}' must return a value of type '{}' on all reachable paths",
                                name, ret_ty
                            ),
                            span: item.span,
                        });
                    }

                    self.current_return_type = Some(sig.return_type.clone());

                    self.enter_scope();
                    for (param_name, param_ty) in sig.params {
                        self.define_variable(param_name, param_ty, item.span);
                    }

                    self.check_block(body)?;
                    self.exit_scope()?;

                    self.current_return_type = None;
                }
            }
        }

        self.current_module_path = Some(root_path.to_path_buf());
        Ok(())
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            match &item.kind {
                ItemKind::Type { name, fields } => {
                    let mut field_map = HashMap::new();
                    for f in fields {
                        field_map.insert(f.name.clone(), Type::from_type_expr(&f.ty));
                    }
                    if self.structs.contains_key(name) || self.errors.contains(name) {
                        return Err(TypeError {
                            message: format!("Duplicate type definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.structs.insert(
                        name.clone(),
                        StructSignature {
                            fields: field_map,
                            is_exported: item.is_exported,
                        },
                    );
                }
                ItemKind::Error { name, fields } => {
                    let mut field_map = HashMap::new();
                    for f in fields {
                        field_map.insert(f.name.clone(), Type::from_type_expr(&f.ty));
                    }
                    if self.structs.contains_key(name) || self.errors.contains(name) {
                        return Err(TypeError {
                            message: format!("Duplicate error definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.structs.insert(
                        name.clone(),
                        StructSignature {
                            fields: field_map,
                            is_exported: item.is_exported,
                        },
                    );
                    self.errors.insert(name.clone());
                }
                ItemKind::Func { name, params, return_type, .. } => {
                    let param_types = params
                        .iter()
                        .map(|p| (p.name.clone(), Type::from_type_expr(&p.ty)))
                        .collect();
                    let ret_type = return_type
                        .as_ref()
                        .map(Type::from_type_expr)
                        .unwrap_or(Type::Void);

                    if self.functions.contains_key(name) {
                        return Err(TypeError {
                            message: format!("Duplicate function definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.functions.insert(
                        name.clone(),
                        FuncSignature {
                            params: param_types,
                            return_type: ret_type,
                            is_exported: item.is_exported,
                        },
                    );
                }
                ItemKind::Import { path, alias } => {
                    if path.starts_with("cdt.") {
                        self.imports.insert(alias.clone(), ModuleTarget::Native(path.clone()));
                    } else {
                        self.imports.insert(alias.clone(), ModuleTarget::Local(PathBuf::from(path)));
                    }
                }
            }
        }

        for item in &program.items {
            if let ItemKind::Func { name, body, return_type, .. } = &item.kind {
                let sig = self.functions.get(name).unwrap().clone();
                let ret_ty = return_type
                    .as_ref()
                    .map(Type::from_type_expr)
                    .unwrap_or(Type::Void);

                let can_fallthrough_void = match &ret_ty {
                    Type::Void => true,
                    Type::Union(vars) => vars.contains(&Type::Void),
                    _ => false,
                };
                if !can_fallthrough_void && !block_always_returns(body) {
                    return Err(TypeError {
                        message: format!("Function '{}' must return a value of type '{}' on all reachable paths", name, ret_ty),
                        span: item.span,
                    });
                }

                self.current_return_type = Some(sig.return_type.clone());

                self.enter_scope();
                for (param_name, param_ty) in sig.params {
                    self.define_variable(param_name, param_ty, item.span);
                }

                self.check_block(body)?;
                self.exit_scope()?;

                self.current_return_type = None;
            }
        }

        Ok(())
    }

    fn check_block(&mut self, block: &Block) -> Result<(), TypeError> {
        for stmt in &block.stmts {
            self.check_stmt(stmt)?;
        }
        Ok(())
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Define { name, value } => {
                let val_ty = self.check_expr(value)?;
                self.define_variable(name.clone(), val_ty, stmt.span);
            }
            StmtKind::DestructureDefine { pattern, value } => {
                let val_ty = self.check_expr(value)?;
                match &pattern.kind {
                    PatternKind::Tuple(pats) => {
                        let elem_types = match val_ty {
                            Type::Tuple(elems) => elems,
                            other => {
                                return Err(TypeError {
                                    message: format!("Cannot destructure non-tuple type '{}'", other),
                                    span: pattern.span,
                                });
                            }
                        };
                        if pats.len() != elem_types.len() {
                            return Err(TypeError {
                                message: format!("Tuple size mismatch: expected {} elements, got {}", elem_types.len(), pats.len()),
                                span: pattern.span,
                            });
                        }
                        for (i, (pat, elem_ty)) in pats.iter().zip(elem_types.iter()).enumerate() {
                            match &pat.kind {
                                PatternKind::Wildcard => {
                                    if self.contains_error(elem_ty) {
                                        return Err(TypeError {
                                            message: format!("Cannot discard element at index {} with error type '{}' using wildcard '_'", i, elem_ty),
                                            span: pat.span,
                                        });
                                    }
                                }
                                PatternKind::Ident(var_name) => {
                                    self.define_variable(var_name.clone(), elem_ty.clone(), pat.span);
                                }
                                _ => {
                                    return Err(TypeError {
                                        message: "Unsupported pattern in destructuring definition".to_string(),
                                        span: pat.span,
                                    });
                                }
                            }
                        }
                    }
                    _ => {
                        return Err(TypeError {
                            message: "Destructuring definition requires a tuple pattern".to_string(),
                            span: pattern.span,
                        });
                    }
                }
            }
            StmtKind::VarDecl { name, ty, init } => {
                let declared_ty = ty.as_ref().map(Type::from_type_expr);
                if let Some(init_expr) = init {
                    let init_ty = self.check_expr(init_expr)?;
                    if let Some(ref target) = declared_ty {
                        if !init_ty.is_assignable_to(target) {
                            return Err(TypeError {
                                message: format!("Type mismatch: variable declared as {}, initialized with {}", target, init_ty),
                                span: init_expr.span,
                            });
                        }
                    }
                    self.define_variable(name.clone(), declared_ty.unwrap_or(init_ty), stmt.span);
                } else {
                    let ty = declared_ty.ok_or_else(|| TypeError {
                        message: format!("Variable '{}' must have an explicit type or default value", name),
                        span: stmt.span,
                    })?;
                    self.define_variable(name.clone(), ty, stmt.span);
                }
            }
            StmtKind::Assign { target, value } => {
                let target_ty = self.check_expr(target)?;
                let value_ty = self.check_expr(value)?;
                if !value_ty.is_assignable_to(&target_ty) {
                    return Err(TypeError {
                        message: format!("Cannot assign value of type {} to target of type {}", value_ty, target_ty),
                        span: value.span,
                    });
                }
            }
            StmtKind::Return(opt_expr) => {
                let actual_ty = if let Some(expr) = opt_expr {
                    self.check_expr(expr)?
                } else {
                    Type::Void
                };

                if let Some(ref expected) = self.current_return_type {
                    if !actual_ty.is_assignable_to(expected) {
                        return Err(TypeError {
                            message: format!("Type mismatch: function expects return type {}, but got {}", expected, actual_ty),
                            span: stmt.span,
                        });
                    }
                }
            }
            StmtKind::If { condition, then_block, else_block } => {
                let cond_ty = self.check_expr(condition)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError {
                        message: format!("Condition in 'if' must be Bool, got {}", cond_ty),
                        span: condition.span,
                    });
                }
                self.enter_scope();
                self.check_block(then_block)?;
                self.exit_scope()?;

                if let Some(eb) = else_block {
                    self.enter_scope();
                    self.check_block(eb)?;
                    self.exit_scope()?;
                }
            }
            StmtKind::Guard { condition, else_block } => {
                let cond_ty = self.check_expr(condition)?;
                if cond_ty != Type::Bool {
                    return Err(TypeError {
                        message: format!("Condition in 'guard' must be Bool, got {}", cond_ty),
                        span: condition.span,
                    });
                }
                self.enter_scope();
                self.check_block(else_block)?;
                self.exit_scope()?;

                if !block_always_returns(else_block) {
                    return Err(TypeError {
                        message: "Guard 'else' block must terminate with a 'return' statement".to_string(),
                        span: else_block.span,
                    });
                }
            }
            StmtKind::ForIn { variable, iterable, body } => {
                let iter_ty = self.check_expr(iterable)?;
                let item_ty = match iter_ty {
                    Type::List(inner) => *inner,
                    other => {
                        return Err(TypeError {
                            message: format!("Expected list in 'for in' loop, got {}", other),
                            span: iterable.span,
                        });
                    }
                };

                self.loop_depth += 1;
                self.enter_scope();
                self.define_variable(variable.clone(), item_ty, stmt.span);
                let res = self.check_block(body);
                let exit_res = self.exit_scope();
                self.loop_depth -= 1;
                res?;
                exit_res?;
            }
            StmtKind::Switch { expr, cases } => {
                let expr_ty = self.check_expr(expr)?;
                let mut remaining = match &expr_ty {
                    Type::Union(variants) => variants.clone(),
                    other => vec![other.clone()],
                };

                for case in cases {
                    self.enter_scope();
                    match &case.pattern.kind {
                        PatternKind::Nil => {
                            let idx = remaining.iter().position(|t| t == &Type::Nil).ok_or_else(|| TypeError {
                                message: format!("Redundant or impossible case 'nil' for type '{}'", expr_ty),
                                span: case.pattern.span,
                            })?;
                            remaining.remove(idx);
                        }
                        PatternKind::Type { name, binding } => {
                            let custom_ty = match name.as_str() {
                                "Int" => Type::Int,
                                "String" => Type::String,
                                "Bool" => Type::Bool,
                                "Void" => Type::Void,
                                other => {
                                    let item_suffix = other.split('.').last().unwrap_or(other);
                                    let matched = remaining.iter().find(|t| match t {
                                        Type::Custom(c) => {
                                            let c_suffix = c.split('.').last().unwrap_or(c);
                                            c == other
                                                || c.ends_with(&format!(".{}", other))
                                                || other.ends_with(&format!(".{}", c))
                                                || (item_suffix == c_suffix && (!c.contains('.') || !other.contains('.') || {
                                                    let (a1, _) = c.split_once('.').unwrap();
                                                    let (a2, _) = other.split_once('.').unwrap();
                                                    a1 == a2 || self.imports.get(a2).map(|t| match t {
                                                        ModuleTarget::Native(n) => n == "cdt.time" && a1 == "time",
                                                        _ => false,
                                                    }).unwrap_or(false)
                                                }))
                                        }
                                        _ => false,
                                    }).cloned();
                                    matched.unwrap_or_else(|| Type::Custom(other.to_string()))
                                }
                            };
                            let idx = remaining.iter().position(|t| t == &custom_ty).ok_or_else(|| TypeError {
                                message: format!("Redundant or impossible case '{}(...)' for type '{}'", name, expr_ty),
                                span: case.pattern.span,
                            })?;
                            remaining.remove(idx);
                            if let Some(scope) = self.scopes.last_mut() {
                                scope.insert(binding.clone(), (custom_ty, case.pattern.span, true));
                            }
                        }
                        PatternKind::Tuple(pats) => {
                            let mut matched_indices = Vec::new();
                            let mut matched_tuples = Vec::new();
                            for (i, t) in remaining.iter().enumerate() {
                                if let Type::Tuple(elems) = t {
                                    if elems.len() == pats.len() {
                                        matched_indices.push(i);
                                        matched_tuples.push(elems.clone());
                                    }
                                }
                            }
                            if matched_indices.is_empty() {
                                return Err(TypeError {
                                    message: format!("Redundant or impossible tuple pattern for type '{}'", expr_ty),
                                    span: case.pattern.span,
                                });
                            }
                            for &idx in matched_indices.iter().rev() {
                                remaining.remove(idx);
                            }

                            for pos in 0..pats.len() {
                                let pos_types: Vec<Type> = matched_tuples.iter().map(|t| t[pos].clone()).collect();
                                let combined_ty = Type::union(pos_types);
                                match &pats[pos].kind {
                                    PatternKind::Ident(var_name) => {
                                        self.define_variable(var_name.clone(), combined_ty, pats[pos].span);
                                    }
                                    PatternKind::Wildcard => {}
                                    _ => {
                                        return Err(TypeError {
                                            message: "Unsupported nested pattern in switch tuple".to_string(),
                                            span: pats[pos].span,
                                        });
                                    }
                                }
                            }
                        }
                        PatternKind::Ident(binding) => {
                            if remaining.is_empty() {
                                return Err(TypeError {
                                    message: format!("Redundant case '{}' for type '{}'", binding, expr_ty),
                                    span: case.pattern.span,
                                });
                            }
                            let bound_ty = Type::union(remaining.clone());
                            remaining.clear();
                            self.define_variable(binding.clone(), bound_ty, case.pattern.span);
                        }
                        PatternKind::Wildcard => {
                            if remaining.is_empty() {
                                return Err(TypeError {
                                    message: format!("Redundant wildcard case for type '{}'", expr_ty),
                                    span: case.pattern.span,
                                });
                            }
                            remaining.clear();
                        }
                    }

                    self.check_block(&case.body)?;
                    self.exit_scope()?;
                }

                if !remaining.is_empty() {
                    let missing = remaining.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
                    return Err(TypeError {
                        message: format!("Non-exhaustive switch: missing cases for {}", missing),
                        span: stmt.span,
                    });
                }
            }
            StmtKind::Break => {
                if self.loop_depth == 0 {
                    return Err(TypeError {
                        message: "'break' outside of loop".to_string(),
                        span: stmt.span,
                    });
                }
            }
            StmtKind::Continue => {
                if self.loop_depth == 0 {
                    return Err(TypeError {
                        message: "'continue' outside of loop".to_string(),
                        span: stmt.span,
                    });
                }
            }
            StmtKind::Expr(expr) => {
                let ty = self.check_expr(expr)?;
                if self.contains_error(&ty) {
                    return Err(TypeError {
                        message: format!("Expression result of type '{}' contains an error and must not be discarded", ty),
                        span: expr.span,
                    });
                }
            }
        }
        Ok(())
    }

    fn check_method_call(
        &mut self,
        target_ty: &Type,
        field: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<Type, TypeError> {
        if target_ty == &Type::String {
            match field {
                "len" if args.is_empty() => return Ok(Type::Int),
                "trim" | "lower" | "upper" if args.is_empty() => return Ok(Type::String),
                "contains" if args.len() == 1 => {
                    let arg_ty = self.check_expr(&args[0])?;
                    if arg_ty == Type::String {
                        return Ok(Type::Bool);
                    }
                }
                _ => {}
            }
        }

        if let Type::List(inner) = target_ty {
            match field {
                "len" if args.is_empty() => return Ok(Type::Int),
                "append" | "contains" if args.len() == 1 => {
                    let arg_ty = self.check_expr(&args[0])?;
                    if arg_ty.is_assignable_to(inner) {
                        return Ok(if field == "append" { Type::Void } else { Type::Bool });
                    }
                }
                _ => {}
            }
        }

        if let Type::Custom(type_name) = target_ty {
            let short_name = type_name.split('.').last().unwrap_or(type_name);
            let method_def = {
                let mut found = None;
                for m in self.native_registry.modules() {
                    if let Some(md) = m.methods.get(&(short_name, field)) {
                        found = Some(md.clone());
                        break;
                    }
                }
                found
            };

            if let Some(method_def) = method_def {
                if args.len() != method_def.params.len() {
                    return Err(TypeError {
                        message: format!(
                            "Method '{}.{}()' expects {} arguments, got {}",
                            type_name, field, method_def.params.len(), args.len()
                        ),
                        span,
                    });
                }
                for (i, (arg_expr, (_, expected_ty))) in args.iter().zip(method_def.params.iter()).enumerate() {
                    let actual_ty = self.check_expr(arg_expr)?;
                    if !actual_ty.is_assignable_to(expected_ty) {
                        return Err(TypeError {
                            message: format!(
                                "Argument {} of method '{}.{}()' expects {}, got {}",
                                i + 1, type_name, field, expected_ty, actual_ty
                            ),
                            span: arg_expr.span,
                        });
                    }
                }
                return Ok(method_def.return_type.clone());
            }
        }

        Err(TypeError {
            message: format!("Unknown method or invalid arguments for '{}.{}()'", target_ty, field),
            span,
        })
    }

    fn resolve_module_member(
        &mut self,
        alias: &str,
        mod_target: &ModuleTarget,
        field: &str,
        span: Span,
    ) -> Result<Type, TypeError> {
        match mod_target {
            ModuleTarget::Native(nat_name) => {
                let m = self.native_registry.get_module(nat_name).ok_or_else(|| TypeError {
                    message: format!("Unknown native module '{}'", nat_name),
                    span,
                })?;

                if let Some(f) = m.functions.get(field) {
                    let p = f.params.iter().map(|p| p.1.clone()).collect();
                    return Ok(Type::Function {
                        params: p,
                        return_type: Box::new(f.return_type.clone()),
                    });
                }

                if let Some(t) = m.types.get(field) {
                    return Ok(Type::Custom(t.qualified_name.to_string()));
                }

                Err(TypeError {
                    message: format!("Module '{}' has no exported item '{}'", nat_name, field),
                    span,
                })
            }
            ModuleTarget::Local(path) => {
                let info = self.module_info.get(path).cloned().ok_or_else(|| TypeError {
                    message: format!("Module '{}' not loaded", path.display()),
                    span,
                })?;

                if let Some(f) = info.functions.get(field) {
                    if !f.is_exported {
                        return Err(TypeError {
                            message: format!("Item '{}' is private in module '{}'", field, alias),
                            span,
                        });
                    }
                    let p = f.params.iter().map(|p| p.1.clone()).collect();
                    return Ok(Type::Function {
                        params: p,
                        return_type: Box::new(f.return_type.clone()),
                    });
                }

                if let Some(s) = info.structs.get(field) {
                    if !s.is_exported {
                        return Err(TypeError {
                            message: format!("Item '{}' is private in module '{}'", field, alias),
                            span,
                        });
                    }
                    return Ok(Type::Custom(format!("{}.{}", alias, field)));
                }

                Err(TypeError {
                    message: format!("Module '{}' has no exported item '{}'", alias, field),
                    span,
                })
            }
        }
    }

    pub fn check_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::String(_) => Ok(Type::String),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Nil => Ok(Type::Nil),
            ExprKind::Ident(name) => {
                if let Some(ty) = self.lookup_variable(name) {
                    Ok(ty)
                } else if let Some(sig) = self.functions.get(name).cloned() {
                    let param_types = sig.params.iter().map(|p| p.1.clone()).collect();
                    Ok(Type::Function {
                        params: param_types,
                        return_type: Box::new(sig.return_type),
                    })
                } else if self.structs.contains_key(name) {
                    Ok(Type::Custom(name.clone()))
                } else {
                    Err(TypeError {
                        message: format!("Unknown variable '{}'", name),
                        span: expr.span,
                    })
                }
            }
            ExprKind::List(elements) => {
                if elements.is_empty() {
                    return Ok(Type::List(Box::new(Type::Any)));
                }

                let first_ty = self.check_expr(&elements[0])?;
                for elem in &elements[1..] {
                    let elem_ty = self.check_expr(elem)?;
                    if !elem_ty.is_assignable_to(&first_ty) {
                        return Err(TypeError {
                            message: format!("List elements must have compatible types: expected {}, got {}", first_ty, elem_ty),
                            span: elem.span,
                        });
                    }
                }
                Ok(Type::List(Box::new(first_ty)))
            }
            ExprKind::Unary { op, expr: inner } => {
                let inner_ty = self.check_expr(inner)?;
                match op {
                    UnaryOp::Neg => {
                        if inner_ty == Type::Int {
                            Ok(Type::Int)
                        } else {
                            Err(TypeError {
                                message: format!("Unary '-' applies to Int only, got {}", inner_ty),
                                span: inner.span,
                            })
                        }
                    }
                    UnaryOp::Not => {
                        if inner_ty == Type::Bool {
                            Ok(Type::Bool)
                        } else {
                            Err(TypeError {
                                message: format!("Unary '!' applies to Bool only, got {}", inner_ty),
                                span: inner.span,
                            })
                        }
                    }
                }
            }
            ExprKind::Binary { op, left, right } => {
                let left_ty = self.check_expr(left)?;
                let right_ty = self.check_expr(right)?;

                match op {
                    BinaryOp::Add => {
                        if left_ty == Type::Int && right_ty == Type::Int {
                            Ok(Type::Int)
                        } else if left_ty == Type::String && right_ty == Type::String {
                            Ok(Type::String)
                        } else {
                            Err(TypeError {
                                message: format!("Operator '+' requires matching Int or String operands, got {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                    BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                        if left_ty == Type::Int && right_ty == Type::Int {
                            Ok(Type::Int)
                        } else {
                            Err(TypeError {
                                message: format!("Arithmetic operator requires Int operands, got {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                    BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                        if left_ty == Type::Int && right_ty == Type::Int {
                            Ok(Type::Bool)
                        } else {
                            Err(TypeError {
                                message: format!("Comparison operator requires Int operands, got {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                    BinaryOp::Eq | BinaryOp::NotEq => {
                        if left_ty.is_assignable_to(&right_ty) || right_ty.is_assignable_to(&left_ty) {
                            Ok(Type::Bool)
                        } else {
                            Err(TypeError {
                                message: format!("Cannot compare incompatible types {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                    BinaryOp::And | BinaryOp::Or => {
                        if left_ty == Type::Bool && right_ty == Type::Bool {
                            Ok(Type::Bool)
                        } else {
                            Err(TypeError {
                                message: format!("Logical operator requires Bool operands, got {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                    BinaryOp::Coalesce => match left_ty {
                        Type::Optional(inner) => {
                            if right_ty.is_assignable_to(&inner) {
                                Ok(*inner)
                            } else {
                                Err(TypeError {
                                    message: format!(
                                        "Coalesce operator '??' default value must match optional inner type {}, got {}",
                                        inner, right_ty
                                    ),
                                    span: expr.span,
                                })
                            }
                        }
                        Type::Nil => Ok(right_ty),
                        other => {
                            if right_ty.is_assignable_to(&other) {
                                Ok(other)
                            } else {
                                Err(TypeError {
                                    message: format!(
                                        "Operator '??' requires compatible operands, got {} and {}",
                                        other, right_ty
                                    ),
                                    span: expr.span,
                                })
                            }
                        }
                    },
                    BinaryOp::Range => {
                        if left_ty == Type::Int && right_ty == Type::Int {
                            Ok(Type::List(Box::new(Type::Int)))
                        } else {
                            Err(TypeError {
                                message: format!("Range '..' requires Int bounds, got {} and {}", left_ty, right_ty),
                                span: expr.span,
                            })
                        }
                    }
                }
            }
            ExprKind::Tuple(elements) => {
                let mut types = Vec::new();
                for e in elements {
                    types.push(self.check_expr(e)?);
                }
                Ok(Type::Tuple(types))
            }
            ExprKind::Try(inner) => {
                let inner_ty = self.check_expr(inner)?;
                let (successes, errors) = self.split_success_and_errors(&inner_ty);
                if errors.is_empty() {
                    return Err(TypeError {
                        message: format!("Operator '?' is only allowed on types containing error variants, got '{}'", inner_ty),
                        span: expr.span,
                    });
                }
                if successes.is_empty() {
                    return Err(TypeError {
                        message: format!("Operator '?' requires at least one success variant, got '{}'", inner_ty),
                        span: expr.span,
                    });
                }

                if let Some(ref current_ret) = self.current_return_type {
                    for err in &errors {
                        if !err.is_assignable_to(current_ret) {
                            return Err(TypeError {
                                message: format!("Error '{}' propagated by '?' is not assignable to function return type '{}'", err, current_ret),
                                span: expr.span,
                            });
                        }
                    }
                } else {
                    return Err(TypeError {
                        message: "Operator '?' cannot be used outside of a function".to_string(),
                        span: expr.span,
                    });
                }

                Ok(Type::union(successes))
            }
            ExprKind::Call { callee, args, named_args } => {
                // 1. Direct local struct constructor
                if let ExprKind::Ident(ref type_name) = callee.kind {
                    if let Some(struct_sig) = self.structs.get(type_name).cloned() {
                        if !args.is_empty() {
                            return Err(TypeError {
                                message: format!("Type '{}' must be constructed using named arguments", type_name),
                                span: callee.span,
                            });
                        }
                        let mut provided_fields = HashSet::new();
                        for arg in named_args {
                            if !provided_fields.insert(arg.name.clone()) {
                                return Err(TypeError {
                                    message: format!("Duplicate field '{}' in construction of '{}'", arg.name, type_name),
                                    span: callee.span,
                                });
                            }
                            let expected_field_ty = struct_sig.fields.get(&arg.name).ok_or_else(|| TypeError {
                                message: format!("Type '{}' has no field '{}'", type_name, arg.name),
                                span: callee.span,
                            })?;
                            let actual_arg_ty = self.check_expr(&arg.value)?;
                            if !actual_arg_ty.is_assignable_to(expected_field_ty) {
                                return Err(TypeError {
                                    message: format!("Field '{}' of type '{}' expects {}, got {}", arg.name, type_name, expected_field_ty, actual_arg_ty),
                                    span: arg.value.span,
                                });
                            }
                        }
                        for required_field in struct_sig.fields.keys() {
                            if !provided_fields.contains(required_field) {
                                return Err(TypeError {
                                    message: format!("Missing required field '{}' in construction of '{}'", required_field, type_name),
                                    span: callee.span,
                                });
                            }
                        }
                        return Ok(Type::Custom(type_name.clone()));
                    }

                    // map(list, callback)
                    if type_name == "map" {
                        if args.len() != 2 {
                            return Err(TypeError {
                                message: format!("'map' takes 2 arguments (list, callback), got {}", args.len()),
                                span: expr.span,
                            });
                        }
                        let list_ty = self.check_expr(&args[0])?;
                        let item_ty = match list_ty {
                            Type::List(inner) => *inner,
                            other => {
                                return Err(TypeError {
                                    message: format!("First argument to 'map' must be a List, got '{}'", other),
                                    span: args[0].span,
                                });
                            }
                        };
                        let callback_ty = self.check_expr(&args[1])?;
                        if let Type::Function { params, return_type } = callback_ty {
                            if params.len() != 1 {
                                return Err(TypeError {
                                    message: format!("Function passed to 'map' must take 1 argument, but takes {}", params.len()),
                                    span: args[1].span,
                                });
                            }
                            if !item_ty.is_assignable_to(&params[0]) {
                                return Err(TypeError {
                                    message: format!("List element type '{}' is not assignable to parameter type '{}'", item_ty, params[0]),
                                    span: args[1].span,
                                });
                            }
                            return Ok(Type::List(Box::new(*return_type)));
                        } else {
                            return Err(TypeError {
                                message: "Second argument to 'map' must be a callable function".to_string(),
                                span: args[1].span,
                            });
                        }
                    }

                    // filter(list, callback)
                    if type_name == "filter" {
                        if args.len() != 2 {
                            return Err(TypeError {
                                message: format!("'filter' takes 2 arguments (list, callback), got {}", args.len()),
                                span: expr.span,
                            });
                        }
                        let list_ty = self.check_expr(&args[0])?;
                        let item_ty = match list_ty {
                            Type::List(inner) => *inner,
                            other => {
                                return Err(TypeError {
                                    message: format!("First argument to 'filter' must be a List, got '{}'", other),
                                    span: args[0].span,
                                });
                            }
                        };
                        let callback_ty = self.check_expr(&args[1])?;
                        if let Type::Function { params, return_type } = callback_ty {
                            if params.len() != 1 {
                                return Err(TypeError {
                                    message: format!("Function passed to 'filter' must take 1 argument, but takes {}", params.len()),
                                    span: args[1].span,
                                });
                            }
                            if !item_ty.is_assignable_to(&params[0]) {
                                return Err(TypeError {
                                    message: format!("List element type '{}' is not assignable to parameter type '{}'", item_ty, params[0]),
                                    span: args[1].span,
                                });
                            }
                            if *return_type != Type::Bool {
                                return Err(TypeError {
                                    message: format!("Function passed to 'filter' must return 'Bool', got '{}'", return_type),
                                    span: args[1].span,
                                });
                            }
                            return Ok(Type::List(Box::new(item_ty)));
                        } else {
                            return Err(TypeError {
                                message: "Second argument to 'filter' must be a callable function".to_string(),
                                span: args[1].span,
                            });
                        }
                    }
                }

                // 2. Check if callee is an imported module member call or constructor: `alias.foo(...)`
                if let ExprKind::Member { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        if let Some(mod_target) = self.imports.get(alias).cloned() {
                            match mod_target {
                                ModuleTarget::Native(nat_name) => {
                                    let (is_type, func_def) = {
                                        let m = self.native_registry.get_module(&nat_name).ok_or_else(|| TypeError {
                                            message: format!("Unknown native module '{}'", nat_name),
                                            span: callee.span,
                                        })?;
                                        (m.types.contains_key(field.as_str()), m.functions.get(field.as_str()).cloned())
                                    };

                                    if is_type {
                                        return Err(TypeError {
                                            message: format!("Native type '{}.{}' cannot be constructed directly", alias, field),
                                            span: callee.span,
                                        });
                                    }

                                    if let Some(f) = func_def {
                                        if !named_args.is_empty() {
                                            return Err(TypeError {
                                                message: format!("Function '{}.{}' does not accept named arguments", alias, field),
                                                span: callee.span,
                                            });
                                        }
                                        if args.len() != f.params.len() {
                                            return Err(TypeError {
                                                message: format!("Function '{}.{}' expects {} arguments, got {}", alias, field, f.params.len(), args.len()),
                                                span: expr.span,
                                            });
                                        }
                                        for (i, (arg_expr, (_, expected_ty))) in args.iter().zip(f.params.iter()).enumerate() {
                                            let actual_ty = self.check_expr(arg_expr)?;
                                            if !actual_ty.is_assignable_to(expected_ty) {
                                                return Err(TypeError {
                                                    message: format!("Argument {} of function '{}.{}' expects {}, got {}", i + 1, alias, field, expected_ty, actual_ty),
                                                    span: arg_expr.span,
                                                });
                                            }
                                        }
                                        return Ok(f.return_type);
                                    }
                                }
                                ModuleTarget::Local(path) => {
                                    let info = self.module_info.get(&path).cloned().ok_or_else(|| TypeError {
                                        message: format!("Module '{}' not loaded", path.display()),
                                        span: callee.span,
                                    })?;

                                    // Check constructor of imported struct / error
                                    if let Some(struct_sig) = info.structs.get(field) {
                                        if !struct_sig.is_exported {
                                            return Err(TypeError {
                                                message: format!("Type '{}' is private in module '{}'", field, alias),
                                                span: callee.span,
                                            });
                                        }
                                        if !args.is_empty() {
                                            return Err(TypeError {
                                                message: format!("Type '{}.{}' must be constructed using named arguments", alias, field),
                                                span: callee.span,
                                            });
                                        }
                                        let mut provided_fields = HashSet::new();
                                        for arg in named_args {
                                            if !provided_fields.insert(arg.name.clone()) {
                                                return Err(TypeError {
                                                    message: format!("Duplicate field '{}' in construction of '{}.{}'", arg.name, alias, field),
                                                    span: callee.span,
                                                });
                                            }
                                            let expected_field_ty = struct_sig.fields.get(&arg.name).ok_or_else(|| TypeError {
                                                message: format!("Type '{}.{}' has no field '{}'", alias, field, arg.name),
                                                span: callee.span,
                                            })?;
                                            let actual_arg_ty = self.check_expr(&arg.value)?;
                                            if !actual_arg_ty.is_assignable_to(expected_field_ty) {
                                                return Err(TypeError {
                                                    message: format!("Field '{}' expects {}, got {}", arg.name, expected_field_ty, actual_arg_ty),
                                                    span: arg.value.span,
                                                });
                                            }
                                        }
                                        for required_field in struct_sig.fields.keys() {
                                            if !provided_fields.contains(required_field) {
                                                return Err(TypeError {
                                                    message: format!("Missing required field '{}' in construction of '{}.{}'", required_field, alias, field),
                                                    span: callee.span,
                                                });
                                            }
                                        }
                                        return Ok(Type::Custom(format!("{}.{}", alias, field)));
                                    }

                                    // Check imported function call
                                    if let Some(fn_sig) = info.functions.get(field) {
                                        if !fn_sig.is_exported {
                                            return Err(TypeError {
                                                message: format!("Function '{}' is private in module '{}'", field, alias),
                                                span: callee.span,
                                            });
                                        }
                                        if args.len() != fn_sig.params.len() {
                                            return Err(TypeError {
                                                message: format!("Function '{}.{}' expects {} arguments, got {}", alias, field, fn_sig.params.len(), args.len()),
                                                span: expr.span,
                                            });
                                        }
                                        for (i, (arg_expr, (_, expected_ty))) in args.iter().zip(fn_sig.params.iter()).enumerate() {
                                            let actual_ty = self.check_expr(arg_expr)?;
                                            if !actual_ty.is_assignable_to(expected_ty) {
                                                return Err(TypeError {
                                                    message: format!("Argument {} expects {}, got {}", i + 1, expected_ty, actual_ty),
                                                    span: arg_expr.span,
                                                });
                                            }
                                        }
                                        return Ok(fn_sig.return_type.clone());
                                    }
                                }
                            }
                        }
                    }

                    // 3. Method call on value
                    let target_ty = self.check_expr(target)?;
                    return self.check_method_call(&target_ty, field, args, callee.span);
                }

                // 4. Function call (by name or through function-typed expression)
                let callee_ty = self.check_expr(callee)?;
                if let Type::Function { params, return_type } = callee_ty {
                    if !named_args.is_empty() {
                        return Err(TypeError {
                            message: "Function calls do not support named arguments".to_string(),
                            span: callee.span,
                        });
                    }
                    if args.len() != params.len() {
                        return Err(TypeError {
                            message: format!("Function expects {} arguments, got {}", params.len(), args.len()),
                            span: expr.span,
                        });
                    }
                    for (i, (arg_expr, expected_ty)) in args.iter().zip(params.iter()).enumerate() {
                        let actual_ty = self.check_expr(arg_expr)?;
                        if !actual_ty.is_assignable_to(expected_ty) {
                            return Err(TypeError {
                                message: format!("Argument {} expects {}, got {}", i + 1, expected_ty, actual_ty),
                                span: arg_expr.span,
                            });
                        }
                    }
                    return Ok(*return_type);
                }

                Err(TypeError {
                    message: format!("Cannot call non-function expression of type '{}'", callee_ty),
                    span: callee.span,
                })
            }
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(ref alias) = target.kind {
                    if let Some(mod_target) = self.imports.get(alias).cloned() {
                        return self.resolve_module_member(alias, &mod_target, field, expr.span);
                    }
                }

                let target_ty = self.check_expr(target)?;

                if let Type::Union(_) = target_ty {
                    return Err(TypeError {
                        message: format!("Cannot access field '{}' on union type '{}' without narrowing", field, target_ty),
                        span: expr.span,
                    });
                }

                if let Type::Custom(ref struct_name) = target_ty {
                    if let Some(struct_sig) = self.structs.get(struct_name) {
                        if let Some(field_ty) = struct_sig.fields.get(field) {
                            return Ok(field_ty.clone());
                        } else {
                            return Err(TypeError {
                                message: format!("Type '{}' has no field '{}'", struct_name, field),
                                span: expr.span,
                            });
                        }
                    }

                    // Check if imported struct: `alias.Type`
                    if let Some((alias, item_name)) = struct_name.split_once('.') {
                        if let Some(mod_target) = self.imports.get(alias) {
                            match mod_target {
                                ModuleTarget::Local(path) => {
                                    if let Some(info) = self.module_info.get(path) {
                                        if let Some(struct_sig) = info.structs.get(item_name) {
                                            if let Some(field_ty) = struct_sig.fields.get(field) {
                                                return Ok(field_ty.clone());
                                            }
                                        }
                                    }
                                }
                                ModuleTarget::Native(nat_name) => {
                                    if let Some(m) = self.native_registry.get_module(nat_name) {
                                        if let Some(t) = m.types.get(item_name) {
                                            for (f_name, f_ty) in &t.fields {
                                                if *f_name == field {
                                                    return Ok(f_ty.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if let Some(m) = self.native_registry.get_module(alias) {
                            if let Some(t) = m.types.get(item_name) {
                                for (f_name, f_ty) in &t.fields {
                                    if *f_name == field {
                                        return Ok(f_ty.clone());
                                    }
                                }
                            }
                        }
                    } else {
                        // Check if struct_name is an exported type from any imported module
                        for (_, mod_target) in &self.imports {
                            match mod_target {
                                ModuleTarget::Local(path) => {
                                    if let Some(info) = self.module_info.get(path) {
                                        if let Some(struct_sig) = info.structs.get(struct_name) {
                                            if struct_sig.is_exported {
                                                if let Some(field_ty) = struct_sig.fields.get(field) {
                                                    return Ok(field_ty.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                                ModuleTarget::Native(nat_name) => {
                                    if let Some(m) = self.native_registry.get_module(nat_name) {
                                        if let Some(t) = m.types.get(struct_name.as_str()) {
                                            for (f_name, f_ty) in &t.fields {
                                                if *f_name == field {
                                                    return Ok(f_ty.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Err(TypeError {
                    message: format!("Type {} has no field '{}'", target_ty, field),
                    span: expr.span,
                })
            }
        }
    }
}

fn block_always_returns(block: &Block) -> bool {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Return(_) => return true,
            StmtKind::If {
                then_block,
                else_block: Some(else_b),
                ..
            } => {
                if block_always_returns(then_block) && block_always_returns(else_b) {
                    return true;
                }
            }
            StmtKind::Switch { cases, .. } => {
                if !cases.is_empty() && cases.iter().all(|c| block_always_returns(&c.body)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::loader::ModuleLoader;
    use std::path::PathBuf;

    fn check_source(source: &str) -> Result<(), TypeError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize_all().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut checker = TypeChecker::new();
        checker.check_program(&program)
    }

    #[test]
    fn test_guard_valid_return() {
        let src = "func f(x: Int) -> Int {\n    guard x > 0 else {\n        return 0\n    }\n    return x\n}\n";
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_guard_missing_return_error() {
        let src = "func f(x: Int) -> Int {\n    guard x > 0 else {\n        var a = 1\n    }\n    return x\n}\n";
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("must terminate with a 'return'"));
    }

    #[test]
    fn test_coalesce_valid() {
        let src = "func f(x: Int?) -> Int {\n    return x ?? 10\n}\n";
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_coalesce_type_mismatch() {
        let src = "func f(x: Int?) -> Int {\n    return x ?? \"wrong\"\n}\n";
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Coalesce operator '??' default value must match optional inner type"));
    }

    #[test]
    fn test_break_outside_loop_error() {
        let src = "func f() {\n    break\n}\n";
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("'break' outside of loop"));
    }

    #[test]
    fn test_continue_outside_loop_error() {
        let src = "func f() {\n    continue\n}\n";
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("'continue' outside of loop"));
    }

    #[test]
    fn test_pipe_valid() {
        let src = "func double(x: Int) -> Int { return x * 2 }\nfunc f() -> Int { return 5 |> double() }\n";
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_pipe_type_mismatch() {
        let src = "func double(x: Int) -> Int { return x * 2 }\nfunc f() -> Int { return \"str\" |> double() }\n";
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("expects Int"));
    }

    #[test]
    fn test_error_and_union_typecheck() {
        let src = r#"
            type User { name: String }
            error InvalidUserName { message: String }

            func create_user(name: String) -> User | InvalidUserName {
                guard name.trim().len() > 0 else {
                    return InvalidUserName(message: "Name is required")
                }
                return User(name: name)
            }

            func register(name: String) -> User | InvalidUserName {
                user := create_user(name)?
                return user
            }

            func main() {
                res := register("Alice")
                switch res {
                    case User(u):
                        print(u.name)
                    case InvalidUserName(e):
                        print(e.message)
                }
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_switch_non_exhaustive_error() {
        let src = r#"
            type User { name: String }
            error InvalidUserName { message: String }

            func main() {
                var res: User | InvalidUserName = User(name: "Alice")
                switch res {
                    case User(u):
                        print(u.name)
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Non-exhaustive switch: missing cases for InvalidUserName"));
    }

    #[test]
    fn test_switch_redundant_case_error() {
        let src = r#"
            type User { name: String }

            func main() {
                var u = User(name: "Alice")
                switch u {
                    case User(a):
                        print(a.name)
                    case nil:
                        print("nil")
                }
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Redundant or impossible case 'nil'"));
    }

    #[test]
    fn test_union_field_access_disallowed_without_narrowing() {
        let src = r#"
            type User { name: String }
            error InvalidUserName { message: String }

            func test_access(res: User | InvalidUserName) {
                print(res.name)
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("without narrowing"));
    }

    #[test]
    fn test_try_operator_unhandled_error_type_mismatch() {
        let src = r#"
            type User { name: String }
            error InvalidUserName { message: String }
            error AnotherError { code: Int }

            func get_user() -> User | InvalidUserName {
                return User(name: "Alice")
            }

            func caller() -> User | AnotherError {
                u := get_user()?
                return u
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("propagated by '?' is not assignable to function return type"));
    }

    #[test]
    fn test_try_on_pure_optional_rejected() {
        let src = r#"
            type User { name: String }

            func get_opt() -> User? {
                return nil
            }

            func caller() -> User {
                u := get_opt()?
                return u
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Operator '?' is only allowed on types containing error variants"));
    }

    #[test]
    fn test_try_preserves_nil() {
        let src = r#"
            type User { name: String }
            error Err { msg: String }

            func get_user_opt() -> (User?) | Err {
                return nil
            }

            func caller() -> User? | Err {
                u := get_user_opt()?
                return u
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_tuples_creation_destructuring_and_switch() {
        let src = r#"
            error DivisionByZero {}

            func divide(a: Int, b: Int) -> (Int, Int) | DivisionByZero {
                guard b != 0 else {
                    return DivisionByZero()
                }
                return (a / b, a % b)
            }

            func quotient(a: Int, b: Int) -> Int | DivisionByZero {
                (val, _) := divide(a, b)?
                return val
            }

            func main() {
                switch divide(17, 5) {
                    case (q, r):
                        print("{q}, {r}")
                    case DivisionByZero(e):
                        print("Cannot divide by zero")
                }
            }
        "#;
        if let Err(e) = check_source(src) {
            panic!("Test failed with error: {}", e);
        }
    }

    #[test]
    fn test_tuple_wildcard_cannot_discard_error() {
        let src = r#"
            error Bad { id: Int }

            func get_pair() -> (Int, Bad) {
                return (1, Bad(id: 10))
            }

            func main() {
                (num, _) := get_pair()
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Cannot discard element at index 1 with error type 'Bad' using wildcard '_'"));
    }

    #[test]
    fn test_discarded_error_statement() {
        let src = r#"
            error Bad { id: Int }

            func produce_error() -> Bad {
                return Bad(id: 1)
            }

            func main() {
                produce_error()
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("contains an error and must not be discarded"));
    }

    #[test]
    fn test_map_and_filter_compatibility() {
        let src = r#"
            type User { name: String }
            error InvalidName { msg: String }

            func make_user(n: String) -> User | InvalidName {
                return User(name: n)
            }

            func is_long(n: String) -> Bool {
                return n.len() > 3
            }

            func main() {
                names := ["Alice", "Bob"]
                users := map(names, make_user)
                filtered := filter(names, is_long)
            }
        "#;
        assert!(check_source(src).is_ok());
    }

    #[test]
    fn test_filter_rejects_error_callback() {
        let src = r#"
            error CheckErr { code: Int }

            func bad_check(n: String) -> Bool | CheckErr {
                return true
            }

            func main() {
                names := ["Alice", "Bob"]
                filtered := filter(names, bad_check)
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("must return 'Bool'"));
    }

    #[test]
    fn test_unused_error_variable_diagnosed() {
        let src = r#"
            error MyError { msg: String }

            func main() {
                err := MyError(msg: "oops")
            }
        "#;
        let err = check_source(src).unwrap_err();
        assert!(err.message.contains("Unused variable 'err' of error type 'MyError'"));
    }

    #[test]
    fn test_private_type_leakage_in_exported_function() {
        let mut loader = ModuleLoader::new();
        let root = PathBuf::from("/virtual/main.cdt");
        loader.with_virtual_file(&root, r#"
            type SecretConfig { key: String }

            export func get_config() -> SecretConfig {
                return SecretConfig(key: "secret")
            }

            func main() {}
        "#);

        let root_canon = loader.load_project(&root).unwrap();
        let mut checker = TypeChecker::new();
        let err = checker.check_project(&root_canon, &loader).unwrap_err();

        assert!(err.to_string().contains("Exported function 'get_config' exposes private type 'SecretConfig'"));
    }

    #[test]
    fn test_cannot_access_private_item_from_imported_module() {
        let mut loader = ModuleLoader::new();
        let helper = PathBuf::from("/virtual/helper.cdt");
        let root = PathBuf::from("/virtual/main.cdt");

        loader.with_virtual_file(&helper, r#"
            func secret() -> Int {
                return 42
            }

            export func public_fn() -> Int {
                return secret()
            }
        "#);

        loader.with_virtual_file(&root, r#"
            import "./helper" as helper

            func main() {
                x := helper.secret()
            }
        "#);

        let root_canon = loader.load_project(&root).unwrap();
        let mut checker = TypeChecker::new();
        let err = checker.check_project(&root_canon, &loader).unwrap_err();

        assert!(err.to_string().contains("is private in module 'helper'"));
    }

    #[test]
    fn test_cyclic_import_detection() {
        let mut loader = ModuleLoader::new();
        let a = PathBuf::from("/virtual/a.cdt");
        let b = PathBuf::from("/virtual/b.cdt");

        loader.with_virtual_file(&a, r#"
            import "./b" as b
            export func fa() {}
        "#);

        loader.with_virtual_file(&b, r#"
            import "./a" as a
            export func fb() {}
        "#);

        let err = loader.load_project(&a).unwrap_err();
        assert!(err.to_string().contains("Cyclic import detected"));
    }
}
