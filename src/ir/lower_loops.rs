use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::types::Type;
use crate::ir::lower::{FnLowerer, LoopPatch, VarSlot};
use crate::vm::opcode::Op;

impl<'a, 'b> FnLowerer<'a, 'b> {
    pub(crate) fn try_lower_inplace_scalar_assign(
        &mut self,
        var_name: &str,
        dst: u16,
        value: &Expr,
    ) -> Result<bool, String> {
        let ExprKind::Binary { op, left, right } = &value.kind else {
            return Ok(false);
        };
        let ExprKind::Ident(lhs_name) = &left.kind else {
            return Ok(false);
        };
        if lhs_name != var_name {
            return Ok(false);
        }
        if value.ty.is_int() {
            match op {
                BinaryOp::Add => {
                    if let Some(imm) = as_i16_lit(right) {
                        self.ops.push(Op::AddIntImm { dst, a: dst, imm });
                        return Ok(true);
                    }
                    let rhs_r = self.lower_scalar(right)?;
                    self.ops.push(Op::AddInt { dst, a: dst, b: rhs_r });
                    return Ok(true);
                }
                BinaryOp::Sub => {
                    if let Some(imm) = as_neg_i16_lit(right) {
                        self.ops.push(Op::AddIntImm { dst, a: dst, imm });
                        return Ok(true);
                    }
                    let rhs_r = self.lower_scalar(right)?;
                    self.ops.push(Op::SubInt { dst, a: dst, b: rhs_r });
                    return Ok(true);
                }
                BinaryOp::Mul => {
                    let rhs_r = self.lower_scalar(right)?;
                    self.ops.push(Op::MulInt { dst, a: dst, b: rhs_r });
                    return Ok(true);
                }
                BinaryOp::Div => {
                    let rhs_r = self.lower_scalar(right)?;
                    self.ops.push(Op::DivInt { dst, a: dst, b: rhs_r });
                    return Ok(true);
                }
                BinaryOp::Mod => {
                    if let Some(imm) = as_i16_lit(right) {
                        self.ops.push(Op::ModIntImm { dst, a: dst, imm });
                        return Ok(true);
                    }
                    let rhs_r = self.lower_scalar(right)?;
                    self.ops.push(Op::ModInt { dst, a: dst, b: rhs_r });
                    return Ok(true);
                }
                _ => return Ok(false),
            }
        } else if value.ty.is_float() {
            match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                    let rhs_r = self.lower_scalar(right)?;
                    let inst = match op {
                        BinaryOp::Add => Op::AddFloat { dst, a: dst, b: rhs_r },
                        BinaryOp::Sub => Op::SubFloat { dst, a: dst, b: rhs_r },
                        BinaryOp::Mul => Op::MulFloat { dst, a: dst, b: rhs_r },
                        BinaryOp::Div => Op::DivFloat { dst, a: dst, b: rhs_r },
                        _ => unreachable!(),
                    };
                    self.ops.push(inst);
                    return Ok(true);
                }
                _ => return Ok(false),
            }
        }
        Ok(false)
    }

    pub(crate) fn lower_for_in(
        &mut self,
        skey: usize,
        pattern: &Pattern,
        iterable: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        let lp = self.loop_slots[&skey];
        if let ExprKind::Binary { op: BinaryOp::Range, left, right } = &iterable.kind {
            let idx_s = match &pattern.kind {
                PatternKind::Ident(name) => {
                    let pkey = pattern as *const Pattern as usize;
                    self.pat_slots[&(pkey, name.clone())].reg
                }
                _ => lp.idx_s,
            };
            let start_r = self.lower_scalar(left)?;
            if start_r != idx_s {
                self.ops.push(Op::MoveScalar { dst: idx_s, src: start_r });
            }
            let end_r = self.lower_scalar(right)?;
            if end_r != lp.aux_s {
                self.ops.push(Op::MoveScalar { dst: lp.aux_s, src: end_r });
            }
            let cond_t = self.alloc_temp_s();
            self.ops.push(Op::LtInt { dst: cond_t, a: idx_s, b: lp.aux_s });
            let jf_pc = self.ops.len();
            self.ops.push(Op::JumpIfFalse { cond: cond_t, target: 0 });
            let top_pc = self.ops.len() as u32;

            self.scopes.push(HashMap::new());
            if let PatternKind::Ident(name) = &pattern.kind {
                self.bind_var(name.clone(), VarSlot { is_scalar: true, reg: idx_s }, Type::Int);
            }
            self.loops.push(LoopPatch { breaks: Vec::new(), continues: Vec::new() });
            self.loop_depth += 1;
            for s in &body.stmts {
                self.lower_stmt(s)?;
            }
            self.loop_depth -= 1;
            let patch = self.loops.pop().unwrap();
            self.scopes.pop();

            let step_pc = self.ops.len() as u32;
            self.ops.push(Op::ForRangeStep { idx_reg: idx_s, limit_reg: lp.aux_s, target: top_pc });
            let done_pc = self.ops.len() as u32;
            patch_jump(&mut self.ops[jf_pc], done_pc);
            for pc in patch.breaks {
                patch_jump(&mut self.ops[pc], done_pc);
            }
            for pc in patch.continues {
                patch_jump(&mut self.ops[pc], step_pc);
            }
            return Ok(());
        }

        let (list_obj, elem_ty) = if let Type::Map(k, v) = &iterable.ty {
            let map_obj = self.lower_obj(iterable)?;
            let aux = (crate::ir::lower_native::elem_tag_for(k) << 4)
                | crate::ir::lower_native::elem_tag_for(v);
            self.ops.push(Op::ExtOp {
                kind: crate::vm::opcode::EXT_MAP_ENTRIES,
                aux,
                a: lp.list_o,
                b: map_obj,
                c: 0,
            });
            (lp.list_o, Type::Tuple(vec![(**k).clone(), (**v).clone()]))
        } else {
            let list_obj = if let ExprKind::Ident(var_name) = &iterable.kind
                && let Some((slot, _)) = self.lookup_local(var_name)
            {
                slot.reg
            } else {
                let r = self.lower_obj(iterable)?;
                if r != lp.list_o {
                    self.ops.push(Op::MoveObj { dst_obj: lp.list_o, src: r });
                }
                lp.list_o
            };
            let elem_ty = match &iterable.ty {
                Type::List(inner) => (**inner).clone(),
                _ => Type::Any,
            };
            (list_obj, elem_ty)
        };
        let elem_reg = match &pattern.kind {
            PatternKind::Ident(name) => {
                let pkey = pattern as *const Pattern as usize;
                self.pat_slots[&(pkey, name.clone())].reg
            }
            _ => lp.elem_reg,
        };

        self.ops.push(Op::LoadImmInt { dst: lp.idx_s, imm: 0 });
        let head_pc = self.ops.len();
        let is_scalar = elem_ty.is_scalar();
        if is_scalar {
            self.ops.push(Op::ForListInt { list_obj, idx_s: lp.idx_s, elem_s: elem_reg, exit_target: 0 });
        } else {
            self.ops.push(Op::ForListObj { list_obj, idx_s: lp.idx_s, elem_obj: elem_reg, exit_target: 0 });
        }
        let top_pc = self.ops.len() as u32;

        self.scopes.push(HashMap::new());
        self.lower_pattern_bind(pattern, &elem_ty, elem_reg, is_scalar)?;
        self.loops.push(LoopPatch { breaks: Vec::new(), continues: Vec::new() });
        self.loop_depth += 1;
        for s in &body.stmts {
            self.lower_stmt(s)?;
        }
        self.loop_depth -= 1;
        let patch = self.loops.pop().unwrap();
        self.scopes.pop();

        let step_pc = self.ops.len() as u32;
        if is_scalar {
            self.ops.push(Op::ForListIntStep { list_obj, idx_s: lp.idx_s, elem_s: elem_reg, target: top_pc });
        } else {
            // ForListObj already increments idx_s on entry, so jump directly back to head_pc.
            self.ops.push(Op::Jump { target: head_pc as u32 });
        }
        let done_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[head_pc], done_pc);
        for pc in patch.breaks {
            patch_jump(&mut self.ops[pc], done_pc);
        }
        let cont_target = if is_scalar { step_pc } else { head_pc as u32 };
        for pc in patch.continues {
            patch_jump(&mut self.ops[pc], cont_target);
        }
        Ok(())
    }

    pub(crate) fn try_lower_counting_while(
        &mut self,
        skey: usize,
        condition: &Expr,
        body: &Block,
    ) -> Result<bool, String> {
        let ExprKind::Binary { op: BinaryOp::Lt, left, right: limit_expr } = &condition.kind else {
            return Ok(false);
        };
        let ExprKind::Ident(idx_name) = &left.kind else {
            return Ok(false);
        };
        let Some((idx_slot, idx_ty)) = self.lookup_local(idx_name) else {
            return Ok(false);
        };
        if !idx_slot.is_scalar || !idx_ty.is_int() {
            return Ok(false);
        }
        let Some(last_stmt) = body.stmts.last() else {
            return Ok(false);
        };
        if !is_plus_one_assign(last_stmt, idx_name) {
            return Ok(false);
        }
        let body_prefix = &body.stmts[..body.stmts.len() - 1];
        if block_assigns_var(body_prefix, idx_name) || block_has_top_continue(body_prefix) {
            return Ok(false);
        }

        let limit_s = if let ExprKind::Ident(lim_name) = &limit_expr.kind
            && let Some((lim_slot, lim_ty)) = self.lookup_local(lim_name)
            && lim_slot.is_scalar
            && lim_ty.is_int()
            && !block_assigns_var(&body.stmts, lim_name)
        {
            lim_slot.reg
        } else {
            let lp = self.loop_slots[&skey];
            let r = self.lower_scalar(limit_expr)?;
            if r != lp.aux_s {
                self.ops.push(Op::MoveScalar { dst: lp.aux_s, src: r });
            }
            lp.aux_s
        };

        let cond_t = self.alloc_temp_s();
        self.ops.push(Op::LtInt { dst: cond_t, a: idx_slot.reg, b: limit_s });
        let jf_pc = self.ops.len();
        self.ops.push(Op::JumpIfFalse { cond: cond_t, target: 0 });
        let top_pc = self.ops.len() as u32;

        self.loops.push(LoopPatch { breaks: Vec::new(), continues: Vec::new() });
        self.loop_depth += 1;
        self.scopes.push(HashMap::new());
        for s in body_prefix {
            self.lower_stmt(s)?;
        }
        self.scopes.pop();
        self.loop_depth -= 1;
        let patch = self.loops.pop().unwrap();

        let step_pc = self.ops.len() as u32;
        self.ops.push(Op::ForRangeStep { idx_reg: idx_slot.reg, limit_reg: limit_s, target: top_pc });
        let done_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[jf_pc], done_pc);
        for pc in patch.breaks {
            patch_jump(&mut self.ops[pc], done_pc);
        }
        for pc in patch.continues {
            patch_jump(&mut self.ops[pc], step_pc);
        }
        Ok(true)
    }

    pub(crate) fn lower_while_generic(&mut self, condition: &Expr, body: &Block) -> Result<(), String> {
        let top_pc = self.ops.len() as u32;
        self.reset_temps();
        let cond_r = self.lower_scalar(condition)?;
        let jf_pc = self.ops.len();
        self.ops.push(Op::JumpIfFalse { cond: cond_r, target: 0 });

        self.loops.push(LoopPatch { breaks: Vec::new(), continues: Vec::new() });
        self.loop_depth += 1;
        self.lower_block_stmts(&body.stmts)?;
        self.loop_depth -= 1;
        let patch = self.loops.pop().unwrap();

        self.ops.push(Op::Jump { target: top_pc });
        let done_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[jf_pc], done_pc);
        for pc in patch.breaks {
            patch_jump(&mut self.ops[pc], done_pc);
        }
        for pc in patch.continues {
            patch_jump(&mut self.ops[pc], top_pc);
        }
        Ok(())
    }

    pub(crate) fn lower_switch(&mut self, expr: &Expr, cases: &[SwitchCase]) -> Result<(), String> {
        let scr_is_scalar = expr.ty.is_scalar();
        let scr_reg = if scr_is_scalar {
            self.lower_scalar(expr)?
        } else {
            self.lower_obj(expr)?
        };
        let mut end_jumps = Vec::new();
        for case in cases {
            match &case.pattern.kind {
                PatternKind::Wildcard => {
                    self.lower_block_stmts(&case.body.stmts)?;
                    break;
                }
                PatternKind::Nil => {
                    let emp = self.alloc_temp_o();
                    let cidx = self.mc.intern_empty()?;
                    self.ops.push(Op::LoadConstObj { dst_obj: emp, idx: cidx });
                    let eq_s = self.alloc_temp_s();
                    self.ops.push(Op::EqObj { dst_s: eq_s, a_obj: scr_reg, b_obj: emp });
                    let jf = self.ops.len();
                    self.ops.push(Op::JumpIfFalse { cond: eq_s, target: 0 });
                    self.lower_block_stmts(&case.body.stmts)?;
                    let je = self.ops.len();
                    self.ops.push(Op::Jump { target: 0 });
                    end_jumps.push(je);
                    self.patch_here(jf);
                }
                PatternKind::Ident(name) => {
                    if let Type::Enum(ename) = &expr.ty
                        && let Some((_, vname)) = name.split_once('.')
                    {
                        let tag = self.mc.checked.enums[ename]
                            .iter()
                            .position(|v| v == vname)
                            .ok_or_else(|| format!("Unknown variant '{}.{}'", ename, vname))?;
                        let eq_s = self.alloc_temp_s();
                        self.ops.push(Op::EqIntImm {
                            dst: eq_s,
                            a: scr_reg,
                            imm: tag as i16,
                        });
                        let jf = self.ops.len();
                        self.ops.push(Op::JumpIfFalse { cond: eq_s, target: 0 });
                        self.lower_block_stmts(&case.body.stmts)?;
                        let je = self.ops.len();
                        self.ops.push(Op::Jump { target: 0 });
                        end_jumps.push(je);
                        self.patch_here(jf);
                        continue;
                    }
                    let pkey = &case.pattern as *const Pattern as usize;
                    let slot = self.pat_slots[&(pkey, name.clone())];
                    if slot.is_scalar && scr_is_scalar {
                        self.ops.push(Op::MoveScalar { dst: slot.reg, src: scr_reg });
                    } else if !slot.is_scalar && !scr_is_scalar {
                        self.ops.push(Op::MoveObj { dst_obj: slot.reg, src: scr_reg });
                    }
                    self.scopes.push(HashMap::new());
                    self.bind_var(name.clone(), slot, expr.ty.clone());
                    for s in &case.body.stmts {
                        self.lower_stmt(s)?;
                    }
                    self.scopes.pop();
                    break;
                }
                PatternKind::Type { name, binding } => {
                    let resolved = crate::frontend::monomorph::resolve_switch_pattern_type(
                        name,
                        &expr.ty,
                        self.mc.checked,
                    )?;
                    let expected_tag =
                        crate::ir::lower_native::type_tag_for_switch(&resolved, self.mc.checked)?;
                    let obj_scr = if scr_is_scalar {
                        self.emit_box_scalar_from(scr_reg, &expr.ty)
                    } else {
                        scr_reg
                    };
                    let cond_s = self.alloc_temp_s();
                    self.ops.push(Op::IsTypeObj {
                        dst_s: cond_s,
                        src_obj: obj_scr,
                        expected_tag,
                    });
                    let jf = self.ops.len();
                    self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
                    self.scopes.push(HashMap::new());
                    if binding != "_" {
                        let pkey = &case.pattern as *const Pattern as usize;
                        let slot = self.pat_slots[&(pkey, binding.clone())];
                        if slot.is_scalar {
                            self.ops.push(Op::UnboxScalar {
                                dst_s: slot.reg,
                                src_obj: obj_scr,
                            });
                        } else {
                            self.ops.push(Op::MoveObj {
                                dst_obj: slot.reg,
                                src: obj_scr,
                            });
                        }
                        self.bind_var(binding.clone(), slot, resolved);
                    }
                    for s in &case.body.stmts {
                        self.lower_stmt(s)?;
                    }
                    self.scopes.pop();
                    let je = self.ops.len();
                    self.ops.push(Op::Jump { target: 0 });
                    end_jumps.push(je);
                    self.patch_here(jf);
                }
                PatternKind::Tuple(_) => {
                    self.scopes.push(HashMap::new());
                    self.lower_pattern_bind(&case.pattern, &expr.ty, scr_reg, scr_is_scalar)?;
                    for s in &case.body.stmts {
                        self.lower_stmt(s)?;
                    }
                    self.scopes.pop();
                    break;
                }
            }
        }
        let end_pc = self.ops.len() as u32;
        for je in end_jumps {
            patch_jump(&mut self.ops[je], end_pc);
        }
        Ok(())
    }
}

