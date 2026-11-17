use crate::frontend::ast::*;
use crate::frontend::types::Type;
use crate::ir::lower::{FnLowerer, LoopPrealloc};

impl<'a, 'b> FnLowerer<'a, 'b> {
    pub(crate) fn prealloc_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.prealloc_stmt(stmt);
        }
    }

    fn prealloc_stmt(&mut self, stmt: &Stmt) {
        let key = stmt as *const Stmt as usize;
        match &stmt.kind {
            StmtKind::Define { value, .. } => {
                self.collect_loop_consts_expr(value);
                let slot = self.prealloc_slot_for_ty(&value.ty);
                self.stmt_slots.insert(key, slot);
            }
            StmtKind::VarDecl { ty, init, .. } => {
                if let Some(e) = init {
                    self.collect_loop_consts_expr(e);
                }
                let var_ty = if let Some(te) = ty {
                    self.resolve_type(te).unwrap_or(Type::Any)
                } else if let Some(e) = init {
                    e.ty.clone()
                } else {
                    Type::Any
                };
                let slot = self.prealloc_slot_for_ty(&var_ty);
                self.stmt_slots.insert(key, slot);
            }
            StmtKind::DestructureDefine { pattern, value } => {
                self.collect_loop_consts_expr(value);
                self.prealloc_pattern(pattern, &value.ty);
            }
            StmtKind::Assign { target, value } => {
                self.collect_loop_consts_expr(target);
                self.collect_loop_consts_expr(value);
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => self.collect_loop_consts_expr(e),
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::If { condition, then_block, else_block } => {
                self.collect_loop_consts_expr(condition);
                self.prealloc_block(then_block);
                if let Some(eb) = else_block {
                    self.prealloc_block(eb);
                }
            }
            StmtKind::Guard { condition, else_block } => {
                self.collect_loop_consts_expr(condition);
                self.prealloc_block(else_block);
            }
            StmtKind::ForIn { pattern, iterable, body } => {
                self.collect_loop_consts_expr(iterable);
                let elem_ty = match &iterable.ty {
                    Type::List(inner) => (**inner).clone(),
                    Type::Map(k, v) => Type::Tuple(vec![(**k).clone(), (**v).clone()]),
                    _ => Type::Int,
                };
                let idx_s = self.alloc_local_s();
                let aux_s = self.alloc_local_s();
                let list_o = self.alloc_local_o();
                let elem_reg = if elem_ty.is_scalar() {
                    self.alloc_local_s()
                } else {
                    self.alloc_local_o()
                };
                self.loop_slots.insert(key, LoopPrealloc { idx_s, aux_s, elem_reg, list_o });
                self.prealloc_pattern(pattern, &elem_ty);
                self.loop_depth += 1;
                self.prealloc_block(body);
                self.loop_depth -= 1;
            }
            StmtKind::While { condition, body } => {
                self.collect_loop_consts_expr(condition);
                let idx_s = self.alloc_local_s();
                let aux_s = self.alloc_local_s();
                self.loop_slots.insert(key, LoopPrealloc { idx_s, aux_s, elem_reg: 0, list_o: 0 });
                self.loop_depth += 1;
                self.prealloc_block(body);
                self.loop_depth -= 1;
            }
            StmtKind::Switch { expr, cases } => {
                self.collect_loop_consts_expr(expr);
                for case in cases {
                    self.prealloc_pattern(&case.pattern, &expr.ty);
                    self.prealloc_block(&case.body);
                }
            }
        }
    }

    fn prealloc_pattern(&mut self, pat: &Pattern, ty: &Type) {
        let pkey = pat as *const Pattern as usize;
        match (&pat.kind, ty) {
            (PatternKind::Wildcard | PatternKind::Nil, _) => {}
            (PatternKind::Ident(name), Type::Enum(_)) if name.contains('.') => {}
            (PatternKind::Ident(name), _) => {
                let slot = self.prealloc_slot_for_ty(ty);
                self.pat_slots.insert((pkey, name.clone()), slot);
            }
            (PatternKind::Tuple(pats), Type::Tuple(tys)) if pats.len() == tys.len() => {
                for (p, t) in pats.iter().zip(tys.iter()) {
                    self.prealloc_pattern(p, t);
                }
            }
            (PatternKind::Tuple(pats), _) => {
                for p in pats {
                    self.prealloc_pattern(p, &Type::Any);
                }
            }
            (PatternKind::Type { name, binding }, _) => {
                if binding != "_" {
                    let resolved = crate::frontend::monomorph::resolve_switch_pattern_type(
                        name,
                        ty,
                        self.mc.checked,
                    )
                    .unwrap_or(Type::Any);
                    let slot = self.prealloc_slot_for_ty(&resolved);
                    self.pat_slots.insert((pkey, binding.clone()), slot);
                }
            }
        }
    }

    fn collect_loop_consts_expr(&mut self, expr: &Expr) {
        if self.loop_depth == 0 {
            return;
        }
        match &expr.kind {
            ExprKind::Float(v) => {
                let bits = v.to_bits();
                if !self.hoisted_floats.contains_key(&bits) {
                    let reg = self.alloc_local_s();
                    self.hoisted_floats.insert(bits, reg);
                    self.pending_hoist_floats.push(bits);
                }
            }
            ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => {
                if let ExprKind::Float(v) = inner.kind {
                    let bits = (-v).to_bits();
                    if !self.hoisted_floats.contains_key(&bits) {
                        let reg = self.alloc_local_s();
                        self.hoisted_floats.insert(bits, reg);
                        self.pending_hoist_floats.push(bits);
                    }
                } else {
                    self.collect_loop_consts_expr(inner);
                }
            }
            ExprKind::String(s) => {
                if !self.hoisted_strs.contains_key(s) {
                    let reg = self.alloc_local_o();
                    self.hoisted_strs.insert(s.clone(), reg);
                    self.pending_hoist_strs.push(s.clone());
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.collect_loop_consts_expr(left);
                self.collect_loop_consts_expr(right);
            }
            ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => {
                self.collect_loop_consts_expr(inner);
            }
            ExprKind::List(elems) | ExprKind::Tuple(elems) => {
                for e in elems {
                    self.collect_loop_consts_expr(e);
                }
            }
            ExprKind::Call { callee, args, named_args, .. } => {
                self.collect_loop_consts_expr(callee);
                for a in args {
                    self.collect_loop_consts_expr(a);
                }
                for na in named_args {
                    self.collect_loop_consts_expr(&na.value);
                }
            }
            ExprKind::Member { target, .. } | ExprKind::OptionalMember { target, .. } => {
                self.collect_loop_consts_expr(target);
            }
            ExprKind::Index { target, index } => {
                self.collect_loop_consts_expr(target);
                self.collect_loop_consts_expr(index);
            }
            ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Nil | ExprKind::Ident(_) | ExprKind::Closure { .. } => {}
        }
    }
}
