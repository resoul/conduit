use super::*;

impl TypeChecker {
    pub(super) fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(super) fn exit_scope(&mut self) -> Result<(), TypeError> {
        if let Some(scope) = self.scopes.pop() {
            for (name, binding) in scope {
                if !binding.used && !name.starts_with('_') && self.contains_error(&binding.base_ty) {
                    return Err(TypeError {
                        message: format!(
                            "Unused variable '{}' of error type '{}' must be handled",
                            name, binding.base_ty
                        ),
                        span: binding.span,
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn define_variable(&mut self, name: String, ty: Type, is_mutable: bool, span: Span) {
        self.narrowed.remove(&name);
        if let Some(current_scope) = self.scopes.last_mut() {
            current_scope.insert(
                name,
                VarBinding {
                    base_ty: ty,
                    is_mutable,
                    span,
                    used: false,
                },
            );
        }
    }

    pub(super) fn define_used_variable(&mut self, name: String, ty: Type, is_mutable: bool, span: Span) {
        self.narrowed.remove(&name);
        if let Some(current_scope) = self.scopes.last_mut() {
            current_scope.insert(
                name,
                VarBinding {
                    base_ty: ty,
                    is_mutable,
                    span,
                    used: true,
                },
            );
        }
    }

    pub(super) fn lookup_variable(&mut self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(name) {
                binding.used = true;
                if let Some(narrowed_ty) = self.narrowed.get(name) {
                    return Some(narrowed_ty.clone());
                }
                return Some(binding.base_ty.clone());
            }
        }
        None
    }

    pub(super) fn lookup_variable_binding_mut(&mut self, name: &str) -> Option<(usize, &mut VarBinding)> {
        let len = self.scopes.len();
        for (rev_idx, scope) in self.scopes.iter_mut().rev().enumerate() {
            if let Some(binding) = scope.get_mut(name) {
                binding.used = true;
                return Some((len - 1 - rev_idx, binding));
            }
        }
        None
    }

    pub(super) fn validate_union_generic_instantiations(
        types: &[Type],
        span: Span,
    ) -> Result<(), TypeError> {
        fn collect_members<'a>(ty: &'a Type, members: &mut Vec<&'a Type>) {
            match ty {
                Type::Union(variants) => {
                    for variant in variants {
                        collect_members(variant, members);
                    }
                }
                Type::Optional(inner) => collect_members(inner, members),
                other => members.push(other),
            }
        }

        let mut members = Vec::new();
        for ty in types {
            collect_members(ty, &mut members);
        }
        let mut seen: Vec<(&TypeId, &Vec<Type>)> = Vec::new();
        for ty in members {
            if let Type::Generic { type_id, type_args } = ty {
                if let Some((_, previous_args)) = seen.iter().find(|(id, _)| *id == type_id) {
                    if *previous_args != type_args {
                        let previous = Type::Generic {
                            type_id: type_id.clone(),
                            type_args: (*previous_args).clone(),
                        };
                        return Err(TypeError {
                            message: format!(
                                "Cannot combine distinct generic instantiations of '{}' ('{}' and '{}') in the same union",
                                type_id, previous, ty
                            ),
                            span,
                        });
                    }
                } else {
                    seen.push((type_id, type_args));
                }
            }
        }
        Ok(())
    }

    fn lookup_effective_type_unmarked(&self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                if let Some(narrowed_ty) = self.narrowed.get(name) {
                    return Some(narrowed_ty.clone());
                }
                return Some(binding.base_ty.clone());
            }
        }
        None
    }

    pub(super) fn narrowed_non_nil_type(ty: &Type) -> Option<Type> {
        match ty {
            Type::Optional(inner) => Some(inner.as_ref().clone()),
            Type::Union(variants) if variants.contains(&Type::Nil) => {
                let remaining: Vec<Type> = variants
                    .iter()
                    .filter(|v| *v != &Type::Nil)
                    .cloned()
                    .collect();
                if remaining.is_empty() {
                    None
                } else {
                    Some(Type::union(remaining))
                }
            }
            _ => None,
        }
    }

    pub(super) fn type_may_be_nil(ty: &Type) -> bool {
        match ty {
            Type::Nil | Type::Optional(_) | Type::Any => true,
            Type::Union(variants) => variants.iter().any(Self::type_may_be_nil),
            _ => false,
        }
    }

    fn extract_nil_check_var(left: &Expr, right: &Expr) -> Option<String> {
        match (&left.kind, &right.kind) {
            (ExprKind::Ident(name), ExprKind::Nil) | (ExprKind::Nil, ExprKind::Ident(name)) => {
                Some(name.clone())
            }
            _ => None,
        }
    }

    pub(super) fn extract_non_nil_vars(expr: &Expr, expect_true: bool) -> Vec<String> {
        match &expr.kind {
            ExprKind::Binary { op: BinaryOp::NotEq, left, right } if expect_true => {
                Self::extract_nil_check_var(left, right).into_iter().collect()
            }
            ExprKind::Binary { op: BinaryOp::Eq, left, right } if !expect_true => {
                Self::extract_nil_check_var(left, right).into_iter().collect()
            }
            ExprKind::Unary { op: UnaryOp::Not, expr: inner } => {
                Self::extract_non_nil_vars(inner, !expect_true)
            }
            ExprKind::Binary { op: BinaryOp::And, left, right } if expect_true => {
                let mut vars = Self::extract_non_nil_vars(left, true);
                vars.extend(Self::extract_non_nil_vars(right, true));
                vars
            }
            ExprKind::Binary { op: BinaryOp::Or, left, right } if !expect_true => {
                let mut vars = Self::extract_non_nil_vars(left, false);
                vars.extend(Self::extract_non_nil_vars(right, false));
                vars
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn apply_non_nil_narrowings(&mut self, names: &[String]) {
        for name in names {
            if let Some(cur_ty) = self.lookup_effective_type_unmarked(name)
                && let Some(non_nil) = Self::narrowed_non_nil_type(&cur_ty) {
                    self.narrowed.insert(name.clone(), non_nil);
                }
        }
    }

    pub(super) fn mark_widened(&mut self, name: &str, def_scope_idx: usize) {
        self.narrowed.remove(name);
        for (branch_depth, widened_set) in &mut self.widened_outer_vars {
            if def_scope_idx < *branch_depth {
                widened_set.insert(name.to_string());
            }
        }
    }

    pub(super) fn check_narrowed_block(&mut self, narrow_vars: &[String], block: &Block) -> Result<(), TypeError> {
        let saved_narrowed = self.narrowed.clone();
        let branch_depth = self.scopes.len();
        self.widened_outer_vars.push((branch_depth, HashSet::new()));
        self.apply_non_nil_narrowings(narrow_vars);

        self.enter_scope();
        let res = self.check_block(block);
        let exit_res = self.exit_scope();

        let (_, widened_set) = self.widened_outer_vars.pop().unwrap_or_default();
        self.narrowed = saved_narrowed;
        if !block_always_exits_branch(block) {
            for name in widened_set {
                self.narrowed.remove(&name);
            }
        }
        res?;
        exit_res?;
        Ok(())
    }

}
