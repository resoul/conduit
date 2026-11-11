use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ast::*;
use crate::lexer::Span;
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::json::{
    decode_node_to_conduit, encode_conduit_value, make_decode_error, make_encode_error, parse_json,
};
use crate::native::NativeRegistry;
use crate::types::{ModuleId, Type, TypeId};
use crate::value::{
    make_index_error, make_key_error, make_parse_error, ClosureData, MapKey, OrderedMap, Value,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Runtime error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for RuntimeError {}

enum ControlFlow {
    Next,
    Return(Value),
    Break,
    Continue,
}

enum InterpFlow {
    Return(Value),
    Error(RuntimeError),
}

impl From<RuntimeError> for InterpFlow {
    fn from(e: RuntimeError) -> Self {
        InterpFlow::Error(e)
    }
}

pub struct Interpreter {
    pub native_registry: NativeRegistry,
    module_functions: HashMap<(Option<PathBuf>, String), (Vec<Param>, Block)>,
    module_imports: HashMap<Option<PathBuf>, HashMap<String, ModuleTarget>>,
    module_structs: HashMap<Option<PathBuf>, HashSet<String>>,
    module_struct_type_params: HashMap<(Option<PathBuf>, String), Vec<String>>,
    module_struct_fields: HashMap<(Option<PathBuf>, String), Vec<crate::ast::FieldDef>>,
    module_errors: HashMap<Option<PathBuf>, HashSet<String>>,
    current_module: Option<PathBuf>,
    env: Vec<HashMap<String, Value>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            native_registry: NativeRegistry::new(),
            module_functions: HashMap::new(),
            module_imports: HashMap::new(),
            module_structs: HashMap::new(),
            module_struct_type_params: HashMap::new(),
            module_struct_fields: HashMap::new(),
            module_errors: HashMap::new(),
            current_module: None,
            env: vec![HashMap::new()],
        }
    }

    fn enter_scope(&mut self) {
        self.env.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.env.pop();
    }

    fn define_variable(&mut self, name: String, val: Value) {
        if let Some(scope) = self.env.last_mut() {
            scope.insert(name, val);
        }
    }

    fn assign_variable(&mut self, name: &str, val: Value, span: Span) -> Result<(), RuntimeError> {
        for scope in self.env.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), val);
                return Ok(());
            }
        }
        Err(RuntimeError {
            message: format!("Cannot assign to undefined variable '{}'", name),
            span,
        })
    }

    fn lookup_variable(&self, name: &str) -> Option<Value> {
        for scope in self.env.iter().rev() {
            if let Some(val) = scope.get(name) {
                return Some(val.clone());
            }
        }
        None
    }

    fn lookup_variable_mut(&mut self, name: &str) -> Option<&mut Value> {
        for scope in self.env.iter_mut().rev() {
            if let Some(val) = scope.get_mut(name) {
                return Some(val);
            }
        }
        None
    }

    fn mutate_place<F>(
        &mut self,
        place: &Expr,
        mutate_fn: F,
    ) -> Result<Value, RuntimeError>
    where
        F: FnOnce(&mut Value) -> Result<Value, RuntimeError>,
    {
        let mut fields = Vec::new();
        let mut cur = place;
        let root_name = loop {
            match &cur.kind {
                ExprKind::Member { target, field } => {
                    fields.push((field.clone(), cur.span));
                    cur = target;
                }
                ExprKind::Ident(name) => break name.clone(),
                _ => {
                    return Err(RuntimeError {
                        message: "Invalid assignment target".to_string(),
                        span: place.span,
                    });
                }
            }
        };
        fields.reverse();

        let mut slot = self.lookup_variable_mut(&root_name).ok_or_else(|| RuntimeError {
            message: format!("Cannot assign to undefined variable '{}'", root_name),
            span: cur.span,
        })?;

        for (field_name, fspan) in fields {
            match slot {
                Value::Struct { fields: map, .. } => {
                    slot = map.get_mut(&field_name).ok_or_else(|| RuntimeError {
                        message: format!("Field '{}' not found", field_name),
                        span: fspan,
                    })?;
                }
                other => {
                    return Err(RuntimeError {
                        message: format!("Field access not supported on type {}", other.type_name()),
                        span: fspan,
                    });
                }
            }
        }

        mutate_fn(slot)
    }

    fn bind_for_pattern(&mut self, pattern: &Pattern, val: Value) {
        match &pattern.kind {
            PatternKind::Ident(var_name) => {
                self.define_variable(var_name.clone(), val);
            }
            PatternKind::Wildcard => {}
            PatternKind::Tuple(sub_pats) => {
                if let Value::Tuple(items) = val {
                    for (sub_pat, item) in sub_pats.iter().zip(items.into_iter()) {
                        self.bind_for_pattern(sub_pat, item);
                    }
                }
            }
            _ => {}
        }
    }

    fn snapshot_env(&self) -> HashMap<String, Value> {
        let mut captured = HashMap::new();
        for scope in &self.env {
            for (k, v) in scope {
                captured.insert(k.clone(), v.clone());
            }
        }
        captured
    }

    fn interpolate(&self, template: &str) -> String {
        let mut result = String::new();
        let mut chars = template.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut var_name = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    var_name.push(inner);
                }
                let clean_name = var_name.trim();
                if clean_name.contains('.') {
                    let parts: Vec<&str> = clean_name.split('.').collect();
                    if let Some(mut current) = self.lookup_variable(parts[0]) {
                        let mut resolved = true;
                        for part in &parts[1..] {
                            match current {
                                Value::Struct { ref fields, .. } | Value::Error { ref fields, .. } => {
                                    if let Some(val) = fields.get(*part) {
                                        current = val.clone();
                                    } else {
                                        resolved = false;
                                        break;
                                    }
                                }
                                _ => {
                                    resolved = false;
                                    break;
                                }
                            }
                        }
                        if resolved {
                            result.push_str(&current.to_string());
                            continue;
                        }
                    }
                }

                if let Some(val) = self.lookup_variable(clean_name) {
                    result.push_str(&val.to_string());
                } else {
                    result.push('{');
                    result.push_str(&var_name);
                    result.push('}');
                }
            } else {
                result.push(ch);
            }
        }

        result
    }

    pub fn run_project(&mut self, root_path: &Path, loader: &ModuleLoader) -> Result<(), RuntimeError> {
        for (path, loaded_mod) in &loader.modules {
            let mut imports_map = HashMap::new();
            for (alias, (target, _)) in &loaded_mod.imports {
                imports_map.insert(alias.clone(), target.clone());
            }
            self.module_imports.insert(Some(path.clone()), imports_map);

            let mut structs_set = HashSet::new();
            let mut errors_set = HashSet::new();

            for item in &loaded_mod.program.items {
                match &item.kind {
                    ItemKind::Func { name, params, body, .. } => {
                        self.module_functions.insert(
                            (Some(path.clone()), name.clone()),
                            (params.clone(), body.clone()),
                        );
                    }
                    ItemKind::Type { name, type_params, fields } => {
                        structs_set.insert(name.clone());
                        self.module_struct_type_params
                            .insert((Some(path.clone()), name.clone()), type_params.clone());
                        self.module_struct_fields
                            .insert((Some(path.clone()), name.clone()), fields.clone());
                    }
                    ItemKind::Error { name, .. } => {
                        errors_set.insert(name.clone());
                    }
                    ItemKind::Import { .. } => {}
                }
            }

            self.module_structs.insert(Some(path.clone()), structs_set);
            self.module_errors.insert(Some(path.clone()), errors_set);
        }

        let root_canon = root_path.to_path_buf();
        if !self.module_functions.contains_key(&(Some(root_canon.clone()), "main".to_string())) {
            return Err(RuntimeError {
                message: "Entry point 'func main()' not found in root module".to_string(),
                span: Span::new(1, 1),
            });
        }

        self.call_module_function(Some(root_canon), "main", vec![], Span::new(1, 1))?;
        Ok(())
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), RuntimeError> {
        let mut structs_set = HashSet::new();
        let mut errors_set = HashSet::new();
        let mut imports_map = HashMap::new();

        for item in &program.items {
            match &item.kind {
                ItemKind::Func { name, params, body, .. } => {
                    self.module_functions.insert((None, name.clone()), (params.clone(), body.clone()));
                }
                ItemKind::Type { name, type_params, fields } => {
                    structs_set.insert(name.clone());
                    self.module_struct_type_params
                        .insert((None, name.clone()), type_params.clone());
                    self.module_struct_fields
                        .insert((None, name.clone()), fields.clone());
                }
                ItemKind::Error { name, .. } => {
                    errors_set.insert(name.clone());
                }
                ItemKind::Import { path, alias } => {
                    if path.starts_with("cdt.") {
                        imports_map.insert(alias.clone(), ModuleTarget::Native(path.clone()));
                    } else {
                        imports_map.insert(alias.clone(), ModuleTarget::Local(PathBuf::from(path)));
                    }
                }
            }
        }

        self.module_structs.insert(None, structs_set);
        self.module_errors.insert(None, errors_set);
        self.module_imports.insert(None, imports_map);

        if !self.module_functions.contains_key(&(None, "main".to_string())) {
            return Err(RuntimeError {
                message: "Entry point 'func main()' not found".to_string(),
                span: Span::new(1, 1),
            });
        }

        self.call_module_function(None, "main", vec![], Span::new(1, 1))?;
        Ok(())
    }

    pub fn execute_validation_check(&mut self, val: Value, span: Span) -> Result<Value, RuntimeError> {
        let mut violations = Vec::new();
        self.validate_value_recursive(&val, "", &mut violations, 0, span)?;
        if violations.is_empty() {
            Ok(val)
        } else {
            Ok(crate::native::validation::make_validation_error(violations))
        }
    }

    fn find_struct_fields(&self, type_id: &TypeId) -> Option<(Option<PathBuf>, Vec<crate::ast::FieldDef>)> {
        let decl_mod = match &type_id.module {
            ModuleId::Program => None,
            ModuleId::Local(path) => Some(path.clone()),
            ModuleId::Builtin | ModuleId::Native(_) => return None,
        };
        let key = (decl_mod.clone(), type_id.name.clone());
        self.module_struct_fields
            .get(&key)
            .map(|fields| (decl_mod, fields.clone()))
    }

    fn validate_value_recursive(
        &mut self,
        val: &Value,
        path: &str,
        violations: &mut Vec<Value>,
        depth: usize,
        span: Span,
    ) -> Result<(), RuntimeError> {
        if depth > 256 {
            return Ok(());
        }

        match val {
            Value::Struct { type_id, fields } => {
                if let Some((decl_mod, field_defs)) = self.find_struct_fields(type_id) {
                    for f in &field_defs {
                        let field_path = if path.is_empty() {
                            f.name.clone()
                        } else {
                            format!("{}.{}", path, f.name)
                        };

                        if let Some(f_val) = fields.get(&f.name) {
                            self.validate_value_recursive(f_val, &field_path, violations, depth + 1, span)?;

                            if let Value::Nil = f_val {
                                continue;
                            }

                            for ann in &f.annotations {
                                let mut eval_args = vec![f_val.clone()];
                                for arg_expr in &ann.args {
                                    let arg_val = self.eval_expr(arg_expr).map_err(|f| match f {
                                        InterpFlow::Error(e) => e,
                                        _ => RuntimeError {
                                            message: "Unexpected flow in annotation argument".to_string(),
                                            span,
                                        },
                                    })?;
                                    eval_args.push(arg_val);
                                }

                                let res = self.invoke_validator(&decl_mod, &ann.target, eval_args, span)?;
                                match res {
                                    Value::Void | Value::Nil => {}
                                    Value::Error { ref fields, .. } => {
                                        if let (Some(Value::String(code)), Some(Value::String(msg))) =
                                            (fields.get("code"), fields.get("message"))
                                        {
                                            let violation = crate::native::validation::make_violation(
                                                &field_path,
                                                code,
                                                msg,
                                            );
                                            violations.push(violation);
                                        } else {
                                            return Err(RuntimeError {
                                                message: format!("Validator '@{}' returned an unexpected error format", ann.target),
                                                span,
                                            });
                                        }
                                    }
                                    other => {
                                        return Err(RuntimeError {
                                            message: format!(
                                                "Validator '@{}' must return Void or Issue, got '{:?}'",
                                                ann.target, other
                                            ),
                                            span,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Value::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    let item_path = format!("{}[{}]", path, i);
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            Value::Map(map) => {
                for (key, item) in map.entries() {
                    let item_path = format!("{}{}", path, key.format_path_segment());
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            Value::Tuple(items) => {
                for (i, item) in items.iter().enumerate() {
                    let item_path = if path.is_empty() {
                        format!("{}", i)
                    } else {
                        format!("{}.{}", path, i)
                    };
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn invoke_validator(
        &mut self,
        decl_mod: &Option<PathBuf>,
        target: &str,
        eval_args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        if let Some((alias, func_name)) = target.split_once('.') {
            let mod_imports = self.module_imports.get(decl_mod).cloned()
                .or_else(|| self.module_imports.get(&self.current_module).cloned());

            if let Some(target_mod) = mod_imports.and_then(|m| m.get(alias).cloned()) {
                match target_mod {
                    ModuleTarget::Native(nat_name) => {
                        let func_impl = {
                            let m = self.native_registry.get_module(&nat_name).ok_or_else(|| RuntimeError {
                                message: format!("Unknown native module '{}'", nat_name),
                                span,
                            })?;
                            m.functions.get(func_name).map(|f| f.implementation).ok_or_else(|| RuntimeError {
                                message: format!("Native module '{}' has no validator '{}'", alias, func_name),
                                span,
                            })?
                        };
                        match (func_impl)(eval_args) {
                            Ok(v) => Ok(v),
                            Err(e) => Ok(e),
                        }
                    }
                    ModuleTarget::Local(path) => {
                        self.call_module_function(Some(path), func_name, eval_args, span)
                    }
                }
            } else {
                Err(RuntimeError {
                    message: format!("Unknown module alias '{}' in validator '@{}'", alias, target),
                    span,
                })
            }
        } else {
            self.call_module_function(decl_mod.clone().or_else(|| self.current_module.clone()), target, eval_args, span)
        }
    }

    pub fn call_module_function(
        &mut self,
        mod_path: Option<PathBuf>,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let key = (mod_path.clone(), name.to_string());
        let (params, body) = if let Some(sig) = self.module_functions.get(&key).cloned() {
            sig
        } else if let Some(sig) = self.module_functions.get(&(None, name.to_string())).cloned() {
            sig
        } else {
            return Err(RuntimeError {
                message: format!("Function '{}' not found", name),
                span,
            });
        };

        let prev_mod = self.current_module.clone();
        let prev_env = std::mem::replace(&mut self.env, vec![HashMap::new()]);
        self.current_module = mod_path;

        self.enter_scope();
        for (i, param) in params.iter().enumerate() {
            let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
            self.define_variable(param.name.clone(), arg_val);
        }

        let mut return_val = Value::Void;
        let mut eval_err = None;
        for stmt in &body.stmts {
            match self.eval_stmt(stmt) {
                Ok(ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue) => {}
                Ok(ControlFlow::Return(val)) => {
                    return_val = val;
                    break;
                }
                Err(e) => {
                    eval_err = Some(e);
                    break;
                }
            }
        }

        self.exit_scope();
        self.env = prev_env;
        self.current_module = prev_mod;
        match eval_err {
            Some(e) => Err(e),
            None => Ok(return_val),
        }
    }

    fn call_callable(&mut self, callee_val: &Value, args: Vec<Value>, span: Span) -> Result<Value, RuntimeError> {
        match callee_val {
            Value::Function(fn_id) => {
                if let Some((mod_str, fn_name)) = fn_id.split_once("::") {
                    let mod_path = PathBuf::from(mod_str);
                    self.call_module_function(Some(mod_path), fn_name, args, span)
                } else {
                    let cur = self.current_module.clone();
                    self.call_module_function(cur, fn_id, args, span)
                }
            }
            Value::Closure(closure) => {
                let prev_mod = self.current_module.clone();
                let prev_env = std::mem::replace(&mut self.env, vec![closure.env.clone()]);
                self.current_module = closure.module.clone();

                self.enter_scope();
                for (i, param) in closure.params.iter().enumerate() {
                    let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
                    self.define_variable(param.name.clone(), arg_val);
                }

                let mut return_val = Value::Void;
                let mut eval_err = None;
                for stmt in &closure.body.stmts {
                    match self.eval_stmt(stmt) {
                        Ok(ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue) => {}
                        Ok(ControlFlow::Return(val)) => {
                            return_val = val;
                            break;
                        }
                        Err(e) => {
                            eval_err = Some(e);
                            break;
                        }
                    }
                }

                self.exit_scope();
                self.env = prev_env;
                self.current_module = prev_mod;
                match eval_err {
                    Some(e) => Err(e),
                    None => Ok(return_val),
                }
            }
            Value::NativeFunction { module, name } => {
                if module == "cdt.validation" && name == "check" {
                    let target_val = args.into_iter().next().unwrap_or(Value::Nil);
                    return self.execute_validation_check(target_val, span);
                }
                if module == "cdt.json" && name == "encode" {
                    let target_val = args.into_iter().next().unwrap_or(Value::Nil);
                    return Ok(self.execute_json_encode(&target_val));
                }
                let m = self.native_registry.get_module(module).ok_or_else(|| RuntimeError {
                    message: format!("Unknown native module '{}'", module),
                    span,
                })?;
                let f = m.functions.get(name.as_str()).ok_or_else(|| RuntimeError {
                    message: format!("Unknown native function '{}.{}'", module, name),
                    span,
                })?;
                match (f.implementation)(args) {
                    Ok(val) => Ok(val),
                    Err(err_val) => Ok(err_val),
                }
            }
            _ => Err(RuntimeError {
                message: format!("Value '{}' is not a callable function", callee_val),
                span,
            }),
        }
    }

    fn type_id_to_mod_key(type_id: &TypeId) -> Option<(Option<PathBuf>, String)> {
        match &type_id.module {
            ModuleId::Program => Some((None, type_id.name.clone())),
            ModuleId::Local(path) => Some((Some(path.clone()), type_id.name.clone())),
            ModuleId::Builtin | ModuleId::Native(_) => None,
        }
    }

    fn struct_field_order(&self, type_id: &TypeId) -> Option<Vec<String>> {
        let key = Self::type_id_to_mod_key(type_id)?;
        let defs = self.module_struct_fields.get(&key)?;
        Some(defs.iter().map(|f| f.name.clone()).collect())
    }

    fn substitute_runtime_type(ty: &Type, bindings: &HashMap<String, Type>) -> Type {
        match ty {
            Type::TypeParam(name) => bindings
                .get(name)
                .cloned()
                .unwrap_or_else(|| Type::TypeParam(name.clone())),
            Type::List(inner) => Type::List(Box::new(Self::substitute_runtime_type(inner, bindings))),
            Type::Map(k, v) => Type::Map(
                Box::new(Self::substitute_runtime_type(k, bindings)),
                Box::new(Self::substitute_runtime_type(v, bindings)),
            ),
            Type::Optional(inner) => {
                Type::Optional(Box::new(Self::substitute_runtime_type(inner, bindings)))
            }
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .iter()
                    .map(|e| Self::substitute_runtime_type(e, bindings))
                    .collect(),
            ),
            Type::Union(vars) => Type::union(
                vars.iter()
                    .map(|v| Self::substitute_runtime_type(v, bindings))
                    .collect(),
            ),
            Type::Generic { type_id, type_args } => Type::Generic {
                type_id: type_id.clone(),
                type_args: type_args
                    .iter()
                    .map(|a| Self::substitute_runtime_type(a, bindings))
                    .collect(),
            },
            other => other.clone(),
        }
    }

    fn resolve_ast_type_in_mod(
        &self,
        ty_expr: &TypeExpr,
        mod_opt: &Option<PathBuf>,
        type_params: &[String],
        span: Span,
    ) -> Result<Type, RuntimeError> {
        let resolve_target_id = |name: &str| -> Result<TypeId, RuntimeError> {
            if let Some((alias, item_name)) = name.split_once('.') {
                let target = self
                    .module_imports
                    .get(mod_opt)
                    .and_then(|m| m.get(alias))
                    .ok_or_else(|| RuntimeError {
                        message: format!("Unknown module alias '{}' in json.decode", alias),
                        span,
                    })?;
                Ok(match target {
                    ModuleTarget::Native(nat_name) => TypeId::native(nat_name.clone(), item_name),
                    ModuleTarget::Local(path) => TypeId::local(path.clone(), item_name),
                })
            } else {
                Ok(TypeId::from_module_opt(mod_opt.as_ref(), name))
            }
        };

        match ty_expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => Ok(Type::Int),
                "Float" => Ok(Type::Float),
                "String" => Ok(Type::String),
                "Bool" => Ok(Type::Bool),
                "Void" => Ok(Type::Void),
                _ => {
                    if !name.contains('.') && type_params.iter().any(|p| p == name) {
                        Ok(Type::TypeParam(name.clone()))
                    } else {
                        Ok(Type::Custom(resolve_target_id(name)?))
                    }
                }
            },
            TypeExpr::Generic { name, type_args } => {
                if name == "Map" && type_args.len() == 2 {
                    let k = self.resolve_ast_type_in_mod(&type_args[0], mod_opt, type_params, span)?;
                    let v = self.resolve_ast_type_in_mod(&type_args[1], mod_opt, type_params, span)?;
                    Ok(Type::Map(Box::new(k), Box::new(v)))
                } else {
                    let type_id = resolve_target_id(name)?;
                    let mut args = Vec::with_capacity(type_args.len());
                    for ta in type_args {
                        args.push(self.resolve_ast_type_in_mod(ta, mod_opt, type_params, span)?);
                    }
                    Ok(Type::Generic {
                        type_id,
                        type_args: args,
                    })
                }
            }
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_ast_type_in_mod(
                inner,
                mod_opt,
                type_params,
                span,
            )?))),
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(
                self.resolve_ast_type_in_mod(inner, mod_opt, type_params, span)?,
            ))),
            TypeExpr::Tuple(elems) => {
                let mut resolved = Vec::with_capacity(elems.len());
                for e in elems {
                    resolved.push(self.resolve_ast_type_in_mod(e, mod_opt, type_params, span)?);
                }
                Ok(Type::Tuple(resolved))
            }
            TypeExpr::Union(vars) => {
                let mut resolved = Vec::with_capacity(vars.len());
                for v in vars {
                    resolved.push(self.resolve_ast_type_in_mod(v, mod_opt, type_params, span)?);
                }
                Ok(Type::union(resolved))
            }
            TypeExpr::Func { params, return_type } => {
                let mut ps = Vec::with_capacity(params.len());
                for p in params {
                    ps.push(self.resolve_ast_type_in_mod(p, mod_opt, type_params, span)?);
                }
                let rt = self.resolve_ast_type_in_mod(return_type, mod_opt, type_params, span)?;
                Ok(Type::Function {
                    params: ps,
                    return_type: Box::new(rt),
                })
            }
        }
    }

    fn resolve_struct_fields_for_json(
        &self,
        type_id: &TypeId,
        type_args: &[Type],
        span: Span,
    ) -> Option<Vec<(String, Type)>> {
        let key = Self::type_id_to_mod_key(type_id)?;
        let defs = self.module_struct_fields.get(&key)?;
        let tparams = self
            .module_struct_type_params
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let mut bindings = HashMap::new();
        for (tp, arg) in tparams.iter().zip(type_args.iter()) {
            bindings.insert(tp.clone(), arg.clone());
        }
        let mod_opt = &key.0;
        let mut out = Vec::with_capacity(defs.len());
        for f in defs {
            let fty = self
                .resolve_ast_type_in_mod(&f.ty, mod_opt, &tparams, span)
                .ok()?;
            let concrete = Self::substitute_runtime_type(&fty, &bindings);
            out.push((f.name.clone(), concrete));
        }
        Some(out)
    }

    fn execute_json_encode(&self, val: &Value) -> Value {
        let mut out = String::new();
        match encode_conduit_value(val, "", &mut out, &|tid| self.struct_field_order(tid)) {
            Ok(()) => Value::String(out),
            Err((path, msg)) => make_encode_error(path, msg),
        }
    }

    fn execute_json_decode(&self, raw: &str, type_expr: &TypeExpr, span: Span) -> Result<Value, RuntimeError> {
        let target_ty =
            self.resolve_ast_type_in_mod(type_expr, &self.current_module, &[], span)?;
        let node = match parse_json(raw) {
            Ok(n) => n,
            Err((path, msg)) => return Ok(make_decode_error(path, msg)),
        };
        match decode_node_to_conduit(&node, &target_ty, "", &|tid, targs| {
            self.resolve_struct_fields_for_json(tid, targs, span)
        }) {
            Ok(v) => Ok(v),
            Err((path, msg)) => Ok(make_decode_error(path, msg)),
        }
    }

    fn eval_block(&mut self, block: &Block) -> Result<ControlFlow, RuntimeError> {
        self.enter_scope();
        for s in &block.stmts {
            match self.eval_stmt(s)? {
                ControlFlow::Next => {}
                flow => {
                    self.exit_scope();
                    return Ok(flow);
                }
            }
        }
        self.exit_scope();
        Ok(ControlFlow::Next)
    }

    fn eval_stmt(&mut self, stmt: &Stmt) -> Result<ControlFlow, RuntimeError> {
        match &stmt.kind {
            StmtKind::Define { name, value } => {
                let val = match self.eval_expr(value) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                self.define_variable(name.clone(), val);
                Ok(ControlFlow::Next)
            }
            StmtKind::DestructureDefine { pattern, value } => {
                let val = match self.eval_expr(value) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if let PatternKind::Tuple(pats) = &pattern.kind {
                    if let Value::Tuple(items) = val {
                        for (pat, item) in pats.iter().zip(items) {
                            match &pat.kind {
                                PatternKind::Ident(var_name) => {
                                    self.define_variable(var_name.clone(), item);
                                }
                                PatternKind::Wildcard => {}
                                _ => {}
                            }
                        }
                    }
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::VarDecl { name, init, .. } => {
                let val = if let Some(init_expr) = init {
                    match self.eval_expr(init_expr) {
                        Ok(v) => v,
                        Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                        Err(InterpFlow::Error(err)) => return Err(err),
                    }
                } else {
                    Value::Nil
                };
                self.define_variable(name.clone(), val);
                Ok(ControlFlow::Next)
            }
            StmtKind::Assign { target, value } => {
                if let ExprKind::Index { target: container, index } = &target.kind {
                    let key_val = match self.eval_expr(index) {
                        Ok(v) => v,
                        Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                        Err(InterpFlow::Error(err)) => return Err(err),
                    };
                    let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                        message: format!(
                            "Map key must be String, Int, or Bool, got {}",
                            key_val.type_name()
                        ),
                        span: index.span,
                    })?;
                    let rhs = match self.eval_expr(value) {
                        Ok(v) => v,
                        Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                        Err(InterpFlow::Error(err)) => return Err(err),
                    };
                    let target_span = target.span;
                    self.mutate_place(container, move |slot| {
                        if let Value::Map(map) = slot {
                            map.set(key, rhs);
                            Ok(Value::Void)
                        } else {
                            Err(RuntimeError {
                                message: format!(
                                    "Index assignment is only supported on Map, got {}",
                                    slot.type_name()
                                ),
                                span: target_span,
                            })
                        }
                    })?;
                    return Ok(ControlFlow::Next);
                }

                let val = match self.eval_expr(value) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if let ExprKind::Ident(ref name) = target.kind {
                    self.assign_variable(name, val, target.span)?;
                } else {
                    self.mutate_place(target, move |slot| {
                        *slot = val;
                        Ok(Value::Void)
                    })?;
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::Return(opt_expr) => {
                let val = if let Some(expr) = opt_expr {
                    match self.eval_expr(expr) {
                        Ok(v) => v,
                        Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                        Err(InterpFlow::Error(err)) => return Err(err),
                    }
                } else {
                    Value::Void
                };
                Ok(ControlFlow::Return(val))
            }
            StmtKind::If { condition, then_block, else_block } => {
                let cond_val = match self.eval_expr(condition) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if cond_val.is_truthy() {
                    self.eval_block(then_block)
                } else if let Some(eb) = else_block {
                    self.eval_block(eb)
                } else {
                    Ok(ControlFlow::Next)
                }
            }
            StmtKind::Guard { condition, else_block } => {
                let cond_val = match self.eval_expr(condition) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if !cond_val.is_truthy() {
                    self.eval_block(else_block)
                } else {
                    Ok(ControlFlow::Next)
                }
            }
            StmtKind::ForIn { pattern, iterable, body } => {
                let iter_val = match self.eval_expr(iterable) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                let items = match iter_val {
                    Value::List(items) => items,
                    Value::Map(map) => map.entry_tuples(),
                    _ => Vec::new(),
                };
                for item in items {
                    self.enter_scope();
                    self.bind_for_pattern(pattern, item);
                    let mut flow = ControlFlow::Next;
                    for s in &body.stmts {
                        match self.eval_stmt(s)? {
                            ControlFlow::Next => {}
                            other => {
                                flow = other;
                                break;
                            }
                        }
                    }
                    self.exit_scope();
                    match flow {
                        ControlFlow::Break => break,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        _ => {}
                    }
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::While { condition, body } => {
                loop {
                    let cond_val = match self.eval_expr(condition) {
                        Ok(v) => v,
                        Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                        Err(InterpFlow::Error(err)) => return Err(err),
                    };
                    if !cond_val.is_truthy() {
                        break;
                    }
                    self.enter_scope();
                    let mut flow = ControlFlow::Next;
                    for s in &body.stmts {
                        match self.eval_stmt(s)? {
                            ControlFlow::Next => {}
                            other => {
                                flow = other;
                                break;
                            }
                        }
                    }
                    self.exit_scope();
                    match flow {
                        ControlFlow::Break => break,
                        ControlFlow::Continue => continue,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::Next => {}
                    }
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::Switch { expr, cases } => {
                let val = match self.eval_expr(expr) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                for case in cases {
                    if let Some(bindings) = self.match_pattern(&case.pattern, &val) {
                        self.enter_scope();
                        for (k, v) in bindings {
                            self.define_variable(k, v);
                        }
                        let flow = self.eval_block(&case.body)?;
                        self.exit_scope();
                        match flow {
                            ControlFlow::Next => return Ok(ControlFlow::Next),
                            other => return Ok(other),
                        }
                    }
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::Break => Ok(ControlFlow::Break),
            StmtKind::Continue => Ok(ControlFlow::Continue),
            StmtKind::Expr(expr) => {
                match self.eval_expr(expr) {
                    Ok(_) => Ok(ControlFlow::Next),
                    Err(InterpFlow::Return(ret)) => Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => Err(err),
                }
            }
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, InterpFlow> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(f) => Ok(Value::Float(*f)),
            ExprKind::String(s) => Ok(Value::String(self.interpolate(s))),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),
            ExprKind::Tuple(elements) => {
                let mut vals = Vec::new();
                for e in elements {
                    vals.push(self.eval_expr(e)?);
                }
                Ok(Value::Tuple(vals))
            }
            ExprKind::Try(inner) => {
                let val = self.eval_expr(inner)?;
                if val.is_error() {
                    return Err(InterpFlow::Return(val));
                }
                Ok(val)
            }
            ExprKind::Ident(name) => {
                if let Some(val) = self.lookup_variable(name) {
                    Ok(val)
                } else if self.module_functions.contains_key(&(self.current_module.clone(), name.clone())) {
                    if let Some(ref cur_path) = self.current_module {
                        Ok(Value::Function(format!("{}::{}", cur_path.display(), name)))
                    } else {
                        Ok(Value::Function(name.clone()))
                    }
                } else if self.module_functions.contains_key(&(None, name.clone())) {
                    Ok(Value::Function(name.clone()))
                } else {
                    Err(RuntimeError {
                        message: format!("Unknown variable '{}'", name),
                        span: expr.span,
                    }.into())
                }
            }
            ExprKind::List(elements) => {
                let mut vals = Vec::new();
                for e in elements {
                    vals.push(self.eval_expr(e)?);
                }
                Ok(Value::List(vals))
            }
            ExprKind::Closure { params, body, .. } => Ok(Value::Closure(Arc::new(ClosureData {
                params: params.clone(),
                body: body.clone(),
                env: self.snapshot_env(),
                module: self.current_module.clone(),
            }))),
            ExprKind::Index { target, index } => {
                let target_val = self.eval_expr(target)?;
                let index_val = self.eval_expr(index)?;
                match (target_val, index_val) {
                    (Value::List(items), Value::Int(idx)) => {
                        if idx < 0 || (idx as usize) >= items.len() {
                            Ok(make_index_error(idx, items.len()))
                        } else {
                            Ok(items[idx as usize].clone())
                        }
                    }
                    (Value::List(_), other) => Err(RuntimeError {
                        message: format!("List index must be Int, got {}", other.type_name()),
                        span: index.span,
                    }.into()),
                    (Value::Map(map), key_val) => {
                        let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                            message: format!(
                                "Map key must be String, Int, or Bool, got {}",
                                key_val.type_name()
                            ),
                            span: index.span,
                        })?;
                        Ok(map.get(&key).cloned().unwrap_or_else(|| make_key_error(&key)))
                    }
                    (other, _) => Err(RuntimeError {
                        message: format!(
                            "Cannot index into value of type {}",
                            other.type_name()
                        ),
                        span: target.span,
                    }.into()),
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                let val = self.eval_expr(inner)?;
                match op {
                    UnaryOp::Neg => match val {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        Value::Float(f) => Ok(Value::Float(-f)),
                        _ => Err(RuntimeError {
                            message: "Unary '-' applies to Int or Float only".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
                }
            }
            ExprKind::Binary { op, left, right } => {
                if *op == BinaryOp::Coalesce {
                    let l = self.eval_expr(left)?;
                    return if l != Value::Nil {
                        Ok(l)
                    } else {
                        self.eval_expr(right)
                    };
                }
                if *op == BinaryOp::And {
                    let l = self.eval_expr(left)?;
                    return if !l.is_truthy() {
                        Ok(Value::Bool(false))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(r.is_truthy()))
                    };
                }
                if *op == BinaryOp::Or {
                    let l = self.eval_expr(left)?;
                    return if l.is_truthy() {
                        Ok(Value::Bool(true))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(r.is_truthy()))
                    };
                }

                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;

                match op {
                    BinaryOp::Add => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                        _ => Err(RuntimeError {
                            message: "Operator '+' requires matching Int, Float, or String operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Sub => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                        _ => Err(RuntimeError {
                            message: "Operator '-' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Mul => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                        _ => Err(RuntimeError {
                            message: "Operator '*' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Div => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span: expr.span,
                        }.into()),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
                        (Value::Float(a), Value::Float(b)) => {
                            let res = a / b;
                            if b == 0.0 || !res.is_finite() {
                                Err(RuntimeError {
                                    message: "Division by zero".to_string(),
                                    span: expr.span,
                                }.into())
                            } else {
                                Ok(Value::Float(res))
                            }
                        }
                        _ => Err(RuntimeError {
                            message: "Operator '/' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Mod => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Modulo by zero".to_string(),
                            span: expr.span,
                        }.into()),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
                        _ => Err(RuntimeError {
                            message: "Operator '%' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Lt => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
                        _ => Err(RuntimeError {
                            message: "Operator '<' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::LtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
                        _ => Err(RuntimeError {
                            message: "Operator '<=' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Gt => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
                        _ => Err(RuntimeError {
                            message: "Operator '>' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::GtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
                        _ => Err(RuntimeError {
                            message: "Operator '>=' requires matching Int or Float operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Eq => Ok(Value::Bool(l == r)),
                    BinaryOp::NotEq => Ok(Value::Bool(l != r)),
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Coalesce => unreachable!(),
                    BinaryOp::Range => match (l, r) {
                        (Value::Int(start), Value::Int(end)) => {
                            let items = (start..end).map(Value::Int).collect();
                            Ok(Value::List(items))
                        }
                        _ => Err(RuntimeError {
                            message: "Range '..' requires Int bounds".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                }
            }
            ExprKind::Call { callee, type_args, args, named_args } => {
                if let ExprKind::Ident(ref name) = callee.kind {
                    let cur_structs = self.module_structs.get(&self.current_module);
                    let is_struct = cur_structs.map(|s| s.contains(name)).unwrap_or(false);
                    let cur_errors = self.module_errors.get(&self.current_module);
                    let is_error = cur_errors.map(|e| e.contains(name)).unwrap_or(false);

                    if is_struct || is_error || matches!(name.as_str(), "IndexError" | "KeyError" | "ParseError") {
                        let mut fields = HashMap::new();
                        for arg in named_args {
                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                        }
                        return Ok(if is_struct {
                            Value::Struct {
                                type_id: TypeId::from_module_opt(self.current_module.as_ref(), name.clone()),
                                fields,
                            }
                        } else {
                            let type_id = if is_error {
                                TypeId::from_module_opt(self.current_module.as_ref(), name.clone())
                            } else {
                                TypeId::builtin(name)
                            };
                            Value::Error { type_id, fields }
                        });
                    }
                    if name == "Map" && args.is_empty() && named_args.is_empty() {
                        return Ok(Value::Map(OrderedMap::new()));
                    }
                    
                    if name == "map" && args.len() == 2 {
                        let list_val = self.eval_expr(&args[0])?;
                        let cb_val = self.eval_expr(&args[1])?;
                        if let Value::List(items) = list_val {
                            let mut results = Vec::new();
                            for item in items {
                                if item.is_error() {
                                    results.push(item);
                                } else {
                                    let res = self.call_callable(&cb_val, vec![item], args[1].span)?;
                                    results.push(res);
                                }
                            }
                            return Ok(Value::List(results));
                        }
                    }

                    if name == "filter" && args.len() == 2 {
                        let list_val = self.eval_expr(&args[0])?;
                        let cb_val = self.eval_expr(&args[1])?;
                        if let Value::List(items) = list_val {
                            let mut results = Vec::new();
                            for item in items {
                                if item.is_error() {
                                    results.push(item);
                                } else {
                                    let res = self.call_callable(&cb_val, vec![item.clone()], args[1].span)?;
                                    if res.is_truthy() {
                                        results.push(item);
                                    }
                                }
                            }
                            return Ok(Value::List(results));
                        }
                    }

                    if name == "print" {
                        for (i, a) in args.iter().enumerate() {
                            let val = self.eval_expr(a)?;
                            if i > 0 {
                                print!(" ");
                            }
                            print!("{}", val);
                        }
                        println!();
                        return Ok(Value::Void);
                    }
                }
                
                if let ExprKind::Member { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        if alias == "Map" && field == "from" && args.len() == 1 {
                            let pairs_val = self.eval_expr(&args[0])?;
                            if let Value::List(items) = pairs_val {
                                let mut map = OrderedMap::new();
                                for item in items {
                                    if let Value::Tuple(elems) = item {
                                        if elems.len() == 2 {
                                            if let Some(key) = MapKey::from_value(&elems[0]) {
                                                map.set(key, elems[1].clone());
                                                continue;
                                            }
                                        }
                                    }
                                    return Err(RuntimeError {
                                        message: "Map.from expects a list of (key, value) pairs with String, Int, or Bool keys".to_string(),
                                        span: args[0].span,
                                    }.into());
                                }
                                return Ok(Value::Map(map));
                            }
                            return Err(RuntimeError {
                                message: format!("Map.from expects a List, got {}", pairs_val.type_name()),
                                span: args[0].span,
                            }.into());
                        }
                        let cur_imports = self.module_imports.get(&self.current_module).cloned();
                        if let Some(target_mod) = cur_imports.and_then(|m| m.get(alias).cloned()) {
                            match target_mod {
                                ModuleTarget::Native(nat_name) => {
                                    if nat_name == "cdt.validation" && field == "check" {
                                        let target_val = if !args.is_empty() {
                                            self.eval_expr(&args[0])?
                                        } else {
                                            Value::Nil
                                        };
                                        return self.execute_validation_check(target_val, callee.span).map_err(InterpFlow::Error);
                                    }

                                    if nat_name == "cdt.json" && field == "decode" {
                                        let raw_val = self.eval_expr(&args[0])?;
                                        let raw = match raw_val {
                                            Value::String(s) => s,
                                            other => {
                                                return Err(RuntimeError {
                                                    message: format!("json.decode expects String, got {}", other.type_name()),
                                                    span: args[0].span,
                                                }.into());
                                            }
                                        };
                                        let target_texpr = type_args.first().ok_or_else(|| RuntimeError {
                                            message: "json.decode requires a target type argument <T>".to_string(),
                                            span: callee.span,
                                        })?;
                                        return self
                                            .execute_json_decode(&raw, target_texpr, callee.span)
                                            .map_err(InterpFlow::Error);
                                    }

                                    if nat_name == "cdt.json" && field == "encode" {
                                        let target_val = self.eval_expr(&args[0])?;
                                        return Ok(self.execute_json_encode(&target_val));
                                    }

                                    let (is_constructible, is_error) = {
                                        let m = self.native_registry.get_module(&nat_name).ok_or_else(|| RuntimeError {
                                            message: format!("Unknown native module '{}'", nat_name),
                                            span: callee.span,
                                        })?;
                                        if let Some(td) = m.types.get(field.as_str()) {
                                            (!td.fields.is_empty(), td.is_error)
                                        } else {
                                            (false, false)
                                        }
                                    };

                                    if is_constructible {
                                        let mut fields = HashMap::new();
                                        for arg in named_args {
                                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                                        }
                                        if is_error {
                                            return Ok(Value::Error {
                                                type_id: TypeId::native(nat_name.clone(), field.clone()),
                                                fields,
                                            });
                                        } else {
                                            return Ok(Value::Struct {
                                                type_id: TypeId::native(nat_name.clone(), field.clone()),
                                                fields,
                                            });
                                        }
                                    }

                                    let func_impl = {
                                        let m = self.native_registry.get_module(&nat_name).ok_or_else(|| RuntimeError {
                                            message: format!("Unknown native module '{}'", nat_name),
                                            span: callee.span,
                                        })?;
                                        m.functions.get(field.as_str()).map(|f| f.implementation)
                                    };
                                    if let Some(impl_fn) = func_impl {
                                        let mut evaluated = Vec::new();
                                        for a in args {
                                            evaluated.push(self.eval_expr(a)?);
                                        }
                                        let res = (impl_fn)(evaluated);
                                        return match res {
                                            Ok(val) => Ok(val),
                                            Err(err_val) => Ok(err_val),
                                        };
                                    }
                                }
                                ModuleTarget::Local(path) => {
                                    let is_struct = self
                                        .module_structs
                                        .get(&Some(path.clone()))
                                        .map(|s| s.contains(field))
                                        .unwrap_or(false);
                                    let is_error = self
                                        .module_errors
                                        .get(&Some(path.clone()))
                                        .map(|e| e.contains(field))
                                        .unwrap_or(false);

                                    if is_struct {
                                        let mut fields = HashMap::new();
                                        for arg in named_args {
                                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                                        }
                                        return Ok(Value::Struct {
                                            type_id: TypeId::local(path.clone(), field.clone()),
                                            fields,
                                        });
                                    }
                                    if is_error {
                                        let mut fields = HashMap::new();
                                        for arg in named_args {
                                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                                        }
                                        return Ok(Value::Error {
                                            type_id: TypeId::local(path.clone(), field.clone()),
                                            fields,
                                        });
                                    }

                                    let mut evaluated = Vec::new();
                                    for a in args {
                                        evaluated.push(self.eval_expr(a)?);
                                    }
                                    return self
                                        .call_module_function(Some(path), field, evaluated, callee.span)
                                        .map_err(Into::into);
                                }
                            }
                        }
                    }

                    let target_val = self.eval_expr(target)?;
                    return self.eval_method_on_value(target_val, Some(target), field, args, expr.span);
                }

                if let ExprKind::OptionalMember { target, field } = &callee.kind {
                    let target_val = self.eval_expr(target)?;
                    if target_val == Value::Nil {
                        return Ok(Value::Nil);
                    }
                    if target_val.is_error() {
                        return Err(InterpFlow::Return(target_val));
                    }
                    return self.eval_method_on_value(target_val, None, field, args, expr.span);
                }

                let callee_val = self.eval_expr(callee)?;
                let mut evaluated_args = Vec::new();
                for a in args {
                    evaluated_args.push(self.eval_expr(a)?);
                }
                self.call_callable(&callee_val, evaluated_args, callee.span).map_err(Into::into)
            }
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(ref alias) = target.kind {
                    let cur_imports = self.module_imports.get(&self.current_module).cloned();
                    if let Some(target_mod) = cur_imports.and_then(|m| m.get(alias).cloned()) {
                        match target_mod {
                            ModuleTarget::Native(nat_name) => {
                                return Ok(Value::NativeFunction {
                                    module: nat_name,
                                    name: field.clone(),
                                });
                            }
                            ModuleTarget::Local(path) => {
                                return Ok(Value::Function(format!("{}::{}", path.display(), field)));
                            }
                        }
                    }
                }

                let target_val = self.eval_expr(target)?;
                self.eval_member_on_value(target_val, field, expr.span)
            }
            ExprKind::OptionalMember { target, field } => {
                let target_val = self.eval_expr(target)?;
                if target_val == Value::Nil {
                    return Ok(Value::Nil);
                }
                if target_val.is_error() {
                    return Err(InterpFlow::Return(target_val));
                }
                self.eval_member_on_value(target_val, field, expr.span)
            }
        }
    }

    fn eval_member_on_value(
        &self,
        target_val: Value,
        field: &str,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        match target_val {
            Value::Struct { ref fields, .. } | Value::Error { ref fields, .. } => {
                if let Some(val) = fields.get(field) {
                    Ok(val.clone())
                } else {
                    Err(RuntimeError {
                        message: format!("Field '{}' not found", field),
                        span,
                    }
                    .into())
                }
            }
            _ => Err(RuntimeError {
                message: format!("Field access not supported on type {}", target_val.type_name()),
                span,
            }
            .into()),
        }
    }

    fn eval_method_on_value(
        &mut self,
        target_val: Value,
        place_expr: Option<&Expr>,
        field: &str,
        args: &[Expr],
        call_span: Span,
    ) -> Result<Value, InterpFlow> {
        if let Value::Native(ref nat) = target_val {
            let type_id = nat.type_id();
            let method_impl = if let ModuleId::Native(ref nat_name) = type_id.module {
                self.native_registry
                    .get_module(nat_name)
                    .and_then(|m| m.methods.get(&(type_id.name.as_str(), field)))
                    .map(|md| md.implementation)
            } else {
                None
            };

            if let Some(impl_fn) = method_impl {
                let mut evaluated = Vec::new();
                for a in args {
                    evaluated.push(self.eval_expr(a)?);
                }
                let res = (impl_fn)(nat, evaluated);
                return match res {
                    Ok(v) => Ok(v),
                    Err(err_v) => Ok(err_v),
                };
            }
        }

        if let (Some(place), Value::List(_)) = (place_expr, &target_val) {
            if field == "append" && args.len() == 1 {
                let val_to_append = self.eval_expr(&args[0])?;
                let pspan = place.span;
                return self
                    .mutate_place(place, move |slot| {
                        if let Value::List(items) = slot {
                            items.push(val_to_append);
                            Ok(Value::Void)
                        } else {
                            Err(RuntimeError {
                                message: format!("Expected List, got {}", slot.type_name()),
                                span: pspan,
                            })
                        }
                    })
                    .map_err(Into::into);
            }
        }

        if let (Some(place), Value::Map(_)) = (place_expr, &target_val) {
            if (field == "set" && args.len() == 2) || (field == "remove" && args.len() == 1) {
                let key_val = self.eval_expr(&args[0])?;
                let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                    message: format!(
                        "Map key must be String, Int, or Bool, got {}",
                        key_val.type_name()
                    ),
                    span: args[0].span,
                })?;
                if field == "set" {
                    let val = self.eval_expr(&args[1])?;
                    let pspan = place.span;
                    return self
                        .mutate_place(place, move |slot| {
                            if let Value::Map(map) = slot {
                                map.set(key, val);
                                Ok(Value::Void)
                            } else {
                                Err(RuntimeError {
                                    message: format!("Expected Map, got {}", slot.type_name()),
                                    span: pspan,
                                })
                            }
                        })
                        .map_err(Into::into);
                } else {
                    let pspan = place.span;
                    return self
                        .mutate_place(place, move |slot| {
                            if let Value::Map(map) = slot {
                                Ok(map.remove(&key).unwrap_or(Value::Nil))
                            } else {
                                Err(RuntimeError {
                                    message: format!("Expected Map, got {}", slot.type_name()),
                                    span: pspan,
                                })
                            }
                        })
                        .map_err(Into::into);
                }
            }
        }

        match target_val {
            Value::Int(n) => match field {
                "to_string" => return Ok(Value::String(n.to_string())),
                "to_float" => return Ok(Value::Float(n as f64)),
                _ => {}
            },
            Value::Float(f) => match field {
                "floor" => return Ok(Value::Int(f.floor() as i64)),
                "ceil" => return Ok(Value::Int(f.ceil() as i64)),
                "round" => return Ok(Value::Int(f.round() as i64)),
                "trunc" => return Ok(Value::Int(f.trunc() as i64)),
                "to_string" => return Ok(Value::String(f.to_string())),
                "is_nan" => return Ok(Value::Bool(f.is_nan())),
                "is_infinite" => return Ok(Value::Bool(f.is_infinite())),
                "is_finite" => return Ok(Value::Bool(f.is_finite())),
                _ => {}
            },
            Value::Bool(b) => {
                if field == "to_string" {
                    return Ok(Value::String(b.to_string()));
                }
            }
            Value::String(s) => match field {
                "trim" => return Ok(Value::String(s.trim().to_string())),
                "lower" => return Ok(Value::String(s.to_lowercase())),
                "upper" => return Ok(Value::String(s.to_uppercase())),
                "to_string" => return Ok(Value::String(s)),
                "len" => return Ok(Value::Int(s.len() as i64)),
                "char_count" => return Ok(Value::Int(s.chars().count() as i64)),
                "to_int" => {
                    return match s.parse::<i64>() {
                        Ok(n) => Ok(Value::Int(n)),
                        Err(e) => Ok(make_parse_error(&s, format!("Invalid Int '{}': {}", s, e))),
                    };
                }
                "to_float" => {
                    return match s.parse::<f64>() {
                        Ok(f) if f.is_finite() => Ok(Value::Float(f)),
                        Ok(_) => Ok(make_parse_error(
                            &s,
                            format!("Invalid Float '{}': non-finite value is not allowed", s),
                        )),
                        Err(e) => Ok(make_parse_error(&s, format!("Invalid Float '{}': {}", s, e))),
                    };
                }
                "contains" => {
                    let search = self.eval_expr(&args[0])?;
                    if let Value::String(sub) = search {
                        return Ok(Value::Bool(s.contains(&sub)));
                    }
                }
                "index_of" => {
                    let search = self.eval_expr(&args[0])?;
                    if let Value::String(sub) = search {
                        return match s.find(&sub) {
                            Some(byte_idx) => {
                                let char_idx = s[..byte_idx].chars().count() as i64;
                                Ok(Value::Int(char_idx))
                            }
                            None => Ok(Value::Nil),
                        };
                    }
                }
                "starts_with" => {
                    let arg = self.eval_expr(&args[0])?;
                    if let Value::String(prefix) = arg {
                        return Ok(Value::Bool(s.starts_with(&prefix)));
                    }
                }
                "ends_with" => {
                    let arg = self.eval_expr(&args[0])?;
                    if let Value::String(suffix) = arg {
                        return Ok(Value::Bool(s.ends_with(&suffix)));
                    }
                }
                "split" => {
                    let arg = self.eval_expr(&args[0])?;
                    if let Value::String(sep) = arg {
                        let parts = if sep.is_empty() {
                            s.chars().map(|c| Value::String(c.to_string())).collect()
                        } else {
                            s.split(&sep).map(|p| Value::String(p.to_string())).collect()
                        };
                        return Ok(Value::List(parts));
                    }
                }
                "slice" => {
                    let start_val = self.eval_expr(&args[0])?;
                    let end_val = self.eval_expr(&args[1])?;
                    if let (Value::Int(start_i), Value::Int(end_i)) = (start_val, end_val) {
                        let start = if start_i <= 0 { 0 } else { start_i as usize };
                        let end = if end_i <= 0 { 0 } else { end_i as usize };
                        if start >= end {
                            return Ok(Value::String(String::new()));
                        }
                        let sliced: String = s.chars().skip(start).take(end - start).collect();
                        return Ok(Value::String(sliced));
                    }
                }
                "replace" => {
                    let from_val = self.eval_expr(&args[0])?;
                    let to_val = self.eval_expr(&args[1])?;
                    if let (Value::String(from), Value::String(to)) = (from_val, to_val) {
                        let replaced = if from.is_empty() {
                            s
                        } else {
                            s.replace(&from, &to)
                        };
                        return Ok(Value::String(replaced));
                    }
                }
                _ => {}
            },
            Value::List(items) => match field {
                "len" => return Ok(Value::Int(items.len() as i64)),
                "reverse" => {
                    let mut rev = items;
                    rev.reverse();
                    return Ok(Value::List(rev));
                }
                "enumerate" => {
                    let enumerated = items
                        .into_iter()
                        .enumerate()
                        .map(|(i, item)| Value::Tuple(vec![Value::Int(i as i64), item]))
                        .collect();
                    return Ok(Value::List(enumerated));
                }
                "slice" => {
                    let start_val = self.eval_expr(&args[0])?;
                    let end_val = self.eval_expr(&args[1])?;
                    if let (Value::Int(start_i), Value::Int(end_i)) = (start_val, end_val) {
                        let len = items.len();
                        let s = if start_i <= 0 { 0 } else { (start_i as usize).min(len) };
                        let e = if end_i <= 0 { 0 } else { (end_i as usize).min(len) };
                        if s >= e {
                            return Ok(Value::List(Vec::new()));
                        }
                        return Ok(Value::List(items[s..e].to_vec()));
                    }
                }
                "join" => {
                    let sep_val = self.eval_expr(&args[0])?;
                    if let Value::String(sep) = sep_val {
                        let mut parts = Vec::with_capacity(items.len());
                        for item in items {
                            if let Value::String(s) = item {
                                parts.push(s);
                            }
                        }
                        return Ok(Value::String(parts.join(&sep)));
                    }
                }
                "get" => {
                    let idx_val = self.eval_expr(&args[0])?;
                    if let Value::Int(idx) = idx_val {
                        if idx < 0 || (idx as usize) >= items.len() {
                            return Ok(Value::Nil);
                        }
                        return Ok(items[idx as usize].clone());
                    }
                    return Err(RuntimeError {
                        message: format!(
                            "List method 'get' expects Int index, got {}",
                            idx_val.type_name()
                        ),
                        span: args[0].span,
                    }
                    .into());
                }
                "contains" => {
                    let search = self.eval_expr(&args[0])?;
                    return Ok(Value::Bool(items.contains(&search)));
                }
                _ => {}
            },
            Value::Map(map) => match field {
                "len" => return Ok(Value::Int(map.len() as i64)),
                "get" => {
                    let key_val = self.eval_expr(&args[0])?;
                    let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                        message: format!(
                            "Map key must be String, Int, or Bool, got {}",
                            key_val.type_name()
                        ),
                        span: args[0].span,
                    })?;
                    return Ok(map.get(&key).cloned().unwrap_or(Value::Nil));
                }
                "contains_key" => {
                    let key_val = self.eval_expr(&args[0])?;
                    let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                        message: format!(
                            "Map key must be String, Int, or Bool, got {}",
                            key_val.type_name()
                        ),
                        span: args[0].span,
                    })?;
                    return Ok(Value::Bool(map.contains_key(&key)));
                }
                "keys" => return Ok(Value::List(map.keys())),
                "values" => return Ok(Value::List(map.values())),
                "entries" => {
                    let list = map
                        .entries()
                        .into_iter()
                        .map(|(k, v)| Value::Tuple(vec![k.to_value(), v.clone()]))
                        .collect();
                    return Ok(Value::List(list));
                }
                _ => {}
            },
            _ => {}
        }

        Err(RuntimeError {
            message: format!("Unknown method '{}()'", field),
            span: call_span,
        }
        .into())
    }
    fn resolve_pattern_type(&self, name: &str) -> ResolvedPatternType {
        match name {
            "Int" => ResolvedPatternType::Int,
            "Float" => ResolvedPatternType::Float,
            "String" => ResolvedPatternType::String,
            "Bool" => ResolvedPatternType::Bool,
            "Void" => ResolvedPatternType::Void,
            _ => {
                if let Some((alias, item_name)) = name.split_once('.') {
                    if let Some(target) = self
                        .module_imports
                        .get(&self.current_module)
                        .and_then(|m| m.get(alias))
                    {
                        match target {
                            ModuleTarget::Native(nat_name) => {
                                return ResolvedPatternType::Custom(TypeId::native(
                                    nat_name.clone(),
                                    item_name,
                                ));
                            }
                            ModuleTarget::Local(path) => {
                                return ResolvedPatternType::Custom(TypeId::local(
                                    path.clone(),
                                    item_name,
                                ));
                            }
                        }
                    }
                    ResolvedPatternType::Unknown
                } else if self
                    .module_structs
                    .get(&self.current_module)
                    .map_or(false, |s| s.contains(name))
                    || self
                        .module_errors
                        .get(&self.current_module)
                        .map_or(false, |e| e.contains(name))
                {
                    ResolvedPatternType::Custom(TypeId::from_module_opt(
                        self.current_module.as_ref(),
                        name,
                    ))
                } else if matches!(name, "IndexError" | "KeyError" | "ParseError") {
                    ResolvedPatternType::Custom(TypeId::builtin(name))
                } else {
                    ResolvedPatternType::Custom(TypeId::from_module_opt(
                        self.current_module.as_ref(),
                        name,
                    ))
                }
            }
        }
    }

    fn match_pattern(&self, pattern: &Pattern, val: &Value) -> Option<Vec<(String, Value)>> {
        match &pattern.kind {
            PatternKind::Nil => {
                if *val == Value::Nil {
                    Some(Vec::new())
                } else {
                    None
                }
            }
            PatternKind::Type { name, binding } => {
                let target = self.resolve_pattern_type(name);
                let matched = match (val, target) {
                    (
                        Value::Struct { type_id, .. } | Value::Error { type_id, .. },
                        ResolvedPatternType::Custom(ref pat_id),
                    ) => type_id == pat_id,
                    (Value::Native(nat), ResolvedPatternType::Custom(ref pat_id)) => {
                        &nat.type_id() == pat_id
                    }
                    (Value::Int(_), ResolvedPatternType::Int) => true,
                    (Value::Float(_), ResolvedPatternType::Float) => true,
                    (Value::String(_), ResolvedPatternType::String) => true,
                    (Value::Bool(_), ResolvedPatternType::Bool) => true,
                    (Value::Void, ResolvedPatternType::Void) => true,
                    _ => false,
                };
                if matched {
                    Some(vec![(binding.clone(), val.clone())])
                } else {
                    None
                }
            }
            PatternKind::Tuple(pats) => {
                if let Value::Tuple(items) = val {
                    if pats.len() == items.len() {
                        let mut bindings = Vec::new();
                        for (pat, item) in pats.iter().zip(items) {
                            match &pat.kind {
                                PatternKind::Ident(var_name) => {
                                    bindings.push((var_name.clone(), item.clone()));
                                }
                                PatternKind::Wildcard => {}
                                _ => return None,
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }
            PatternKind::Ident(var_name) => {
                Some(vec![(var_name.clone(), val.clone())])
            }
            PatternKind::Wildcard => {
                Some(Vec::new())
            }
        }
    }
}

enum ResolvedPatternType {
    Int,
    Float,
    String,
    Bool,
    Void,
    Custom(TypeId),
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::TypeChecker;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn run_code(src: &str) -> Result<Interpreter, String> {
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize_all().map_err(|e| e.to_string())?;
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().map_err(|e| e.to_string())?;
        let mut checker = TypeChecker::new();
        checker.check_program(&program).map_err(|e| e.to_string())?;
        let mut interp = Interpreter::new();
        interp.run_program(&program).map_err(|e| e.to_string())?;
        Ok(interp)
    }

    #[test]
    fn test_error_return_and_switch() {
        let src = r#"
            type User { name: String }
            error InvalidName { msg: String }

            func make_user(n: String) -> User | InvalidName {
                guard n.len() > 0 else {
                    return InvalidName(msg: "empty name")
                }
                return User(name: n)
            }

            func main() {
                u1 := make_user("Alice")
                var out1 = ""
                switch u1 {
                    case User(u):
                        out1 = u.name
                    case InvalidName(e):
                        out1 = e.msg
                }

                u2 := make_user("")
                var out2 = ""
                switch u2 {
                    case User(u):
                        out2 = u.name
                    case InvalidName(e):
                        out2 = e.msg
                }

                print(out1)
                print(out2)
            }
        "#;
        assert!(run_code(src).is_ok());
    }

    #[test]
    fn test_try_operator_flow() {
        let src = r#"
            type User { name: String }
            error InvalidName { msg: String }

            func make_user(n: String) -> User | InvalidName {
                guard n.len() > 0 else {
                    return InvalidName(msg: "empty name")
                }
                return User(name: n)
            }

            func register(n: String) -> User | InvalidName {
                u := make_user(n)?
                return u
            }

            func main() {
                u := register("Bob")
                switch u {
                    case User(user):
                        print(user.name)
                    case InvalidName(e):
                        print(e.msg)
                }
            }
        "#;
        assert!(run_code(src).is_ok());
    }

    #[test]
    fn test_tuples_and_destructuring() {
        let src = r#"
            error DivisionByZero {}

            func divide(a: Int, b: Int) -> (Int, Int) | DivisionByZero {
                guard b != 0 else {
                    return DivisionByZero()
                }
                return (a / b, a % b)
            }

            func quotient(a: Int, b: Int) -> Int | DivisionByZero {
                (q, _) := divide(a, b)?
                return q
            }

            func main() {
                switch quotient(17, 5) {
                    case Int(v):
                        print(v)
                    case DivisionByZero(e):
                        print("error")
                }
            }
        "#;
        assert!(run_code(src).is_ok());
    }

    #[test]
    fn test_map_preserves_errors_as_values() {
        let src = r#"
            error Bad { msg: String }

            func validate(n: Int) -> Int | Bad {
                guard n > 0 else {
                    return Bad(msg: "negative")
                }
                return n * 2
            }

            func main() {
                nums := [10, -5, 20]
                results := map(nums, validate)
                print(results)
            }
        "#;
        assert!(run_code(src).is_ok());
    }
}
