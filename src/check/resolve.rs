use super::*;

impl TypeChecker {
    pub(super) fn current_module_id(&self) -> ModuleId {
        match &self.current_module_path {
            Some(path) => ModuleId::Local(path.clone()),
            None => ModuleId::Program,
        }
    }

    pub(super) fn current_type_id(&self, name: &str) -> TypeId {
        TypeId {
            module: self.current_module_id(),
            name: name.to_string(),
        }
    }

    pub(super) fn validate_type_params(type_params: &[String], span: Span) -> Result<(), TypeError> {
        for p in type_params {
            if matches!(
                p.as_str(),
                "Int"
                    | "Float"
                    | "String"
                    | "Bool"
                    | "Void"
                    | "Nil"
                    | "Map"
                    | "IndexError"
                    | "KeyError"
                    | "ParseError"
            ) {
                return Err(TypeError {
                    message: format!("Type parameter cannot use built-in name '{}'", p),
                    span,
                });
            }
        }
        Ok(())
    }

    pub(super) fn validate_map_key_type(key_ty: &Type, span: Span, allow_type_param: bool) -> Result<(), TypeError> {
        match key_ty {
            Type::String | Type::Int | Type::Bool => Ok(()),
            Type::TypeParam(_) if allow_type_param => Ok(()),
            other => Err(TypeError {
                message: format!(
                    "Map key type must be 'String', 'Int', or 'Bool', got '{}'",
                    other
                ),
                span,
            }),
        }
    }

    fn is_locally_declared_type(
        &self,
        name: &str,
        current_module: &ModuleId,
        local_types: Option<&HashSet<String>>,
    ) -> bool {
        if let Some(set) = local_types
            && set.contains(name) {
                return true;
            }
        match current_module {
            ModuleId::Program => self.structs.contains_key(name),
            ModuleId::Local(path) => {
                if self.current_module_path.as_deref() == Some(path.as_path()) {
                    self.structs.contains_key(name)
                } else {
                    self.module_info
                        .get(path)
                        .is_some_and(|info| info.structs.contains_key(name))
                }
            }
            ModuleId::Builtin | ModuleId::Native(_) => false,
        }
    }

    pub(super) fn resolve_target_type_id(
        &self,
        name: &str,
        current_module: &ModuleId,
        imports: &HashMap<String, ModuleTarget>,
        local_types: Option<&HashSet<String>>,
        span: Span,
    ) -> Result<TypeId, TypeError> {
        if let Some((alias, item_name)) = name.split_once('.') {
            let target = imports.get(alias).ok_or_else(|| TypeError {
                message: format!("Unknown module alias '{}' in type '{}'", alias, name),
                span,
            })?;
            match target {
                ModuleTarget::Native(nat_name) => Ok(TypeId::native(nat_name.clone(), item_name)),
                ModuleTarget::Local(path) => Ok(TypeId::local(path.clone(), item_name)),
            }
        } else if matches!(name, "IndexError" | "KeyError" | "ParseError")
            && !self.is_locally_declared_type(name, current_module, local_types)
        {
            Ok(TypeId::builtin(name))
        } else {
            Ok(TypeId {
                module: current_module.clone(),
                name: name.to_string(),
            })
        }
    }

    pub(super) fn resolve_type_name_in_ctx(
        &self,
        name: &str,
        current_module: &ModuleId,
        imports: &HashMap<String, ModuleTarget>,
        type_params: &[String],
        local_types: Option<&HashSet<String>>,
        span: Span,
    ) -> Result<Type, TypeError> {
        match name {
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Bool" => Ok(Type::Bool),
            "Void" => Ok(Type::Void),
            "Map" => Err(TypeError {
                message: "Type 'Map' requires 2 type arguments: Map<K, V>".to_string(),
                span,
            }),
            _ => {
                if !name.contains('.') && type_params.iter().any(|p| p == name) {
                    return Ok(Type::TypeParam(name.to_string()));
                }
                let type_id = self.resolve_target_type_id(name, current_module, imports, local_types, span)?;
                Ok(Type::Custom(type_id))
            }
        }
    }

