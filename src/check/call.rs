use super::*;

impl TypeChecker {
    pub(super) fn optional_of(inner: Type) -> Type {
        match inner {
            Type::Optional(_) => inner,
            Type::Union(ref vars) if vars.contains(&Type::Nil) => {
                let non_nil: Vec<Type> = vars.iter().filter(|t| *t != &Type::Nil).cloned().collect();
                if non_nil.len() == 1 {
                    Type::Optional(Box::new(non_nil.into_iter().next().unwrap()))
                } else {
                    inner
                }
            }
            other => Type::Optional(Box::new(other)),
        }
    }

    pub(super) fn check_method_call(
        &mut self,
        target_ty: &Type,
        field: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<Type, TypeError> {
        if target_ty == &Type::String {
            if let Some(res) = self.try_check_string_method(field, args) {
                return res;
            }
        } else if target_ty == &Type::Int {
            match field {
                "to_string" if args.is_empty() => return Ok(Type::String),
                "to_float" if args.is_empty() => return Ok(Type::Float),
                _ => {}
            }
        } else if target_ty == &Type::Bool {
            if field == "to_string" && args.is_empty() {
                return Ok(Type::String);
            }
        } else if target_ty == &Type::Float {
            match field {
                "floor" | "ceil" | "round" | "trunc" if args.is_empty() => return Ok(Type::Int),
                "to_string" if args.is_empty() => return Ok(Type::String),
                "is_nan" | "is_infinite" | "is_finite" if args.is_empty() => {
                    return Ok(Type::Bool);
                }
                _ => {}
            }
        } else if let Type::List(inner) = target_ty {
            if let Some(res) = self.try_check_list_method(target_ty, inner, field, args, span) {
                return res;
            }
        } else if let Type::Map(key_ty, val_ty) = target_ty {
            if let Some(res) = self.try_check_map_method(key_ty, val_ty, field, args) {
                return res;
            }
        } else if let Type::Custom(type_id) = target_ty
            && let Some(res) = self.try_check_native_method(type_id, field, args, span)
        {
            return res;
        }

        Err(TypeError {
            message: format!(
                "Unknown method or invalid arguments for '{}.{}()'",
                target_ty, field
            ),
            span,
        })
    }

    fn try_check_string_method(
        &mut self,
        field: &str,
        args: &[Expr],
    ) -> Option<Result<Type, TypeError>> {
        match field {
            "len" | "char_count" if args.is_empty() => Some(Ok(Type::Int)),
            "trim" | "lower" | "upper" | "to_string" if args.is_empty() => Some(Ok(Type::String)),
            "to_int" if args.is_empty() => Some(Ok(Type::union(vec![
                Type::Int,
                Type::Custom(TypeId::builtin("ParseError")),
            ]))),
            "to_float" if args.is_empty() => Some(Ok(Type::union(vec![
                Type::Float,
                Type::Custom(TypeId::builtin("ParseError")),
            ]))),
            "contains" | "starts_with" | "ends_with" if args.len() == 1 => {
                let arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty == Type::String {
                    return Some(Ok(Type::Bool));
                }
                Some(Err(TypeError {
                    message: format!(
                        "String method '{}()' expects String argument, got {}",
                        field, arg_ty
                    ),
                    span: args[0].span,
                }))
            }
            "index_of" if args.len() == 1 => {
                let arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty == Type::String {
                    return Some(Ok(Self::optional_of(Type::Int)));
                }
                Some(Err(TypeError {
                    message: format!(
                        "String method 'index_of()' expects String argument, got {}",
                        arg_ty
                    ),
                    span: args[0].span,
                }))
            }
            "split" if args.len() == 1 => {
                let arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty == Type::String {
                    return Some(Ok(Type::List(Box::new(Type::String))));
                }
                Some(Err(TypeError {
                    message: format!(
                        "String method 'split()' expects String argument, got {}",
                        arg_ty
                    ),
                    span: args[0].span,
                }))
            }
            "slice" if args.len() == 2 => {
                let start_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if start_ty != Type::Int {
                    return Some(Err(TypeError {
                        message: format!(
                            "String method 'slice()' expects Int for first argument, got {}",
                            start_ty
                        ),
                        span: args[0].span,
                    }));
                }
                let end_ty = match self.check_expr(&args[1]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if end_ty != Type::Int {
                    return Some(Err(TypeError {
                        message: format!(
                            "String method 'slice()' expects Int for second argument, got {}",
                            end_ty
                        ),
                        span: args[1].span,
                    }));
                }
                Some(Ok(Type::String))
            }
            "replace" if args.len() == 2 => {
                let from_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if from_ty != Type::String {
                    return Some(Err(TypeError {
                        message: format!(
                            "String method 'replace()' expects String for first argument, got {}",
                            from_ty
                        ),
                        span: args[0].span,
                    }));
                }
                let to_ty = match self.check_expr(&args[1]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if to_ty != Type::String {
                    return Some(Err(TypeError {
                        message: format!(
                            "String method 'replace()' expects String for second argument, got {}",
                            to_ty
                        ),
                        span: args[1].span,
                    }));
                }
                Some(Ok(Type::String))
            }
            _ => None,
        }
    }

    fn try_check_list_method(
        &mut self,
        target_ty: &Type,
        inner: &Type,
        field: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        match field {
            "len" if args.is_empty() => Some(Ok(Type::Int)),
            "reverse" if args.is_empty() => Some(Ok(Type::List(Box::new(inner.clone())))),
            "enumerate" if args.is_empty() => Some(Ok(Type::List(Box::new(Type::Tuple(vec![
                Type::Int,
                inner.clone(),
            ]))))),
            "slice" if args.len() == 2 => {
                let start_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if start_ty != Type::Int {
                    return Some(Err(TypeError {
                        message: format!(
                            "List method 'slice()' expects Int for first argument, got {}",
                            start_ty
                        ),
                        span: args[0].span,
                    }));
                }
                let end_ty = match self.check_expr(&args[1]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if end_ty != Type::Int {
                    return Some(Err(TypeError {
                        message: format!(
                            "List method 'slice()' expects Int for second argument, got {}",
                            end_ty
                        ),
                        span: args[1].span,
                    }));
                }
                Some(Ok(Type::List(Box::new(inner.clone()))))
            }
            "join" if args.len() == 1 => {
                if *inner != Type::String && *inner != Type::Any {
                    return Some(Err(TypeError {
                        message: format!(
                            "List method 'join()' is only supported on '[String]', got '{}'",
                            target_ty
                        ),
                        span,
                    }));
                }
                let sep_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if sep_ty != Type::String {
                    return Some(Err(TypeError {
                        message: format!(
                            "List method 'join()' expects String argument, got {}",
                            sep_ty
                        ),
                        span: args[0].span,
                    }));
                }
                Some(Ok(Type::String))
            }
            "get" if args.len() == 1 => {
                let arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty == Type::Int {
                    return Some(Ok(Self::optional_of(inner.clone())));
                }
                Some(Err(TypeError {
                    message: format!("List method 'get()' expects Int argument, got {}", arg_ty),
                    span: args[0].span,
                }))
            }
            "append" | "contains" if args.len() == 1 => {
                let arg_ty = match self.check_expr_with_expected(&args[0], inner) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty.is_assignable_to(inner) {
                    return Some(Ok(if field == "append" {
                        Type::Void
                    } else {
                        Type::Bool
                    }));
                }
                None
            }
            _ => None,
        }
    }

    fn try_check_map_method(
        &mut self,
        key_ty: &Type,
        val_ty: &Type,
        field: &str,
        args: &[Expr],
    ) -> Option<Result<Type, TypeError>> {
        match field {
            "len" if args.is_empty() => Some(Ok(Type::Int)),
            "keys" if args.is_empty() => Some(Ok(Type::List(Box::new(key_ty.clone())))),
            "values" if args.is_empty() => Some(Ok(Type::List(Box::new(val_ty.clone())))),
            "entries" if args.is_empty() => Some(Ok(Type::List(Box::new(Type::Tuple(vec![
                key_ty.clone(),
                val_ty.clone(),
            ]))))),
            "get" | "remove" | "contains_key" if args.len() == 1 => {
                let arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if arg_ty.is_assignable_to(key_ty) {
                    return Some(Ok(if field == "contains_key" {
                        Type::Bool
                    } else {
                        Self::optional_of(val_ty.clone())
                    }));
                }
                Some(Err(TypeError {
                    message: format!(
                        "Map method '{}()' expects key of type {}, got {}",
                        field, key_ty, arg_ty
                    ),
                    span: args[0].span,
                }))
            }
            "set" if args.len() == 2 => {
                let k_arg_ty = match self.check_expr(&args[0]) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if !k_arg_ty.is_assignable_to(key_ty) {
                    return Some(Err(TypeError {
                        message: format!(
                            "Map method 'set()' expects key of type {}, got {}",
                            key_ty, k_arg_ty
                        ),
                        span: args[0].span,
                    }));
                }
                let v_arg_ty = match self.check_expr_with_expected(&args[1], val_ty) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                if !v_arg_ty.is_assignable_to(val_ty) {
                    return Some(Err(TypeError {
                        message: format!(
                            "Map method 'set()' expects value of type {}, got {}",
                            val_ty, v_arg_ty
                        ),
                        span: args[1].span,
                    }));
                }
                Some(Ok(Type::Void))
            }
            _ => None,
        }
    }

    fn try_check_native_method(
        &mut self,
        type_id: &TypeId,
        field: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        let method_def = if let ModuleId::Native(ref nat_name) = type_id.module {
            self.native_registry
                .get_module(nat_name)
                .and_then(|m| m.methods.get(&(type_id.name.as_str(), field)))
                .cloned()
        } else {
            None
        }?;

        if args.len() != method_def.params.len() {
            return Some(Err(TypeError {
                message: format!(
                    "Method '{}.{}()' expects {} arguments, got {}",
                    type_id,
                    field,
                    method_def.params.len(),
                    args.len()
                ),
                span,
            }));
        }
        for (i, (arg_expr, (_, expected_ty))) in
            args.iter().zip(method_def.params.iter()).enumerate()
        {
            let actual_ty = match self.check_expr(arg_expr) {
                Ok(t) => t,
                Err(e) => return Some(Err(e)),
            };
            if !actual_ty.is_assignable_to(expected_ty) {
                return Some(Err(TypeError {
                    message: format!(
                        "Argument {} of method '{}.{}()' expects {}, got {}",
                        i + 1,
                        type_id,
                        field,
                        expected_ty,
                        actual_ty
                    ),
                    span: arg_expr.span,
                }));
            }
        }
        Some(Ok(method_def.return_type.clone()))
    }

    pub(super) fn resolve_module_member(
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
                    if !f.type_params.is_empty() {
                        return Err(TypeError {
                            message: format!("Generic function '{}.{}' cannot be used as a first-class value", alias, field),
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

    pub(super) fn check_struct_construction(
        &mut self,
        type_id: TypeId,
        struct_sig: &StructSignature,
        display_name: &str,
        call: CallArgs<'_>,
        callee_span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if !args.is_empty() {
            return Err(TypeError {
                message: format!("Type '{}' must be constructed using named arguments", display_name),
                span: callee_span,
            });
        }

        let mut bindings: HashMap<String, Type> = HashMap::new();
        let mut resolved_type_args = Vec::new();
        if struct_sig.type_params.is_empty() {
            if !type_args.is_empty() {
                return Err(TypeError {
                    message: format!("Type '{}' is not generic", display_name),
                    span: callee_span,
                });
            }
        } else if !type_args.is_empty() {
            if type_args.len() != struct_sig.type_params.len() {
                return Err(TypeError {
                    message: format!(
                        "Type '{}' expects {} type argument(s), got {}",
                        display_name,
                        struct_sig.type_params.len(),
                        type_args.len()
                    ),
                    span: callee_span,
                });
            }
            for (tp, ta_expr) in struct_sig.type_params.iter().zip(type_args.iter()) {
                let ta = self.resolve_type_expr(ta_expr, callee_span)?;
                self.check_generic_arity_in_type(&ta, callee_span)?;
                bindings.insert(tp.clone(), ta.clone());
                resolved_type_args.push(ta);
            }
        }

        let mut provided_fields = HashSet::new();
        let mut evaluated_args = Vec::with_capacity(named_args.len());
        for arg in named_args {
            if !provided_fields.insert(arg.name.clone()) {
                return Err(TypeError {
                    message: format!(
                        "Duplicate field '{}' in construction of '{}'",
                        arg.name, display_name
                    ),
                    span: callee_span,
                });
            }
            let expected_field_ty = struct_sig.fields.get(&arg.name).ok_or_else(|| TypeError {
                message: format!("Type '{}' has no field '{}'", display_name, arg.name),
                span: callee_span,
            })?;
            let expected_for_check = if struct_sig.type_params.is_empty() {
                expected_field_ty.clone()
            } else {
                Self::substitute_type(expected_field_ty, &bindings, arg.value.span)?
            };
            let actual_arg_ty = if type_args.is_empty()
                && Self::contains_type_param(&expected_for_check)
            {
                self.check_expr(&arg.value)?
            } else {
                self.check_expr_with_expected(&arg.value, &expected_for_check)?
            };
            if !struct_sig.type_params.is_empty() && type_args.is_empty() {
                Self::unify_type(expected_field_ty, &actual_arg_ty, &mut bindings, arg.value.span)?;
            }
            evaluated_args.push((arg.name.clone(), expected_field_ty.clone(), actual_arg_ty, arg.value.span));
        }

        for required_field in struct_sig.fields.keys() {
            if !provided_fields.contains(required_field) {
                return Err(TypeError {
                    message: format!(
                        "Missing required field '{}' in construction of '{}'",
                        required_field, display_name
                    ),
                    span: callee_span,
                });
            }
        }

        if !struct_sig.type_params.is_empty() && type_args.is_empty() {
            for tp in &struct_sig.type_params {
                let bound = bindings.get(tp).cloned().ok_or_else(|| TypeError {
                    message: format!(
                        "Cannot infer type parameter '{}' for '{}'; specify type arguments explicitly with '<...>'",
                        tp, display_name
                    ),
                    span: callee_span,
                })?;
                resolved_type_args.push(bound);
            }
        }

        for (field_name, expected_raw, actual_ty, arg_span) in evaluated_args {
            let expected_ty = if struct_sig.type_params.is_empty() {
                expected_raw
            } else {
                Self::substitute_type(&expected_raw, &bindings, arg_span)?
            };
            if !actual_ty.is_assignable_to(&expected_ty) {
                return Err(TypeError {
                    message: format!(
                        "Field '{}' of type '{}' expects {}, got {}",
                        field_name, display_name, expected_ty, actual_ty
                    ),
                    span: arg_span,
                });
            }
        }

        if struct_sig.type_params.is_empty() {
            Ok(Type::Custom(type_id))
        } else {
            Ok(Type::Generic {
                type_id,
                type_args: resolved_type_args,
            })
        }
    }

    pub(super) fn check_func_call_with_sig(
        &mut self,
        fn_sig: &FuncSignature,
        display_name: &str,
        call: CallArgs<'_>,
        call_span: Span,
        callee_span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if !named_args.is_empty() {
            return Err(TypeError {
                message: "Function calls do not support named arguments".to_string(),
                span: callee_span,
            });
        }
        if args.len() != fn_sig.params.len() {
            return Err(TypeError {
                message: format!(
                    "Function '{}' expects {} arguments, got {}",
                    display_name,
                    fn_sig.params.len(),
                    args.len()
                ),
                span: call_span,
            });
        }

        let mut bindings: HashMap<String, Type> = HashMap::new();
        if fn_sig.type_params.is_empty() {
            if !type_args.is_empty() {
                return Err(TypeError {
                    message: format!("Function '{}' is not generic", display_name),
                    span: callee_span,
                });
            }
        } else if !type_args.is_empty() {
            if type_args.len() != fn_sig.type_params.len() {
                return Err(TypeError {
                    message: format!(
                        "Function '{}' expects {} type argument(s), got {}",
                        display_name,
                        fn_sig.type_params.len(),
                        type_args.len()
                    ),
                    span: callee_span,
                });
            }
            for (tp, ta_expr) in fn_sig.type_params.iter().zip(type_args.iter()) {
                let ta = self.resolve_type_expr(ta_expr, callee_span)?;
                self.check_generic_arity_in_type(&ta, callee_span)?;
                bindings.insert(tp.clone(), ta);
            }
        }

        let mut actual_arg_tys = Vec::with_capacity(args.len());
        for (arg_expr, (_, param_ty)) in args.iter().zip(fn_sig.params.iter()) {
            let expected_for_check = Self::substitute_type(param_ty, &bindings, arg_expr.span)?;
            let actual_ty = if type_args.is_empty()
                && Self::contains_type_param(&expected_for_check)
            {
                self.check_expr(arg_expr)?
            } else {
                self.check_expr_with_expected(arg_expr, &expected_for_check)?
            };
            if !fn_sig.type_params.is_empty() && type_args.is_empty() {
                Self::unify_type(param_ty, &actual_ty, &mut bindings, arg_expr.span)?;
            }
            actual_arg_tys.push((actual_ty, arg_expr.span));
        }

        if !fn_sig.type_params.is_empty() {
            for tp in &fn_sig.type_params {
                if !bindings.contains_key(tp) {
                    return Err(TypeError {
                        message: format!(
                            "Cannot infer type parameter '{}' in call to '{}'; specify type arguments explicitly with '<...>'",
                            tp, display_name
                        ),
                        span: callee_span,
                    });
                }
            }
        }

        for (i, ((actual_ty, arg_span), (_, expected_raw))) in
            actual_arg_tys.iter().zip(fn_sig.params.iter()).enumerate()
        {
            let expected_ty = if fn_sig.type_params.is_empty() {
                expected_raw.clone()
            } else {
                Self::substitute_type(expected_raw, &bindings, *arg_span)?
            };
            if !actual_ty.is_assignable_to(&expected_ty) {
                return Err(TypeError {
                    message: format!(
                        "Argument {} expects {}, got {}",
                        i + 1,
                        expected_ty,
                        actual_ty
                    ),
                    span: *arg_span,
                });
            }
        }

        if fn_sig.type_params.is_empty() {
            Ok(fn_sig.return_type.clone())
        } else {
            Self::substitute_type(&fn_sig.return_type, &bindings, call_span)
        }
    }

}
