use super::*;

impl TypeChecker {
    pub(super) fn substitute_type(
        ty: &Type,
        bindings: &HashMap<String, Type>,
        span: Span,
    ) -> Result<Type, TypeError> {
        match ty {
            Type::TypeParam(name) => {
                Ok(bindings.get(name).cloned().unwrap_or_else(|| Type::TypeParam(name.clone())))
            }
            Type::List(inner) => {
                Ok(Type::List(Box::new(Self::substitute_type(inner, bindings, span)?)))
            }
            Type::Map(k, v) => {
                let sk = Self::substitute_type(k, bindings, span)?;
                Self::validate_map_key_type(&sk, span, true)?;
                let sv = Self::substitute_type(v, bindings, span)?;
                Ok(Type::Map(Box::new(sk), Box::new(sv)))
            }
            Type::Optional(inner) => {
                Ok(Type::Optional(Box::new(Self::substitute_type(inner, bindings, span)?)))
            }
            Type::Tuple(elems) => {
                let mut res = Vec::with_capacity(elems.len());
                for e in elems {
                    res.push(Self::substitute_type(e, bindings, span)?);
                }
                Ok(Type::Tuple(res))
            }
            Type::Union(vars) => {
                let mut res = Vec::with_capacity(vars.len());
                for v in vars {
                    res.push(Self::substitute_type(v, bindings, span)?);
                }
                Self::validate_union_generic_instantiations(&res, span)?;
                Ok(Type::union(res))
            }
            Type::Generic { type_id, type_args } => {
                let mut res = Vec::with_capacity(type_args.len());
                for ta in type_args {
                    res.push(Self::substitute_type(ta, bindings, span)?);
                }
                Ok(Type::Generic {
                    type_id: type_id.clone(),
                    type_args: res,
                })
            }
            Type::Function { params, return_type } => {
                let mut p = Vec::with_capacity(params.len());
                for param in params {
                    p.push(Self::substitute_type(param, bindings, span)?);
                }
                let r = Box::new(Self::substitute_type(return_type, bindings, span)?);
                Ok(Type::Function {
                    params: p,
                    return_type: r,
                })
            }
            other => Ok(other.clone()),
        }
    }