    pub(super) fn resolve_type_expr_in_ctx(
        &self,
        expr: &TypeExpr,
        current_module: &ModuleId,
        imports: &HashMap<String, ModuleTarget>,
        type_params: &[String],
        local_types: Option<&HashSet<String>>,
        span: Span,
    ) -> Result<Type, TypeError> {
        match expr {
            TypeExpr::Named(name) => {
                self.resolve_type_name_in_ctx(name, current_module, imports, type_params, local_types, span)
            }
            TypeExpr::Generic { name, type_args } => {
                if name == "Map" {
                    if type_args.len() != 2 {
                        return Err(TypeError {
                            message: format!(
                                "Map requires 2 type arguments (K, V), got {}",
                                type_args.len()
                            ),
                            span,
                        });
                    }
                    let key_ty = self.resolve_type_expr_in_ctx(
                        &type_args[0],
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?;
                    Self::validate_map_key_type(&key_ty, span, true)?;
                    let val_ty = self.resolve_type_expr_in_ctx(
                        &type_args[1],
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?;
                    return Ok(Type::Map(Box::new(key_ty), Box::new(val_ty)));
                }
                if !name.contains('.') && type_params.iter().any(|p| p == name) {
                    return Err(TypeError {
                        message: format!("Type parameter '{}' cannot take type arguments", name),
                        span,
                    });
                }
                if matches!(name.as_str(), "Int" | "Float" | "String" | "Bool" | "Void")
                    || (matches!(name.as_str(), "IndexError" | "KeyError" | "ParseError")
                        && !self.is_locally_declared_type(name, current_module, local_types))
                {
                    return Err(TypeError {
                        message: format!("Type '{}' is not generic", name),
                        span,
                    });
                }
                let type_id = self.resolve_target_type_id(name, current_module, imports, local_types, span)?;
                let mut resolved_args = Vec::with_capacity(type_args.len());
                for ta in type_args {
                    resolved_args.push(self.resolve_type_expr_in_ctx(
                        ta,
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?);
                }
                Ok(Type::Generic {
                    type_id,
                    type_args: resolved_args,
                })
            }
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_type_expr_in_ctx(
                inner,
                current_module,
                imports,
                type_params,
                local_types,
                span,
            )?))),
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(self.resolve_type_expr_in_ctx(
                inner,
                current_module,
                imports,
                type_params,
                local_types,
                span,
            )?))),
            TypeExpr::Union(variants) => {
                let mut types = Vec::with_capacity(variants.len());
                for v in variants {
                    types.push(self.resolve_type_expr_in_ctx(
                        v,
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?);
                }
                Self::validate_union_generic_instantiations(&types, span)?;
                Ok(Type::union(types))
            }
            TypeExpr::Tuple(elements) => {
                let mut types = Vec::with_capacity(elements.len());
                for e in elements {
                    types.push(self.resolve_type_expr_in_ctx(
                        e,
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?);
                }
                Ok(Type::Tuple(types))
            }
            TypeExpr::Func { params, return_type } => {
                let mut p = Vec::with_capacity(params.len());
                for param in params {
                    p.push(self.resolve_type_expr_in_ctx(
                        param,
                        current_module,
                        imports,
                        type_params,
                        local_types,
                        span,
                    )?);
                }
                let r = Box::new(self.resolve_type_expr_in_ctx(
                    return_type,
                    current_module,
                    imports,
                    type_params,
                    local_types,
                    span,
                )?);
                Ok(Type::Function {
                    params: p,
                    return_type: r,
                })
            }
        }
    }

    pub(super) fn resolve_type_expr(&self, expr: &TypeExpr, span: Span) -> Result<Type, TypeError> {
        let mod_id = self.current_module_id();
        self.resolve_type_expr_in_ctx(expr, &mod_id, &self.imports, &self.current_type_params, None, span)
    }

