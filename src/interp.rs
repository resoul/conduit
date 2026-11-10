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
    functions: HashMap<String, (Vec<Param>, Block)>,
    structs: std::collections::HashSet<String>,
    errors: std::collections::HashSet<String>,
    env: Vec<HashMap<String, Value>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            structs: std::collections::HashSet::new(),
            errors: std::collections::HashSet::new(),
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

    pub fn run_program(&mut self, program: &Program) -> Result<(), RuntimeError> {
        for item in &program.items {
            match &item.kind {
                ItemKind::Func { name, params, body, .. } => {
                    self.functions.insert(name.clone(), (params.clone(), body.clone()));
                }
                ItemKind::Type { name, .. } => {
                    self.structs.insert(name.clone());
                }
                ItemKind::Error { name, .. } => {
                    self.errors.insert(name.clone());
                }
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
                ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue => {}
                ControlFlow::Return(val) => {
                    return_val = val;
                    break;
                }
            }
        }

        self.exit_scope();
        Ok(return_val)
    }

    fn eval_block(&mut self, block: &crate::ast::Block) -> Result<ControlFlow, RuntimeError> {
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
                self.lookup_variable(name).ok_or_else(|| RuntimeError {
                    message: format!("Unknown variable '{}'", name),
                    span: expr.span,
                }.into())
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
                            message: "Operator '+' requires Int or String".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Sub => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
                        _ => Err(RuntimeError {
                            message: "Operator '-' requires Int".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Mul => match (l, r) {
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
                        _ => Err(RuntimeError {
                            message: "Operator '*' requires Int".to_string(),
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
                            message: "Operator '/' requires Int".to_string(),
                            span: expr.span,
                        }.into()),
                    },
                    BinaryOp::Mod => match (l, r) {
                        (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span: expr.span,
                        }.into()),
                        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
                        _ => Err(RuntimeError {
                            message: "Operator '%' requires Int".to_string(),
                            span: expr.span,
                        }.into()),
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
                        }.into()),
                    },
                    BinaryOp::Coalesce => match l {
                        Value::Nil => Ok(r),
                        other => Ok(other),
                    },
                }
            }
            ExprKind::Call { callee, args, named_args } => {
                if let ExprKind::Ident(ref type_name) = callee.kind {
                    if self.errors.contains(type_name) || self.structs.contains(type_name) || !named_args.is_empty() {
                        let mut fields = HashMap::new();
                        for arg in named_args {
                            let val = self.eval_expr(&arg.value)?;
                            fields.insert(arg.name.clone(), val);
                        }
                        return Ok(if self.errors.contains(type_name) {
                            Value::Error {
                                name: type_name.clone(),
                                fields,
                            }
                        } else {
                            Value::Struct {
                                name: type_name.clone(),
                                fields,
                            }
                        });
                    }

                    if type_name == "map" && args.len() == 2 {
                        let list_val = self.eval_expr(&args[0])?;
                        let fn_name = match &args[1].kind {
                            ExprKind::Ident(f) => f.clone(),
                            _ => {
                                return Err(RuntimeError {
                                    message: "Second argument to 'map' must be a function name".to_string(),
                                    span: args[1].span,
                                }.into());
                            }
                        };
                        if let Value::List(items) = list_val {
                            let mut results = Vec::new();
                            for item in items {
                                let res = self.call_function(&fn_name, vec![item], args[1].span)?;
                                results.push(res);
                            }
                            return Ok(Value::List(results));
                        }
                    }

                    if type_name == "filter" && args.len() == 2 {
                        let list_val = self.eval_expr(&args[0])?;
                        let fn_name = match &args[1].kind {
                            ExprKind::Ident(f) => f.clone(),
                            _ => {
                                return Err(RuntimeError {
                                    message: "Second argument to 'filter' must be a function name".to_string(),
                                    span: args[1].span,
                                }.into());
                            }
                        };
                        if let Value::List(items) = list_val {
                            let mut filtered = Vec::new();
                            for item in items {
                                let res = self.call_function(&fn_name, vec![item.clone()], args[1].span)?;
                                if res.is_truthy() {
                                    filtered.push(item);
                                }
                            }
                            return Ok(Value::List(filtered));
                        }
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
                    return self.call_function(name, evaluated_args, callee.span).map_err(Into::into);
                }

                Err(RuntimeError {
                    message: "Unsupported call expression".to_string(),
                    span: callee.span,
                }.into())
            }
            ExprKind::Member { target, field } => {
                let target_val = self.eval_expr(target)?;
                match target_val {
                    Value::Struct { ref fields, .. } | Value::Error { ref fields, .. } => {
                        if let Some(val) = fields.get(field) {
                            Ok(val.clone())
                        } else {
                            Err(RuntimeError {
                                message: format!("Field '{}' not found in struct or error", field),
                                span: expr.span,
                            }.into())
                        }
                    }
                    _ => Err(RuntimeError {
                        message: format!("Field access not supported on {}", target_val.type_name()),
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
            if val == &Value::Nil {
                Some(Vec::new())
            } else {
                None
            }
        }
        PatternKind::Type { name, binding } => {
            match val {
                Value::Int(_) if name == "Int" => {
                    Some(vec![(binding.clone(), val.clone())])
                }
                Value::String(_) if name == "String" => {
                    Some(vec![(binding.clone(), val.clone())])
                }
                Value::Bool(_) if name == "Bool" => {
                    Some(vec![(binding.clone(), val.clone())])
                }
                Value::Struct { name: sname, .. } if sname == name => {
                    Some(vec![(binding.clone(), val.clone())])
                }
                Value::Error { name: ename, .. } if ename == name => {
                    Some(vec![(binding.clone(), val.clone())])
                }
                _ => None,
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
                res_ok := register("Bob")
                switch res_ok {
                    case User(u):
                        print(u.name)
                    case InvalidName(e):
                        print(e.msg)
                }

                res_err := register("")
                switch res_err {
                    case User(u):
                        print(u.name)
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
            error DivZero {}

            func divide(a: Int, b: Int) -> (Int, Int) | DivZero {
                guard b != 0 else {
                    return DivZero()
                }
                return (a / b, a % b)
            }

            func main() {
                res := divide(17, 5)
                switch res {
                    case (q, r):
                        print(q)
                        print(r)
                    case DivZero(e):
                        print("error")
                }
            }
        "#;
        assert!(run_code(src).is_ok());
    }

    #[test]
    fn test_map_preserves_errors_as_values() {
        let src = r#"
            type User { name: String }
            error InvalidName { msg: String }

            func create_user(name: String) -> User | InvalidName {
                guard name.len() > 0 else {
                    return InvalidName(msg: "empty")
                }
                return User(name: name)
            }

            func main() {
                names := ["Alice", "", "Charlie"]
                results := map(names, create_user)
                for r in results {
                    switch r {
                        case User(u):
                            print(u.name)
                        case InvalidName(e):
                            print(e.msg)
                    }
                }
            }
        "#;
        assert!(run_code(src).is_ok());
    }
}

