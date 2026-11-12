use super::*;

impl Interpreter {
    pub(super) fn type_id_to_mod_key(type_id: &TypeId) -> Option<(Option<PathBuf>, String)> {
        match &type_id.module {
            ModuleId::Program => Some((None, type_id.name.clone())),
            ModuleId::Local(path) => Some((Some(path.clone()), type_id.name.clone())),
            ModuleId::Builtin | ModuleId::Native(_) => None,
        }
    }

    fn resolve_struct_schema(&self, type_id: &TypeId) -> Option<Arc<StructSchema>> {
        if let Some(schema) = self.struct_schemas.borrow().get(type_id) {
            return Some(Arc::clone(schema));
        }
        let schema = match &type_id.module {
            ModuleId::Builtin => return builtin_error_schema(&type_id.name),
            ModuleId::Native(nat_name) => {
                let module = self.native_registry.get_module(nat_name)?;
                let td = module.types.get(type_id.name.as_str())?;
                StructSchema::new(
                    type_id.clone(),
                    td.fields.iter().map(|(name, _)| (*name).to_string()),
                )
            }
            ModuleId::Program | ModuleId::Local(_) => {
                let key = Self::type_id_to_mod_key(type_id)?;
                if let Some(defs) = self.module_struct_fields.get(&key) {
                    let fields = defs
                        .iter()
                        .map(|f| {
                            let json_name = f.json_name.clone().unwrap_or_else(|| f.name.clone());
                            (f.name.clone(), json_name)
                        })
                        .collect();
                    StructSchema::with_json_names(type_id.clone(), fields)
                } else {
                    let defs = self.module_error_fields.get(&key)?;
                    StructSchema::new(type_id.clone(), defs.iter().map(|f| f.name.clone()))
                }
            }
        };
        self.struct_schemas
            .borrow_mut()
            .insert(type_id.clone(), Arc::clone(&schema));
        Some(schema)
    }

    pub(super) fn construct_record(
        &mut self,
        type_id: TypeId,
        is_error: bool,
        named_args: &[NamedArg],
    ) -> Result<Value, InterpFlow> {
        let (schema, values) = if let Some(schema) = self.resolve_struct_schema(&type_id) {
            let mut values = vec![Value::Nil; schema.field_count()];
            for arg in named_args {
                let val = self.eval_expr(&arg.value)?;
                if let Some(idx) = schema.index_of(&arg.name) {
                    values[idx] = val;
                }
            }
            (schema, values)
        } else {
            let mut names = Vec::with_capacity(named_args.len());
            let mut values = Vec::with_capacity(named_args.len());
            for arg in named_args {
                names.push(arg.name.clone());
                values.push(self.eval_expr(&arg.value)?);
            }
            (StructSchema::new(type_id, names), values)
        };
        Ok(if is_error {
            Value::Error { schema, values }
        } else {
            Value::Struct { schema, values }
        })
    }

    pub(super) fn substitute_runtime_type(ty: &Type, bindings: &HashMap<String, Type>) -> Type {
        match ty {
            Type::TypeParam(name) => bindings
                .get(name)
                .cloned()
                .unwrap_or_else(|| Type::TypeParam(name.clone())),
            Type::List(inner) => Type::List(Box::new(Self::substitute_runtime_type(inner, bindings))),
            Type::Map(k, v) => Type::Map(
                Box::new(Self::substitute_runtime_type(k, bindings)),
                Box::new(Self::substitute_runtime_type(v, bindings)),
            ),
            Type::Optional(inner) => {
                Type::Optional(Box::new(Self::substitute_runtime_type(inner, bindings)))
            }
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .iter()
                    .map(|e| Self::substitute_runtime_type(e, bindings))
                    .collect(),
            ),
            Type::Union(vars) => Type::union(
                vars.iter()
                    .map(|v| Self::substitute_runtime_type(v, bindings))
                    .collect(),
            ),
            Type::Generic { type_id, type_args } => Type::Generic {
                type_id: type_id.clone(),
                type_args: type_args
                    .iter()
                    .map(|a| Self::substitute_runtime_type(a, bindings))
                    .collect(),
            },
            other => other.clone(),
        }
    }

    pub(super) fn resolve_ast_type_in_mod(
        &self,
        ty_expr: &TypeExpr,
        mod_opt: &Option<PathBuf>,
        type_params: &[String],
        span: Span,
    ) -> Result<Type, RuntimeError> {
        let resolve_target_id = |name: &str| -> Result<TypeId, RuntimeError> {
            if let Some((alias, item_name)) = name.split_once('.') {
                let target = self
                    .module_imports
                    .get(mod_opt)
                    .and_then(|m| m.get(alias))
                    .ok_or_else(|| RuntimeError {
                        message: format!("Unknown module alias '{}' in json.decode", alias),
                        span,
                    })?;
                Ok(match target {
                    ModuleTarget::Native(nat_name) => TypeId::native(nat_name.clone(), item_name),
                    ModuleTarget::Local(path) => TypeId::local(path.clone(), item_name),
                })
            } else {
                Ok(TypeId::from_module_opt(mod_opt.as_ref(), name))
            }
        };

        match ty_expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => Ok(Type::Int),
                "Float" => Ok(Type::Float),
                "String" => Ok(Type::String),
                "Bool" => Ok(Type::Bool),
                "Void" => Ok(Type::Void),
                _ => {
                    if !name.contains('.') && type_params.iter().any(|p| p == name) {
                        Ok(Type::TypeParam(name.clone()))
                    } else {
                        Ok(Type::Custom(resolve_target_id(name)?))
                    }
                }
            },
            TypeExpr::Generic { name, type_args } => {
                if name == "Map" && type_args.len() == 2 {
                    let k = self.resolve_ast_type_in_mod(&type_args[0], mod_opt, type_params, span)?;
                    let v = self.resolve_ast_type_in_mod(&type_args[1], mod_opt, type_params, span)?;
                    Ok(Type::Map(Box::new(k), Box::new(v)))
                } else {
                    let type_id = resolve_target_id(name)?;
                    let mut args = Vec::with_capacity(type_args.len());
                    for ta in type_args {
                        args.push(self.resolve_ast_type_in_mod(ta, mod_opt, type_params, span)?);
                    }
                    Ok(Type::Generic {
                        type_id,
                        type_args: args,
                    })
                }
            }
            TypeExpr::List(inner) => Ok(Type::List(Box::new(self.resolve_ast_type_in_mod(
                inner,
                mod_opt,
                type_params,
                span,
            )?))),
            TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(
                self.resolve_ast_type_in_mod(inner, mod_opt, type_params, span)?,
            ))),
            TypeExpr::Tuple(elems) => {
                let mut resolved = Vec::with_capacity(elems.len());
                for e in elems {
                    resolved.push(self.resolve_ast_type_in_mod(e, mod_opt, type_params, span)?);
                }
                Ok(Type::Tuple(resolved))
            }
            TypeExpr::Union(vars) => {
                let mut resolved = Vec::with_capacity(vars.len());
                for v in vars {
                    resolved.push(self.resolve_ast_type_in_mod(v, mod_opt, type_params, span)?);
                }
                Ok(Type::union(resolved))
            }
            TypeExpr::Func { params, return_type } => {
                let mut ps = Vec::with_capacity(params.len());
                for p in params {
                    ps.push(self.resolve_ast_type_in_mod(p, mod_opt, type_params, span)?);
                }
                let rt = self.resolve_ast_type_in_mod(return_type, mod_opt, type_params, span)?;
                Ok(Type::Function {
                    params: ps,
                    return_type: Box::new(rt),
                })
            }
        }
    }

}
