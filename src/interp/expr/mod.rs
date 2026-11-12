mod call;

use super::*;

impl Interpreter {
    #[inline]
    pub(super) fn eval_expr(&mut self, expr: &Expr) -> Result<Value, InterpFlow> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(f) => Ok(Value::Float(*f)),
            ExprKind::String(s) => Ok(Value::String(self.interpolate(s))),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Nil => Ok(Value::Nil),
            ExprKind::Tuple(elements) => self.eval_tuple_expr(elements),
            ExprKind::Try(inner) => self.eval_try_expr(inner),
            ExprKind::Ident(name) => self.eval_ident_expr(name, expr.span),
            ExprKind::List(elements) => self.eval_list_expr(elements),
            ExprKind::Closure { params, body, .. } => Ok(Value::Closure(Arc::new(ClosureData {
                params: params.clone(),
                body: body.clone(),
                env: self.snapshot_env(),
                module: self.current_module.clone(),
            }))),
            ExprKind::Index { target, index } => self.eval_index_expr(target, index),
            ExprKind::Unary { op, expr: inner } => self.eval_unary_expr(*op, inner, expr.span),
            ExprKind::Binary { op, left, right } => {
                self.eval_binary_expr(*op, left, right, expr.span)
            }
            ExprKind::Call {
                callee,
                type_args,
                args,
                named_args,
            } => self.eval_call_expr(callee, type_args, args, named_args, expr.span),
            ExprKind::Member { target, field } => {
                self.eval_member_expr(target, field, expr.span)
            }
            ExprKind::OptionalMember { target, field } => {
                self.eval_optional_member_expr(target, field, expr.span)
            }
        }
    }

    #[inline]
    fn eval_tuple_expr(&mut self, elements: &[Expr]) -> Result<Value, InterpFlow> {
        let mut vals = Vec::new();
        for e in elements {
            vals.push(self.eval_expr(e)?);
        }
        Ok(Value::Tuple(vals))
    }

    #[inline]
    fn eval_try_expr(&mut self, inner: &Expr) -> Result<Value, InterpFlow> {
        let val = self.eval_expr(inner)?;
        if val.is_error() {
            return Err(InterpFlow::Return(val));
        }
        Ok(val)
    }

    #[inline]
    fn eval_ident_expr(&mut self, name: &str, span: Span) -> Result<Value, InterpFlow> {
        if let Some(val) = self.lookup_variable(name) {
            Ok(val)
        } else if self
            .module_functions
            .contains_key(&(self.current_module.clone(), name.to_string()))
        {
            if let Some(ref cur_path) = self.current_module {
                Ok(Value::Function(format!("{}::{}", cur_path.display(), name)))
            } else {
                Ok(Value::Function(name.to_string()))
            }
        } else if self
            .module_functions
            .contains_key(&(None, name.to_string()))
        {
            Ok(Value::Function(name.to_string()))
        } else {
            Err(RuntimeError {
                message: format!("Unknown variable '{}'", name),
                span,
            }
            .into())
        }
    }

    #[inline]
    fn eval_list_expr(&mut self, elements: &[Expr]) -> Result<Value, InterpFlow> {
        let mut vals = Vec::new();
        for e in elements {
            vals.push(self.eval_expr(e)?);
        }
        Ok(Value::List(vals))
    }

    fn eval_index_expr(&mut self, target: &Expr, index: &Expr) -> Result<Value, InterpFlow> {
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
            }
            .into()),
            (Value::Map(map), key_val) => {
                let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                    message: format!(
                        "Map key must be String, Int, or Bool, got {}",
                        key_val.type_name()
                    ),
                    span: index.span,
                })?;
                Ok(map
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| make_key_error(&key)))
            }
            (other, _) => Err(RuntimeError {
                message: format!("Cannot index into value of type {}", other.type_name()),
                span: target.span,
            }
            .into()),
        }
    }

    #[inline]
    fn eval_unary_expr(
        &mut self,
        op: UnaryOp,
        inner: &Expr,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        let val = self.eval_expr(inner)?;
        match op {
            UnaryOp::Neg => match val {
                Value::Int(n) => n.checked_neg().map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                Value::Float(f) => Ok(Value::Float(-f)),
                _ => Err(RuntimeError {
                    message: "Unary '-' applies to Int or Float only".to_string(),
                    span,
                }
                .into()),
            },
            UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
        }
    }

    #[inline]
    fn eval_binary_expr(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        if op == BinaryOp::Coalesce {
            let l = self.eval_expr(left)?;
            return if l != Value::Nil {
                Ok(l)
            } else {
                self.eval_expr(right)
            };
        }
        if op == BinaryOp::And {
            let l = self.eval_expr(left)?;
            return if !l.is_truthy() {
                Ok(Value::Bool(false))
            } else {
                let r = self.eval_expr(right)?;
                Ok(Value::Bool(r.is_truthy()))
            };
        }
        if op == BinaryOp::Or {
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
                (Value::Int(a), Value::Int(b)) => a.checked_add(b).map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
                _ => Err(RuntimeError {
                    message: "Operator '+' requires matching Int, Float, or String operands"
                        .to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Sub => match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.checked_sub(b).map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(RuntimeError {
                    message: "Operator '-' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Mul => match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.checked_mul(b).map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                _ => Err(RuntimeError {
                    message: "Operator '*' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Div => match (l, r) {
                (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                    message: "Division by zero".to_string(),
                    span,
                }
                .into()),
                (Value::Int(a), Value::Int(b)) => a.checked_div(b).map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                (Value::Float(a), Value::Float(b)) => {
                    let res = a / b;
                    if b == 0.0 || !res.is_finite() {
                        Err(RuntimeError {
                            message: "Division by zero".to_string(),
                            span,
                        }
                        .into())
                    } else {
                        Ok(Value::Float(res))
                    }
                }
                _ => Err(RuntimeError {
                    message: "Operator '/' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Mod => match (l, r) {
                (Value::Int(_), Value::Int(0)) => Err(RuntimeError {
                    message: "Modulo by zero".to_string(),
                    span,
                }
                .into()),
                (Value::Int(a), Value::Int(b)) => a.checked_rem(b).map(Value::Int).ok_or_else(|| {
                    RuntimeError {
                        message: "Integer overflow".to_string(),
                        span,
                    }
                    .into()
                }),
                _ => Err(RuntimeError {
                    message: "Operator '%' requires Int operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Lt => match (l, r) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
                _ => Err(RuntimeError {
                    message: "Operator '<' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::LtEq => match (l, r) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
                _ => Err(RuntimeError {
                    message: "Operator '<=' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::Gt => match (l, r) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
                _ => Err(RuntimeError {
                    message: "Operator '>' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
            },
            BinaryOp::GtEq => match (l, r) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
                _ => Err(RuntimeError {
                    message: "Operator '>=' requires matching Int or Float operands".to_string(),
                    span,
                }
                .into()),
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
                    span,
                }
                .into()),
            },
        }
    }

    #[inline]
    fn eval_member_expr(
        &mut self,
        target: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        if let ExprKind::Ident(ref alias) = target.kind
            && let Some(target_mod) = self
                .module_imports
                .get(&self.current_module)
                .and_then(|m| m.get(alias))
        {
            match target_mod {
                ModuleTarget::Native(nat_name) => {
                    return Ok(Value::NativeFunction {
                        module: nat_name.clone(),
                        name: field.to_string(),
                    });
                }
                ModuleTarget::Local(path) => {
                    return Ok(Value::Function(format!("{}::{}", path.display(), field)));
                }
            }
        }

        if let Some(target_ref) = self.json_argument_ref(target) {
            return Self::eval_member_on_value(target_ref, field, span);
        }

        let target_val = self.eval_expr(target)?;
        Self::eval_member_on_value(&target_val, field, span)
    }

    fn eval_optional_member_expr(
        &mut self,
        target: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        let target_val = self.eval_expr(target)?;
        if target_val == Value::Nil {
            return Ok(Value::Nil);
        }
        if target_val.is_error() {
            return Err(InterpFlow::Return(target_val));
        }
        Self::eval_member_on_value(&target_val, field, span)
    }
}

