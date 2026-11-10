use std::collections::HashMap;

use crate::ast::*;
use crate::lexer::Span;
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
}

pub struct Interpreter {
    functions: HashMap<String, (Vec<Param>, Block)>,
    env: Vec<HashMap<String, Value>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
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
                                Value::Struct { ref fields, .. } => {
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

    pub fn run_program(&mut self, program: &Program) -> Result<(), RuntimeError> {
        for item in &program.items {
            if let ItemKind::Func { name, params, body, .. } = &item.kind {
                self.functions.insert(name.clone(), (params.clone(), body.clone()));
            }
        }

        if !self.functions.contains_key("main") {
            return Err(RuntimeError {
                message: "Entry point 'func main()' not found".to_string(),
                span: Span::new(1, 1),
            });
        }

        self.call_function("main", vec![], Span::new(1, 1))?;
        Ok(())
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, RuntimeError> {
        let (params, body) = self.functions.get(name).cloned().ok_or_else(|| RuntimeError {
            message: format!("Function '{}' not found", name),
            span,
        })?;

        self.enter_scope();
        for (i, param) in params.iter().enumerate() {
            let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
            self.define_variable(param.name.clone(), arg_val);
        }

        let mut return_val = Value::Void;
        for stmt in &body.stmts {
            match self.eval_stmt(stmt)? {
                ControlFlow::Next => {}
                ControlFlow::Return(val) => {
                    return_val = val;
                    break;
                }
            }
        }

        self.exit_scope();
        Ok(return_val)
    }

    fn eval_stmt(&mut self, stmt: &Stmt) -> Result<ControlFlow, RuntimeError> {
        match &stmt.kind {
            StmtKind::Define { name, value } => {
                let val = self.eval_expr(value)?;
                self.define_variable(name.clone(), val);
                Ok(ControlFlow::Next)
            }
            StmtKind::VarDecl { name, init, .. } => {
                let val = if let Some(init_expr) = init {
                    self.eval_expr(init_expr)?
                } else {
                    Value::Nil
                };
                self.define_variable(name.clone(), val);
                Ok(ControlFlow::Next)
            }
            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(value)?;
                if let ExprKind::Ident(ref name) = target.kind {
                    self.assign_variable(name, val, target.span)?;
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::Return(opt_expr) => {
                let val = if let Some(expr) = opt_expr {
                    self.eval_expr(expr)?
                } else {
                    Value::Void
                };
                Ok(ControlFlow::Return(val))
            }
            StmtKind::If { condition, then_block, else_block } => {
                let cond_val = self.eval_expr(condition)?;
                if cond_val.is_truthy() {
                    self.enter_scope();
                    for s in &then_block.stmts {
                        if let ControlFlow::Return(v) = self.eval_stmt(s)? {
                            self.exit_scope();
                            return Ok(ControlFlow::Return(v));
                        }
                    }
                    self.exit_scope();
                } else if let Some(eb) = else_block {
                    self.enter_scope();
                    for s in &eb.stmts {
                        if let ControlFlow::Return(v) = self.eval_stmt(s)? {
                            self.exit_scope();
                            return Ok(ControlFlow::Return(v));
                        }
                    }
                    self.exit_scope();
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::ForIn { variable, iterable, body } => {
                let iter_val = self.eval_expr(iterable)?;
                if let Value::List(items) = iter_val {
                    for item in items {
                        self.enter_scope();
                        self.define_variable(variable.clone(), item);
                        let mut returned = None;
                        for s in &body.stmts {
                            if let ControlFlow::Return(v) = self.eval_stmt(s)? {
                                returned = Some(v);
                                break;
                            }
                        }
                        self.exit_scope();
                        if let Some(v) = returned {
                            return Ok(ControlFlow::Return(v));
                        }
                    }
                }
                Ok(ControlFlow::Next)
            }
            StmtKind::Expr(expr) => {
                self.eval_expr(expr)?;
                Ok(ControlFlow::Next)
            }
        }
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::String(s) => Ok(Value::String(self.interpolate(s))),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),
            ExprKind::Ident(name) => {
                self.lookup_variable(name).ok_or_else(|| RuntimeError {
                    message: format!("Unknown variable '{}'", name),
                    span: expr.span,
                })
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
                            message: "Unary '-' requires Int".to_string(),
                            span: expr.span,
                        }),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
                }
            }
            ExprKind::Binary { op, left, right } => {
                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;

                match op {
                    BinaryOp::Add => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                        _ => Err(RuntimeError {
                            message: "Operator '+' requires Int or String".to_string(),
                            span: expr.span,
                        }),
                    },
                    BinaryOp::Sub => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
                        _ => Err(RuntimeError {
                            message: "Operator '-' requires Int".to_string(),
                            span: expr.span,
                        }),
                    },
                    BinaryOp::Mul => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
                        _ => Err(RuntimeError {
                            message: "Operator '*' requires Int".to_string(),
                            span: expr.span,
                        }),
                    },
                    BinaryOp::Div => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span: expr.span,
                        }),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
                        _ => Err(RuntimeError {
                            message: "Operator '/' requires Int".to_string(),
                            span: expr.span,
                        }),
                    },
                    BinaryOp::Mod => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span: expr.span,
                        }),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
                        _ => Err(RuntimeError {
                            message: "Operator '%' requires Int".to_string(),
                            span: expr.span,
                        }),
                    },
                    BinaryOp::Eq => Ok(Value::Bool(l == r)),
                    BinaryOp::NotEq => Ok(Value::Bool(l != r)),
                    BinaryOp::Lt => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                        _ => Ok(Value::Bool(false)),
                    },
                    BinaryOp::LtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                        _ => Ok(Value::Bool(false)),
                    },
                    BinaryOp::Gt => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                        _ => Ok(Value::Bool(false)),
                    },
                    BinaryOp::GtEq => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                        _ => Ok(Value::Bool(false)),
                    },
                    BinaryOp::And => Ok(Value::Bool(l.is_truthy() && r.is_truthy())),
                    BinaryOp::Or => Ok(Value::Bool(l.is_truthy() || r.is_truthy())),
                    BinaryOp::Range => match (l, r) {
                        (Value::Int(start), Value::Int(end)) => {
                            let list = (start..end).map(Value::Int).collect();
                            Ok(Value::List(list))
                        }
                        _ => Err(RuntimeError {
                            message: "Range '..' requires Int bounds".to_string(),
                            span: expr.span,
                        }),
                    },
                }
            }
            ExprKind::Call { callee, args, named_args } => {
                if let ExprKind::Ident(ref type_name) = callee.kind {
                    if !named_args.is_empty() {
                        let mut fields = HashMap::new();
                        for arg in named_args {
                            let val = self.eval_expr(&arg.value)?;
                            fields.insert(arg.name.clone(), val);
                        }
                        return Ok(Value::Struct {
                            name: type_name.clone(),
                            fields,
                        });
                    }
                }

                if let ExprKind::Member { target, field } = &callee.kind {
                    if field == "append" {
                        if let ExprKind::Ident(ref list_var_name) = target.kind {
                            let item_to_append = self.eval_expr(&args[0])?;
                            if let Some(Value::List(ref mut list)) = self.lookup_variable(list_var_name) {
                                let mut updated = list.clone();
                                updated.push(item_to_append);
                                self.assign_variable(list_var_name, Value::List(updated), target.span)?;
                                return Ok(Value::Void);
                            }
                        }
                    }

                    let target_val = self.eval_expr(target)?;
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

                if let ExprKind::Ident(ref name) = callee.kind {
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

                if let ExprKind::Ident(ref name) = callee.kind {
                    let mut evaluated_args = Vec::new();
                    for a in args {
                        evaluated_args.push(self.eval_expr(a)?);
                    }
                    return self.call_function(name, evaluated_args, callee.span);
                }

                Err(RuntimeError {
                    message: "Unsupported call expression".to_string(),
                    span: callee.span,
                })
            }
            ExprKind::Member { target, field } => {
                let target_val = self.eval_expr(target)?;
                match target_val {
                    Value::Struct { ref fields, .. } => {
                        if let Some(val) = fields.get(field) {
                            Ok(val.clone())
                        } else {
                            Err(RuntimeError {
                                message: format!("Field '{}' not found in struct", field),
                                span: expr.span,
                            })
                        }
                    }
                    _ => Err(RuntimeError {
                        message: format!("Field access not supported on {}", target_val.type_name()),
                        span: expr.span,
                    }),
                }
            }
        }
    }
}

