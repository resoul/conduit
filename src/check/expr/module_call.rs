use super::super::*;

impl TypeChecker {
    pub(super) fn try_check_native_module_call(
        &mut self,
        alias: &str,
        nat_name: &str,
        field: &str,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        let (type_def, func_def) = {
            let m = match self.native_registry.get_module(nat_name) {
                Some(m) => m,
                None => {
                    return Some(Err(TypeError {
                        message: format!("Unknown native module '{}'", nat_name),
                        span: callee_span,
                    }));
                }
            };
            (
                m.types.get(field).cloned(),
                m.functions.get(field).cloned(),
            )
        };

        if let Some(ty_def) = type_def {
            return Some(self.check_native_type_construction(
                (alias, nat_name, field),
                &ty_def,
                callee_span,
                call,
                span,
            ));
        }

        if let Some(f) = func_def {
            return Some(self.check_native_func_call(
                (alias, nat_name, field),
                &f,
                callee_span,
                call,
                span,
            ));
        }

        None
    }

    fn check_native_type_construction(
        &mut self,
        (alias, nat_name, field): (&str, &str, &str),
        ty_def: &crate::native::NativeTypeDef,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            args, named_args, ..
        } = call;
        if ty_def.fields.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Native type '{}.{}' cannot be constructed directly",
                    alias, field
                ),
                span: callee_span,
            });
        }
        if !args.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Constructor for '{}.{}' does not accept positional arguments",
                    alias, field
                ),
                span,
            });
        }
        let mut provided_fields = HashSet::new();
        for na in named_args {
            if let Some((_, expected_ty)) = ty_def
                .fields
                .iter()
                .find(|(name, _)| *name == na.name.as_str())
            {
                let arg_ty = self.check_expr_with_expected(&na.value, expected_ty)?;
                if !arg_ty.is_assignable_to(expected_ty) {
                    return Err(TypeError {
                        message: format!(
                            "Type mismatch for field '{}' in construction of '{}.{}': expected {}, got {}",
                            na.name, alias, field, expected_ty, arg_ty
                        ),
                        span: na.value.span,
                    });
                }
                provided_fields.insert(na.name.clone());
            } else {
                return Err(TypeError {
                    message: format!(
                        "Unknown field '{}' in construction of '{}.{}'",
                        na.name, alias, field
                    ),
                    span: na.value.span,
                });
            }
        }
        for (required_field, _) in &ty_def.fields {
            if !provided_fields.contains(*required_field) {
                return Err(TypeError {
                    message: format!(
                        "Missing required field '{}' in construction of '{}.{}'",
                        required_field, alias, field
                    ),
                    span: callee_span,
                });
            }
        }
        Ok(Type::Custom(TypeId::native(nat_name.to_string(), field)))
    }

    fn check_native_func_call(
        &mut self,
        (alias, nat_name, field): (&str, &str, &str),
        f: &crate::native::NativeFuncDef,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Result<Type, TypeError> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if nat_name == "cdt.validation" && field == "check" {
            if !named_args.is_empty() {
                return Err(TypeError {
                    message: format!(
                        "Function '{}.{}' does not accept named arguments",
                        alias, field
                    ),
                    span: callee_span,
                });
            }
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "Function '{}.{}' expects 1 argument, got {}",
                        alias,
                        field,
                        args.len()
                    ),
                    span,
                });
            }
            let val_ty = self.check_expr(&args[0])?;
            let err_ty = Type::Custom(TypeId::native("cdt.validation", "ValidationError"));
            return Ok(Type::union(vec![val_ty, err_ty]));
        }

        if nat_name == "cdt.json" && field == "decode" {
            if !named_args.is_empty() {
                return Err(TypeError {
                    message: format!(
                        "Function '{}.{}' does not accept named arguments",
                        alias, field
                    ),
                    span: callee_span,
                });
            }
            if type_args.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "Function '{}.decode' expects 1 type argument <T>, got {}",
                        alias,
                        type_args.len()
                    ),
                    span: callee_span,
                });
            }
            if args.len() != 1 {
                return Err(TypeError {
                    message: format!(
                        "Function '{}.decode' expects 1 argument, got {}",
                        alias,
                        args.len()
                    ),
                    span,
                });
            }
            let arg_ty = self.check_expr(&args[0])?;
            if !arg_ty.is_assignable_to(&Type::String) {
                return Err(TypeError {
                    message: format!(
                        "Argument 1 of function '{}.decode' expects String, got {}",
                        alias, arg_ty
                    ),
                    span: args[0].span,
                });
            }
            let target_ty = self.resolve_type_expr(&type_args[0], callee_span)?;
            self.check_generic_arity_in_type(&target_ty, callee_span)?;
            self.check_json_decodable(&target_ty, callee_span, &mut HashSet::new())?;
            let err_ty = Type::Custom(TypeId::native("cdt.json", "DecodeError"));
            return Ok(Type::union(vec![target_ty, err_ty]));
        }

        if !type_args.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Function '{}.{}' does not accept type arguments",
                    alias, field
                ),
                span: callee_span,
            });
        }

        if !named_args.is_empty() {
            return Err(TypeError {
                message: format!(
                    "Function '{}.{}' does not accept named arguments",
                    alias, field
                ),
                span: callee_span,
            });
        }
        if args.len() != f.params.len() {
            return Err(TypeError {
                message: format!(
                    "Function '{}.{}' expects {} arguments, got {}",
                    alias,
                    field,
                    f.params.len(),
                    args.len()
                ),
                span,
            });
        }
        for (i, (arg_expr, (_, expected_ty))) in args.iter().zip(f.params.iter()).enumerate() {
            let actual_ty = self.check_expr(arg_expr)?;
            if !actual_ty.is_assignable_to(expected_ty) {
                return Err(TypeError {
                    message: format!(
                        "Argument {} of function '{}.{}' expects {}, got {}",
                        i + 1,
                        alias,
                        field,
                        expected_ty,
                        actual_ty
                    ),
                    span: arg_expr.span,
                });
            }
        }
        Ok(f.return_type.clone())
    }

    pub(super) fn try_check_local_module_call(
        &mut self,
        alias: &str,
        path: &PathBuf,
        field: &str,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Option<Result<Type, TypeError>> {
        let info = match self.module_info.get(path).cloned() {
            Some(info) => info,
            None => {
                return Some(Err(TypeError {
                    message: format!("Module '{}' not loaded", path.display()),
                    span: callee_span,
                }));
            }
        };

        if let Some(struct_sig) = info.structs.get(field) {
            if !struct_sig.is_exported {
                return Some(Err(TypeError {
                    message: format!("Type '{}' is private in module '{}'", field, alias),
                    span: callee_span,
                }));
            }
            let display = format!("{}.{}", alias, field);
            return Some(self.check_struct_construction(
                TypeId::local(path.clone(), field),
                struct_sig,
                &display,
                call,
                callee_span,
            ));
        }

        if let Some(fn_sig) = info.functions.get(field) {
            if !fn_sig.is_exported {
                return Some(Err(TypeError {
                    message: format!("Function '{}' is private in module '{}'", field, alias),
                    span: callee_span,
                }));
            }
            let display = format!("{}.{}", alias, field);
            return Some(self.check_func_call_with_sig(
                fn_sig,
                &display,
                call,
                span,
                callee_span,
            ));
        }

        None
    }

}
