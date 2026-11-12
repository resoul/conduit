use super::*;

impl TypeChecker {
    pub fn check_project(&mut self, root_path: &Path, loader: &ModuleLoader) -> Result<(), TypeError> {
        let mut module_paths: Vec<PathBuf> = loader.modules.keys().cloned().collect();
        module_paths.sort();

        self.collect_project_signatures(&module_paths, loader)?;

        let mut all_module_paths: Vec<PathBuf> = self.module_info.keys().cloned().collect();
        all_module_paths.sort();

        self.validate_project_generic_arity(&all_module_paths)?;
        self.check_project_privacy(&all_module_paths)?;
        self.check_project_bodies(&module_paths, loader)?;

        self.current_module_path = Some(root_path.to_path_buf());
        Ok(())
    }

    fn collect_project_signatures(
        &mut self,
        module_paths: &[PathBuf],
        loader: &ModuleLoader,
    ) -> Result<(), TypeError> {
        for path in module_paths {
            let loaded_mod = &loader.modules[path];
            let mod_id = ModuleId::Local(path.clone());
            let mut info = ModuleInfo::default();
            for (alias, (target, _)) in &loaded_mod.imports {
                info.imports.insert(alias.clone(), target.clone());
            }

            let mut local_types = HashSet::new();
            for item in &loaded_mod.program.items {
                if let ItemKind::Type { name, .. } | ItemKind::Error { name, .. } = &item.kind {
                    local_types.insert(name.clone());
                }
            }

            for item in &loaded_mod.program.items {
                match &item.kind {
                    ItemKind::Type { name, type_params, fields } => {
                        Self::validate_type_params(type_params, item.span)?;
                        let mut field_map = HashMap::new();
                        let mut ann_map = HashMap::new();
                        for f in fields {
                            let resolved_ty = self.resolve_type_expr_in_ctx(
                                &f.ty,
                                &mod_id,
                                &info.imports,
                                type_params,
                                Some(&local_types),
                                item.span,
                            )?;
                            field_map.insert(f.name.clone(), resolved_ty);
                            ann_map.insert(f.name.clone(), f.annotations.clone());
                        }
                        if info.structs.contains_key(name) || info.errors.contains(name) {
                            return Err(TypeError {
                                message: format!("Duplicate type definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.structs.insert(
                            name.clone(),
                            StructSignature {
                                type_params: type_params.clone(),
                                fields: field_map,
                                field_annotations: ann_map,
                                is_exported: item.is_exported,
                            },
                        );
                    }
                    ItemKind::Error { name, fields } => {
                        let mut field_map = HashMap::new();
                        let mut ann_map = HashMap::new();
                        for f in fields {
                            let resolved_ty = self.resolve_type_expr_in_ctx(
                                &f.ty,
                                &mod_id,
                                &info.imports,
                                &[],
                                Some(&local_types),
                                item.span,
                            )?;
                            field_map.insert(f.name.clone(), resolved_ty);
                            ann_map.insert(f.name.clone(), f.annotations.clone());
                        }
                        if info.structs.contains_key(name) || info.errors.contains(name) {
                            return Err(TypeError {
                                message: format!("Duplicate error definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.structs.insert(
                            name.clone(),
                            StructSignature {
                                type_params: Vec::new(),
                                fields: field_map,
                                field_annotations: ann_map,
                                is_exported: item.is_exported,
                            },
                        );
                        info.errors.insert(name.clone());
                    }
                    ItemKind::Func { name, type_params, params, return_type, is_validator, .. } => {
                        Self::validate_type_params(type_params, item.span)?;
                        let mut param_types = Vec::with_capacity(params.len());
                        for p in params {
                            let resolved_ty = self.resolve_type_expr_in_ctx(
                                &p.ty,
                                &mod_id,
                                &info.imports,
                                type_params,
                                Some(&local_types),
                                item.span,
                            )?;
                            param_types.push((p.name.clone(), resolved_ty));
                        }
                        let ret_type = match return_type.as_ref() {
                            Some(rt) => self.resolve_type_expr_in_ctx(
                                rt,
                                &mod_id,
                                &info.imports,
                                type_params,
                                Some(&local_types),
                                item.span,
                            )?,
                            None => Type::Void,
                        };

                        if info.functions.contains_key(name) {
                            return Err(TypeError {
                                message: format!("Duplicate function definition '{}'", name),
                                span: item.span,
                            });
                        }
                        if item.is_exported {
                            info.exports.insert(name.clone());
                        }
                        info.functions.insert(
                            name.clone(),
                            FuncSignature {
                                type_params: type_params.clone(),
                                params: param_types,
                                return_type: ret_type,
                                is_exported: item.is_exported,
                                is_validator: *is_validator,
                            },
                        );
                    }
                    ItemKind::Import { .. } => {}
                }
            }

            self.module_info.insert(path.clone(), info);
        }
        Ok(())
    }

    fn validate_project_generic_arity(
        &mut self,
        all_module_paths: &[PathBuf],
    ) -> Result<(), TypeError> {
        for path in all_module_paths {
            self.current_module_path = Some(path.clone());
            let info = self.module_info.get(path).cloned().unwrap();
            for sig in info.functions.values() {
                for (_, param_ty) in &sig.params {
                    self.check_generic_arity_in_type(param_ty, Span::new(1, 1))?;
                }
                self.check_generic_arity_in_type(&sig.return_type, Span::new(1, 1))?;
            }
            for sig in info.structs.values() {
                for field_ty in sig.fields.values() {
                    self.check_generic_arity_in_type(field_ty, Span::new(1, 1))?;
                }
            }
        }
        self.current_module_path = None;
        Ok(())
    }

    fn check_project_privacy(&self, all_module_paths: &[PathBuf]) -> Result<(), TypeError> {
        for path in all_module_paths {
            let info = self.module_info.get(path).cloned().unwrap();
            for (fn_name, sig) in &info.functions {
                if sig.is_exported {
                    for (param_name, param_ty) in &sig.params {
                        if let Some(leak) = self.contains_private_type(param_ty, Some(path)) {
                            return Err(TypeError {
                                message: format!(
                                    "Exported function '{}' exposes private type '{}' in parameter '{}'",
                                    fn_name, leak, param_name
                                ),
                                span: Span::new(1, 1),
                            });
                        }
                    }
                    if let Some(leak) = self.contains_private_type(&sig.return_type, Some(path)) {
                        return Err(TypeError {
                            message: format!(
                                "Exported function '{}' exposes private type '{}' in return type",
                                fn_name, leak
                            ),
                            span: Span::new(1, 1),
                        });
                    }
                }
            }

            for (type_name, sig) in &info.structs {
                if sig.is_exported {
                    for (field_name, field_ty) in &sig.fields {
                        if let Some(leak) = self.contains_private_type(field_ty, Some(path)) {
                            return Err(TypeError {
                                message: format!(
                                    "Exported type '{}' exposes private type '{}' in field '{}'",
                                    type_name, leak, field_name
                                ),
                                span: Span::new(1, 1),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_project_bodies(
        &mut self,
        module_paths: &[PathBuf],
        loader: &ModuleLoader,
    ) -> Result<(), TypeError> {
        for path in module_paths {
            let loaded_mod = &loader.modules[path];
            self.current_module_path = Some(path.clone());
            if let Some(info) = self.module_info.get(path) {
                self.functions = info.functions.clone();
                self.structs = info.structs.clone();
                self.errors = info.errors.clone();
                self.imports = info.imports.clone();
            }

            self.functions.insert(
                "print".to_string(),
                FuncSignature {
                    type_params: Vec::new(),
                    params: vec![("msg".to_string(), Type::Any)],
                    return_type: Type::Void,
                    is_exported: false,
                    is_validator: false,
                },
            );

            for item in &loaded_mod.program.items {
                if let ItemKind::Func { name, body, is_validator, params, .. } = &item.kind {
                    let sig = self.functions.get(name).unwrap().clone();
                    let ret_ty = sig.return_type.clone();

                    if *is_validator {
                        if params.is_empty() {
                            return Err(TypeError {
                                message: format!("Validator function '{}' must have at least one parameter (the value to validate)", name),
                                span: item.span,
                            });
                        }
                        let ret = sig.return_type.clone();
                        let has_void = match &ret {
                            Type::Void => true,
                            Type::Union(vars) => vars.contains(&Type::Void),
                            _ => false,
                        };
                        let issue_id = TypeId::native("cdt.validation", "Issue");
                        let has_issue = match &ret {
                            Type::Custom(id) => id == &issue_id,
                            Type::Union(vars) => vars.iter().any(|v| matches!(v, Type::Custom(id) if id == &issue_id)),
                            _ => false,
                        };
                        let is_valid_ret = match &ret {
                            Type::Union(vars) if vars.len() == 2 => has_void && has_issue,
                            _ => false,
                        };
                        if !is_valid_ret {
                            return Err(TypeError {
                                message: format!("Validator function '{}' must return 'Void | validation.Issue', got '{}'", name, ret),
                                span: item.span,
                            });
                        }
                    }

                    let can_fallthrough_void = match &ret_ty {
                        Type::Void => true,
                        Type::Union(vars) => vars.contains(&Type::Void),
                        _ => false,
                    };
                    if !can_fallthrough_void && !block_always_returns(body) {
                        return Err(TypeError {
                            message: format!(
                                "Function '{}' must return a value of type '{}' on all reachable paths",
                                name, ret_ty
                            ),
                            span: item.span,
                        });
                    }

                    self.current_return_type = Some(sig.return_type.clone());
                    self.current_type_params = sig.type_params.clone();

                    self.narrowed.clear();
                    self.widened_outer_vars.clear();
                    self.enter_scope();
                    for (param_name, param_ty) in sig.params {
                        self.define_variable(param_name, param_ty, false, item.span);
                    }

                    let body_res = self.check_block(body);
                    let exit_res = self.exit_scope();
                    self.current_type_params.clear();
                    self.current_return_type = None;
                    body_res?;
                    exit_res?;
                }

                if let ItemKind::Type { type_params, fields, .. } = &item.kind {
                    self.current_type_params = type_params.clone();
                    for f in fields {
                        for ann in &f.annotations {
                            if let Err(e) = self.check_field_annotation(&f.name, f, ann) {
                                self.current_type_params.clear();
                                return Err(e);
                            }
                        }
                    }
                    self.current_type_params.clear();
                }
            }
        }
        Ok(())
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            if let ItemKind::Import { path, alias } = &item.kind {
                if path.starts_with("cdt.") {
                    self.imports.insert(alias.clone(), ModuleTarget::Native(path.clone()));
                } else {
                    self.imports.insert(alias.clone(), ModuleTarget::Local(PathBuf::from(path)));
                }
            }
        }

        let mut local_types = HashSet::new();
        for item in &program.items {
            if let ItemKind::Type { name, .. } | ItemKind::Error { name, .. } = &item.kind {
                local_types.insert(name.clone());
            }
        }

        for item in &program.items {
            match &item.kind {
                ItemKind::Type { name, type_params, fields } => {
                    Self::validate_type_params(type_params, item.span)?;
                    let mod_id = self.current_module_id();
                    let mut field_map = HashMap::new();
                    let mut ann_map = HashMap::new();
                    for f in fields {
                        field_map.insert(
                            f.name.clone(),
                            self.resolve_type_expr_in_ctx(&f.ty, &mod_id, &self.imports, type_params, Some(&local_types), item.span)?,
                        );
                        ann_map.insert(f.name.clone(), f.annotations.clone());
                    }
                    if self.structs.contains_key(name) || self.errors.contains(name) {
                        return Err(TypeError {
                            message: format!("Duplicate type definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.structs.insert(
                        name.clone(),
                        StructSignature {
                            type_params: type_params.clone(),
                            fields: field_map,
                            field_annotations: ann_map,
                            is_exported: item.is_exported,
                        },
                    );
                }
                ItemKind::Error { name, fields } => {
                    let mod_id = self.current_module_id();
                    let mut field_map = HashMap::new();
                    let mut ann_map = HashMap::new();
                    for f in fields {
                        field_map.insert(
                            f.name.clone(),
                            self.resolve_type_expr_in_ctx(&f.ty, &mod_id, &self.imports, &[], Some(&local_types), item.span)?,
                        );
                        ann_map.insert(f.name.clone(), f.annotations.clone());
                    }
                    if self.structs.contains_key(name) || self.errors.contains(name) {
                        return Err(TypeError {
                            message: format!("Duplicate error definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.structs.insert(
                        name.clone(),
                        StructSignature {
                            type_params: Vec::new(),
                            fields: field_map,
                            field_annotations: ann_map,
                            is_exported: item.is_exported,
                        },
                    );
                    self.errors.insert(name.clone());
                }
                ItemKind::Func { name, type_params, params, return_type, is_validator, .. } => {
                    Self::validate_type_params(type_params, item.span)?;
                    let mod_id = self.current_module_id();
                    let mut param_types = Vec::with_capacity(params.len());
                    for p in params {
                        param_types.push((
                            p.name.clone(),
                            self.resolve_type_expr_in_ctx(&p.ty, &mod_id, &self.imports, type_params, Some(&local_types), item.span)?,
                        ));
                    }
                    let ret_type = match return_type.as_ref() {
                        Some(rt) => self.resolve_type_expr_in_ctx(rt, &mod_id, &self.imports, type_params, Some(&local_types), item.span)?,
                        None => Type::Void,
                    };

                    if self.functions.contains_key(name) {
                        return Err(TypeError {
                            message: format!("Duplicate function definition '{}'", name),
                            span: item.span,
                        });
                    }
                    self.functions.insert(
                        name.clone(),
                        FuncSignature {
                            type_params: type_params.clone(),
                            params: param_types,
                            return_type: ret_type,
                            is_exported: item.is_exported,
                            is_validator: *is_validator,
                        },
                    );
                }
                ItemKind::Import { .. } => {}
            }
        }

        let fn_sigs: Vec<FuncSignature> = self.functions.values().cloned().collect();
        for sig in &fn_sigs {
            for (_, param_ty) in &sig.params {
                self.check_generic_arity_in_type(param_ty, Span::new(1, 1))?;
            }
            self.check_generic_arity_in_type(&sig.return_type, Span::new(1, 1))?;
        }
        let st_sigs: Vec<StructSignature> = self.structs.values().cloned().collect();
        for sig in &st_sigs {
            for field_ty in sig.fields.values() {
                self.check_generic_arity_in_type(field_ty, Span::new(1, 1))?;
            }
        }

        for item in &program.items {
            if let ItemKind::Func { name, body, is_validator, params, .. } = &item.kind {
                let sig = self.functions.get(name).unwrap().clone();
                let ret_ty = sig.return_type.clone();

                if *is_validator {
                    if params.is_empty() {
                        return Err(TypeError {
                            message: format!("Validator function '{}' must have at least one parameter (the value to validate)", name),
                            span: item.span,
                        });
                    }
                    let ret = sig.return_type.clone();
                    let has_void = match &ret {
                        Type::Void => true,
                        Type::Union(vars) => vars.contains(&Type::Void),
                        _ => false,
                    };
                    let issue_id = TypeId::native("cdt.validation", "Issue");
                    let has_issue = match &ret {
                        Type::Custom(id) => id == &issue_id,
                        Type::Union(vars) => vars.iter().any(|v| matches!(v, Type::Custom(id) if id == &issue_id)),
                        _ => false,
                    };
                    if !has_void || !has_issue {
                        return Err(TypeError {
                            message: format!("Validator function '{}' must return 'Void | validation.Issue', got '{}'", name, ret),
                            span: item.span,
                        });
                    }
                }

                let can_fallthrough_void = match &ret_ty {
                    Type::Void => true,
                    Type::Union(vars) => vars.contains(&Type::Void),
                    _ => false,
                };
                if !can_fallthrough_void && !block_always_returns(body) {
                    return Err(TypeError {
                        message: format!("Function '{}' must return a value of type '{}' on all reachable paths", name, ret_ty),
                        span: item.span,
                    });
                }

                self.current_return_type = Some(sig.return_type.clone());
                self.current_type_params = sig.type_params.clone();

                self.narrowed.clear();
                self.widened_outer_vars.clear();
                self.enter_scope();
                for (param_name, param_ty) in sig.params {
                    self.define_variable(param_name, param_ty, false, item.span);
                }

                let body_res = self.check_block(body);
                let exit_res = self.exit_scope();
                self.current_type_params.clear();
                self.current_return_type = None;
                body_res?;
                exit_res?;
            }

            if let ItemKind::Type { type_params, fields, .. } = &item.kind {
                self.current_type_params = type_params.clone();
                for f in fields {
                    for ann in &f.annotations {
                        if let Err(e) = self.check_field_annotation(&f.name, f, ann) {
                            self.current_type_params.clear();
                            return Err(e);
                        }
                    }
                }
                self.current_type_params.clear();
            }
        }

        Ok(())
    }

    fn check_field_annotation(
        &mut self,
        field_name: &str,
        field: &crate::ast::FieldDef,
        ann: &crate::ast::FieldAnnotation,
    ) -> Result<(), TypeError> {
        let (target_alias, func_name) = if let Some((alias, name)) = ann.target.split_once('.') {
            (Some(alias), name)
        } else {
            (None, ann.target.as_str())
        };

        let (params, _return_type, is_validator, is_exported) = if let Some(alias) = target_alias {
            if let Some(target) = self.imports.get(alias) {
                match target {
                    ModuleTarget::Native(nat_mod) => {
                        let m = self.native_registry.get_module(nat_mod).ok_or_else(|| TypeError {
                            message: format!("Unknown native module '{}'", nat_mod),
                            span: ann.span,
                        })?;
                        let f = m.functions.get(func_name).ok_or_else(|| TypeError {
                            message: format!("Native module '{}' has no validator '{}'", alias, func_name),
                            span: ann.span,
                        })?;
                        (
                            f.params.iter().map(|(p, t)| (p.to_string(), t.clone())).collect::<Vec<_>>(),
                            f.return_type.clone(),
                            f.is_validator,
                            true,
                        )
                    }
                    ModuleTarget::Local(path) => {
                        let info = self.module_info.get(path).ok_or_else(|| TypeError {
                            message: format!("Cannot resolve imported module '{}'", alias),
                            span: ann.span,
                        })?;
                        let sig = info.functions.get(func_name).ok_or_else(|| TypeError {
                            message: format!("Module '{}' has no function '{}'", alias, func_name),
                            span: ann.span,
                        })?;
                        (
                            sig.params.clone(),
                            sig.return_type.clone(),
                            sig.is_validator,
                            sig.is_exported,
                        )
                    }
                }
            } else {
                return Err(TypeError {
                    message: format!("Unknown module alias '{}' in annotation '@{}'", alias, ann.target),
                    span: ann.span,
                });
            }
        } else {
            if let Some(sig) = self.functions.get(func_name) {
                (sig.params.clone(), sig.return_type.clone(), sig.is_validator, sig.is_exported)
            } else {
                return Err(TypeError {
                    message: format!("Unknown validator '{}' referenced in field annotation", ann.target),
                    span: ann.span,
                });
            }
        };

        if target_alias.is_some() && !is_exported {
            return Err(TypeError {
                message: format!("Cannot use private validator '{}' in field annotation", ann.target),
                span: ann.span,
            });
        }

        if !is_validator {
            return Err(TypeError {
                message: format!(
                    "Function '{}' cannot be used as an annotation because it is not marked with '[#validator]'",
                    ann.target
                ),
                span: ann.span,
            });
        }

        let expected_extra = if params.is_empty() { 0 } else { params.len() - 1 };
        if ann.args.len() != expected_extra {
            return Err(TypeError {
                message: format!(
                    "Validator '{}' expects {} additional argument(s), but {} were provided",
                    ann.target, expected_extra, ann.args.len()
                ),
                span: ann.span,
            });
        }

        let raw_field_ty = self.resolve_type_expr(&field.ty, ann.span)?;
        let effective_field_ty = match raw_field_ty {
            Type::Optional(inner) => *inner,
            other => other,
        };
        let first_param_ty = &params[0].1;
        if !effective_field_ty.is_assignable_to(first_param_ty) {
            return Err(TypeError {
                message: format!(
                    "Field '{}' of type '{}' is not compatible with validator parameter type '{}'",
                    field_name, effective_field_ty, first_param_ty
                ),
                span: ann.span,
            });
        }

        let is_validation_bound = matches!(
            target_alias.and_then(|a| self.imports.get(a)),
            Some(ModuleTarget::Native(m)) if m == "cdt.validation"
        ) && (func_name == "min" || func_name == "max");

        for (idx, arg_expr) in ann.args.iter().enumerate() {
            let arg_ty = self.check_expr(arg_expr)?;
            let expected_param_ty = &params[idx + 1].1;
            if is_validation_bound && idx == 0 {
                let required = if effective_field_ty == Type::Float {
                    Type::Float
                } else {
                    Type::Int
                };
                if arg_ty != required {
                    return Err(TypeError {
                        message: format!(
                            "Argument 1 for validator '{}' on '{}' field must be '{}', got '{}'",
                            ann.target, effective_field_ty, required, arg_ty
                        ),
                        span: arg_expr.span,
                    });
                }
            }
            if !arg_ty.is_assignable_to(expected_param_ty) {
                return Err(TypeError {
                    message: format!(
                        "Argument {} for validator '{}': expected type '{}', got '{}'",
                        idx + 1, ann.target, expected_param_ty, arg_ty
                    ),
                    span: arg_expr.span,
                });
            }
        }

        Ok(())
    }

}
