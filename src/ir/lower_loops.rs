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
                    self.lower_add_int(dst, right, Some(dst))?;
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
                    if let Some(imm) = as_i16_lit(right) {
                        self.ops.push(Op::MulIntImm { dst, a: dst, imm });
                        return Ok(true);
                    }
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
                    self.lower_remainder(dst, right, Some(dst))?;
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
        if matches!(&iterable.kind, ExprKind::Binary { op: BinaryOp::Range, .. })
            || iterable.ty == Type::Int
        {
            let idx_s = match &pattern.kind {
                PatternKind::Ident(name) => {
                    let pkey = pattern as *const Pattern as usize;
                    self.pat_slots[&(pkey, name.clone())].reg
                }
                _ => lp.idx_s,
            };
            if let ExprKind::Binary { op: BinaryOp::Range, left, right } = &iterable.kind {
                let start_r = self.lower_scalar(left)?;
                let end_r = self.lower_scalar(right)?;
                if start_r != idx_s {
                    self.ops.push(Op::MoveScalar { dst: idx_s, src: start_r });
                }
                if end_r != lp.aux_s {
                    self.ops.push(Op::MoveScalar { dst: lp.aux_s, src: end_r });
                }
            } else {
                let end_r = self.lower_scalar(iterable)?;
                if end_r != lp.aux_s {
                    self.ops.push(Op::MoveScalar { dst: lp.aux_s, src: end_r });
                }
                self.ops.push(Op::LoadImmInt { dst: idx_s, imm: 0 });
            }
            let jf_pc = self.ops.len();
            self.ops.push(Op::JumpIfLtInt { a: idx_s, b: lp.aux_s, target: 0, when_true: false });
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
            self.emit_range_step(idx_s, lp.aux_s, top_pc);
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

        if let Type::Generic { type_id, type_args } = &iterable.ty
            && type_id.module == crate::frontend::types::ModuleId::Native("cdt.chan".to_string())
            && type_id.name == "Chan"
        {
            let elem_ty = type_args.first().cloned().unwrap_or(Type::Any);
            let chan_r = self.lower_obj(iterable)?;
            if chan_r != lp.list_o {
                self.ops.push(Op::MoveObj { dst_obj: lp.list_o, src: chan_r });
            }
            let recv_o = self.alloc_temp_o();
            let head_pc = self.ops.len();
            self.ops.push(Op::ExtOp {
                kind: crate::vm::opcode::EXT_CHAN_OP,
                aux: crate::native::chan::CHAN_RECV,
                a: recv_o,
                b: lp.list_o,
                c: 0,
            });
            self.ops.push(Op::IsTypeObj {
                dst_s: lp.idx_s,
                src_obj: recv_o,
                expected_tag: crate::vm::value::SCHEMA_EMPTY_TAG,
            });
            let exit_pc = self.ops.len();
            self.ops.push(Op::JumpIfTrue { cond: lp.idx_s, target: 0 });

            self.scopes.push(HashMap::new());
            self.lower_pattern_bind(pattern, &elem_ty, recv_o, false)?;
            self.loops.push(LoopPatch { breaks: Vec::new(), continues: Vec::new() });
            self.loop_depth += 1;
            for s in &body.stmts {
                self.lower_stmt(s)?;
            }
            self.loop_depth -= 1;
            let patch = self.loops.pop().unwrap();
            self.scopes.pop();

            self.ops.push(Op::Jump { target: head_pc as u32 });
            let done_pc = self.ops.len() as u32;
            patch_jump(&mut self.ops[exit_pc], done_pc);
            for pc in patch.breaks {
                patch_jump(&mut self.ops[pc], done_pc);
            }
            for pc in patch.continues {
                patch_jump(&mut self.ops[pc], head_pc as u32);
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
            let step = match &self.ops[top_pc as usize..] {
                [Op::AddInt { dst, a, b }] if elem_ty == Type::Int && dst == a && *b == elem_reg
                    && *dst != lp.idx_s && *dst != elem_reg =>
                {
                    Op::SumListIntTail { list_obj, idx_s: lp.idx_s, elem_s: elem_reg, acc_s: *dst }
                }
                _ => Op::ForListIntStep { list_obj, idx_s: lp.idx_s, elem_s: elem_reg, target: top_pc },
            };
            self.ops.push(step);
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

        let jf_pc = self.ops.len();
        self.ops.push(Op::JumpIfLtInt { a: idx_slot.reg, b: limit_s, target: 0, when_true: false });
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
        self.emit_range_step(idx_slot.reg, limit_s, top_pc);
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

    fn emit_range_step(&mut self, idx_s: u16, limit_s: u16, top: u32) {
        let step = match &self.ops[top as usize..] {
            [Op::ListAppendInt { list_obj, val_s }] if *val_s == idx_s => {
                Op::AppendIntRangeTail { list_obj: *list_obj, idx_s, limit_s }
            }
            [Op::AddIntMapGetOrImm { dst_s, a, map_obj, key_s, default }]
                if dst_s == a && *key_s == idx_s && *dst_s != idx_s && *dst_s != limit_s =>
            {
                Op::SumIntMapRangeTail {
                    acc_s: *dst_s, map_obj: *map_obj, idx_s, limit_s, default: *default,
                }
            }
            _ => Op::ForRangeStep { idx_reg: idx_s, limit_reg: limit_s, target: top },
        };
        self.ops.push(step);
    }

    pub(crate) fn lower_while_generic(&mut self, condition: &Expr, body: &Block) -> Result<(), String> {
        let top_pc = self.ops.len() as u32;
        self.reset_temps();
        let jf_pc = self.lower_conditional_jump(condition, false)?;

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

    pub(crate) fn try_lower_int_if_chain(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
    ) -> Result<bool, String> {
        if else_block.is_none() {
            return Ok(false);
        }
        let Some((name, key)) = int_equality_test(condition) else {
            return Ok(false);
        };
        let Some((slot, _)) = self.lookup_local(name) else {
            return Ok(false);
        };
        let mut keys = HashSet::from([key]);
        let mut cases = vec![(key, then_block)];
        let mut fallback = else_block;
        while let Some([Stmt { kind: StmtKind::If { condition, then_block, else_block }, .. }]) =
            fallback.map(|block| block.stmts.as_slice())
        {
            let Some((next_name, key)) = int_equality_test(condition) else { break };
            if name != next_name {
                break;
            }
            if !keys.insert(key) {
                return Ok(false);
            }
            cases.push((key, then_block));
            fallback = else_block.as_ref();
        }
        if cases.len() < 3 {
            return Ok(false);
        }
        let first = *keys.iter().min().unwrap();
        let last = *keys.iter().max().unwrap();
        let Ok(count) = u16::try_from(i32::from(last) - i32::from(first) + 1) else {
            return Ok(false);
        };
        if usize::from(count) > cases.len() * 2 {
            return Ok(false);
        }

        self.ops.push(Op::SwitchIntImm { src: slot.reg, first, count });
        let table = self.ops.len();
        self.ops.resize(table + 1 + usize::from(count), Op::Jump { target: 0 });

        let mut targets = vec![None; usize::from(count)];
        let mut end_jumps = Vec::with_capacity(cases.len());
        for (key, block) in cases {
            targets[(i32::from(key) - i32::from(first)) as usize] = Some(self.ops.len() as u32);
            self.lower_block_stmts(&block.stmts)?;
            end_jumps.push(self.ops.len());
            self.ops.push(Op::Jump { target: 0 });
        }
        let default = self.ops.len() as u32;
        if let Some(block) = fallback {
            self.lower_block_stmts(&block.stmts)?;
        }
        patch_jump(&mut self.ops[table], default);
        for (i, target) in targets.into_iter().enumerate() {
            patch_jump(&mut self.ops[table + i + 1], target.unwrap_or(default));
        }
        for jump in end_jumps {
            self.patch_here(jump);
        }
        Ok(true)
    }

    fn lower_tag_switch(
        &mut self,
        ename: &str,
        scr_reg: u16,
        layout: Option<super::lower::CompactEnum>,
        cases: &[SwitchCase],
    ) -> Result<(), String> {
        let variants = &self.mc.checked.data_enums[ename];
        let first_tag = variants[0].schema_id;
        let tag_count = u16::try_from(variants.len()).map_err(|_| "Too many variants in switch")?;
        self.ops.push(if let Some(layout) = layout {
            Op::SwitchScalarTag { tag_s: layout.tag_s, src_obj: scr_reg, first_tag, tag_count }
        } else {
            Op::SwitchTag { src_obj: scr_reg, first_tag, tag_count }
        });
        let table = self.ops.len();
        self.ops.resize(table + 1 + usize::from(tag_count), Op::Jump { target: 0 });

        let mut targets = vec![None; usize::from(tag_count)];
        let mut default = None;
        let mut end_jumps = Vec::new();
        for case in cases {
            let (variant, bindings): (_, &[(Option<String>, String)]) = match &case.pattern.kind {
                PatternKind::Ident(name) => (name.split_once('.').map(|(_, v)| v), &[]),
                PatternKind::EnumVariant { variant_name, bindings, .. } => {
                    (Some(variant_name.as_str()), bindings)
                }
                PatternKind::Type { name, binding } => (
                    name.rsplit_once('.').map(|(_, v)| v),
                    &[(None, binding.clone())],
                ),
                _ => (None, &[]),
            };

            let pc = self.ops.len() as u32;
            let pkey = &case.pattern as *const Pattern as usize;
            self.scopes.push(HashMap::new());
            if let Some(variant) = variant {
                let (var, fields) = crate::frontend::check::resolve_data_enum_bindings(
                    self.mc.checked, ename, variant, bindings,
                )?;
                let entry = &mut targets[(var.schema_id - first_tag) as usize];
                if entry.is_some() {
                    self.scopes.pop();
                    continue;
                }
                *entry = Some(pc);
                if let Some(layout) = layout
                    && let [(name_a, field_a), (name_b, field_b)] = fields.as_slice()
                {
                    let slot_a = self.pat_slots[&(pkey, name_a.clone())];
                    let slot_b = self.pat_slots[&(pkey, name_b.clone())];
                    self.ops.push(Op::ReadScalarEnumPair {
                        dst_a: slot_a.reg, dst_b: slot_b.reg, tag_s: layout.tag_s, src_obj: scr_reg,
                        field_a: field_a.slot_idx as u8, field_b: field_b.slot_idx as u8,
                    });
                    self.bind_var(name_a.clone(), slot_a, field_a.ty.clone());
                    self.bind_var(name_b.clone(), slot_b, field_b.ty.clone());
                } else {
                    for (local, field) in fields {
                        let slot = self.pat_slots[&(pkey, local.clone())];
                        self.ops.push(if let Some(layout) = layout {
                            Op::ReadScalarEnumField {
                                dst_s: slot.reg, tag_s: layout.tag_s,
                                src_obj: scr_reg, field_idx: field.slot_idx as u8,
                            }
                        } else if field.is_scalar {
                            Op::GetFieldScalar { dst_s: slot.reg, rec_obj: scr_reg, field_idx: field.slot_idx }
                        } else {
                            Op::GetFieldObj { dst_obj: slot.reg, rec_obj: scr_reg, field_idx: field.slot_idx }
                        });
                        self.bind_var(local, slot, field.ty.clone());
                    }
                }
            } else {
                default = Some(pc);
                if let PatternKind::Ident(name) = &case.pattern.kind {
                    let slot = self.pat_slots[&(pkey, name.clone())];
                    let src = self.materialize_local_obj(scr_reg, Some(slot.reg));
                    if src != slot.reg {
                        self.ops.push(Op::MoveObj { dst_obj: slot.reg, src });
                    }
                    self.bind_var(name.clone(), slot, Type::DataEnum(ename.to_owned()));
                }
            }
            for stmt in &case.body.stmts {
                self.lower_stmt(stmt)?;
            }
            self.scopes.pop();

            if default.is_some() {
                break;
            }
            end_jumps.push(self.ops.len());
            self.ops.push(Op::Jump { target: 0 });
        }

        let end = self.ops.len() as u32;
        let default = default.unwrap_or(end);
        patch_jump(&mut self.ops[table], default);
        for (i, target) in targets.into_iter().enumerate() {
            patch_jump(&mut self.ops[table + 1 + i], target.unwrap_or(default));
        }
        for jump in end_jumps {
            patch_jump(&mut self.ops[jump], end);
        }
        Ok(())
    }

    pub(crate) fn lower_switch(&mut self, expr: &Expr, cases: &[SwitchCase]) -> Result<(), String> {
        if let Type::DataEnum(ename) = &expr.ty
            && cases.iter().all(|case| match &case.pattern.kind {
                PatternKind::Nil | PatternKind::Tuple(_) => false,
                PatternKind::Type { name, .. } => name.contains('.'),
                _ => true,
            })
            && u16::try_from(self.mc.checked.data_enums[ename].len()).is_ok()
        {
            if let ExprKind::Ident(name) = &expr.kind
                && let Some((slot, _)) = self.lookup_local(name)
                && let Some(layout) = self.compact_enums.get(&slot.reg).copied()
            {
                return self.lower_tag_switch(ename, slot.reg, Some(layout), cases);
            }
            let scr_reg = self.lower_obj(expr)?;
            return self.lower_tag_switch(ename, scr_reg, None, cases);
        }
        let scr_is_scalar = expr.ty.is_scalar();
        let scr_reg = if scr_is_scalar { self.lower_scalar(expr)? } else { self.lower_obj(expr)? };
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
                    if let Type::DataEnum(ename) = &expr.ty
                        && let Some((_, vname)) = name.split_once('.')
                    {
                        let (var, _) = crate::frontend::check::resolve_data_enum_bindings(
                            self.mc.checked,
                            ename,
                            vname,
                            &[],
                        )?;
                        let cond_s = self.alloc_temp_s();
                        self.ops.push(Op::IsTypeObj {
                            dst_s: cond_s,
                            src_obj: scr_reg,
                            expected_tag: var.schema_id,
                        });
                        let jf = self.ops.len();
                        self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
                        self.lower_block_stmts(&case.body.stmts)?;
                        let je = self.ops.len();
                        self.ops.push(Op::Jump { target: 0 });
                        end_jumps.push(je);
                        self.patch_here(jf);
                        continue;
                    }
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
                PatternKind::EnumVariant {
                    enum_name,
                    variant_name,
                    bindings,
                } => {
                    let (var, resolved) = crate::frontend::check::resolve_data_enum_bindings(
                        self.mc.checked,
                        enum_name,
                        variant_name,
                        bindings,
                    )?;
                    let expected_tag = var.schema_id;
                    let cond_s = self.alloc_temp_s();
                    self.ops.push(Op::IsTypeObj {
                        dst_s: cond_s,
                        src_obj: scr_reg,
                        expected_tag,
                    });
                    let jf = self.ops.len();
                    self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
                    self.scopes.push(HashMap::new());
                    let pkey = &case.pattern as *const Pattern as usize;
                    for (local, field) in resolved {
                        let slot = self.pat_slots[&(pkey, local.clone())];
                        if field.is_scalar {
                            self.ops.push(Op::GetFieldScalar {
                                dst_s: slot.reg,
                                rec_obj: scr_reg,
                                field_idx: field.slot_idx,
                            });
                        } else {
                            self.ops.push(Op::GetFieldObj {
                                dst_obj: slot.reg,
                                rec_obj: scr_reg,
                                field_idx: field.slot_idx,
                            });
                        }
                        self.bind_var(local, slot, field.ty.clone());
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
                PatternKind::Type { name, binding } => {
                    if let Type::DataEnum(ename) = &expr.ty
                        && let Some((_, vname)) = name.rsplit_once('.')
                    {
                        let (var, resolved) = crate::frontend::check::resolve_data_enum_bindings(
                            self.mc.checked,
                            ename,
                            vname,
                            &[(None, binding.clone())],
                        )?;
                        let expected_tag = var.schema_id;
                        let cond_s = self.alloc_temp_s();
                        self.ops.push(Op::IsTypeObj {
                            dst_s: cond_s,
                            src_obj: scr_reg,
                            expected_tag,
                        });
                        let jf = self.ops.len();
                        self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
                        self.scopes.push(HashMap::new());
                        let pkey = &case.pattern as *const Pattern as usize;
                        for (local, field) in resolved {
                            let slot = self.pat_slots[&(pkey, local.clone())];
                            if field.is_scalar {
                                self.ops.push(Op::GetFieldScalar {
                                    dst_s: slot.reg,
                                    rec_obj: scr_reg,
                                    field_idx: field.slot_idx,
                                });
                            } else {
                                self.ops.push(Op::GetFieldObj {
                                    dst_obj: slot.reg,
                                    rec_obj: scr_reg,
                                    field_idx: field.slot_idx,
                                });
                            }
                            self.bind_var(local, slot, field.ty.clone());
                        }
                        for s in &case.body.stmts {
                            self.lower_stmt(s)?;
                        }
                        self.scopes.pop();
                        let je = self.ops.len();
                        self.ops.push(Op::Jump { target: 0 });
                        end_jumps.push(je);
                        self.patch_here(jf);
                        continue;
                    }
                    let resolved = crate::frontend::monomorph::resolve_switch_pattern_type(
                        name,
                        &expr.ty,
                        self.mc.checked,
                    )?;
                    let obj_scr = if scr_is_scalar {
                        self.emit_box_scalar_from(scr_reg, &expr.ty)
                    } else {
                        scr_reg
                    };
                    let jf = if let Type::DataEnum(ename) = &resolved {
                        let schema_ids: Vec<u32> = self.mc.checked.data_enums[ename]
                            .iter()
                            .map(|v| v.schema_id)
                            .collect();
                        let cond_s = self.alloc_temp_s();
                        let mut hit_jumps = Vec::with_capacity(schema_ids.len());
                        for sid in schema_ids {
                            self.ops.push(Op::IsTypeObj {
                                dst_s: cond_s,
                                src_obj: obj_scr,
                                expected_tag: sid,
                            });
                            let jh = self.ops.len();
                            self.ops.push(Op::JumpIfTrue { cond: cond_s, target: 0 });
                            hit_jumps.push(jh);
                        }
                        let miss = self.ops.len();
                        self.ops.push(Op::Jump { target: 0 });
                        for jh in hit_jumps {
                            self.patch_here(jh);
                        }
                        miss
                    } else {
                        let expected_tag =
                            crate::ir::lower_native::type_tag_for_switch(&resolved, self.mc.checked)?;
                        let cond_s = self.alloc_temp_s();
                        self.ops.push(Op::IsTypeObj {
                            dst_s: cond_s,
                            src_obj: obj_scr,
                            expected_tag,
                        });
                        let j = self.ops.len();
                        self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
                        j
                    };
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
        | Op::JumpIfLtInt { target, .. }
        | Op::JumpIfLtIntImm { target, .. }
        | Op::JumpIfEqIntImm { target, .. }
        | Op::ForRangeStep { target, .. }
        | Op::ForListIntStep { target, .. } => *target = new_target,
        Op::ForListInt { exit_target, .. } | Op::ForListObj { exit_target, .. } => {
            *exit_target = new_target;
        }
        _ => panic!("patch_jump called on non-jump op {:?}", op),
    }
}

pub(crate) fn as_i16_lit(expr: &Expr) -> Option<i16> {
    i16::try_from(int_literal(expr)?).ok()
}

pub(super) fn int_literal(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Int(n) => Some(*n),
        ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => match inner.kind {
            ExprKind::Int(n) => n.checked_neg(),
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

fn int_equality_test(condition: &Expr) -> Option<(&str, i16)> {
    let ExprKind::Binary { op: BinaryOp::Eq, left, right } = &condition.kind else {
        return None;
    };
    if left.ty.is_int() && let ExprKind::Ident(name) = &left.kind {
        return Some((name, as_i16_lit(right)?));
    }
    if right.ty.is_int() && let ExprKind::Ident(name) = &right.kind {
        return Some((name, as_i16_lit(left)?));
    }
    None
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
        PatternKind::EnumVariant { bindings, .. } => {
            if let Some(top) = bound.last_mut() {
                for (_, v) in bindings {
                    if v != "_" {
                        top.insert(v.clone());
                    }
                }
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
        StmtKind::Defer(block) | StmtKind::Spawn(block) => {
            collect_free_block(block, bound, out);
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
