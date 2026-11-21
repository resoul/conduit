use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::{can_propagate_try, native_member_field, scalar_kind};
use crate::frontend::types::{ModuleId, Type};
use crate::ir::lower::FnLowerer;
use crate::ir::lower_loops::{as_i16_lit, as_neg_i16_lit};
use crate::ir::lower_native::elem_tag_for;
use crate::vm::opcode::{EXT_MAP_INDEX, Op};

fn empty_is_err_for_try(ty: &Type) -> bool {
    match ty {
        Type::Optional(_) | Type::Nil => true,
        Type::Union(vars) => vars.iter().any(empty_is_err_for_try),
        _ => false,
    }
}

fn is_json_call_expr(expr: &Expr, checked: &CheckedProgram) -> bool {
    let ExprKind::Call { callee, .. } = &expr.kind else {
        return false;
    };
    match &callee.kind {
        ExprKind::Member { target, .. } => {
            matches!(&target.kind, ExprKind::Ident(a) if checked.is_native_alias(a, "cdt.json"))
        }
        ExprKind::Ident(name) => checked.native_direct_fn(name, "cdt.json").is_some(),
        _ => false,
    }
}

impl<'a, 'b> FnLowerer<'a, 'b> {
    pub(crate) fn emit_defer_try_check(
        &mut self,
        inner_ty: &Type,
        obj_reg: u16,
        empty_is_err: bool,
    ) -> Result<(), String> {
        let mut fail_tys: Vec<Type> = Vec::new();
        match inner_ty {
            Type::Optional(_) => fail_tys.push(Type::Nil),
            Type::Union(vars) => {
                for v in vars {
                    if crate::frontend::check_native::is_error_type(v, self.mc.checked)
                        || (empty_is_err && matches!(v, Type::Nil | Type::Void))
                    {
                        fail_tys.push(v.clone());
                    }
                }
            }
            other if crate::frontend::check_native::is_error_type(other, self.mc.checked) => {
                fail_tys.push(other.clone());
            }
            _ => {}
        }
        for fail_ty in &fail_tys {
            let expected_tag =
                crate::ir::lower_native::type_tag_for_switch(fail_ty, self.mc.checked)?;
            let is_err_s = self.alloc_temp_s();
            self.ops.push(Op::IsTypeObj {
                dst_s: is_err_s,
                src_obj: obj_reg,
                expected_tag,
            });
            let j_next = self.ops.len();
            self.ops.push(Op::JumpIfFalse {
                cond: is_err_s,
                target: 0,
            });
            if let Some(ret_slot) = self.defer_ret_slot {
                self.ops.push(Op::MoveObj {
                    dst_obj: ret_slot.reg,
                    src: obj_reg,
                });
            }
            let j_epi = self.ops.len();
            self.ops.push(Op::Jump { target: 0 });
            self.defer_epilogue_jumps.push(j_epi);
            self.patch_here(j_next);
        }
        Ok(())
    }

    pub(crate) fn load_int_into(&mut self, dst: u16, n: i64) -> Result<(), String> {
        if let Ok(imm) = i32::try_from(n) {
            self.ops.push(Op::LoadImmInt { dst, imm });
        } else {
            let idx = self.mc.intern_int(n)?;
            self.ops.push(Op::LoadConstInt { dst, idx });
        }
        Ok(())
    }

    pub(crate) fn load_float_into(&mut self, dst: u16, v: f64) -> Result<(), String> {
        let idx = self.mc.intern_float(v.to_bits())?;
        self.ops.push(Op::LoadConstFloat { dst, idx });
        Ok(())
    }

    pub(crate) fn lower_expr_discard(&mut self, expr: &Expr) -> Result<(), String> {
        if expr.ty.is_scalar() {
            let _ = self.lower_scalar(expr)?;
        } else if expr.ty.is_obj() || matches!(expr.kind, ExprKind::Try(_)) {
            let _ = self.lower_obj(expr)?;
        } else {
            self.lower_void_call(expr)?;
        }
        Ok(())
    }