    pub(super) fn unify_type(
        pattern: &Type,
        actual: &Type,
        bindings: &mut HashMap<String, Type>,
        span: Span,
    ) -> Result<(), TypeError> {
        if *actual == Type::Any {
            return Ok(());
        }
        match pattern {
            Type::TypeParam(name) => {
                if let Some(existing) = bindings.get(name) {
                    if existing != actual {
                        return Err(TypeError {
                            message: format!(
                                "Conflicting types inferred for type parameter '{}': '{}' vs '{}'",
                                name, existing, actual
                            ),
                            span,
                        });
                    }
                } else {
                    bindings.insert(name.clone(), actual.clone());
                }
                Ok(())
            }
            Type::List(p_inner) => {
                if let Type::List(a_inner) = actual {
                    Self::unify_type(p_inner, a_inner, bindings, span)?;
                }
                Ok(())
            }
            Type::Map(pk, pv) => {
                if let Type::Map(ak, av) = actual {
                    Self::unify_type(pk, ak, bindings, span)?;
                    Self::unify_type(pv, av, bindings, span)?;
                }
                Ok(())
            }
            Type::Optional(p_inner) => {
                match actual {
                    Type::Optional(a_inner) => Self::unify_type(p_inner, a_inner, bindings, span),
                    Type::Nil => Ok(()),
                    other => Self::unify_type(p_inner, other, bindings, span),
                }
            }
            Type::Tuple(p_elems) => {
                if let Type::Tuple(a_elems) = actual
                    && p_elems.len() == a_elems.len() {
                        for (p, a) in p_elems.iter().zip(a_elems.iter()) {
                            Self::unify_type(p, a, bindings, span)?;
                        }
                    }
                Ok(())
            }
            Type::Generic {
                type_id: pid,
                type_args: pa,
            } => {
                if let Type::Generic {
                    type_id: aid,
                    type_args: aa,
                } = actual
                    && pid == aid && pa.len() == aa.len() {
                        for (p, a) in pa.iter().zip(aa.iter()) {
                            Self::unify_type(p, a, bindings, span)?;
                        }
                    }
                Ok(())
            }
            Type::Function {
                params: pp,
                return_type: pr,
            } => {
                if let Type::Function {
                    params: ap,
                    return_type: ar,
                } = actual
                    && pp.len() == ap.len() {
                        for (p, a) in pp.iter().zip(ap.iter()) {
                            Self::unify_type(p, a, bindings, span)?;
                        }
                        Self::unify_type(pr, ar, bindings, span)?;
                    }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn is_error_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Custom(type_id) => match &type_id.module {
                ModuleId::Builtin => matches!(type_id.name.as_str(), "IndexError" | "KeyError" | "ParseError"),
                ModuleId::Program => self.errors.contains(&type_id.name),
                ModuleId::Local(path) => {
                    if Some(path) == self.current_module_path.as_ref() {
                        self.errors.contains(&type_id.name)
                    } else {
                        self.module_info
                            .get(path)
                            .map(|info| info.errors.contains(&type_id.name))
                            .unwrap_or(false)
                    }
                }
                ModuleId::Native(nat_name) => self
                    .native_registry
                    .get_module(nat_name)
                    .and_then(|m| m.types.get(type_id.name.as_str()))
                    .map(|t| t.is_error)
                    .unwrap_or(false),
            },
            _ => false,
        }
    }

    pub fn split_success_and_errors(&self, ty: &Type) -> (Vec<Type>, Vec<Type>) {
        match ty {
            Type::Union(variants) => {
                let mut successes = Vec::new();
                let mut errors = Vec::new();
                for v in variants {
                    if self.is_error_type(v) {
                        errors.push(v.clone());
                    } else {
                        successes.push(v.clone());
                    }
                }
                (successes, errors)
            }
            other => {
                if self.is_error_type(other) {
                    (Vec::new(), vec![other.clone()])
                } else {
                    (vec![other.clone()], Vec::new())
                }
            }
        }
    }

    pub fn contains_error(&self, ty: &Type) -> bool {
        !self.split_success_and_errors(ty).1.is_empty()
    }

    fn check_type_id_privacy(&self, type_id: &TypeId, current_mod: Option<&Path>) -> Option<String> {
        match &type_id.module {
            ModuleId::Local(path) => {
                if current_mod.map(|c| c == path.as_path()).unwrap_or(false)
                    && let Some(info) = self.module_info.get(path) {
                        let exists = info.structs.contains_key(&type_id.name)
                            || info.errors.contains(&type_id.name);
                        let is_exported = info.exports.contains(&type_id.name);
                        if exists && !is_exported {
                            return Some(type_id.name.clone());
                        }
                    }
                None
            }
            ModuleId::Program => {
                let exists = self.structs.contains_key(&type_id.name)
                    || self.errors.contains(&type_id.name);
                let is_exported = self
                    .structs
                    .get(&type_id.name)
                    .map(|s| s.is_exported)
                    .unwrap_or(false);
                if exists && !is_exported {
                    Some(type_id.name.clone())
                } else {
                    None
                }
            }
            ModuleId::Builtin | ModuleId::Native(_) => None,
        }
    }

    pub(super) fn contains_private_type(&self, ty: &Type, current_mod: Option<&Path>) -> Option<String> {
        match ty {
            Type::Custom(type_id) => self.check_type_id_privacy(type_id, current_mod),
            Type::Generic { type_id, type_args } => {
                if let Some(leak) = self.check_type_id_privacy(type_id, current_mod) {
                    return Some(leak);
                }
                for ta in type_args {
                    if let Some(leak) = self.contains_private_type(ta, current_mod) {
                        return Some(leak);
                    }
                }
                None
            }
            Type::List(inner) | Type::Optional(inner) => self.contains_private_type(inner, current_mod),
            Type::Map(k, v) => self
                .contains_private_type(k, current_mod)
                .or_else(|| self.contains_private_type(v, current_mod)),
            Type::Tuple(elems) => {
                for e in elems {
                    if let Some(leak) = self.contains_private_type(e, current_mod) {
                        return Some(leak);
                    }
                }
                None
            }
            Type::Union(vars) => {
                for v in vars {
                    if let Some(leak) = self.contains_private_type(v, current_mod) {
                        return Some(leak);
                    }
                }
                None
            }
            Type::Function { params, return_type } => {
                for p in params {
                    if let Some(leak) = self.contains_private_type(p, current_mod) {
                        return Some(leak);
                    }
                }
                self.contains_private_type(return_type, current_mod)
            }
            _ => None,
        }
    }

}
