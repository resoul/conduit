use super::*;

impl TypeChecker {
    pub(super) fn check_block(&mut self, block: &Block) -> Result<(), TypeError> {
        for stmt in &block.stmts {
            self.check_stmt(stmt)?;
        }
        Ok(())
    }

    pub(super) fn check_lvalue_root_mutability(
        &mut self,
        expr: &Expr,
        for_method: Option<&str>,
    ) -> Result<(String, usize, Type), TypeError> {
        let mut cur = expr;
        loop {
            match &cur.kind {
                ExprKind::Member { target, .. } => {
                    cur = target;
                }
                ExprKind::Ident(root_name) => {
                    if self.imports.contains_key(root_name) {
                        return Err(TypeError {
                            message: "Invalid assignment target".to_string(),
                            span: expr.span,
                        });
                    }
                    let closure_boundary = self.closure_boundary_scope_depth.last().copied();
                    if let Some((scope_idx, binding)) = self.lookup_variable_binding_mut(root_name) {
                        if closure_boundary.is_some_and(|boundary| scope_idx < boundary) {
                            return Err(TypeError {
                                message: format!(
                                    "Cannot mutate captured variable '{}' inside a closure",
                                    root_name
                                ),
                                span: expr.span,
                            });
                        }
                        if !binding.is_mutable {
                            return Err(TypeError {
                                message: format!("Cannot mutate immutable variable '{}'", root_name),
                                span: expr.span,
                            });
                        }
                        return Ok((root_name.clone(), scope_idx, binding.base_ty.clone()));
                    }
                    return Err(TypeError {
                        message: format!("Unknown variable '{}'", root_name),
                        span: expr.span,
                    });
                }
                _ => {
                    let msg = if let Some(m) = for_method {
                        format!("Cannot call mutating method '{}()' on non-variable expression", m)
                    } else {
                        "Invalid assignment target".to_string()
                    };
                    return Err(TypeError {
                        message: msg,
                        span: expr.span,
                    });
                }
            }
        }
    }