    pub(crate) fn lower_into_slot(&mut self, expr: &Expr, slot: super::lower::VarSlot) -> Result<(), String> {
        let dst = slot.reg;
        if slot.is_scalar {
            let src = self.lower_scalar_with_dest(expr, Some(dst))?;
            if src != dst {
                self.ops.push(Op::MoveScalar { dst, src });
            }
        } else if let Some(layout) = self.compact_enums.get(&dst).copied() {
            if !self.try_lower_compact_enum_ctor(expr, layout)? {
                let src = self.lower_obj_with_dest(expr, Some(dst))?;
                if src != dst {
                    self.ops.push(Op::MoveObj { dst_obj: dst, src });
                }
                self.load_int_into(layout.tag_s, -1)?;
            }
        } else {
            let src = self.lower_obj_with_dest(expr, Some(dst))?;
            if src != dst {
                self.ops.push(Op::MoveObj { dst_obj: dst, src });
            }
        }
        Ok(())
    }

    fn try_lower_compact_enum_ctor(
        &mut self,
        expr: &Expr,
        layout: super::lower::CompactEnum,
    ) -> Result<bool, String> {
        let (member, args, named_args): (_, &[_], &[_]) = match &expr.kind {
            ExprKind::Call { callee, args, named_args, .. } => (callee.as_ref(), args, named_args),
            _ => (expr, &[], &[]),
        };
        let ExprKind::Member { target, field } = &member.kind else { return Ok(false) };
        let ExprKind::Ident(name) = &target.kind else { return Ok(false) };
        if !matches!(&expr.ty, Type::DataEnum(ename) if ename == name)
            || self.lookup_local(name).is_some()
        {
            return Ok(false);
        }
        let Some(variant) = self.mc.checked.data_enums[name].iter().find(|v| v.name == *field) else {
            return Ok(false);
        };
        let st = self.mc.checked.structs[self.mc.checked.struct_by_name[&variant.struct_name]].clone();
        let (payload, _) = self.lower_struct_fields(&st, args, named_args)?;
        if payload.len() > usize::from(layout.width) {
            return Err("Enum payload exceeds the scalar register allocation".to_owned());
        }
        if payload.is_empty() {
            self.load_int_into(layout.tag_s, i64::from(st.schema_id))?;
            return Ok(true);
        }
        self.ops.push(Op::MakeScalarEnum {
            dst_s: layout.tag_s,
            schema_id: st.schema_id,
            a: payload.first().copied().unwrap_or(0),
            b: payload.get(1).copied().unwrap_or(0),
            len: payload.len() as u8,
        });
        Ok(true)
    }

    pub(crate) fn materialize_local_obj(&mut self, src_obj: u16, dest: Option<u16>) -> u16 {
        let Some(layout) = self.compact_enums.get(&src_obj).copied() else { return src_obj };
        let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
        self.ops.push(Op::BoxScalarEnum { dst_obj, src_obj, tag_s: layout.tag_s, width: layout.width });
        dst_obj
    }

    pub(crate) fn lower_conditional_jump(&mut self, condition: &Expr, when_true: bool) -> Result<usize, String> {
        let start = self.ops.len();
        let cond = self.lower_scalar(condition)?;
        // A comparison used only by this branch has no need to materialize its boolean.
        let fused = match self.ops.last().copied().filter(|_| self.ops.len() > start) {
            Some(Op::LtInt { dst, a, b }) if dst == cond => {
                Some(Op::JumpIfLtInt { a, b, target: 0, when_true })
            }
            Some(Op::EqIntImm { dst, a, imm }) if dst == cond => {
                Some(Op::JumpIfEqIntImm { a, imm, target: 0, when_true })
            }
            Some(Op::LtIntImm { dst, a, imm }) if dst == cond => {
                Some(Op::JumpIfLtIntImm { a, imm, target: 0, when_true })
            }
            _ => None,
        };
        if fused.is_some() {
            self.ops.pop();
        }
        let pc = self.ops.len();
        self.ops.push(fused.unwrap_or(if when_true {
            Op::JumpIfTrue { cond, target: 0 }
        } else {
            Op::JumpIfFalse { cond, target: 0 }
        }));
        Ok(pc)
    }

    pub(crate) fn lower_scalar(&mut self, expr: &Expr) -> Result<u16, String> {
        self.lower_scalar_with_dest(expr, None)
    }

