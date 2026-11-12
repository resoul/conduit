use super::*;

impl Interpreter {
    pub fn execute_validation_check(&mut self, val: Value, span: Span) -> Result<Value, RuntimeError> {
        let mut violations = Vec::new();
        self.validate_value_recursive(&val, "", &mut violations, 0, span)?;
        if violations.is_empty() {
            Ok(val)
        } else {
            Ok(crate::native::validation::make_validation_error(violations))
        }
    }

    fn find_struct_fields(&self, type_id: &TypeId) -> Option<(Option<PathBuf>, Vec<crate::ast::FieldDef>)> {
        let decl_mod = match &type_id.module {
            ModuleId::Program => None,
            ModuleId::Local(path) => Some(path.clone()),
            ModuleId::Builtin | ModuleId::Native(_) => return None,
        };
        let key = (decl_mod.clone(), type_id.name.clone());
        self.module_struct_fields
            .get(&key)
            .map(|fields| (decl_mod, fields.clone()))
    }

    fn validate_value_recursive(
        &mut self,
        val: &Value,
        path: &str,
        violations: &mut Vec<Value>,
        depth: usize,
        span: Span,
    ) -> Result<(), RuntimeError> {
        if depth > 256 {
            return Ok(());
        }

        match val {
            Value::Struct { schema, .. } => {
                if let Some((decl_mod, field_defs)) = self.find_struct_fields(&schema.type_id) {
                    for f in &field_defs {
                        let field_path = if path.is_empty() {
                            f.name.clone()
                        } else {
                            format!("{}.{}", path, f.name)
                        };

                        if let Some(f_val) = val.field(&f.name) {
                            self.validate_value_recursive(f_val, &field_path, violations, depth + 1, span)?;

                            if let Value::Nil = f_val {
                                continue;
                            }

                            for ann in &f.annotations {
                                let mut eval_args = vec![f_val.clone()];
                                for arg_expr in &ann.args {
                                    let arg_val = self.eval_expr(arg_expr).map_err(|f| match f {
                                        InterpFlow::Error(e) => e,
                                        _ => RuntimeError {
                                            message: "Unexpected flow in annotation argument".to_string(),
                                            span,
                                        },
                                    })?;
                                    eval_args.push(arg_val);
                                }

                                let res = self.invoke_validator(&decl_mod, &ann.target, eval_args, span)?;
                                match res {
                                    Value::Void | Value::Nil => {}
                                    Value::Error { .. } => {
                                        if let (Some(Value::String(code)), Some(Value::String(msg))) =
                                            (res.field("code"), res.field("message"))
                                        {
                                            let violation = crate::native::validation::make_violation(
                                                &field_path,
                                                code,
                                                msg,
                                            );
                                            violations.push(violation);
                                        } else {
                                            return Err(RuntimeError {
                                                message: format!("Validator '@{}' returned an unexpected error format", ann.target),
                                                span,
                                            });
                                        }
                                    }
                                    other => {
                                        return Err(RuntimeError {
                                            message: format!(
                                                "Validator '@{}' must return Void or Issue, got '{:?}'",
                                                ann.target, other
                                            ),
                                            span,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Value::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    let item_path = format!("{}[{}]", path, i);
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            Value::Map(map) => {
                for (key, item) in map.entries() {
                    let item_path = format!("{}{}", path, key.format_path_segment());
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            Value::Tuple(items) => {
                for (i, item) in items.iter().enumerate() {
                    let item_path = if path.is_empty() {
                        format!("{}", i)
                    } else {
                        format!("{}.{}", path, i)
                    };
                    self.validate_value_recursive(item, &item_path, violations, depth + 1, span)?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn invoke_validator(
        &mut self,
        decl_mod: &Option<PathBuf>,
        target: &str,
        eval_args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        if let Some((alias, func_name)) = target.split_once('.') {
            let mod_imports = self.module_imports.get(decl_mod).cloned()
                .or_else(|| self.module_imports.get(&self.current_module).cloned());

            if let Some(target_mod) = mod_imports.and_then(|m| m.get(alias).cloned()) {
                match target_mod {
                    ModuleTarget::Native(nat_name) => {
                        let func_impl = {
                            let m = self.native_registry.get_module(&nat_name).ok_or_else(|| RuntimeError {
                                message: format!("Unknown native module '{}'", nat_name),
                                span,
                            })?;
                            m.functions.get(func_name).map(|f| f.implementation).ok_or_else(|| RuntimeError {
                                message: format!("Native module '{}' has no validator '{}'", alias, func_name),
                                span,
                            })?
                        };
                        match (func_impl)(eval_args) {
                            Ok(v) => Ok(v),
                            Err(e) => Ok(e),
                        }
                    }
                    ModuleTarget::Local(path) => {
                        self.call_module_function(Some(path), func_name, eval_args, span)
                    }
                }
            } else {
                Err(RuntimeError {
                    message: format!("Unknown module alias '{}' in validator '@{}'", alias, target),
                    span,
                })
            }
        } else {
            self.call_module_function(decl_mod.clone().or_else(|| self.current_module.clone()), target, eval_args, span)
        }
    }

}
