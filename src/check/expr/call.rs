use super::super::*;

impl TypeChecker {
    pub(super) fn check_call_expr(
        &mut self,
        callee: &Expr,
        call: CallArgs<'_>,
        span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if let ExprKind::Ident(name) = &callee.kind
            && let Some(res) =
                self.try_check_local_var_call(name, callee.span, type_args, args, named_args, span)
        {
            return res;
        }

        if let ExprKind::Ident(ref type_name) = callee.kind
            && let Some(res) =
                self.try_check_named_call(type_name, callee.span, call, span)
            {
                return res;
            }

        if let ExprKind::Member { target, field } = &callee.kind {
            return self.check_member_call_expr(target, field, callee.span, call, span);
        }

        if let ExprKind::OptionalMember { target, field } = &callee.kind {
            return self.check_optional_member_call_expr(
                target,
                field,
                callee.span,
                type_args,
                args,
            );
        }

        if !type_args.is_empty() {
            return Err(TypeError {
                message:
                    "Type arguments can only be applied to named generic functions or types"
                        .to_string(),
                span: callee.span,
            });
        }

        self.check_indirect_call_expr(callee, args, named_args, span)
    }

    fn try_check_local_var_call(
        &mut self,
        name: &str,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        let is_constructor = name == "Map"
            || self.structs.contains_key(name)
            || self.errors.contains(name)
            || Self::builtin_error_sig(name).is_some();
        let is_builtin = matches!(name, "map" | "filter" | "print");
        if is_constructor || is_builtin || !type_args.is_empty() || !named_args.is_empty() {
            return None;
        }
        let callee_ty = self.lookup_variable(name)?;
        if let Type::Function {
            params,
            return_type,
        } = callee_ty
        {
            if args.len() != params.len() {
                return Some(Err(TypeError {
                    message: format!(
                        "Function expects {} arguments, got {}",
                        params.len(),
                        args.len()
                    ),
                    span,
                }));
            }
            for (i, (arg, expected)) in args.iter().zip(params.iter()).enumerate() {
                let actual = match self.check_expr_with_expected(arg, expected) {
                    Ok(ty) => ty,
                    Err(e) => return Some(Err(e)),
                };
                if !actual.is_assignable_to(expected) {
                    return Some(Err(TypeError {
                        message: format!(
                            "Argument {} expects {}, got {}",
                            i + 1,
                            expected,
                            actual
                        ),
                        span: arg.span,
                    }));
                }
            }
            Some(Ok(*return_type))
        } else {
            Some(Err(TypeError {
                message: format!("Cannot call non-function expression of type '{}'", callee_ty),
                span: callee_span,
            }))
        }
    }

    fn try_check_named_call(
        &mut self,
        type_name: &str,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if type_name == "Map" {
            return Some(self.check_map_constructor_call(
                callee_span,
                type_args,
                args,
                named_args,
                span,
            ));
        }

        let direct_struct = self
            .structs
            .get(type_name)
            .cloned()
            .map(|sig| (self.current_type_id(type_name), sig))
            .or_else(|| Self::builtin_error_sig(type_name));

        if let Some((type_id, struct_sig)) = direct_struct {
            return Some(self.check_struct_construction(
                type_id,
                &struct_sig,
                type_name,
                call,
                callee_span,
            ));
        }

        if type_name == "map" {
            return Some(self.check_map_builtin_call(callee_span, type_args, args, span));
        }

        if type_name == "filter" {
            return Some(self.check_filter_builtin_call(callee_span, type_args, args, span));
        }

        if let Some(fn_sig) = self.functions.get(type_name).cloned() {
            return Some(self.check_func_call_with_sig(
                &fn_sig,
                type_name,
                call,
                span,
                callee_span,
            ));
        }

        None
    }