    fn lower_scalar_with_dest(&mut self, expr: &Expr, dest: Option<u16>) -> Result<u16, String> {
        match &expr.kind {
            ExprKind::Int(n) => {
                if self.loop_depth > 0
                    && let Some(&reg) = self.hoisted_ints.get(n)
                {
                    return Ok(reg);
                }
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.load_int_into(dst, *n)?;
                Ok(dst)
            }
            ExprKind::Float(v) => {
                let bits = v.to_bits();
                if let Some(&reg) = self.hoisted_floats.get(&bits) {
                    return Ok(reg);
                }
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.load_float_into(dst, *v)?;
                Ok(dst)
            }
            ExprKind::Bool(b) => {
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::LoadBool { dst, val: *b });
                Ok(dst)
            }
            ExprKind::Ident(name) => {
                let (slot, _) = self
                    .lookup_local(name)
                    .ok_or_else(|| format!("Undefined scalar identifier '{}'", name))?;
                if !slot.is_scalar {
                    return Err(format!("Expected scalar '{}' but got object", name));
                }
                Ok(slot.reg)
            }
            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Neg => {
                    if let ExprKind::Int(n) = inner.kind {
                        let neg = n.wrapping_neg();
                        if self.loop_depth > 0
                            && let Some(&reg) = self.hoisted_ints.get(&neg)
                        {
                            return Ok(reg);
                        }
                        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.load_int_into(dst, neg)?;
                        return Ok(dst);
                    }
                    if let ExprKind::Float(v) = inner.kind {
                        let bits = (-v).to_bits();
                        if let Some(&reg) = self.hoisted_floats.get(&bits) {
                            return Ok(reg);
                        }
                        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.load_float_into(dst, -v)?;
                        return Ok(dst);
                    }
                    let src = self.lower_scalar(inner)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    if expr.ty.is_int() {
                        self.ops.push(Op::NegInt { dst, src });
                    } else {
                        let zero = self.alloc_temp_s();
                        self.load_float_into(zero, 0.0)?;
                        self.ops.push(Op::SubFloat { dst, a: zero, b: src });
                    }
                    Ok(dst)
                }
                UnaryOp::Not => {
                    let src = self.lower_scalar(inner)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::NotBool { dst, src });
                    Ok(dst)
                }
            },
            ExprKind::Binary { op, left, right } => self.lower_binary_scalar(*op, left, right, &expr.ty, dest),
            ExprKind::Member { target, field } => {
                let target_ty = target.ty.clone();
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Field '.{}' on non-struct {}", field, target_ty));
                };
                if tid.module != ModuleId::Program {
                    return self.lower_native_member_scalar(tid, target, field);
                }
                let rec_obj = self.lower_obj(target)?;
                let sid = self.mc.checked.struct_by_name[&tid.name];
                let st = &self.mc.checked.structs[sid];
                let f = st
                    .field(field)
                    .ok_or_else(|| format!("Unknown field '.{}' on {}", field, tid.name))?;
                let dst_s = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::GetFieldScalar { dst_s, rec_obj, field_idx: f.slot_idx });
                Ok(dst_s)
            }
            ExprKind::Try(inner) => {
                if inner.ty.is_scalar() {
                    return self.lower_scalar(inner);
                }
                let src_obj = self.lower_obj(inner)?;
                let dst_s = dest.unwrap_or_else(|| self.alloc_temp_s());
                let ret_on_err = can_propagate_try(&inner.ty, &self.return_ty, self.mc.checked);
                if ret_on_err && self.has_defer {
                    self.emit_defer_try_check(&inner.ty, src_obj, true)?;
                    self.ops.push(Op::UnboxScalar { dst_s, src_obj });
                } else {
                    self.ops.push(Op::TryUnwrapScalar {
                        dst_s,
                        src_obj,
                        ret_on_err,
                    });
                }
                Ok(dst_s)
            }
            ExprKind::Call { .. } => self.lower_call_scalar(expr),
            _ => Err(format!("Unsupported scalar expression {:?}", expr.kind)),
        }
    }

    pub(crate) fn lower_obj(&mut self, expr: &Expr) -> Result<u16, String> {
        self.lower_obj_with_dest(expr, None)
    }

    fn lower_obj_with_dest(&mut self, expr: &Expr, dest: Option<u16>) -> Result<u16, String> {
        if expr.ty.is_scalar() {
            let s = self.lower_scalar(expr)?;
            return Ok(self.emit_box_scalar_from(s, &expr.ty));
        }
        match &expr.kind {
            ExprKind::String(s) => {
                if let Some(&reg) = self.hoisted_strs.get(s) {
                    return Ok(reg);
                }
                let idx = self.mc.intern_str(s)?;
                let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                self.ops.push(Op::LoadConstObj { dst_obj, idx });
                Ok(dst_obj)
            }
            ExprKind::Nil => {
                let idx = self.mc.intern_empty()?;
                let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                self.ops.push(Op::LoadConstObj { dst_obj, idx });
                Ok(dst_obj)
            }
            ExprKind::Ident(name) => {
                if let Some((slot, _)) = self.lookup_local(name) {
                    if slot.is_scalar {
                        return Err(format!("Expected object '{}' but got scalar", name));
                    }
                    return Ok(self.materialize_local_obj(slot.reg, dest));
                }
                if let Some(&fid) = self.mc.checked.func_by_name.get(name) {
                    let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                    self.ops.push(Op::LoadFuncObj { dst_obj, func_id: fid as u16 });
                    return Ok(dst_obj);
                }
                if self.mc.checked.native_direct_fn(name, "cdt.validation") == Some("email") {
                    return self.lower_email_fn_val();
                }
                Err(format!("Undefined object identifier '{}'", name))
            }
            ExprKind::Binary { op: BinaryOp::Add, left, right } if expr.ty == Type::String => {
                let a_obj = self.lower_obj(left)?;
                if let ExprKind::Call { callee, args, named_args, .. } = &right.kind
                    && args.is_empty() && named_args.is_empty()
                    && let ExprKind::Member { target, field } = &callee.kind
                    && target.ty == Type::Int && field == "to_string"
                {
                    let src_s = self.lower_scalar(target)?;
                    let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                    self.ops.push(Op::StrConcatInt { dst_obj, prefix_obj: a_obj, src_s });
                    return Ok(dst_obj);
                }
                let b_obj = self.lower_obj(right)?;
                let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                self.ops.push(Op::StrConcat { dst_obj, a_obj, b_obj });
                Ok(dst_obj)
            }
            ExprKind::Binary { op: BinaryOp::Range, left, right } => {
                let start_s = self.lower_scalar(left)?;
                let end_s = self.lower_scalar(right)?;
                let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                self.ops.push(Op::MakeRangeList { dst_obj, start_s, end_s });
                Ok(dst_obj)
            }
            ExprKind::Binary { op: BinaryOp::Coalesce, left, right } => {
                let lhs_o = self.lower_obj(left)?;
                let emp = self.alloc_temp_o();
                let emp_idx = self.mc.intern_empty()?;
                self.ops.push(Op::LoadConstObj { dst_obj: emp, idx: emp_idx });
                let is_nil = self.alloc_temp_s();
                self.ops.push(Op::EqObj { dst_s: is_nil, a_obj: lhs_o, b_obj: emp });
                let dst_obj = self.alloc_temp_o();
                let jf_pc = self.ops.len();
                self.ops.push(Op::JumpIfFalse { cond: is_nil, target: 0 });
                let rhs_o = self.lower_obj(right)?;
                self.ops.push(Op::MoveObj { dst_obj, src: rhs_o });
                let je_pc = self.ops.len();
                self.ops.push(Op::Jump { target: 0 });
                self.patch_here(jf_pc);
                self.ops.push(Op::MoveObj { dst_obj, src: lhs_o });
                self.patch_here(je_pc);
                Ok(dst_obj)
            }
            ExprKind::List(elems) => self.lower_list_lit(elems, &expr.ty),
            ExprKind::Tuple(elems) => {
                let mut vals = Vec::with_capacity(elems.len());
                for el in elems {
                    vals.push(self.lower_obj(el)?);
                }
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewListObj { dst_obj, cap: elems.len() as u16 });
                for v in vals {
                    self.ops.push(Op::ListAppendObj { list_obj: dst_obj, val_obj: v });
                }
                Ok(dst_obj)
            }
            ExprKind::Index { target, index } => match &target.ty {
                ty @ (Type::List(_) | Type::Bytes) => {
                    let list_obj = self.lower_obj(target)?;
                    let idx_s = self.lower_scalar(index)?;
                    let elem_tag = if let Type::List(inner) = ty { elem_tag_for(inner) } else { 1 };
                    let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                    self.ops.push(Op::ListIndex { dst_obj, list_obj, idx_s, elem_tag });
                    Ok(dst_obj)
                }
                Type::Map(k, v) => {
                    let map_obj = self.lower_obj(target)?;
                    let key_reg = if k.is_scalar() {
                        self.lower_scalar(index)?
                    } else {
                        self.lower_obj(index)?
                    };
                    let aux = (elem_tag_for(k) << 4) | elem_tag_for(v);
                    let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                    self.ops.push(Op::ExtOp { kind: EXT_MAP_INDEX, aux, a: dst_obj, b: map_obj, c: key_reg });
                    Ok(dst_obj)
                }
                _ => Err("Unsupported index target".to_string()),
            },
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(ename) = &target.kind
                    && self.lookup_local(ename).is_none()
                    && let Some(dvars) = self.mc.checked.data_enums.get(ename).cloned()
                {
                    let var = dvars.iter().find(|v| &v.name == field).ok_or_else(|| {
                        format!("Enum '{}' has no variant '{}'", ename, field)
                    })?;
                    return self.lower_struct_ctor(&var.struct_name, &[], &[], dest);
                }
                if let ExprKind::Ident(alias) = &target.kind
                    && self.lookup_local(alias).is_none()
                    && self.mc.checked.is_native_alias(alias, "cdt.validation")
                    && field == "email"
                {
                    return self.lower_email_fn_val();
                }
                let Type::Custom(tid) = &target.ty else {
                    return Err(format!("Field '.{}' on non-struct {}", field, target.ty));
                };
                if tid.module != ModuleId::Program {
                    return self.lower_native_member_obj(tid, target, field);
                }
                let rec_obj = self.lower_obj(target)?;
                let sid = self.mc.checked.struct_by_name[&tid.name];
                let st = &self.mc.checked.structs[sid];
                let f = st
                    .field(field)
                    .ok_or_else(|| format!("Unknown field '.{}' on {}", field, tid.name))?;
                let dst_obj = dest.unwrap_or_else(|| self.alloc_temp_o());
                self.ops.push(Op::GetFieldObj { dst_obj, rec_obj, field_idx: f.slot_idx });
                Ok(dst_obj)
            }
            ExprKind::OptionalMember { target, field } => {
                let Type::Optional(inner_ty) = &target.ty else {
                    return Err(format!("Optional member '.{}' on non-optional {}", field, target.ty));
                };
                let Type::Custom(tid) = inner_ty.as_ref() else {
                    return Err(format!("Optional member '.{}' on non-struct {}", field, inner_ty));
                };
                let (field_ty, field_idx) = if tid.module != ModuleId::Program {
                    native_member_field(tid, field)?
                } else {
                    let sid = self.mc.checked.struct_by_name[&tid.name];
                    let f = self.mc.checked.structs[sid]
                        .field(field)
                        .ok_or_else(|| format!("Unknown field '.{}' on {}", field, tid.name))?;
                    (f.ty.clone(), f.slot_idx)
                };
                let lhs_o = self.lower_obj(target)?;
                let emp = self.alloc_temp_o();
                let emp_idx = self.mc.intern_empty()?;
                self.ops.push(Op::LoadConstObj { dst_obj: emp, idx: emp_idx });
                let is_nil = self.alloc_temp_s();
                self.ops.push(Op::EqObj { dst_s: is_nil, a_obj: lhs_o, b_obj: emp });
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::LoadConstObj { dst_obj, idx: emp_idx });
                let jt_pc = self.ops.len();
                self.ops.push(Op::JumpIfTrue { cond: is_nil, target: 0 });
                if field_ty.is_scalar() {
                    let tmp_s = self.alloc_temp_s();
                    self.ops.push(Op::GetFieldScalar { dst_s: tmp_s, rec_obj: lhs_o, field_idx });
                    self.ops.push(Op::BoxScalar {
                        dst_obj,
                        src_s: tmp_s,
                        kind: scalar_kind(&field_ty),
                    });
                } else {
                    self.ops.push(Op::GetFieldObj { dst_obj, rec_obj: lhs_o, field_idx });
                }
                self.patch_here(jt_pc);
                Ok(dst_obj)
            }
            ExprKind::Try(inner) => {
                if inner.ty.is_scalar() {
                    let s = self.lower_scalar(inner)?;
                    return Ok(self.emit_box_scalar_from(s, &inner.ty));
                }
                let ret_on_err = can_propagate_try(&inner.ty, &self.return_ty, self.mc.checked);
                if !ret_on_err && is_json_call_expr(inner, self.mc.checked) {
                    return self.lower_obj(inner);
                }
                let obj_reg = self.lower_obj(inner)?;
                let empty_is_err = empty_is_err_for_try(&inner.ty);
                if ret_on_err && self.has_defer {
                    self.emit_defer_try_check(&inner.ty, obj_reg, empty_is_err)?;
                } else {
                    self.ops.push(Op::TryCheckObj {
                        src_obj: obj_reg,
                        ret_on_err,
                        empty_is_err,
                    });
                }
                Ok(obj_reg)
            }
            ExprKind::Closure { params, return_type, body } => {
                self.lower_closure(params, return_type, body, &expr.ty)
            }
            ExprKind::Call { .. } => self.lower_call_obj(expr, dest),
            _ => Err(format!("Unsupported object expression {:?}", expr.kind)),
        }
    }

    fn lower_add_int_imm(
        &mut self,
        arg: &Expr,
        imm: i16,
        dest: Option<u16>,
    ) -> Result<u16, String> {
        let a = self.lower_scalar(arg)?;
        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
        if matches!(&arg.kind, ExprKind::Binary { op: BinaryOp::Mul, .. })
            && let Some(Op::MulIntImm { dst: product, a: src, imm: mul }) = self.ops.last().copied()
            && product == a
        {
            self.ops.pop();
            self.ops.push(Op::MulAddIntImm { dst, src, mul, add: imm });
        } else {
            self.ops.push(Op::AddIntImm { dst, a, imm });
        }
        Ok(dst)
    }

    pub(super) fn lower_remainder(&mut self, a: u16, right: &Expr, dest: Option<u16>) -> Result<u16, String> {
        let divisor = crate::ir::lower_loops::int_literal(right)
            .and_then(|n| u32::try_from(n.unsigned_abs()).ok());
        if let Some(divisor) = divisor
            && let Some((magic, shift)) = crate::vm::opcode::remainder_magic(divisor)
            && let Ok(magic_idx) = self.mc.intern_int(magic)
        {
            let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
            self.ops.push(Op::ModIntConst { dst, a, divisor, magic_idx, shift });
            return Ok(dst);
        }
        if let Some(imm) = as_i16_lit(right) {
            let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
            self.ops.push(Op::ModIntImm { dst, a, imm });
            return Ok(dst);
        }
        let b = self.lower_scalar(right)?;
        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
        self.ops.push(Op::ModInt { dst, a, b });
        Ok(dst)
    }

    pub(crate) fn lower_add_int(
        &mut self,
        a: u16,
        right: &Expr,
        dest: Option<u16>,
    ) -> Result<u16, String> {
        let rhs_start = self.ops.len();
        let b = self.lower_scalar(right)?;
        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
        if matches!(&right.kind, ExprKind::Binary { op: BinaryOp::Coalesce, .. })
            && self.ops.len() > rhs_start
            && let Some(Op::MapGetIntIntOrImm { dst_s, map_obj, key_s, default }) = self.ops.last().copied()
            && dst_s == b
            && a != b
        {
            // The lookup's result is used only by this addition.
            self.ops.pop();
            self.ops.push(Op::AddIntMapGetOrImm { dst_s: dst, a, map_obj, key_s, default });
        } else {
            self.ops.push(Op::AddInt { dst, a, b });
        }
        Ok(dst)
    }

    fn lower_binary_scalar(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        res_ty: &Type,
        dest: Option<u16>,
    ) -> Result<u16, String> {
        match op {
            BinaryOp::And => {
                let r1 = self.lower_scalar(left)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::MoveScalar { dst, src: r1 });
                let jf = self.ops.len();
                self.ops.push(Op::JumpIfFalse { cond: dst, target: 0 });
                let r2 = self.lower_scalar(right)?;
                self.ops.push(Op::MoveScalar { dst, src: r2 });
                self.patch_here(jf);
                Ok(dst)
            }
            BinaryOp::Or => {
                let r1 = self.lower_scalar(left)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::MoveScalar { dst, src: r1 });
                let jt = self.ops.len();
                self.ops.push(Op::JumpIfTrue { cond: dst, target: 0 });
                let r2 = self.lower_scalar(right)?;
                self.ops.push(Op::MoveScalar { dst, src: r2 });
                self.patch_here(jt);
                Ok(dst)
            }
            BinaryOp::Coalesce => {
                if let ExprKind::Call { callee, args, .. } = &left.kind
                    && let ExprKind::Member { target, field } = &callee.kind
                    && field == "get"
                    && matches!(&target.ty, Type::Map(k, v) if **k == Type::Int && **v == Type::Int)
                {
                    let map_obj = self.lower_obj(target)?;
                    let key_s = self.lower_scalar(&args[0])?;
                    if let Some(default) = as_i16_lit(right) {
                        let dst_s = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.ops.push(Op::MapGetIntIntOrImm { dst_s, map_obj, key_s, default });
                        return Ok(dst_s);
                    }
                    let def_s = self.lower_scalar(right)?;
                    let dst_s = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::MapGetIntIntOr { dst_s, map_obj, key_s, def_s });
                    return Ok(dst_s);
                }
                let lhs_o = self.lower_obj(left)?;
                let emp = self.alloc_temp_o();
                let emp_idx = self.mc.intern_empty()?;
                self.ops.push(Op::LoadConstObj { dst_obj: emp, idx: emp_idx });
                let is_nil = self.alloc_temp_s();
                self.ops.push(Op::EqObj { dst_s: is_nil, a_obj: lhs_o, b_obj: emp });
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                let jf_pc = self.ops.len();
                self.ops.push(Op::JumpIfFalse { cond: is_nil, target: 0 });
                let rhs_s = self.lower_scalar(right)?;
                self.ops.push(Op::MoveScalar { dst, src: rhs_s });
                let je_pc = self.ops.len();
                self.ops.push(Op::Jump { target: 0 });
                self.patch_here(jf_pc);
                self.ops.push(Op::UnboxScalar { dst_s: dst, src_obj: lhs_o });
                self.patch_here(je_pc);
                Ok(dst)
            }
            BinaryOp::Add if res_ty.is_float() => {
                if let ExprKind::Binary { op: BinaryOp::Mul, left: ma, right: mb } = &left.kind {
                    let ra = self.lower_scalar(ma)?;
                    let rb = self.lower_scalar(mb)?;
                    let rc = self.lower_scalar(right)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::FmaFloat { dst, a: ra, b: rb, c: rc });
                    return Ok(dst);
                }
                if let ExprKind::Binary { op: BinaryOp::Mul, left: ma, right: mb } = &right.kind {
                    let rc = self.lower_scalar(left)?;
                    let ra = self.lower_scalar(ma)?;
                    let rb = self.lower_scalar(mb)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::FmaFloat { dst, a: ra, b: rb, c: rc });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::AddFloat { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Add if res_ty.is_int() => {
                if let Some(imm) = as_i16_lit(right) {
                    return self.lower_add_int_imm(left, imm, dest);
                }
                if let Some(imm) = as_i16_lit(left) {
                    return self.lower_add_int_imm(right, imm, dest);
                }
                // A literal or variable has no side effects, so its load can
                // precede the checked product without changing error order.
                if let ExprKind::Binary { op: BinaryOp::Mul, left: x, right: y } = &left.kind
                    && matches!(right.kind, ExprKind::Ident(_) | ExprKind::Int(_))
                    && as_i16_lit(x).is_none() && as_i16_lit(y).is_none()
                {
                    let a = self.lower_scalar(x)?;
                    let b = self.lower_scalar(y)?;
                    let c = self.lower_scalar(right)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::MulAddInt { dst, a, b, c });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                self.lower_add_int(a, right, dest)
            }
            BinaryOp::Sub if res_ty.is_int() => {
                if let Some(imm) = as_neg_i16_lit(right) {
                    let a = self.lower_scalar(left)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::AddIntImm { dst, a, imm });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::SubInt { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Mod if res_ty.is_int() => {
                let a = self.lower_scalar(left)?;
                self.lower_remainder(a, right, dest)
            }
            BinaryOp::Mul if res_ty.is_int()
                && let Some((arg, imm)) = as_i16_lit(right).map(|n| (left, n))
                    .or_else(|| as_i16_lit(left).map(|n| (right, n))) =>
            {
                let a = self.lower_scalar(arg)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::MulIntImm { dst, a, imm });
                Ok(dst)
            }
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Sub => {
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                let inst = match (op, res_ty.is_float()) {
                    (BinaryOp::Mul, false) => Op::MulInt { dst, a, b },
                    (BinaryOp::Div, false) => Op::DivInt { dst, a, b },
                    (BinaryOp::Sub, true) => Op::SubFloat { dst, a, b },
                    (BinaryOp::Mul, true) => Op::MulFloat { dst, a, b },
                    (BinaryOp::Div, true) => Op::DivFloat { dst, a, b },
                    _ => unreachable!(),
                };
                self.ops.push(inst);
                Ok(dst)
            }
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
            | BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(match op {
                    BinaryOp::BitAnd => Op::BitAndInt { dst, a, b },
                    BinaryOp::BitOr => Op::BitOrInt { dst, a, b },
                    BinaryOp::BitXor => Op::BitXorInt { dst, a, b },
                    BinaryOp::ShiftLeft => Op::ShiftLeftInt { dst, a, b },
                    BinaryOp::ShiftRight => Op::ShiftRightInt { dst, a, b },
                    _ => unreachable!(),
                });
                Ok(dst)
            }
            BinaryOp::Eq | BinaryOp::NotEq => {
                let is_neq = op == BinaryOp::NotEq;
                if (left.ty.is_int() && right.ty.is_int()) || (left.ty.is_bool() && right.ty.is_bool()) {
                    if let Some(imm) = as_i16_lit(right) && !is_neq {
                        let a = self.lower_scalar(left)?;
                        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.ops.push(Op::EqIntImm { dst, a, imm });
                        return Ok(dst);
                    }
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(if is_neq { Op::NeInt { dst, a, b } } else { Op::EqInt { dst, a, b } });
                    return Ok(dst);
                }
                if left.ty.is_float() && right.ty.is_float() {
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::EqFloat { dst, a, b });
                    if is_neq {
                        let nd = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.ops.push(Op::NotBool { dst: nd, src: dst });
                        return Ok(nd);
                    }
                    return Ok(dst);
                }
                let a_obj = self.lower_obj(left)?;
                let b_obj = self.lower_obj(right)?;
                let dst_s = dest.unwrap_or_else(|| self.alloc_temp_s());
                self.ops.push(Op::EqObj { dst_s, a_obj, b_obj });
                if is_neq {
                    let nd = dest.unwrap_or_else(|| self.alloc_temp_s());
                    self.ops.push(Op::NotBool { dst: nd, src: dst_s });
                    return Ok(nd);
                }
                Ok(dst_s)
            }
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                if left.ty.is_int() {
                    if op == BinaryOp::Lt
                        && let Some(imm) = as_i16_lit(right)
                    {
                        let a = self.lower_scalar(left)?;
                        let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                        self.ops.push(Op::LtIntImm { dst, a, imm });
                        return Ok(dst);
                    }
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                    let inst = match op {
                        BinaryOp::Lt => Op::LtInt { dst, a, b },
                        BinaryOp::LtEq => Op::LeInt { dst, a, b },
                        BinaryOp::Gt => Op::GtInt { dst, a, b },
                        BinaryOp::GtEq => Op::GeInt { dst, a, b },
                        _ => unreachable!(),
                    };
                    self.ops.push(inst);
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = dest.unwrap_or_else(|| self.alloc_temp_s());
                let inst = match op {
                    BinaryOp::Lt => Op::LtFloat { dst, a, b },
                    BinaryOp::LtEq => Op::LeFloat { dst, a, b },
                    BinaryOp::Gt => Op::LtFloat { dst, a: b, b: a },
                    BinaryOp::GtEq => Op::LeFloat { dst, a: b, b: a },
                    _ => unreachable!(),
                };
                self.ops.push(inst);
                Ok(dst)
            }
            _ => Err(format!("Unsupported scalar binary op {:?}", op)),
        }
    }

    pub(crate) fn lower_list_lit(&mut self, elems: &[Expr], list_ty: &Type) -> Result<u16, String> {
        let elem_ty = match list_ty {
            Type::List(inner) => inner.as_ref(),
            _ => &Type::Any,
        };
        let cap = elems.len() as u16;
        if elem_ty.is_int() || elem_ty.is_bool() {
            let mut vals = Vec::with_capacity(elems.len());
            for el in elems {
                vals.push(self.lower_scalar(el)?);
            }
            let dst_obj = self.alloc_temp_o();
            self.ops.push(Op::NewListInt { dst_obj, cap });
            for v in vals {
                self.ops.push(Op::ListAppendInt { list_obj: dst_obj, val_s: v });
            }
            Ok(dst_obj)
        } else if elem_ty.is_float() {
            let mut vals = Vec::with_capacity(elems.len());
            for el in elems {
                vals.push(self.lower_scalar(el)?);
            }
            let dst_obj = self.alloc_temp_o();
            self.ops.push(Op::NewListFloat { dst_obj, cap });
            for v in vals {
                self.ops.push(Op::ListAppendFloat { list_obj: dst_obj, val_s: v });
            }
            Ok(dst_obj)
        } else {
            let mut vals = Vec::with_capacity(elems.len());
            for el in elems {
                vals.push(self.lower_obj(el)?);
            }
            let dst_obj = self.alloc_temp_o();
            self.ops.push(Op::NewListObj { dst_obj, cap });
            for v in vals {
                self.ops.push(Op::ListAppendObj { list_obj: dst_obj, val_obj: v });
            }
            Ok(dst_obj)
        }
    }
}