pub(crate) fn patch_jump(op: &mut Op, new_target: u32) {
    match op {
        Op::Jump { target }
        | Op::JumpIfFalse { target, .. }
        | Op::JumpIfTrue { target, .. }
        | Op::ForRangeStep { target, .. }
        | Op::ForListIntStep { target, .. } => *target = new_target,
        Op::ForListInt { exit_target, .. } | Op::ForListObj { exit_target, .. } => {
            *exit_target = new_target;
        }
        _ => panic!("patch_jump called on non-jump op {:?}", op),
    }
}

pub(crate) fn as_i16_lit(expr: &Expr) -> Option<i16> {
    match &expr.kind {
        ExprKind::Int(n) => i16::try_from(*n).ok(),
        ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => match inner.kind {
            ExprKind::Int(n) => n.checked_neg().and_then(|v| i16::try_from(v).ok()),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn as_neg_i16_lit(expr: &Expr) -> Option<i16> {
    match &expr.kind {
        ExprKind::Int(n) => n.checked_neg().and_then(|v| i16::try_from(v).ok()),
        ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => match inner.kind {
            ExprKind::Int(n) => i16::try_from(n).ok(),
            _ => None,
        },
        _ => None,
    }
}

fn is_plus_one_assign(stmt: &Stmt, var_name: &str) -> bool {
    let StmtKind::Assign { target, value } = &stmt.kind else { return false };
    let ExprKind::Ident(tname) = &target.kind else { return false };
    if tname != var_name {
        return false;
    }
    let ExprKind::Binary { op: BinaryOp::Add, left, right } = &value.kind else { return false };
    (matches!(&left.kind, ExprKind::Ident(n) if n == var_name) && matches!(right.kind, ExprKind::Int(1)))
        || (matches!(&right.kind, ExprKind::Ident(n) if n == var_name) && matches!(left.kind, ExprKind::Int(1)))
}

fn block_assigns_var(stmts: &[Stmt], var_name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Assign { target, .. } => {
                if matches!(&target.kind, ExprKind::Ident(n) if n == var_name) {
                    return true;
                }
            }
            StmtKind::If { then_block, else_block, .. } => {
                if block_assigns_var(&then_block.stmts, var_name)
                    || else_block.as_ref().is_some_and(|eb| block_assigns_var(&eb.stmts, var_name))
                {
                    return true;
                }
            }
            StmtKind::Guard { else_block, .. } => {
                if block_assigns_var(&else_block.stmts, var_name) {
                    return true;
                }
            }
            StmtKind::ForIn { body, .. } | StmtKind::While { body, .. } => {
                if block_assigns_var(&body.stmts, var_name) {
                    return true;
                }
            }
            StmtKind::Switch { cases, .. }
                if cases.iter().any(|c| block_assigns_var(&c.body.stmts, var_name)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn block_has_top_continue(stmts: &[Stmt]) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Continue => return true,
            StmtKind::If { then_block, else_block, .. } => {
                if block_has_top_continue(&then_block.stmts)
                    || else_block.as_ref().is_some_and(|eb| block_has_top_continue(&eb.stmts))
                {
                    return true;
                }
            }
            StmtKind::Guard { else_block, .. } => {
                if block_has_top_continue(&else_block.stmts) {
                    return true;
                }
            }
            StmtKind::Switch { cases, .. }
                if cases.iter().any(|c| block_has_top_continue(&c.body.stmts)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn collect_free_vars(block: &Block, params: &[Param], out: &mut HashSet<String>) {
    let mut bound: Vec<HashSet<String>> = vec![params.iter().map(|p| p.name.clone()).collect()];
    collect_free_block(block, &mut bound, out);
}

fn collect_free_block(block: &Block, bound: &mut Vec<HashSet<String>>, out: &mut HashSet<String>) {
    bound.push(HashSet::new());
    for s in &block.stmts {
        collect_free_stmt(s, bound, out);
    }
    bound.pop();
}

fn bind_free_pat(pat: &Pattern, bound: &mut Vec<HashSet<String>>) {
    match &pat.kind {
        PatternKind::Ident(n) => {
            if let Some(top) = bound.last_mut() {
                top.insert(n.clone());
            }
        }
        PatternKind::Type { binding, .. } => {
            if let Some(top) = bound.last_mut() {
                top.insert(binding.clone());
            }
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                bind_free_pat(p, bound);
            }
        }
        PatternKind::Wildcard | PatternKind::Nil => {}
    }
}

fn collect_free_stmt(stmt: &Stmt, bound: &mut Vec<HashSet<String>>, out: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Define { name, value } => {
            collect_free_expr(value, bound, out);
            if let Some(top) = bound.last_mut() {
                top.insert(name.clone());
            }
        }
        StmtKind::VarDecl { name, init, .. } => {
            if let Some(e) = init {
                collect_free_expr(e, bound, out);
            }
            if let Some(top) = bound.last_mut() {
                top.insert(name.clone());
            }
        }
        StmtKind::DestructureDefine { pattern, value } => {
            collect_free_expr(value, bound, out);
            bind_free_pat(pattern, bound);
        }
        StmtKind::Assign { target, value } => {
            collect_free_expr(target, bound, out);
            collect_free_expr(value, bound, out);
        }
        StmtKind::Return(Some(e)) | StmtKind::Expr(e) => collect_free_expr(e, bound, out),
        StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::If { condition, then_block, else_block } => {
            collect_free_expr(condition, bound, out);
            collect_free_block(then_block, bound, out);
            if let Some(eb) = else_block {
                collect_free_block(eb, bound, out);
            }
        }
        StmtKind::Guard { condition, else_block } => {
            collect_free_expr(condition, bound, out);
            collect_free_block(else_block, bound, out);
        }
        StmtKind::ForIn { pattern, iterable, body } => {
            collect_free_expr(iterable, bound, out);
            bound.push(HashSet::new());
            bind_free_pat(pattern, bound);
            collect_free_block(body, bound, out);
            bound.pop();
        }
        StmtKind::While { condition, body } => {
            collect_free_expr(condition, bound, out);
            collect_free_block(body, bound, out);
        }
        StmtKind::Switch { expr, cases } => {
            collect_free_expr(expr, bound, out);
            for c in cases {
                bound.push(HashSet::new());
                bind_free_pat(&c.pattern, bound);
                collect_free_block(&c.body, bound, out);
                bound.pop();
            }
        }
    }
}

fn collect_free_expr(expr: &Expr, bound: &mut Vec<HashSet<String>>, out: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            if !bound.iter().rev().any(|s| s.contains(name)) {
                out.insert(name.clone());
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_free_expr(left, bound, out);
            collect_free_expr(right, bound, out);
        }
        ExprKind::Unary { expr: inner, .. } | ExprKind::Try(inner) => {
            collect_free_expr(inner, bound, out);
        }
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                collect_free_expr(e, bound, out);
            }
        }
        ExprKind::Call { callee, args, named_args, .. } => {
            collect_free_expr(callee, bound, out);
            for a in args {
                collect_free_expr(a, bound, out);
            }
            for na in named_args {
                collect_free_expr(&na.value, bound, out);
            }
        }
        ExprKind::Member { target, .. } | ExprKind::OptionalMember { target, .. } => {
            collect_free_expr(target, bound, out);
        }
        ExprKind::Index { target, index } => {
            collect_free_expr(target, bound, out);
            collect_free_expr(index, bound, out);
        }
        ExprKind::Closure { params, body, .. } => {
            bound.push(params.iter().map(|p| p.name.clone()).collect());
            collect_free_block(body, bound, out);
            bound.pop();
        }
        _ => {}
    }
}
