use std::collections::{HashMap, HashSet};

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
    errors: HashSet<String>,
    scopes: Vec<HashMap<String, (Type, Span, bool)>>,
    current_return_type: Option<Type>,
    loop_depth: usize,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            functions: HashMap::new(),
            structs: HashMap::new(),
            errors: HashSet::new(),
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
            Type::Custom(name) => self.errors.contains(name),
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
                    self.structs.insert(name.clone(), StructSignature { fields: field_map });
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
                    self.structs.insert(name.clone(), StructSignature { fields: field_map });
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
                        },
                    );
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

                if ret_ty != Type::Void && !block_always_returns(body) {
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
                                other => Type::Custom(other.to_string()),
                            };
                            let idx = remaining.iter().position(|t| t == &custom_ty).ok_or_else(|| TypeError {
                                message: format!("Redundant or impossible case '{}(...)' for type '{}'", name, expr_ty),
                                span: case.pattern.span,
                            })?;
                            remaining.remove(idx);
                            // In switch pattern matching, the case branch handles the variant
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
                        let fn_name = match &args[1].kind {
                            ExprKind::Ident(f) => f,
                            _ => {
                                return Err(TypeError {
                                    message: "Second argument to 'map' must be a function name".to_string(),
                                    span: args[1].span,
                                });
                            }
                        };
                        let sig = self.functions.get(fn_name).cloned().ok_or_else(|| TypeError {
                            message: format!("Unknown function '{}' in 'map'", fn_name),
                            span: args[1].span,
                        })?;
                        if sig.params.len() != 1 {
                            return Err(TypeError {
                                message: format!("Function '{}' passed to 'map' must take 1 argument, but takes {}", fn_name, sig.params.len()),
                                span: args[1].span,
                            });
                        }
                        if !item_ty.is_assignable_to(&sig.params[0].1) {
                            return Err(TypeError {
                                message: format!("List element type '{}' is not assignable to parameter type '{}' of function '{}'", item_ty, sig.params[0].1, fn_name),
                                span: args[1].span,
                            });
                        }
                        return Ok(Type::List(Box::new(sig.return_type)));
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
                        let fn_name = match &args[1].kind {
                            ExprKind::Ident(f) => f,
                            _ => {
                                return Err(TypeError {
                                    message: "Second argument to 'filter' must be a function name".to_string(),
                                    span: args[1].span,
                                });
                            }
                        };
                        let sig = self.functions.get(fn_name).cloned().ok_or_else(|| TypeError {
                            message: format!("Unknown function '{}' in 'filter'", fn_name),
                            span: args[1].span,
                        })?;
                        if sig.params.len() != 1 {
                            return Err(TypeError {
                                message: format!("Function '{}' passed to 'filter' must take 1 argument, but takes {}", fn_name, sig.params.len()),
                                span: args[1].span,
                            });
                        }
                        if !item_ty.is_assignable_to(&sig.params[0].1) {
                            return Err(TypeError {
                                message: format!("List element type '{}' is not assignable to parameter type '{}' of function '{}'", item_ty, sig.params[0].1, fn_name),
                                span: args[1].span,
                            });
                        }
                        if sig.return_type != Type::Bool {
                            return Err(TypeError {
                                message: format!("Function '{}' passed to 'filter' must return 'Bool', got '{}'", fn_name, sig.return_type),
                                span: args[1].span,
                            });
                        }
                        return Ok(Type::List(Box::new(item_ty)));
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
            crate::ast::StmtKind::Switch { cases, .. } => {
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
}
