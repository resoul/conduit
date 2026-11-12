use super::super::*;

impl TypeChecker {
    pub(super) fn check_string_expr(&mut self, value: &str, span: Span) -> Result<Type, TypeError> {
        let mut chars = value.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '{' {
                continue;
            }
            let mut segment = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                segment.push(inner);
            }
            if closed {
                self.check_interpolation_path(&segment, span)?;
            }
        }
        Ok(Type::String)
    }

    fn is_interpolation_identifier(value: &str) -> bool {
        let mut bytes = value.bytes();
        matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
            && bytes.all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
    }

    fn check_interpolation_path(&mut self, segment: &str, span: Span) -> Result<(), TypeError> {
        let parts: Vec<&str> = segment.split('.').collect();
        if parts.is_empty() || !parts.iter().all(|p| Self::is_interpolation_identifier(p)) {
            return Ok(());
        }
        let mut cur_ty = self.lookup_variable(parts[0]).ok_or_else(|| TypeError {
            message: format!("Unknown variable '{}' in string interpolation", parts[0]),
            span,
        })?;
        for field in &parts[1..] {
            cur_ty = self.check_member_on_type(&cur_ty, field, span)?;
        }
        Ok(())
    }

    pub(super) fn check_ident_expr(&mut self, name: &str, span: Span) -> Result<Type, TypeError> {
        if let Some(ty) = self.lookup_variable(name) {
            Ok(ty)
        } else if let Some(sig) = self.functions.get(name).cloned() {
            if !sig.type_params.is_empty() {
                return Err(TypeError {
                    message: format!(
                        "Generic function '{}' cannot be used as a first-class value",
                        name
                    ),
                    span,
                });
            }
            let param_types = sig.params.iter().map(|p| p.1.clone()).collect();
            Ok(Type::Function {
                params: param_types,
                return_type: Box::new(sig.return_type),
            })
        } else if self.structs.contains_key(name) {
            Ok(Type::Custom(self.current_type_id(name)))
        } else if matches!(name, "IndexError" | "KeyError" | "ParseError") {
            Ok(Type::Custom(TypeId::builtin(name)))
        } else {
            Err(TypeError {
                message: format!("Unknown variable '{}'", name),
                span,
            })
        }
    }

    pub(super) fn check_list_expr(
        &mut self,
        elements: &[Expr],
        span: Span,
    ) -> Result<Type, TypeError> {
        if elements.is_empty() {
            return Err(TypeError {
                message: "Cannot infer element type for empty list '[]'; add an explicit type annotation".to_string(),
                span,
            });
        }

        let first_ty = self.check_expr(&elements[0])?;
        for elem in &elements[1..] {
            let elem_ty = self.check_expr_with_expected(elem, &first_ty)?;
            if !elem_ty.is_assignable_to(&first_ty) {
                return Err(TypeError {
                    message: format!(
                        "List elements must have compatible types: expected {}, got {}",
                        first_ty, elem_ty
                    ),
                    span: elem.span,
                });
            }
        }
        Ok(Type::List(Box::new(first_ty)))
    }

    pub(super) fn check_index_expr(
        &mut self,
        target: &Expr,
        index: &Expr,
    ) -> Result<Type, TypeError> {
        let target_ty = self.check_expr(target)?;
        match target_ty {
            Type::List(elem_ty) => {
                let index_ty = self.check_expr(index)?;
                if index_ty != Type::Int {
                    return Err(TypeError {
                        message: format!("List index must be Int, got '{}'", index_ty),
                        span: index.span,
                    });
                }
                Ok(Type::union(vec![
                    *elem_ty,
                    Type::Custom(TypeId::builtin("IndexError")),
                ]))
            }
            Type::Map(key_ty, val_ty) => {
                let index_ty = self.check_expr(index)?;
                if !index_ty.is_assignable_to(&key_ty) {
                    return Err(TypeError {
                        message: format!("Map key must be '{}', got '{}'", key_ty, index_ty),
                        span: index.span,
                    });
                }
                Ok(Type::union(vec![
                    *val_ty,
                    Type::Custom(TypeId::builtin("KeyError")),
                ]))
            }
            other => Err(TypeError {
                message: format!(
                    "Cannot index into non-list type '{}' (only List and Map support indexing)",
                    other
                ),
                span: target.span,
            }),
        }
    }

    pub(super) fn check_closure_expr(
        &mut self,
        params: &[Param],
        return_type: Option<&TypeExpr>,
        body: &Block,
        span: Span,
    ) -> Result<Type, TypeError> {
        let mut param_types = Vec::with_capacity(params.len());
        for p in params {
            let ty = self.resolve_type_expr(&p.ty, p.span)?;
            self.check_generic_arity_in_type(&ty, p.span)?;
            param_types.push((p.name.clone(), ty));
        }
        let ret_ty = match return_type {
            Some(rt) => {
                let resolved = self.resolve_type_expr(rt, span)?;
                self.check_generic_arity_in_type(&resolved, span)?;
                resolved
            }
            None => Type::Void,
        };

        let can_fallthrough_void = match &ret_ty {
            Type::Void => true,
            Type::Union(vars) => vars.contains(&Type::Void),
            _ => false,
        };
        if !can_fallthrough_void && !block_always_returns(body) {
            return Err(TypeError {
                message: format!(
                    "Closure must return a value of type '{}' on all reachable paths",
                    ret_ty
                ),
                span,
            });
        }

        let prev_return_type = self.current_return_type.replace(ret_ty.clone());
        let prev_loop_depth = self.loop_depth;
        let saved_narrowed = self.narrowed.clone();
        let saved_widened = std::mem::take(&mut self.widened_outer_vars);
        self.loop_depth = 0;

        self.closure_boundary_scope_depth.push(self.scopes.len());
        self.enter_scope();
        for (param_name, param_ty) in &param_types {
            self.define_variable(param_name.clone(), param_ty.clone(), false, span);
        }

        let body_res = self.check_block(body);
        let exit_res = self.exit_scope();
        self.closure_boundary_scope_depth.pop();

        self.narrowed = saved_narrowed;
        self.widened_outer_vars = saved_widened;
        self.loop_depth = prev_loop_depth;
        self.current_return_type = prev_return_type;

        body_res?;
        exit_res?;

        Ok(Type::Function {
            params: param_types.into_iter().map(|(_, t)| t).collect(),
            return_type: Box::new(ret_ty),
        })
    }

    pub(super) fn check_unary_expr(&mut self, op: UnaryOp, inner: &Expr) -> Result<Type, TypeError> {
        let inner_ty = self.check_expr(inner)?;
        match op {
            UnaryOp::Neg => {
                if inner_ty == Type::Int || inner_ty == Type::Float {
                    Ok(inner_ty)
                } else {
                    Err(TypeError {
                        message: format!("Unary '-' applies to Int or Float only, got {}", inner_ty),
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

    pub(super) fn check_binary_expr(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> Result<Type, TypeError> {
        let left_ty = self.check_expr(left)?;
        let right_ty = match op {
            BinaryOp::And => {
                let saved = self.narrowed.clone();
                let vars = Self::extract_non_nil_vars(left, true);
                self.apply_non_nil_narrowings(&vars);
                let res = self.check_expr(right);
                self.narrowed = saved;
                res?
            }
            BinaryOp::Or => {
                let saved = self.narrowed.clone();
                let vars = Self::extract_non_nil_vars(left, false);
                self.apply_non_nil_narrowings(&vars);
                let res = self.check_expr(right);
                self.narrowed = saved;
                res?
            }
            BinaryOp::Coalesce => {
                let expected = match &left_ty {
                    Type::Optional(inner) => Some(inner.as_ref().clone()),
                    Type::Union(variants) if variants.contains(&Type::Nil) => {
                        let remaining: Vec<Type> = variants
                            .iter()
                            .filter(|variant| **variant != Type::Nil)
                            .cloned()
                            .collect();
                        Some(Type::union(remaining))
                    }
                    _ => None,
                };
                if let Some(expected) = expected {
                    self.check_expr_with_expected(right, &expected)?
                } else {
                    self.check_expr(right)?
                }
            }
            _ => self.check_expr(right)?,
        };

        match op {
            BinaryOp::Add => {
                if (left_ty == Type::Int && right_ty == Type::Int)
                    || (left_ty == Type::Float && right_ty == Type::Float)
                    || (left_ty == Type::String && right_ty == Type::String)
                {
                    Ok(left_ty)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Operator '+' requires matching Int, Float, or String operands, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
            BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                if (left_ty == Type::Int && right_ty == Type::Int)
                    || (left_ty == Type::Float && right_ty == Type::Float)
                {
                    Ok(left_ty)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Arithmetic operator requires matching Int or Float operands, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
            BinaryOp::Mod => {
                if left_ty == Type::Int && right_ty == Type::Int {
                    Ok(Type::Int)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Operator '%' requires Int operands, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                if (left_ty == Type::Int && right_ty == Type::Int)
                    || (left_ty == Type::Float && right_ty == Type::Float)
                {
                    Ok(Type::Bool)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Comparison operator requires matching Int or Float operands, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
            BinaryOp::Eq | BinaryOp::NotEq => {
                if left_ty.is_assignable_to(&right_ty) || right_ty.is_assignable_to(&left_ty) {
                    Ok(Type::Bool)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Cannot compare incompatible types {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                if left_ty == Type::Bool && right_ty == Type::Bool {
                    Ok(Type::Bool)
                } else {
                    Err(TypeError {
                        message: format!(
                            "Logical operator requires Bool operands, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
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
                            span,
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
                            span,
                        })
                    }
                }
            },
            BinaryOp::Range => {
                if left_ty == Type::Int && right_ty == Type::Int {
                    Ok(Type::List(Box::new(Type::Int)))
                } else {
                    Err(TypeError {
                        message: format!(
                            "Range '..' requires Int bounds, got {} and {}",
                            left_ty, right_ty
                        ),
                        span,
                    })
                }
            }
        }
    }

    pub(super) fn check_tuple_expr(&mut self, elements: &[Expr]) -> Result<Type, TypeError> {
        let mut types = Vec::new();
        for e in elements {
            types.push(self.check_expr(e)?);
        }
        Ok(Type::Tuple(types))
    }

    pub(super) fn check_try_expr(&mut self, inner: &Expr, span: Span) -> Result<Type, TypeError> {
        let inner_ty = self.check_expr(inner)?;
        let (successes, errors) = self.split_success_and_errors(&inner_ty);
        if errors.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Operator '?' is only allowed on types containing error variants, got '{}'",
                    inner_ty
                ),
                span,
            });
        }
        if successes.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Operator '?' requires at least one success variant, got '{}'",
                    inner_ty
                ),
                span,
            });
        }

        if let Some(ref current_ret) = self.current_return_type {
            for err in &errors {
                if !err.is_assignable_to(current_ret) {
                    return Err(TypeError {
                        message: format!(
                            "Error '{}' propagated by '?' is not assignable to function return type '{}'",
                            err, current_ret
                        ),
                        span,
                    });
                }
            }
        } else {
            return Err(TypeError {
                message: "Operator '?' cannot be used outside of a function".to_string(),
                span,
            });
        }

        Ok(Type::union(successes))
    }
}

