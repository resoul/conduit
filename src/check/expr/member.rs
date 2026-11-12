use super::super::*;

impl TypeChecker {
    pub(super) fn check_member_expr(
        &mut self,
        target: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Type, TypeError> {
        if let ExprKind::Ident(ref alias) = target.kind
            && let Some(mod_target) = self.imports.get(alias).cloned()
        {
            return self.resolve_module_member(alias, &mod_target, field, span);
        }

        let target_ty = self.check_expr(target)?;
        self.check_member_on_type(&target_ty, field, span)
    }

    pub(super) fn check_optional_member_expr(
        &mut self,
        target: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Type, TypeError> {
        let target_ty = self.check_expr(target)?;
        match self.classify_optional_chain(&target_ty) {
            OptionalChainTarget::Optional(inner_ty) => {
                let field_ty = self.check_member_on_type(&inner_ty, field, span)?;
                Ok(Self::optional_of(field_ty))
            }
            OptionalChainTarget::Fallible(successes, errors) => {
                self.validate_propagated_errors(&errors, span)?;
                let inner_ty = Type::union(successes);
                self.check_member_on_type(&inner_ty, field, span)
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

    pub(super) fn check_member_on_type(
        &self,
        target_ty: &Type,
        field: &str,
        span: Span,
    ) -> Result<Type, TypeError> {
        if let Type::Union(_) = target_ty {
            return Err(TypeError {
                message: format!(
                    "Cannot access field '{}' on union type '{}' without narrowing",
                    field, target_ty
                ),
                span,
            });
        }

        if let Type::Generic {
            type_id,
            type_args,
        } = target_ty
            && let Some(struct_sig) = self
                .lookup_struct_sig_in(type_id, self.current_module_path.as_deref())
                .cloned()
            && let Some(raw_field_ty) = struct_sig.fields.get(field)
        {
            let mut bindings = HashMap::new();
            for (tp, ta) in struct_sig.type_params.iter().zip(type_args.iter()) {
                bindings.insert(tp.clone(), ta.clone());
            }
            return Self::substitute_type(raw_field_ty, &bindings, span);
        }

        if let Type::Custom(type_id) = target_ty {
            match &type_id.module {
                ModuleId::Builtin => {
                    if type_id.name == "IndexError" {
                        match field {
                            "index" | "length" => return Ok(Type::Int),
                            "message" => return Ok(Type::String),
                            _ => {}
                        }
                    } else if type_id.name == "KeyError" {
                        match field {
                            "key" | "message" => return Ok(Type::String),
                            _ => {}
                        }
                    } else if type_id.name == "ParseError" {
                        match field {
                            "value" | "message" => return Ok(Type::String),
                            _ => {}
                        }
                    }
                }
                ModuleId::Program | ModuleId::Local(_) => {
                    if let Some(struct_sig) =
                        self.lookup_struct_sig_in(type_id, self.current_module_path.as_deref())
                        && let Some(field_ty) = struct_sig.fields.get(field)
                    {
                        return Ok(field_ty.clone());
                    }
                }
                ModuleId::Native(nat_name) => {
                    if let Some(m) = self.native_registry.get_module(nat_name)
                        && let Some(t) = m.types.get(type_id.name.as_str())
                    {
                        for (f_name, f_ty) in &t.fields {
                            if *f_name == field {
                                return Ok(f_ty.clone());
                            }
                        }
                    }
                }
            }
        }

        Err(TypeError {
            message: format!("Type {} has no field '{}'", target_ty, field),
            span,
        })
    }
}

