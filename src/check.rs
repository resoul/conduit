use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::lexer::Span;
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::NativeRegistry;
use crate::types::{ModuleId, Type, TypeId};

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
    pub is_validator: bool,
}

#[derive(Debug, Clone)]
pub struct StructSignature {
    pub fields: HashMap<String, Type>,
    pub field_annotations: HashMap<String, Vec<crate::ast::FieldAnnotation>>,
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
                is_validator: false,
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

    fn current_module_id(&self) -> ModuleId {
        match &self.current_module_path {
            Some(path) => ModuleId::Local(path.clone()),
            None => ModuleId::Program,
        }
    }

    fn current_type_id(&self, name: &str) -> TypeId {
        TypeId {
            module: self.current_module_id(),
            name: name.to_string(),
        }
    }

    fn resolve_type_name_in_ctx(
        &self,
        name: &str,
        current_module: &ModuleId,
        imports: &HashMap<String, ModuleTarget>,
        span: Span,
    ) -> Result<Type, TypeError> {
        match name {
            "Int" => Ok(Type::Int),
            "String" => Ok(Type::String),
            "Bool" => Ok(Type::Bool),
            "Void" => Ok(Type::Void),
            _ => {
                if let Some((alias, item_name)) = name.split_once('.') {
                    let target = imports.get(alias).ok_or_else(|| TypeError {
                        message: format!("Unknown module alias '{}' in type '{}'", alias, name),
                        span,
                    })?;
                    match target {
                        ModuleTarget::Native(nat_name) => {
                            Ok(Type::Custom(TypeId::native(nat_name.clone(), item_name)))
                        }
                        ModuleTarget::Local(path) => {
                            Ok(Type::Custom(TypeId::local(path.clone(), item_name)))
                        }
                    }
                } else if name == "IndexError" {
                    Ok(Type::Custom(TypeId::builtin("IndexError")))
                } else {
                    Ok(Type::Custom(TypeId {
                        module: current_module.clone(),
                        name: name.to_string(),
                    }))
                }
            }
        }
    }