    pub(super) fn lookup_struct_sig_in<'a>(
        &'a self,
        type_id: &TypeId,
        current_module: Option<&Path>,
    ) -> Option<&'a StructSignature> {
        match &type_id.module {
            ModuleId::Program => self.structs.get(&type_id.name),
            ModuleId::Local(path) => {
                if current_module.map(|c| c == path.as_path()).unwrap_or(false) {
                    self.structs
                        .get(&type_id.name)
                        .or_else(|| self.module_info.get(path).and_then(|i| i.structs.get(&type_id.name)))
                } else {
                    self.module_info
                        .get(path)
                        .and_then(|info| info.structs.get(&type_id.name))
                }
            }
            ModuleId::Builtin | ModuleId::Native(_) => None,
        }
    }

    pub(super) fn builtin_error_sig(name: &str) -> Option<(TypeId, StructSignature)> {
        if name == "IndexError" {
            let mut fields = HashMap::new();
            fields.insert("index".to_string(), Type::Int);
            fields.insert("length".to_string(), Type::Int);
            fields.insert("message".to_string(), Type::String);
            Some((
                TypeId::builtin("IndexError"),
                StructSignature {
                    type_params: Vec::new(),
                    fields,
                    field_annotations: HashMap::new(),
                    is_exported: true,
                },
            ))
        } else if name == "KeyError" {
            let mut fields = HashMap::new();
            fields.insert("key".to_string(), Type::String);
            fields.insert("message".to_string(), Type::String);
            Some((
                TypeId::builtin("KeyError"),
                StructSignature {
                    type_params: Vec::new(),
                    fields,
                    field_annotations: HashMap::new(),
                    is_exported: true,
                },
            ))
        } else if name == "ParseError" {
            let mut fields = HashMap::new();
            fields.insert("value".to_string(), Type::String);
            fields.insert("message".to_string(), Type::String);
            Some((
                TypeId::builtin("ParseError"),
                StructSignature {
                    type_params: Vec::new(),
                    fields,
                    field_annotations: HashMap::new(),
                    is_exported: true,
                },
            ))
        } else {
            None
        }
    }

    pub(super) fn check_generic_arity_in_type(&self, ty: &Type, span: Span) -> Result<(), TypeError> {
        let cur_mod = self.current_module_path.as_deref();
        match ty {
            Type::Custom(type_id) => {
                if let Some(sig) = self.lookup_struct_sig_in(type_id, cur_mod)
                    && !sig.type_params.is_empty() {
                        return Err(TypeError {
                            message: format!(
                                "Generic type '{}' requires {} type argument(s)",
                                type_id,
                                sig.type_params.len()
                            ),
                            span,
                        });
                    }
                Ok(())
            }
            Type::Generic { type_id, type_args } => {
                match self.lookup_struct_sig_in(type_id, cur_mod) {
                    Some(sig) => {
                        if sig.type_params.is_empty() {
                            return Err(TypeError {
                                message: format!("Type '{}' is not generic", type_id),
                                span,
                            });
                        }
                        if sig.type_params.len() != type_args.len() {
                            return Err(TypeError {
                                message: format!(
                                    "Type '{}' expects {} type argument(s), got {}",
                                    type_id,
                                    sig.type_params.len(),
                                    type_args.len()
                                ),
                                span,
                            });
                        }
                        let mut subst = HashMap::new();
                        for (param, arg) in sig.type_params.iter().zip(type_args.iter()) {
                            subst.insert(param.clone(), arg.clone());
                        }
                        for field_ty in sig.fields.values() {
                            Self::substitute_type(field_ty, &subst, span)?;
                        }
                    }
                    None => {
                        return Err(TypeError {
                            message: format!("Unknown generic type '{}'", type_id),
                            span,
                        });
                    }
                }
                for ta in type_args {
                    self.check_generic_arity_in_type(ta, span)?;
                }
                Ok(())
            }
            Type::List(inner) | Type::Optional(inner) => {
                self.check_generic_arity_in_type(inner, span)
            }
            Type::Map(k, v) => {
                self.check_generic_arity_in_type(k, span)?;
                self.check_generic_arity_in_type(v, span)
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                for e in elems {
                    self.check_generic_arity_in_type(e, span)?;
                }
                Ok(())
            }
            Type::Function { params, return_type } => {
                for p in params {
                    self.check_generic_arity_in_type(p, span)?;
                }
                self.check_generic_arity_in_type(return_type, span)
            }
            _ => Ok(()),
        }
    }

    pub(super) fn check_json_decodable(
        &self,
        ty: &Type,
        span: Span,
        visited: &mut HashSet<TypeId>,
    ) -> Result<(), TypeError> {
        let cur_mod = self.current_module_path.as_deref();
        match ty {
            Type::Int | Type::Float | Type::Bool | Type::String => Ok(()),
            Type::Custom(type_id) => {
                if type_id == &TypeId::native("cdt.json", "Value") {
                    return Ok(());
                }
                if self.is_error_type(ty) {
                    return Err(TypeError {
                        message: format!("Error type '{}' cannot be used as json.decode target", type_id),
                        span,
                    });
                }
                let sig = self.lookup_struct_sig_in(type_id, cur_mod).ok_or_else(|| TypeError {
                    message: format!("Type '{}' cannot be decoded from JSON", type_id),
                    span,
                })?;
                if visited.insert(type_id.clone()) {
                    for fty in sig.fields.values() {
                        self.check_json_decodable(fty, span, visited)?;
                    }
                    visited.remove(type_id);
                }
                Ok(())
            }
            Type::Generic { type_id, type_args } => {
                let sig = self.lookup_struct_sig_in(type_id, cur_mod).ok_or_else(|| TypeError {
                    message: format!("Type '{}' cannot be decoded from JSON", type_id),
                    span,
                })?;
                let mut bindings = HashMap::new();
                for (tp, arg) in sig.type_params.iter().zip(type_args.iter()) {
                    bindings.insert(tp.clone(), arg.clone());
                }
                if visited.insert(type_id.clone()) {
                    for fty in sig.fields.values() {
                        let resolved = Self::substitute_type(fty, &bindings, span)?;
                        self.check_json_decodable(&resolved, span, visited)?;
                    }
                    visited.remove(type_id);
                }
                Ok(())
            }
            Type::List(inner) | Type::Optional(inner) => {
                self.check_json_decodable(inner, span, visited)
            }
            Type::Tuple(elems) => {
                for e in elems {
                    self.check_json_decodable(e, span, visited)?;
                }
                Ok(())
            }
            Type::Map(k, v) => {
                if **k != Type::String {
                    return Err(TypeError {
                        message: format!(
                            "json.decode only supports String-keyed Maps, got 'Map<{}, {}>'",
                            k, v
                        ),
                        span,
                    });
                }
                self.check_json_decodable(v, span, visited)
            }
            Type::Union(variants) => {
                let mut num_count = 0usize;
                let mut array_count = 0usize;
                let mut obj_count = 0usize;
                let mut bool_count = 0usize;
                let mut str_count = 0usize;
                for v in variants {
                    if matches!(v, Type::Nil) {
                        continue;
                    }
                    self.check_json_decodable(v, span, visited)?;
                    match v {
                        Type::Int | Type::Float => num_count += 1,
                        Type::Bool => bool_count += 1,
                        Type::String => str_count += 1,
                        Type::List(_) | Type::Tuple(_) => array_count += 1,
                        Type::Map(_, _) | Type::Custom(_) | Type::Generic { .. } => obj_count += 1,
                        Type::Optional(_) => {}
                        _ => {}
                    }
                }
                if num_count > 1 || array_count > 1 || obj_count > 1 || bool_count > 1 || str_count > 1 {
                    return Err(TypeError {
                        message: format!(
                            "Ambiguous union type '{}' cannot be unambiguously decoded from JSON",
                            ty
                        ),
                        span,
                    });
                }
                Ok(())
            }
            other => Err(TypeError {
                message: format!("Type '{}' is not supported by json.decode", other),
                span,
            }),
        }
    }

}
