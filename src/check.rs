use std::collections::HashMap;

use crate::ast::*;
use crate::lexer::Span;
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
}

#[derive(Debug, Clone)]
pub struct StructSignature {
    pub fields: HashMap<String, Type>,
}

pub struct TypeChecker {
    functions: HashMap<String, FuncSignature>,
    structs: HashMap<String, StructSignature>,
    scopes: Vec<HashMap<String, Type>>,
    current_return_type: Option<Type>,
    loop_depth: usize,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            functions: HashMap::new(),
            structs: HashMap::new(),
            scopes: Vec::new(),
            current_return_type: None,
            loop_depth: 0,
        };

        checker.functions.insert(
            "print".to_string(),
            FuncSignature {
                params: vec![("msg".to_string(), Type::Any)],
                return_type: Type::Void,
            },
        );

        checker
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_variable(&mut self, name: String, ty: Type) {
        if let Some(current_scope) = self.scopes.last_mut() {
            current_scope.insert(name, ty);
        }
    }

    fn lookup_variable(&self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            match &item.kind {
                ItemKind::Type { name, fields } => {
                    let mut field_map = HashMap::new();
                    for f in fields {
                        field_map.insert(f.name.clone(), Type::from_type_expr(&f.ty));
                    }
                    if self.structs.contains_key(name) {
                        return Err(TypeError {
                            message: format!("Duplicate struct definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.structs.insert(name.clone(), StructSignature { fields: field_map });
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
                        },
                    );
                }
            }
        }

        for item in &program.items {
            if let ItemKind::Func { name, body, .. } = &item.kind {
                let sig = self.functions.get(name).unwrap().clone();
                self.current_return_type = Some(sig.return_type.clone());

                self.enter_scope();
                for (param_name, param_ty) in sig.params {
                    self.define_variable(param_name, param_ty);
                }

                self.check_block(body)?;
                self.exit_scope();

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
                self.define_variable(name.clone(), val_ty);
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
                    self.define_variable(name.clone(), declared_ty.unwrap_or(init_ty));
                } else {
                    let ty = declared_ty.ok_or_else(|| TypeError {
                        message: format!("Variable '{}' must have an explicit type or default value", name),
                        span: stmt.span,
                    })?;
                    self.define_variable(name.clone(), ty);
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
                self.exit_scope();

                if let Some(eb) = else_block {
                    self.enter_scope();
                    self.check_block(eb)?;
                    self.exit_scope();
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
                self.exit_scope();

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
                self.define_variable(variable.clone(), item_ty);
                let res = self.check_block(body);
                self.exit_scope();
                self.loop_depth -= 1;
                res?;
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
                self.check_expr(expr)?;
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

        Err(TypeError {
            message: format!("Unknown method or invalid arguments for '{}.{}()'", target_ty, field),
            span,
        })
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
            ExprKind::Call { callee, args, named_args } => {
                if let ExprKind::Ident(ref type_name) = callee.kind {
                    if let Some(struct_sig) = self.structs.get(type_name).cloned() {
                        for arg in named_args {
                            let expected_field_ty = struct_sig.fields.get(&arg.name).ok_or_else(|| TypeError {
                                message: format!("Struct '{}' has no field '{}'", type_name, arg.name),
                                span: callee.span,
                            })?;
                            let actual_arg_ty = self.check_expr(&arg.value)?;
                            if !actual_arg_ty.is_assignable_to(expected_field_ty) {
                                return Err(TypeError {
                                    message: format!("Field '{}' of struct '{}' expects {}, got {}", arg.name, type_name, expected_field_ty, actual_arg_ty),
                                    span: arg.value.span,
                                });
                            }
                        }
                        return Ok(Type::Custom(type_name.clone()));
                    }
                }

                if let ExprKind::Member { target, field } = &callee.kind {
                    let target_ty = self.check_expr(target)?;
                    return self.check_method_call(&target_ty, field, args, callee.span);
                }

                let func_name = match callee.kind {
                    ExprKind::Ident(ref name) => name.clone(),
                    _ => {
                        return Err(TypeError {
                            message: "Calling non-identifier expressions is not supported".to_string(),
                            span: callee.span,
                        });
                    }
                };

                let sig = self.functions.get(&func_name).cloned().ok_or_else(|| TypeError {
                    message: format!("Unknown function '{}'", func_name),
                    span: callee.span,
                })?;

                if func_name != "print" {
                    if args.len() != sig.params.len() {
                        return Err(TypeError {
                            message: format!("Function '{}' expects {} arguments, got {}", func_name, sig.params.len(), args.len()),
                            span: expr.span,
                        });
                    }

                    for (i, arg_expr) in args.iter().enumerate() {
                        let actual_ty = self.check_expr(arg_expr)?;
                        let expected_ty = &sig.params[i].1;
                        if !actual_ty.is_assignable_to(expected_ty) {
                            return Err(TypeError {
                                message: format!("Argument {} of function '{}' expects {}, got {}", i + 1, func_name, expected_ty, actual_ty),
                                span: arg_expr.span,
                            });
                        }
                    }
                } else {
                    for a in args {
                        self.check_expr(a)?;
                    }
                }

                Ok(sig.return_type)
            }
            ExprKind::Member { target, field } => {
                let target_ty = self.check_expr(target)?;

                if let Type::Custom(ref struct_name) = target_ty {
                    if let Some(struct_sig) = self.structs.get(struct_name) {
                        if let Some(field_ty) = struct_sig.fields.get(field) {
                            return Ok(field_ty.clone());
                        } else {
                            return Err(TypeError {
                                message: format!("Struct '{}' has no field '{}'", struct_name, field),
                                span: expr.span,
                            });
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

fn block_always_returns(block: &crate::ast::Block) -> bool {
    for stmt in &block.stmts {
        match &stmt.kind {
            crate::ast::StmtKind::Return(_) => return true,
            crate::ast::StmtKind::If {
                then_block,
                else_block: Some(else_b),
                ..
            } => {
                if block_always_returns(then_block) && block_always_returns(else_b) {
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
}
