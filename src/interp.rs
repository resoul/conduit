use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::lexer::Span;
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::NativeRegistry;
use crate::value::Value;

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
                    ItemKind::Type { name, .. } => {
                        structs_set.insert(name.clone());
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
                ItemKind::Type { name, .. } => {
                    structs_set.insert(name.clone());
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
        self.current_module = mod_path;

        self.enter_scope();
        for (i, param) in params.iter().enumerate() {
            let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
            self.define_variable(param.name.clone(), arg_val);
        }

        let mut return_val = Value::Void;
        for stmt in &body.stmts {
            match self.eval_stmt(stmt)? {
                ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue => {}
                ControlFlow::Return(val) => {
                    return_val = val;
                    break;
                }
            }
        }

        self.exit_scope();
        self.current_module = prev_mod;
        Ok(return_val)
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
            Value::NativeFunction { module, name } => {
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
                let val = match self.eval_expr(value) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if let ExprKind::Ident(ref name) = target.kind {
                    self.assign_variable(name, val, target.span)?;
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
            StmtKind::ForIn { variable, iterable, body } => {
                let iter_val = match self.eval_expr(iterable) {
                    Ok(v) => v,
                    Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                    Err(InterpFlow::Error(err)) => return Err(err),
                };
                if let Value::List(items) = iter_val {
                    for item in items {
                        self.enter_scope();
                        self.define_variable(variable.clone(), item);
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
                    if let Some(bindings) = match_pattern(&case.pattern, &val) {
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
                } else if self.module_functions.contains_key(&(self.current_module.clone(), name.clone()))
                    || self.module_functions.contains_key(&(None, name.clone()))
                {
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
            ExprKind::Unary { op, expr: inner } => {
                let val = self.eval_expr(inner)?;
                match op {
                    UnaryOp::Neg => match val {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        _ => Err(RuntimeError {
                            message: "Unary '-' applies to Int only".to_string(),
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

                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;

                match op {
                    BinaryOp::Add => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                        _ => Err(RuntimeError {
                            message: "Operator '+' requires matching Int or String operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Sub => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
                        _ => Err(RuntimeError {
                            message: "Operator '-' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Mul => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
                        _ => Err(RuntimeError {
                            message: "Operator '*' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Div => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span: expr.span,
                        }.into()),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
                        _ => Err(RuntimeError {
                            message: "Operator '/' requires Int operands".to_string(),
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
                        _ => Err(RuntimeError {
                            message: "Operator '<' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::LtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                        _ => Err(RuntimeError {
                            message: "Operator '<=' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Gt => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                        _ => Err(RuntimeError {
                            message: "Operator '>' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::GtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                        _ => Err(RuntimeError {
                            message: "Operator '>=' requires Int operands".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Eq => Ok(Value::Bool(l == r)),
                    BinaryOp::NotEq => Ok(Value::Bool(l != r)),
                    BinaryOp::And => Ok(Value::Bool(l.is_truthy() && r.is_truthy())),
                    BinaryOp::Or => Ok(Value::Bool(l.is_truthy() || r.is_truthy())),
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
                    BinaryOp::Coalesce => unreachable!(),
                }
            }
            ExprKind::Call { callee, args, named_args } => {
                // 1. Direct local struct or error constructor
                if let ExprKind::Ident(ref name) = callee.kind {
                    let cur_structs = self.module_structs.get(&self.current_module);
                    let is_struct = cur_structs.map(|s| s.contains(name)).unwrap_or(false);
                    let cur_errors = self.module_errors.get(&self.current_module);
                    let is_error = cur_errors.map(|e| e.contains(name)).unwrap_or(false);

                    if is_struct {
                        let mut fields = HashMap::new();
                        for arg in named_args {
                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                        }
                        return Ok(Value::Struct {
                            name: name.clone(),
                            fields,
                        });
                    }
                    if is_error {
                        let mut fields = HashMap::new();
                        for arg in named_args {
                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                        }
                        return Ok(Value::Error {
                            name: name.clone(),
                            fields,
                        });
                    }

                    // map(list, callback)
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

                    // filter(list, callback)
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

                // 2. Member call on imported module: `alias.foo(...)`
                if let ExprKind::Member { target, field } = &callee.kind {
                    if let ExprKind::Ident(ref alias) = target.kind {
                        let cur_imports = self.module_imports.get(&self.current_module).cloned();
                        if let Some(target_mod) = cur_imports.and_then(|m| m.get(alias).cloned()) {
                            match target_mod {
                                ModuleTarget::Native(nat_name) => {
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
                                            name: format!("{}.{}", alias, field),
                                            fields,
                                        });
                                    }
                                    if is_error {
                                        let mut fields = HashMap::new();
                                        for arg in named_args {
                                            fields.insert(arg.name.clone(), self.eval_expr(&arg.value)?);
                                        }
                                        return Ok(Value::Error {
                                            name: format!("{}.{}", alias, field),
                                            fields,
                                        });
                                    }

                                    // Function in imported module
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

                    // 3. Method call on value: `target.method(...)`
                    let target_val = self.eval_expr(target)?;
                    if let Value::Native(ref nat) = target_val {
                        let short_type = nat.type_name().split('.').last().unwrap_or(nat.type_name());
                        let method_impl = {
                            let mut found = None;
                            for m in self.native_registry.modules() {
                                if let Some(method_def) = m.methods.get(&(short_type, field.as_str())) {
                                    found = Some(method_def.implementation);
                                    break;
                                }
                            }
                            found
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

                    if field == "append" && args.len() == 1 {
                        if let ExprKind::Ident(ref target_var) = target.kind {
                            let val_to_append = self.eval_expr(&args[0])?;
                            if let Some(mut current_val) = self.lookup_variable(target_var) {
                                if let Value::List(ref mut items) = current_val {
                                    items.push(val_to_append);
                                    self.assign_variable(target_var, current_val, target.span)?;
                                    return Ok(Value::Void);
                                }
                            }
                        }
                    }

                    match target_val {
                        Value::String(s) => match field.as_str() {
                            "trim" => return Ok(Value::String(s.trim().to_string())),
                            "lower" => return Ok(Value::String(s.to_lowercase())),
                            "upper" => return Ok(Value::String(s.to_uppercase())),
                            "len" => return Ok(Value::Int(s.len() as i64)),
                            "contains" => {
                                let search = self.eval_expr(&args[0])?;
                                if let Value::String(sub) = search {
                                    return Ok(Value::Bool(s.contains(&sub)));
                                }
                            }
                            _ => {}
                        },
                        Value::List(items) => match field.as_str() {
                            "len" => return Ok(Value::Int(items.len() as i64)),
                            "contains" => {
                                let search = self.eval_expr(&args[0])?;
                                return Ok(Value::Bool(items.contains(&search)));
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }

                // 4. General function or callable expression call
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
                match target_val {
                    Value::Struct { ref fields, .. } | Value::Error { ref fields, .. } => {
                        if let Some(val) = fields.get(field) {
                            Ok(val.clone())
                        } else {
                            Err(RuntimeError {
                                message: format!("Field '{}' not found", field),
                                span: expr.span,
                            }.into())
                        }
                    }
                    _ => Err(RuntimeError {
                        message: format!("Field access not supported on type {}", target_val.type_name()),
                        span: expr.span,
                    }.into()),
                }
            }
        }
    }
}

fn match_pattern(pattern: &Pattern, val: &Value) -> Option<Vec<(String, Value)>> {
    match &pattern.kind {
        PatternKind::Nil => {
            if *val == Value::Nil {
                Some(Vec::new())
            } else {
                None
            }
        }
        PatternKind::Type { name, binding } => {
            let name_item = name.split('.').last().unwrap_or(name);
            let matched = match val {
                Value::Struct { name: struct_name, .. } | Value::Error { name: struct_name, .. } => {
                    let struct_item = struct_name.split('.').last().unwrap_or(struct_name);
                    struct_name == name
                        || struct_name.ends_with(&format!(".{}", name))
                        || name.ends_with(&format!(".{}", struct_name))
                        || struct_item == name_item
                }
                Value::Native(nat) => {
                    let nat_item = nat.type_name().split('.').last().unwrap_or(nat.type_name());
                    nat.type_name() == name
                        || nat.type_name().ends_with(&format!(".{}", name))
                        || name.ends_with(&format!(".{}", nat.type_name()))
                        || nat_item == name_item
                }
                Value::Int(_) => name == "Int",
                Value::String(_) => name == "String",
                Value::Bool(_) => name == "Bool",
                Value::Void => name == "Void",
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