    fn bind_for_pattern(&mut self, pattern: &Pattern, ty: &Type) -> Result<(), TypeError> {
        match &pattern.kind {
            PatternKind::Wildcard => {
                if self.contains_error(ty) {
                    return Err(TypeError {
                        message: format!(
                            "Cannot discard potential error type '{}' with wildcard '_'",
                            ty
                        ),
                        span: pattern.span,
                    });
                }
                Ok(())
            }
            PatternKind::Ident(var_name) => {
                self.define_variable(var_name.clone(), ty.clone(), false, pattern.span);
                Ok(())
            }
            PatternKind::Tuple(sub_pats) => {
                let elem_types = match ty {
                    Type::Tuple(elems) => elems,
                    other => {
                        return Err(TypeError {
                            message: format!(
                                "Cannot destructure non-tuple type '{}' in 'for in' loop",
                                other
                            ),
                            span: pattern.span,
                        });
                    }
                };
                if sub_pats.len() != elem_types.len() {
                    return Err(TypeError {
                        message: format!(
                            "Tuple size mismatch in 'for in': expected {} elements, got {}",
                            elem_types.len(),
                            sub_pats.len()
                        ),
                        span: pattern.span,
                    });
                }
                for (sub_pat, elem_ty) in sub_pats.iter().zip(elem_types.iter()) {
                    self.bind_for_pattern(sub_pat, elem_ty)?;
                }
                Ok(())
            }
            _ => Err(TypeError {
                message: "Unsupported pattern in 'for in' loop".to_string(),
                span: pattern.span,
            }),
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Define { name, value } => self.check_define_stmt(name, value, stmt.span),
            StmtKind::DestructureDefine { pattern, value } => {
                self.check_destructure_define_stmt(pattern, value)
            }
            StmtKind::VarDecl { name, ty, init } => {
                self.check_var_decl_stmt(name, ty.as_ref(), init.as_ref(), stmt.span)
            }
            StmtKind::Assign { target, value } => self.check_assign_stmt(target, value),
            StmtKind::Return(opt_expr) => self.check_return_stmt(opt_expr.as_ref(), stmt.span),
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => self.check_if_stmt(condition, then_block, else_block.as_ref()),
            StmtKind::Guard {
                condition,
                else_block,
            } => self.check_guard_stmt(condition, else_block),
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            } => self.check_for_in_stmt(pattern, iterable, body),
            StmtKind::While { condition, body } => self.check_while_stmt(condition, body),
            StmtKind::Switch { expr, cases } => self.check_switch_stmt(expr, cases, stmt.span),
            StmtKind::Break => {
                if self.loop_depth == 0 {
                    return Err(TypeError {
                        message: "'break' outside of loop".to_string(),
                        span: stmt.span,
                    });
                }
                Ok(())
            }
            StmtKind::Continue => {
                if self.loop_depth == 0 {
                    return Err(TypeError {
                        message: "'continue' outside of loop".to_string(),
                        span: stmt.span,
                    });
                }
                Ok(())
            }
            StmtKind::Expr(expr) => self.check_expr_stmt(expr),
        }
    }

    fn check_define_stmt(
        &mut self,
        name: &str,
        value: &Expr,
        span: Span,
    ) -> Result<(), TypeError> {
        let val_ty = self.check_expr(value)?;
        self.define_variable(name.to_string(), val_ty, false, span);
        Ok(())
    }

    fn check_destructure_define_stmt(
        &mut self,
        pattern: &Pattern,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let val_ty = self.check_expr(value)?;
        match &pattern.kind {
            PatternKind::Tuple(pats) => {
                let elem_types = match val_ty {
                    Type::Tuple(elems) => elems,
                    other => {
                        return Err(TypeError {
                            message: format!("Cannot destructure non-tuple type '{}'", other),
                            span: pattern.span,
                        });
                    }
                };
                if pats.len() != elem_types.len() {
                    return Err(TypeError {
                        message: format!(
                            "Tuple size mismatch: expected {} elements, got {}",
                            elem_types.len(),
                            pats.len()
                        ),
                        span: pattern.span,
                    });
                }
                for (i, (pat, elem_ty)) in pats.iter().zip(elem_types.iter()).enumerate() {
                    match &pat.kind {
                        PatternKind::Wildcard => {
                            if self.contains_error(elem_ty) {
                                return Err(TypeError {
                                    message: format!(
                                        "Cannot discard element at index {} with error type '{}' using wildcard '_'",
                                        i, elem_ty
                                    ),
                                    span: pat.span,
                                });
                            }
                        }
                        PatternKind::Ident(var_name) => {
                            self.define_variable(
                                var_name.clone(),
                                elem_ty.clone(),
                                false,
                                pat.span,
                            );
                        }
                        _ => {
                            return Err(TypeError {
                                message: "Unsupported pattern in destructuring definition"
                                    .to_string(),
                                span: pat.span,
                            });
                        }
                    }
                }
            }
            _ => {
                return Err(TypeError {
                    message: "Destructuring definition requires a tuple pattern".to_string(),
                    span: pattern.span,
                });
            }
        }
        Ok(())
    }

    fn check_var_decl_stmt(
        &mut self,
        name: &str,
        ty: Option<&TypeExpr>,
        init: Option<&Expr>,
        span: Span,
    ) -> Result<(), TypeError> {
        let declared_ty = match ty {
            Some(t) => {
                let resolved = self.resolve_type_expr(t, span)?;
                self.check_generic_arity_in_type(&resolved, span)?;
                Some(resolved)
            }
            None => None,
        };
        if let Some(init_expr) = init {
            let init_ty = if let Some(ref target) = declared_ty {
                self.check_expr_with_expected(init_expr, target)?
            } else {
                self.check_expr(init_expr)?
            };
            if let Some(ref target) = declared_ty
                && !init_ty.is_assignable_to(target)
            {
                return Err(TypeError {
                    message: format!(
                        "Type mismatch: variable declared as {}, initialized with {}",
                        target, init_ty
                    ),
                    span: init_expr.span,
                });
            }
            self.define_variable(
                name.to_string(),
                declared_ty.unwrap_or(init_ty),
                true,
                span,
            );
        } else {
            let ty = declared_ty.ok_or_else(|| TypeError {
                message: format!(
                    "Variable '{}' must have an explicit type or default value",
                    name
                ),
                span,
            })?;
            self.define_variable(name.to_string(), ty, true, span);
        }
        Ok(())
    }

    fn check_assign_stmt(&mut self, target: &Expr, value: &Expr) -> Result<(), TypeError> {
        if let ExprKind::Index {
            target: container,
            index,
        } = &target.kind
        {
            let container_ty = self.check_expr(container)?;
            match container_ty {
                Type::List(_) => {
                    return Err(TypeError {
                        message: "Index assignment 'list[i] = value' is not supported".to_string(),
                        span: target.span,
                    });
                }
                Type::Map(key_ty, val_ty) => {
                    self.check_lvalue_root_mutability(container, None)?;
                    let index_ty = self.check_expr(index)?;
                    if !index_ty.is_assignable_to(&key_ty) {
                        return Err(TypeError {
                            message: format!("Map key must be '{}', got '{}'", key_ty, index_ty),
                            span: index.span,
                        });
                    }
                    let rhs_ty = self.check_expr_with_expected(value, &val_ty)?;
                    if !rhs_ty.is_assignable_to(&val_ty) {
                        return Err(TypeError {
                            message: format!(
                                "Cannot assign value of type {} to target of type {}",
                                rhs_ty, val_ty
                            ),
                            span: value.span,
                        });
                    }
                    return Ok(());
                }
                other => {
                    return Err(TypeError {
                        message: format!(
                            "Cannot index into non-list type '{}' (only List and Map support indexing)",
                            other
                        ),
                        span: container.span,
                    });
                }
            }
        }

        let (root_name, root_scope_idx, root_base_ty) =
            self.check_lvalue_root_mutability(target, None)?;
        let is_direct_ident = matches!(target.kind, ExprKind::Ident(_));
        let target_ty = if is_direct_ident {
            root_base_ty
        } else {
            self.check_expr(target)?
        };
        let value_ty = self.check_expr_with_expected(value, &target_ty)?;
        if !value_ty.is_assignable_to(&target_ty) {
            return Err(TypeError {
                message: format!(
                    "Cannot assign value of type {} to target of type {}",
                    value_ty, target_ty
                ),
                span: value.span,
            });
        }
        if is_direct_ident {
            if Self::type_may_be_nil(&value_ty) {
                self.mark_widened(&root_name, root_scope_idx);
            } else if Self::narrowed_non_nil_type(&target_ty).is_some() {
                self.narrowed.insert(root_name, value_ty);
            }
        }
        Ok(())
    }

    fn check_return_stmt(
        &mut self,
        opt_expr: Option<&Expr>,
        span: Span,
    ) -> Result<(), TypeError> {
        let actual_ty = if let Some(expr) = opt_expr {
            if let Some(expected) = self.current_return_type.clone() {
                self.check_expr_with_expected(expr, &expected)?
            } else {
                self.check_expr(expr)?
            }
        } else {
            Type::Void
        };

        if let Some(ref expected) = self.current_return_type
            && !actual_ty.is_assignable_to(expected)
        {
            return Err(TypeError {
                message: format!(
                    "Type mismatch: function expects return type {}, but got {}",
                    expected, actual_ty
                ),
                span,
            });
        }
        Ok(())
    }

    fn check_if_stmt(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
    ) -> Result<(), TypeError> {
        let cond_ty = self.check_expr(condition)?;
        if cond_ty != Type::Bool {
            return Err(TypeError {
                message: format!("Condition in 'if' must be Bool, got {}", cond_ty),
                span: condition.span,
            });
        }
        let then_vars = Self::extract_non_nil_vars(condition, true);
        let else_vars = Self::extract_non_nil_vars(condition, false);

        self.check_narrowed_block(&then_vars, then_block)?;

        if let Some(eb) = else_block {
            let saved_after_then = self.narrowed.clone();
            let branch_depth = self.scopes.len();
            self.widened_outer_vars.push((branch_depth, HashSet::new()));
            self.apply_non_nil_narrowings(&else_vars);
            self.enter_scope();
            let res = self.check_block(eb);
            let exit_res = self.exit_scope();
            let (_, widened_set) = self.widened_outer_vars.pop().unwrap_or_default();
            self.narrowed = saved_after_then;
            if !block_always_exits_branch(eb) {
                for name in widened_set {
                    self.narrowed.remove(&name);
                }
            }
            res?;
            exit_res?;
        }
        Ok(())
    }

    fn check_guard_stmt(&mut self, condition: &Expr, else_block: &Block) -> Result<(), TypeError> {
        let cond_ty = self.check_expr(condition)?;
        if cond_ty != Type::Bool {
            return Err(TypeError {
                message: format!("Condition in 'guard' must be Bool, got {}", cond_ty),
                span: condition.span,
            });
        }
        let else_vars = Self::extract_non_nil_vars(condition, false);
        self.check_narrowed_block(&else_vars, else_block)?;

        if !block_always_exits_branch(else_block) {
            return Err(TypeError {
                message: "Guard 'else' block must terminate with a 'return', 'break', or 'continue' statement".to_string(),
                span: else_block.span,
            });
        }

        let guard_vars = Self::extract_non_nil_vars(condition, true);
        self.apply_non_nil_narrowings(&guard_vars);
        Ok(())
    }

    fn check_for_in_stmt(
        &mut self,
        pattern: &Pattern,
        iterable: &Expr,
        body: &Block,
    ) -> Result<(), TypeError> {
        let iter_ty = self.check_expr(iterable)?;
        let item_ty = match iter_ty {
            Type::List(inner) => *inner,
            Type::Map(k, v) => Type::Tuple(vec![*k, *v]),
            other => {
                return Err(TypeError {
                    message: format!("Expected List or Map in 'for in' loop, got {}", other),
                    span: iterable.span,
                });
            }
        };

        let saved_narrowed = self.narrowed.clone();
        let branch_depth = self.scopes.len();
        self.widened_outer_vars.push((branch_depth, HashSet::new()));

        self.loop_depth += 1;
        self.enter_scope();
        let bind_res = self.bind_for_pattern(pattern, &item_ty);
        let body_res = if bind_res.is_ok() {
            self.check_block(body)
        } else {
            Ok(())
        };
        let exit_res = self.exit_scope();
        self.loop_depth -= 1;

        let (_, widened_set) = self.widened_outer_vars.pop().unwrap_or_default();
        self.narrowed = saved_narrowed;
        for name in widened_set {
            self.narrowed.remove(&name);
        }

        bind_res?;
        body_res?;
        exit_res?;
        Ok(())
    }

    fn check_while_stmt(&mut self, condition: &Expr, body: &Block) -> Result<(), TypeError> {
        let cond_ty = self.check_expr(condition)?;
        if cond_ty != Type::Bool {
            return Err(TypeError {
                message: format!("Condition in 'while' must be Bool, got {}", cond_ty),
                span: condition.span,
            });
        }

        let while_vars = Self::extract_non_nil_vars(condition, true);
        let saved_narrowed = self.narrowed.clone();
        let branch_depth = self.scopes.len();
        self.widened_outer_vars.push((branch_depth, HashSet::new()));
        self.apply_non_nil_narrowings(&while_vars);

        self.loop_depth += 1;
        self.enter_scope();
        let body_res = self.check_block(body);
        let exit_res = self.exit_scope();
        self.loop_depth -= 1;

        let (_, widened_set) = self.widened_outer_vars.pop().unwrap_or_default();
        self.narrowed = saved_narrowed;
        for name in widened_set {
            self.narrowed.remove(&name);
        }

        body_res?;
        exit_res?;
        Ok(())
    }

    fn check_switch_stmt(
        &mut self,
        expr: &Expr,
        cases: &[SwitchCase],
        span: Span,
    ) -> Result<(), TypeError> {
        let expr_ty = self.check_expr(expr)?;
        let mut remaining = match Type::union(vec![expr_ty.clone()]) {
            Type::Union(variants) => variants,
            other => vec![other],
        };

        let saved_before_switch = self.narrowed.clone();
        let mut widened_in_switch = HashSet::new();

        for case in cases {
            self.narrowed = saved_before_switch.clone();
            let branch_depth = self.scopes.len();
            self.widened_outer_vars.push((branch_depth, HashSet::new()));
            self.enter_scope();
            self.check_switch_case_pattern(&case.pattern, &expr_ty, &mut remaining)?;

            let body_res = self.check_block(&case.body);
            let exit_res = self.exit_scope();
            let (_, case_widened) = self.widened_outer_vars.pop().unwrap_or_default();
            if !block_always_exits_branch(&case.body) {
                widened_in_switch.extend(case_widened);
            }
            body_res?;
            exit_res?;
        }

        self.narrowed = saved_before_switch;
        for name in widened_in_switch {
            self.narrowed.remove(&name);
        }

        if !remaining.is_empty() {
            let missing = remaining
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(TypeError {
                message: format!("Non-exhaustive switch: missing cases for {}", missing),
                span,
            });
        }
        Ok(())
    }

    fn check_switch_case_pattern(
        &mut self,
        pattern: &Pattern,
        expr_ty: &Type,
        remaining: &mut Vec<Type>,
    ) -> Result<(), TypeError> {
        match &pattern.kind {
            PatternKind::Nil => {
                let idx = remaining
                    .iter()
                    .position(|t| t == &Type::Nil)
                    .ok_or_else(|| TypeError {
                        message: format!(
                            "Redundant or impossible case 'nil' for type '{}'",
                            expr_ty
                        ),
                        span: pattern.span,
                    })?;
                remaining.remove(idx);
            }
            PatternKind::Type { name, binding } => {
                let mod_id = self.current_module_id();
                let target_type_id = self.resolve_target_type_id(
                    name,
                    &mod_id,
                    &self.imports,
                    None,
                    pattern.span,
                )?;
                let custom_ty = match self.resolve_type_name_in_ctx(
                    name,
                    &mod_id,
                    &self.imports,
                    &self.current_type_params,
                    None,
                    pattern.span,
                ) {
                    Ok(t) => Some(t),
                    Err(_) if name == "Map" => None,
                    Err(e) => return Err(e),
                };
                let idx = remaining
                    .iter()
                    .position(|t| match t {
                        Type::Generic { type_id, .. } if type_id == &target_type_id => true,
                        Type::Map(_, _) if name == "Map" => true,
                        other => custom_ty.as_ref() == Some(other),
                    })
                    .ok_or_else(|| TypeError {
                        message: format!(
                            "Redundant or impossible case '{}(...)' for type '{}'",
                            name, expr_ty
                        ),
                        span: pattern.span,
                    })?;
                let matched_ty = remaining.remove(idx);
                self.define_used_variable(binding.clone(), matched_ty, false, pattern.span);
            }
            PatternKind::Tuple(pats) => {
                let mut matched_indices = Vec::new();
                let mut matched_tuples = Vec::new();
                for (i, t) in remaining.iter().enumerate() {
                    if let Type::Tuple(elems) = t
                        && elems.len() == pats.len()
                    {
                        matched_indices.push(i);
                        matched_tuples.push(elems.clone());
                    }
                }
                if matched_indices.is_empty() {
                    return Err(TypeError {
                        message: format!(
                            "Redundant or impossible tuple pattern for type '{}'",
                            expr_ty
                        ),
                        span: pattern.span,
                    });
                }
                for &idx in matched_indices.iter().rev() {
                    remaining.remove(idx);
                }

                for (pos, sub_pat) in pats.iter().enumerate() {
                    let pos_types: Vec<Type> =
                        matched_tuples.iter().map(|t| t[pos].clone()).collect();
                    let combined_ty = Type::union(pos_types);
                    match &sub_pat.kind {
                        PatternKind::Ident(var_name) => {
                            self.define_variable(
                                var_name.clone(),
                                combined_ty,
                                false,
                                sub_pat.span,
                            );
                        }
                        PatternKind::Wildcard => {}
                        _ => {
                            return Err(TypeError {
                                message: "Unsupported nested pattern in switch tuple".to_string(),
                                span: sub_pat.span,
                            });
                        }
                    }
                }
            }
            PatternKind::Ident(binding) => {
                if remaining.is_empty() {
                    return Err(TypeError {
                        message: format!("Redundant case '{}' for type '{}'", binding, expr_ty),
                        span: pattern.span,
                    });
                }
                let bound_ty = Type::union(remaining.clone());
                remaining.clear();
                self.define_variable(binding.clone(), bound_ty, false, pattern.span);
            }
            PatternKind::Wildcard => {
                if remaining.is_empty() {
                    return Err(TypeError {
                        message: format!("Redundant wildcard case for type '{}'", expr_ty),
                        span: pattern.span,
                    });
                }
                remaining.clear();
            }
        }
        Ok(())
    }

    fn check_expr_stmt(&mut self, expr: &Expr) -> Result<(), TypeError> {
        let ty = self.check_expr(expr)?;
        if self.contains_error(&ty) {
            return Err(TypeError {
                message: format!(
                    "Expression result of type '{}' contains an error and must not be discarded",
                    ty
                ),
                span: expr.span,
            });
        }
        Ok(())
    }

}

fn block_always_returns_inner(block: &Block, allow_loop_ctrl: bool) -> bool {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Return(_) => return true,
            StmtKind::Break | StmtKind::Continue if allow_loop_ctrl => return true,
            StmtKind::If {
                then_block,
                else_block: Some(else_b),
                ..
            } => {
                if block_always_returns_inner(then_block, allow_loop_ctrl)
                    && block_always_returns_inner(else_b, allow_loop_ctrl)
                {
                    return true;
                }
            }
            StmtKind::Switch { cases, .. }
                if !cases.is_empty()
                    && cases
                        .iter()
                        .all(|c| block_always_returns_inner(&c.body, allow_loop_ctrl))
                => {
                    return true;
                }
            _ => {}
        }
    }
    false
}

pub(super) fn block_always_returns(block: &Block) -> bool {
    block_always_returns_inner(block, false)
}

pub(super) fn block_always_exits_branch(block: &Block) -> bool {
    block_always_returns_inner(block, true)
}