    fn resolve_type_expr_in_ctx(
        &self,
        expr: &TypeExpr,
        current_module: &ModuleId,
        imports: &HashMap<String, ModuleTarget>,
        span: Span,
    ) -> Result<Type, TypeError> {
        match expr {
            TypeExpr::Named(name) => self.resolve_type_name_in_ctx(name, current_module, imports, span),
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_type_expr_in_ctx(
                inner,
                current_module,
                imports,
                span,
            )?))),
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(self.resolve_type_expr_in_ctx(
                inner,
                current_module,
                imports,
                span,
            )?))),
            TypeExpr::Union(variants) => {
                let mut types = Vec::with_capacity(variants.len());
                for v in variants {
                    types.push(self.resolve_type_expr_in_ctx(v, current_module, imports, span)?);
                }
                Ok(Type::union(types))
            }
            TypeExpr::Tuple(elements) => {
                let mut types = Vec::with_capacity(elements.len());
                for e in elements {
                    types.push(self.resolve_type_expr_in_ctx(e, current_module, imports, span)?);
                }
                Ok(Type::Tuple(types))
            }
            TypeExpr::Func { params, return_type } => {
                let mut p = Vec::with_capacity(params.len());
                for param in params {
                    p.push(self.resolve_type_expr_in_ctx(param, current_module, imports, span)?);
                }
                let r = Box::new(self.resolve_type_expr_in_ctx(return_type, current_module, imports, span)?);
                Ok(Type::Function {
                    params: p,
                    return_type: r,
                })
            }
        }
    }

    fn resolve_type_expr(&self, expr: &TypeExpr, span: Span) -> Result<Type, TypeError> {
        let mod_id = self.current_module_id();
        self.resolve_type_expr_in_ctx(expr, &mod_id, &self.imports, span)
    }

    pub fn is_error_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Custom(type_id) => match &type_id.module {
                ModuleId::Builtin => type_id.name == "IndexError",
                ModuleId::Program => self.errors.contains(&type_id.name),
                ModuleId::Local(path) => {
                    if Some(path) == self.current_module_path.as_ref() {
                        self.errors.contains(&type_id.name)
                    } else {
                        self.module_info
                            .get(path)
                            .map(|info| info.errors.contains(&type_id.name))
                            .unwrap_or(false)
                    }
                }
                ModuleId::Native(nat_name) => self
                    .native_registry
                    .get_module(nat_name)
                    .and_then(|m| m.types.get(type_id.name.as_str()))
                    .map(|t| t.is_error)
                    .unwrap_or(false),
            },
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

    fn contains_private_type(&self, ty: &Type, current_mod: Option<&Path>) -> Option<String> {
        match ty {
            Type::Custom(type_id) => match &type_id.module {
                ModuleId::Local(path) => {
                    if current_mod.map(|c| c == path.as_path()).unwrap_or(false) {
                        if let Some(info) = self.module_info.get(path) {
                            let exists = info.structs.contains_key(&type_id.name)
                                || info.errors.contains(&type_id.name);
                            let is_exported = info.exports.contains(&type_id.name);
                            if exists && !is_exported {
                                return Some(type_id.name.clone());
                            }
                        }
                    }
                    None
                }
                ModuleId::Program => {
                    let exists = self.structs.contains_key(&type_id.name)
                        || self.errors.contains(&type_id.name);
                    let is_exported = self
                        .structs
                        .get(&type_id.name)
                        .map(|s| s.is_exported)
                        .unwrap_or(false);
                    if exists && !is_exported {
                        Some(type_id.name.clone())
                    } else {
                        None
                    }
                }
                ModuleId::Builtin | ModuleId::Native(_) => None,
            },
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

    pub fn check_project(&mut self, root_path: &Path, loader: &ModuleLoader) -> Result<(), TypeError> {
        for (path, loaded_mod) in &loader.modules {
            let mod_id = ModuleId::Local(path.clone());
            let mut info = ModuleInfo::default();
            for (alias, (target, _)) in &loaded_mod.imports {
                info.imports.insert(alias.clone(), target.clone());
            }

            for item in &loaded_mod.program.items {
                match &item.kind {
                    ItemKind::Type { name, fields } => {
                        let mut field_map = HashMap::new();
                        let mut ann_map = HashMap::new();
                        for f in fields {
                            let resolved_ty =
                                self.resolve_type_expr_in_ctx(&f.ty, &mod_id, &info.imports, item.span)?;
                            field_map.insert(f.name.clone(), resolved_ty);
                            ann_map.insert(f.name.clone(), f.annotations.clone());
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
                                field_annotations: ann_map,
                                is_exported: item.is_exported,
                            },
                        );
                    }
                    ItemKind::Error { name, fields } => {
                        let mut field_map = HashMap::new();
                        let mut ann_map = HashMap::new();
                        for f in fields {
                            let resolved_ty =
                                self.resolve_type_expr_in_ctx(&f.ty, &mod_id, &info.imports, item.span)?;
                            field_map.insert(f.name.clone(), resolved_ty);
                            ann_map.insert(f.name.clone(), f.annotations.clone());
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
                                field_annotations: ann_map,
                                is_exported: item.is_exported,
                            },
                        );
                        info.errors.insert(name.clone());
                    }
                    ItemKind::Func { name, params, return_type, is_validator, .. } => {
                        let mut param_types = Vec::with_capacity(params.len());
                        for p in params {
                            let resolved_ty =
                                self.resolve_type_expr_in_ctx(&p.ty, &mod_id, &info.imports, item.span)?;
                            param_types.push((p.name.clone(), resolved_ty));
                        }
                        let ret_type = match return_type.as_ref() {
                            Some(rt) => self.resolve_type_expr_in_ctx(rt, &mod_id, &info.imports, item.span)?,
                            None => Type::Void,
                        };

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
                                is_validator: *is_validator,
                            },
                        );
                    }
                    ItemKind::Import { .. } => {}
                }
            }

            self.module_info.insert(path.clone(), info);
        }

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

        for (path, loaded_mod) in &loader.modules {
            self.current_module_path = Some(path.clone());
            if let Some(info) = self.module_info.get(path) {
                self.functions = info.functions.clone();
                self.structs = info.structs.clone();
                self.errors = info.errors.clone();
                self.imports = info.imports.clone();
            }

            self.functions.insert(
                "print".to_string(),
                FuncSignature {
                    params: vec![("msg".to_string(), Type::Any)],
                    return_type: Type::Void,
                    is_exported: false,
                    is_validator: false,
                },
            );

            for item in &loaded_mod.program.items {
                if let ItemKind::Func { name, body, is_validator, params, .. } = &item.kind {
                    let sig = self.functions.get(name).unwrap().clone();
                    let ret_ty = sig.return_type.clone();

                    if *is_validator {
                        if params.is_empty() {
                            return Err(TypeError {
                                message: format!("Validator function '{}' must have at least one parameter (the value to validate)", name),
                                span: item.span,
                            });
                        }
                        let ret = sig.return_type.clone();
                        let has_void = match &ret {
                            Type::Void => true,
                            Type::Union(vars) => vars.contains(&Type::Void),
                            _ => false,
                        };
                        let issue_id = TypeId::native("cdt.validation", "Issue");
                        let has_issue = match &ret {
                            Type::Custom(id) => id == &issue_id,
                            Type::Union(vars) => vars.iter().any(|v| matches!(v, Type::Custom(id) if id == &issue_id)),
                            _ => false,
                        };
                        let is_valid_ret = match &ret {
                            Type::Union(vars) if vars.len() == 2 => has_void && has_issue,
                            _ => false,
                        };
                        if !is_valid_ret {
                            return Err(TypeError {
                                message: format!("Validator function '{}' must return 'Void | validation.Issue', got '{}'", name, ret),
                                span: item.span,
                            });
                        }
                    }

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

                if let ItemKind::Type { fields, .. } = &item.kind {
                    for f in fields {
                        for ann in &f.annotations {
                            self.check_field_annotation(&f.name, f, ann)?;
                        }
                    }
                }
            }
        }

        self.current_module_path = Some(root_path.to_path_buf());
        Ok(())
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            if let ItemKind::Import { path, alias } = &item.kind {
                if path.starts_with("cdt.") {
                    self.imports.insert(alias.clone(), ModuleTarget::Native(path.clone()));
                } else {
                    self.imports.insert(alias.clone(), ModuleTarget::Local(PathBuf::from(path)));
                }
            }
        }

        for item in &program.items {
            match &item.kind {
                ItemKind::Type { name, fields } => {
                    let mut field_map = HashMap::new();
                    let mut ann_map = HashMap::new();
                    for f in fields {
                        field_map.insert(f.name.clone(), self.resolve_type_expr(&f.ty, item.span)?);
                        ann_map.insert(f.name.clone(), f.annotations.clone());
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
                            field_annotations: ann_map,
                            is_exported: item.is_exported,
                        },
                    );
                }
                ItemKind::Error { name, fields } => {
                    let mut field_map = HashMap::new();
                    let mut ann_map = HashMap::new();
                    for f in fields {
                        field_map.insert(f.name.clone(), self.resolve_type_expr(&f.ty, item.span)?);
                        ann_map.insert(f.name.clone(), f.annotations.clone());
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
                            field_annotations: ann_map,
                            is_exported: item.is_exported,
                        },
                    );
                    self.errors.insert(name.clone());
                }
                ItemKind::Func { name, params, return_type, is_validator, .. } => {
                    let mut param_types = Vec::with_capacity(params.len());
                    for p in params {
                        param_types.push((p.name.clone(), self.resolve_type_expr(&p.ty, item.span)?));
                    }
                    let ret_type = match return_type.as_ref() {
                        Some(rt) => self.resolve_type_expr(rt, item.span)?,
                        None => Type::Void,
                    };

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
                            is_validator: *is_validator,
                        },
                    );
                }
                ItemKind::Import { .. } => {}
            }
        }

        for item in &program.items {
            if let ItemKind::Func { name, body, is_validator, params, .. } = &item.kind {
                let sig = self.functions.get(name).unwrap().clone();
                let ret_ty = sig.return_type.clone();

                if *is_validator {
                    if params.is_empty() {
                        return Err(TypeError {
                            message: format!("Validator function '{}' must have at least one parameter (the value to validate)", name),
                            span: item.span,
                        });
                    }
                    let ret = sig.return_type.clone();
                    let has_void = match &ret {
                        Type::Void => true,
                        Type::Union(vars) => vars.contains(&Type::Void),
                        _ => false,
                    };
                    let issue_id = TypeId::native("cdt.validation", "Issue");
                    let has_issue = match &ret {
                        Type::Custom(id) => id == &issue_id,
                        Type::Union(vars) => vars.iter().any(|v| matches!(v, Type::Custom(id) if id == &issue_id)),
                        _ => false,
                    };
                    if !has_void || !has_issue {
                        return Err(TypeError {
                            message: format!("Validator function '{}' must return 'Void | validation.Issue', got '{}'", name, ret),
                            span: item.span,
                        });
                    }
                }

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

            if let ItemKind::Type { fields, .. } = &item.kind {
                for f in fields {
                    for ann in &f.annotations {
                        self.check_field_annotation(&f.name, f, ann)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn check_field_annotation(
        &mut self,
        field_name: &str,
        field: &crate::ast::FieldDef,
        ann: &crate::ast::FieldAnnotation,
    ) -> Result<(), TypeError> {
        let (target_alias, func_name) = if let Some((alias, name)) = ann.target.split_once('.') {
            (Some(alias), name)
        } else {
            (None, ann.target.as_str())
        };

        let (params, _return_type, is_validator, is_exported) = if let Some(alias) = target_alias {
            if let Some(target) = self.imports.get(alias) {
                match target {
                    ModuleTarget::Native(nat_mod) => {
                        let m = self.native_registry.get_module(nat_mod).ok_or_else(|| TypeError {
                            message: format!("Unknown native module '{}'", nat_mod),
                            span: ann.span,
                        })?;
                        let f = m.functions.get(func_name).ok_or_else(|| TypeError {
                            message: format!("Native module '{}' has no validator '{}'", alias, func_name),
                            span: ann.span,
                        })?;
                        (
                            f.params.iter().map(|(p, t)| (p.to_string(), t.clone())).collect::<Vec<_>>(),
                            f.return_type.clone(),
                            f.is_validator,
                            true,
                        )
                    }
                    ModuleTarget::Local(path) => {
                        let info = self.module_info.get(path).ok_or_else(|| TypeError {
                            message: format!("Cannot resolve imported module '{}'", alias),
                            span: ann.span,
                        })?;
                        let sig = info.functions.get(func_name).ok_or_else(|| TypeError {
                            message: format!("Module '{}' has no function '{}'", alias, func_name),
                            span: ann.span,
                        })?;
                        (
                            sig.params.clone(),
                            sig.return_type.clone(),
                            sig.is_validator,
                            sig.is_exported,
                        )
                    }
                }
            } else {
                return Err(TypeError {
                    message: format!("Unknown module alias '{}' in annotation '@{}'", alias, ann.target),
                    span: ann.span,
                });
            }
        } else {
            if let Some(sig) = self.functions.get(func_name) {
                (sig.params.clone(), sig.return_type.clone(), sig.is_validator, sig.is_exported)
            } else {
                return Err(TypeError {
                    message: format!("Unknown validator '{}' referenced in field annotation", ann.target),
                    span: ann.span,
                });
            }
        };

        if target_alias.is_some() && !is_exported {
            return Err(TypeError {
                message: format!("Cannot use private validator '{}' in field annotation", ann.target),
                span: ann.span,
            });
        }

        if !is_validator {
            return Err(TypeError {
                message: format!(
                    "Function '{}' cannot be used as an annotation because it is not marked with '[#validator]'",
                    ann.target
                ),
                span: ann.span,
            });
        }

        let expected_extra = if params.is_empty() { 0 } else { params.len() - 1 };
        if ann.args.len() != expected_extra {
            return Err(TypeError {
                message: format!(
                    "Validator '{}' expects {} additional argument(s), but {} were provided",
                    ann.target, expected_extra, ann.args.len()
                ),
                span: ann.span,
            });
        }

        let raw_field_ty = self.resolve_type_expr(&field.ty, ann.span)?;
        let effective_field_ty = match raw_field_ty {
            Type::Optional(inner) => *inner,
            other => other,
        };
        let first_param_ty = &params[0].1;
        if !effective_field_ty.is_assignable_to(first_param_ty) {
            return Err(TypeError {
                message: format!(
                    "Field '{}' of type '{}' is not compatible with validator parameter type '{}'",
                    field_name, effective_field_ty, first_param_ty
                ),
                span: ann.span,
            });
        }

        for (idx, arg_expr) in ann.args.iter().enumerate() {
            let arg_ty = self.check_expr(arg_expr)?;
            let expected_param_ty = &params[idx + 1].1;
            if !arg_ty.is_assignable_to(expected_param_ty) {
                return Err(TypeError {
                    message: format!(
                        "Argument {} for validator '{}': expected type '{}', got '{}'",
                        idx + 1, ann.target, expected_param_ty, arg_ty
                    ),
                    span: arg_expr.span,
                });
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
                let declared_ty = match ty.as_ref() {
                    Some(t) => Some(self.resolve_type_expr(t, stmt.span)?),
                    None => None,
                };
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
                if matches!(target.kind, ExprKind::Index { .. }) {
                    return Err(TypeError {
                        message: "Index assignment 'list[i] = value' is not supported".to_string(),
                        span: target.span,
                    });
                }
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
                let mut remaining = match Type::union(vec![expr_ty.clone()]) {
                    Type::Union(variants) => variants,
                    other => vec![other],
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
                            let mod_id = self.current_module_id();
                            let custom_ty = self.resolve_type_name_in_ctx(
                                name,
                                &mod_id,
                                &self.imports,
                                case.pattern.span,
                            )?;
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
                "get" if args.len() == 1 => {
                    let arg_ty = self.check_expr(&args[0])?;
                    if arg_ty == Type::Int {
                        let res_ty = match inner.as_ref() {
                            Type::Optional(_) => inner.as_ref().clone(),
                            _ => Type::Optional(inner.clone()),
                        };
                        return Ok(res_ty);
                    }
                    return Err(TypeError {
                        message: format!("List method 'get()' expects Int argument, got {}", arg_ty),
                        span: args[0].span,
                    });
                }
                "append" | "contains" if args.len() == 1 => {
                    let arg_ty = self.check_expr(&args[0])?;
                    if arg_ty.is_assignable_to(inner) {
                        return Ok(if field == "append" { Type::Void } else { Type::Bool });
                    }
                }
                _ => {}
            }
        }

        if let Type::Custom(type_id) = target_ty {
            let method_def = if let ModuleId::Native(ref nat_name) = type_id.module {
                self.native_registry
                    .get_module(nat_name)
                    .and_then(|m| m.methods.get(&(type_id.name.as_str(), field)))
                    .cloned()
            } else {
                None
            };

            if let Some(method_def) = method_def {
                if args.len() != method_def.params.len() {
                    return Err(TypeError {
                        message: format!(
                            "Method '{}.{}()' expects {} arguments, got {}",
                            type_id, field, method_def.params.len(), args.len()
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
                                i + 1, type_id, field, expected_ty, actual_ty
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
                    return Ok(Type::Custom(TypeId::native(m.name, t.name)));
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
                    return Ok(Type::Custom(TypeId::local(path.clone(), field)));
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
                    Ok(Type::Custom(self.current_type_id(name)))
                } else if name == "IndexError" {
                    Ok(Type::Custom(TypeId::builtin("IndexError")))
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
            ExprKind::Index { target, index } => {
                let target_ty = self.check_expr(target)?;
                let elem_ty = match target_ty {
                    Type::List(inner) => *inner,
                    other => {
                        return Err(TypeError {
                            message: format!("Cannot index into non-list type '{}'", other),
                            span: target.span,
                        });
                    }
                };
                let index_ty = self.check_expr(index)?;
                if index_ty != Type::Int {
                    return Err(TypeError {
                        message: format!("List index must be Int, got '{}'", index_ty),
                        span: index.span,
                    });
                }
                Ok(Type::union(vec![elem_ty, Type::Custom(TypeId::builtin("IndexError"))]))
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
                if let ExprKind::Ident(ref type_name) = callee.kind {
                    let builtin_index_error = if type_name == "IndexError" && !self.structs.contains_key("IndexError") {
                        let mut fields = HashMap::new();
                        fields.insert("index".to_string(), Type::Int);
                        fields.insert("length".to_string(), Type::Int);
                        fields.insert("message".to_string(), Type::String);
                        Some(StructSignature {
                            fields,
                            field_annotations: HashMap::new(),
                            is_exported: true,
                        })
                    } else {
                        None
                    };
                    let direct_struct = self
                        .structs
                        .get(type_name)
                        .cloned()
                        .map(|sig| (self.current_type_id(type_name), sig))
                        .or_else(|| builtin_index_error.map(|sig| (TypeId::builtin("IndexError"), sig)));

                    if let Some((type_id, struct_sig)) = direct_struct {
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
                        return Ok(Type::Custom(type_id));
                    }
                    
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
                
                if let ExprKind::Member { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        if let Some(mod_target) = self.imports.get(alias).cloned() {
                            match mod_target {
                                ModuleTarget::Native(nat_name) => {
                                    let (type_def, func_def) = {
                                        let m = self.native_registry.get_module(&nat_name).ok_or_else(|| TypeError {
                                            message: format!("Unknown native module '{}'", nat_name),
                                            span: callee.span,
                                        })?;
                                        (m.types.get(field.as_str()).cloned(), m.functions.get(field.as_str()).cloned())
                                    };

                                    if let Some(ty_def) = type_def {
                                        if ty_def.fields.is_empty() {
                                            return Err(TypeError {
                                                message: format!("Native type '{}.{}' cannot be constructed directly", alias, field),
                                                span: callee.span,
                                            });
                                        }
                                        if !args.is_empty() {
                                            return Err(TypeError {
                                                message: format!("Constructor for '{}.{}' does not accept positional arguments", alias, field),
                                                span: expr.span,
                                            });
                                        }
                                        let mut provided_fields = HashSet::new();
                                        for na in named_args {
                                            let arg_ty = self.check_expr(&na.value)?;
                                            if let Some((_, expected_ty)) = ty_def.fields.iter().find(|(name, _)| *name == na.name.as_str()) {
                                                if !arg_ty.is_assignable_to(expected_ty) {
                                                    return Err(TypeError {
                                                        message: format!("Type mismatch for field '{}' in construction of '{}.{}': expected {}, got {}", na.name, alias, field, expected_ty, arg_ty),
                                                        span: na.value.span,
                                                    });
                                                }
                                                provided_fields.insert(na.name.clone());
                                            } else {
                                                return Err(TypeError {
                                                    message: format!("Unknown field '{}' in construction of '{}.{}'", na.name, alias, field),
                                                    span: na.value.span,
                                                });
                                            }
                                        }
                                        for (required_field, _) in &ty_def.fields {
                                            if !provided_fields.contains(*required_field) {
                                                return Err(TypeError {
                                                    message: format!("Missing required field '{}' in construction of '{}.{}'", required_field, alias, field),
                                                    span: callee.span,
                                                });
                                            }
                                        }
                                        return Ok(Type::Custom(TypeId::native(nat_name.clone(), field)));
                                    }

                                    if let Some(f) = func_def {
                                        if nat_name == "cdt.validation" && field == "check" {
                                            if !named_args.is_empty() {
                                                return Err(TypeError {
                                                    message: format!("Function '{}.{}' does not accept named arguments", alias, field),
                                                    span: callee.span,
                                                });
                                            }
                                            if args.len() != 1 {
                                                return Err(TypeError {
                                                    message: format!("Function '{}.{}' expects 1 argument, got {}", alias, field, args.len()),
                                                    span: expr.span,
                                                });
                                            }
                                            let val_ty = self.check_expr(&args[0])?;
                                            let err_ty = Type::Custom(TypeId::native("cdt.validation", "ValidationError"));
                                            return Ok(Type::union(vec![val_ty, err_ty]));
                                        }

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
                                        return Ok(Type::Custom(TypeId::local(path.clone(), field)));
                                    }

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

                    let target_ty = self.check_expr(target)?;
                    return self.check_method_call(&target_ty, field, args, callee.span);
                }
                
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

                if let Type::Custom(ref type_id) = target_ty {
                    match &type_id.module {
                        ModuleId::Builtin => {
                            if type_id.name == "IndexError" {
                                match field.as_str() {
                                    "index" | "length" => return Ok(Type::Int),
                                    "message" => return Ok(Type::String),
                                    _ => {}
                                }
                            }
                        }
                        ModuleId::Program => {
                            if let Some(struct_sig) = self.structs.get(&type_id.name) {
                                if let Some(field_ty) = struct_sig.fields.get(field) {
                                    return Ok(field_ty.clone());
                                }
                            }
                        }
                        ModuleId::Local(path) => {
                            if Some(path) == self.current_module_path.as_ref() {
                                if let Some(struct_sig) = self.structs.get(&type_id.name) {
                                    if let Some(field_ty) = struct_sig.fields.get(field) {
                                        return Ok(field_ty.clone());
                                    }
                                }
                            } else if let Some(info) = self.module_info.get(path) {
                                if let Some(struct_sig) = info.structs.get(&type_id.name) {
                                    if let Some(field_ty) = struct_sig.fields.get(field) {
                                        return Ok(field_ty.clone());
                                    }
                                }
                            }
                        }
                        ModuleId::Native(nat_name) => {
                            if let Some(m) = self.native_registry.get_module(nat_name) {
                                if let Some(t) = m.types.get(type_id.name.as_str()) {
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