    fn check_map_constructor_call(
        &self,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Result<Type, TypeError> {
        if type_args.len() != 2 {
            return Err(TypeError {
                message: format!(
                    "Map constructor expects 2 type arguments (K, V), got {}",
                    type_args.len()
                ),
                span: callee_span,
            });
        }
        if !args.is_empty() || !named_args.is_empty() {
            return Err(TypeError {
                message:
                    "Map<K, V>() takes no arguments; use Map.from([...]) to initialize entries"
                        .to_string(),
                span,
            });
        }
        let key_ty = self.resolve_type_expr(&type_args[0], callee_span)?;
        Self::validate_map_key_type(&key_ty, callee_span, true)?;
        let val_ty = self.resolve_type_expr(&type_args[1], callee_span)?;
        self.check_generic_arity_in_type(&val_ty, callee_span)?;
        Ok(Type::Map(Box::new(key_ty), Box::new(val_ty)))
    }

    fn check_map_builtin_call(
        &mut self,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        span: Span,
    ) -> Result<Type, TypeError> {
        if !type_args.is_empty() {
            return Err(TypeError {
                message: "'map' does not accept explicit type arguments".to_string(),
                span: callee_span,
            });
        }
        if args.len() != 2 {
            return Err(TypeError {
                message: format!(
                    "'map' takes 2 arguments (list, callback), got {}",
                    args.len()
                ),
                span,
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
        if let Type::Function {
            params,
            return_type,
        } = callback_ty
        {
            if params.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "Function passed to 'map' must take 1 argument, but takes {}",
                        params.len()
                    ),
                    span: args[1].span,
                });
            }
            if !item_ty.is_assignable_to(&params[0]) {
                return Err(TypeError {
                    message: format!(
                        "List element type '{}' is not assignable to parameter type '{}'",
                        item_ty, params[0]
                    ),
                    span: args[1].span,
                });
            }
            Ok(Type::List(Box::new(*return_type)))
        } else {
            Err(TypeError {
                message: "Second argument to 'map' must be a callable function".to_string(),
                span: args[1].span,
            })
        }
    }

