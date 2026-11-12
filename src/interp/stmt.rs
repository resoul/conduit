use super::*;

impl Interpreter {
    pub(super) fn eval_block(&mut self, block: &Block) -> Result<ControlFlow, RuntimeError> {
        self.enter_scope();
        for s in &block.stmts {
            match self.eval_stmt(s)? {
                ControlFlow::Next => {}
                flow => {
                    self.exit_scope();
                    return Ok(flow);
                }
            }
        }
        self.exit_scope();
        Ok(ControlFlow::Next)
    }

    #[inline]
    pub(super) fn eval_stmt(&mut self, stmt: &Stmt) -> Result<ControlFlow, RuntimeError> {
        match &stmt.kind {
            StmtKind::Define { name, value } => self.eval_define_stmt(name, value),
            StmtKind::DestructureDefine { pattern, value } => {
                self.eval_destructure_define_stmt(pattern, value)
            }
            StmtKind::VarDecl { name, init, .. } => {
                self.eval_var_decl_stmt(name, init.as_ref())
            }
            StmtKind::Assign { target, value } => self.eval_assign_stmt(target, value),
            StmtKind::Return(opt_expr) => self.eval_return_stmt(opt_expr.as_ref()),
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => self.eval_if_stmt(condition, then_block, else_block.as_ref()),
            StmtKind::Guard {
                condition,
                else_block,
            } => self.eval_guard_stmt(condition, else_block),
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            } => self.eval_for_in_stmt(pattern, iterable, body),
            StmtKind::While { condition, body } => self.eval_while_stmt(condition, body),
            StmtKind::Switch { expr, cases } => self.eval_switch_stmt(expr, cases),
            StmtKind::Break => Ok(ControlFlow::Break),
            StmtKind::Continue => Ok(ControlFlow::Continue),
            StmtKind::Expr(expr) => match self.eval_expr(expr) {
                Ok(_) => Ok(ControlFlow::Next),
                Err(InterpFlow::Return(ret)) => Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => Err(err),
            },
        }
    }

    #[inline]
    fn eval_define_stmt(&mut self, name: &str, value: &Expr) -> Result<ControlFlow, RuntimeError> {
        let val = match self.eval_expr(value) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        self.define_variable(name.to_string(), val);
        Ok(ControlFlow::Next)
    }

    fn eval_destructure_define_stmt(
        &mut self,
        pattern: &Pattern,
        value: &Expr,
    ) -> Result<ControlFlow, RuntimeError> {
        let val = match self.eval_expr(value) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        if let PatternKind::Tuple(pats) = &pattern.kind
            && let Value::Tuple(items) = val
        {
            for (pat, item) in pats.iter().zip(items) {
                match &pat.kind {
                    PatternKind::Ident(var_name) => {
                        self.define_variable(var_name.clone(), item);
                    }
                    PatternKind::Wildcard => {}
                    _ => {}
                }
            }
        }
        Ok(ControlFlow::Next)
    }

    #[inline]
    fn eval_var_decl_stmt(
        &mut self,
        name: &str,
        init: Option<&Expr>,
    ) -> Result<ControlFlow, RuntimeError> {
        let val = if let Some(init_expr) = init {
            match self.eval_expr(init_expr) {
                Ok(v) => v,
                Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => return Err(err),
            }
        } else {
            Value::Nil
        };
        self.define_variable(name.to_string(), val);
        Ok(ControlFlow::Next)
    }

    fn eval_assign_stmt(
        &mut self,
        target: &Expr,
        value: &Expr,
    ) -> Result<ControlFlow, RuntimeError> {
        if let ExprKind::Index {
            target: container,
            index,
        } = &target.kind
        {
            let key_val = match self.eval_expr(index) {
                Ok(v) => v,
                Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => return Err(err),
            };
            let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                message: format!(
                    "Map key must be String, Int, or Bool, got {}",
                    key_val.type_name()
                ),
                span: index.span,
            })?;
            let rhs = match self.eval_expr(value) {
                Ok(v) => v,
                Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => return Err(err),
            };
            let target_span = target.span;
            self.mutate_place(container, move |slot| {
                if let Value::Map(map) = slot {
                    map.set(key, rhs);
                    Ok(Value::Void)
                } else {
                    Err(RuntimeError {
                        message: format!(
                            "Index assignment is only supported on Map, got {}",
                            slot.type_name()
                        ),
                        span: target_span,
                    })
                }
            })?;
            return Ok(ControlFlow::Next);
        }

        let val = match self.eval_expr(value) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        if let ExprKind::Ident(ref name) = target.kind {
            self.assign_variable(name, val, target.span)?;
        } else {
            self.mutate_place(target, move |slot| {
                *slot = val;
                Ok(Value::Void)
            })?;
        }
        Ok(ControlFlow::Next)
    }

    #[inline]
    fn eval_return_stmt(&mut self, opt_expr: Option<&Expr>) -> Result<ControlFlow, RuntimeError> {
        let val = if let Some(expr) = opt_expr {
            match self.eval_expr(expr) {
                Ok(v) => v,
                Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => return Err(err),
            }
        } else {
            Value::Void
        };
        Ok(ControlFlow::Return(val))
    }

    fn eval_if_stmt(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
    ) -> Result<ControlFlow, RuntimeError> {
        let cond_val = match self.eval_expr(condition) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        if cond_val.is_truthy() {
            self.eval_block(then_block)
        } else if let Some(eb) = else_block {
            self.eval_block(eb)
        } else {
            Ok(ControlFlow::Next)
        }
    }

    fn eval_guard_stmt(
        &mut self,
        condition: &Expr,
        else_block: &Block,
    ) -> Result<ControlFlow, RuntimeError> {
        let cond_val = match self.eval_expr(condition) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        if !cond_val.is_truthy() {
            self.eval_block(else_block)
        } else {
            Ok(ControlFlow::Next)
        }
    }

    fn eval_for_in_stmt(
        &mut self,
        pattern: &Pattern,
        iterable: &Expr,
        body: &Block,
    ) -> Result<ControlFlow, RuntimeError> {
        let iter_val = match self.eval_expr(iterable) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        let items = match iter_val {
            Value::List(items) => items,
            Value::Map(map) => map.entry_tuples(),
            _ => Vec::new(),
        };
        for item in items {
            self.enter_scope();
            self.bind_for_pattern(pattern, item);
            let mut flow = ControlFlow::Next;
            for s in &body.stmts {
                match self.eval_stmt(s)? {
                    ControlFlow::Next => {}
                    other => {
                        flow = other;
                        break;
                    }
                }
            }
            self.exit_scope();
            match flow {
                ControlFlow::Break => break,
                ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                _ => {}
            }
        }
        Ok(ControlFlow::Next)
    }

    fn eval_while_stmt(
        &mut self,
        condition: &Expr,
        body: &Block,
    ) -> Result<ControlFlow, RuntimeError> {
        loop {
            let cond_val = match self.eval_expr(condition) {
                Ok(v) => v,
                Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
                Err(InterpFlow::Error(err)) => return Err(err),
            };
            if !cond_val.is_truthy() {
                break;
            }
            self.enter_scope();
            let mut flow = ControlFlow::Next;
            for s in &body.stmts {
                match self.eval_stmt(s)? {
                    ControlFlow::Next => {}
                    other => {
                        flow = other;
                        break;
                    }
                }
            }
            self.exit_scope();
            match flow {
                ControlFlow::Break => break,
                ControlFlow::Continue => continue,
                ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                ControlFlow::Next => {}
            }
        }
        Ok(ControlFlow::Next)
    }

    fn eval_switch_stmt(
        &mut self,
        expr: &Expr,
        cases: &[SwitchCase],
    ) -> Result<ControlFlow, RuntimeError> {
        let val = match self.eval_expr(expr) {
            Ok(v) => v,
            Err(InterpFlow::Return(ret)) => return Ok(ControlFlow::Return(ret)),
            Err(InterpFlow::Error(err)) => return Err(err),
        };
        for case in cases {
            if let Some(bindings) = self.match_pattern(&case.pattern, &val) {
                self.enter_scope();
                for (k, v) in bindings {
                    self.define_variable(k, v);
                }
                let flow = self.eval_block(&case.body)?;
                self.exit_scope();
                match flow {
                    ControlFlow::Next => return Ok(ControlFlow::Next),
                    other => return Ok(other),
                }
            }
        }
        Ok(ControlFlow::Next)
    }

}