    fn check_filter_builtin_call(
        &mut self,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        span: Span,
    ) -> Result<Type, TypeError> {
        if !type_args.is_empty() {
            return Err(TypeError {
                message: "'filter' does not accept explicit type arguments".to_string(),
                span: callee_span,
            });
        }
        if args.len() != 2 {
            return Err(TypeError {
                message: format!(
                    "'filter' takes 2 arguments (list, callback), got {}",
                    args.len()
                ),
                span,
            });
        }
        let list_ty = self.check_expr(&args[0])?;
        let item_ty = match list_ty {
            Type::List(inner) => *inner,
            other => {
                return Err(TypeError {
                    message: format!(
                        "First argument to 'filter' must be a List, got '{}'",
                        other
                    ),
                    span: args[0].span,
                });
            }
        };
        let callback_ty = self.check_expr(&args[1])?;
        if let Type::Function {
            params,
            return_type,
        } = callback_ty
        {
            if params.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "Function passed to 'filter' must take 1 argument, but takes {}",
                        params.len()
                    ),
                    span: args[1].span,
                });
            }
            if !item_ty.is_assignable_to(&params[0]) {
                return Err(TypeError {
                    message: format!(
                        "List element type '{}' is not assignable to parameter type '{}'",
                        item_ty, params[0]
                    ),
                    span: args[1].span,
                });
            }
            if *return_type != Type::Bool {
                return Err(TypeError {
                    message: format!(
                        "Function passed to 'filter' must return 'Bool', got '{}'",
                        return_type
                    ),
                    span: args[1].span,
                });
            }
            Ok(Type::List(Box::new(item_ty)))
        } else {
            Err(TypeError {
                message: "Second argument to 'filter' must be a callable function".to_string(),
                span: args[1].span,
            })
        }
    }

    fn check_member_call_expr(
        &mut self,
        target: &Expr,
        field: &str,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if let ExprKind::Ident(ref alias) = target.kind {
            if alias == "Map" && field == "from" && !self.imports.contains_key("Map") {
                return self.check_map_from_call(callee_span, type_args, args, named_args, span);
            }

            if let Some(mod_target) = self.imports.get(alias).cloned() {
                match mod_target {
                    ModuleTarget::Native(nat_name) => {
                        if let Some(res) = self.try_check_native_module_call(
                            alias,
                            &nat_name,
                            field,
                            callee_span,
                            call,
                            span,
                        ) {
                            return res;
                        }
                    }
                    ModuleTarget::Local(path) => {
                        if let Some(res) = self.try_check_local_module_call(
                            alias,
                            &path,
                            field,
                            callee_span,
                            call,
                            span,
                        ) {
                            return res;
                        }
                    }
                }
            }
        }

        if !type_args.is_empty() {
            return Err(TypeError {
                message: "Method calls do not support explicit type arguments".to_string(),
                span: callee_span,
            });
        }
        let target_ty = self.check_expr(target)?;
        let ret_ty = self.check_method_call(&target_ty, field, args, callee_span)?;
        let is_mutating_method = (matches!(target_ty, Type::List(_)) && field == "append")
            || (matches!(target_ty, Type::Map(_, _)) && (field == "set" || field == "remove"));
        if is_mutating_method {
            self.check_lvalue_root_mutability(target, Some(field))?;
        }
        Ok(ret_ty)
    }

    fn check_map_from_call(
        &mut self,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Result<Type, TypeError> {
        if !named_args.is_empty() {
            return Err(TypeError {
                message: "Map.from() does not accept named arguments".to_string(),
                span: callee_span,
            });
        }
        if args.len() != 1 {
            return Err(TypeError {
                message: format!("Map.from() expects 1 argument, got {}", args.len()),
                span,
            });
        }
        if !type_args.is_empty() {
            if type_args.len() != 2 {
                return Err(TypeError {
                    message: format!(
                        "Map.from<> expects 2 type arguments, got {}",
                        type_args.len()
                    ),
                    span: callee_span,
                });
            }
            let key_ty = self.resolve_type_expr(&type_args[0], callee_span)?;
            Self::validate_map_key_type(&key_ty, callee_span, true)?;
            let val_ty = self.resolve_type_expr(&type_args[1], callee_span)?;
            self.check_generic_arity_in_type(&val_ty, callee_span)?;
            let expected_list = Type::List(Box::new(Type::Tuple(vec![
                key_ty.clone(),
                val_ty.clone(),
            ])));
            let arg_ty = self.check_expr_with_expected(&args[0], &expected_list)?;
            if !arg_ty.is_assignable_to(&expected_list) {
                return Err(TypeError {
                    message: format!(
                        "Map.from() expects '{}', got '{}'",
                        expected_list, arg_ty
                    ),
                    span: args[0].span,
                });
            }
            return Ok(Type::Map(Box::new(key_ty), Box::new(val_ty)));
        }
        let arg_ty = self.check_expr(&args[0])?;
        if let Type::List(elem_ty) = arg_ty {
            if let Type::Tuple(elems) = *elem_ty {
                if elems.len() == 2 {
                    Self::validate_map_key_type(&elems[0], args[0].span, true)?;
                    return Ok(Type::Map(
                        Box::new(elems[0].clone()),
                        Box::new(elems[1].clone()),
                    ));
                }
            } else if *elem_ty == Type::Any {
                return Err(TypeError {
                    message: "Cannot infer Map key/value types from empty list; use Map<K, V>() or Map.from<K, V>([])".to_string(),
                    span: args[0].span,
                });
            }
        }
        Err(TypeError {
            message: "Map.from() expects a list of (key, value) tuples".to_string(),
            span: args[0].span,
        })
    }

    fn check_optional_member_call_expr(
        &mut self,
        target: &Expr,
        field: &str,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<Type, TypeError> {
        if !type_args.is_empty() {
            return Err(TypeError {
                message: "Method calls do not support explicit type arguments".to_string(),
                span: callee_span,
            });
        }
        let target_ty = self.check_expr(target)?;
        match self.classify_optional_chain(&target_ty) {
            OptionalChainTarget::Optional(inner_ty) => {
                let ret_ty = self.check_method_call(&inner_ty, field, args, callee_span)?;
                if ret_ty == Type::Void {
                    return Err(TypeError {
                        message: format!(
                            "Cannot use '?.' with mutating or Void-returning method '{}()'",
                            field
                        ),
                        span: callee_span,
                    });
                }
                if self.contains_error(&ret_ty) {
                    return Err(TypeError {
                        message: format!(
                            "Cannot use '?.' with error-returning method '{}()' (returns '{}')",
                            field, ret_ty
                        ),
                        span: callee_span,
                    });
                }
                Ok(Self::optional_of(ret_ty))
            }
            OptionalChainTarget::Fallible(successes, errors) => {
                self.validate_propagated_errors(&errors, callee_span)?;
                let inner_ty = Type::union(successes);
                self.check_method_call(&inner_ty, field, args, callee_span)
            }
            OptionalChainTarget::Invalid => Err(TypeError {
                message: format!(
                    "Operator '?.' requires an optional type 'T?', got '{}'",
                    target_ty
                ),
                span: target.span,
            }),
        }
    }

    fn check_indirect_call_expr(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Result<Type, TypeError> {
        let callee_ty = self.check_expr(callee)?;
        if let Type::Function {
            params,
            return_type,
        } = callee_ty
        {
            if !named_args.is_empty() {
                return Err(TypeError {
                    message: "Function calls do not support named arguments".to_string(),
                    span: callee.span,
                });
            }
            if args.len() != params.len() {
                return Err(TypeError {
                    message: format!(
                        "Function expects {} arguments, got {}",
                        params.len(),
                        args.len()
                    ),
                    span,
                });
            }
            for (i, (arg_expr, expected_ty)) in args.iter().zip(params.iter()).enumerate() {
                let actual_ty = self.check_expr_with_expected(arg_expr, expected_ty)?;
                if !actual_ty.is_assignable_to(expected_ty) {
                    return Err(TypeError {
                        message: format!(
                            "Argument {} expects {}, got {}",
                            i + 1,
                            expected_ty,
                            actual_ty
                        ),
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
}
