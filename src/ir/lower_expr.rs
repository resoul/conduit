use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::can_propagate_try;
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
            matches!(&target.kind, ExprKind::Ident(a) if checked.json_aliases.contains(a))
        }
        ExprKind::Ident(name) => checked.json_direct_fns.contains_key(name),
        _ => false,
    }
}

impl<'a, 'b> FnLowerer<'a, 'b> {
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

    pub(crate) fn lower_scalar(&mut self, expr: &Expr) -> Result<u16, String> {
        match &expr.kind {
            ExprKind::Int(n) => {
                let dst = self.alloc_temp_s();
                self.load_int_into(dst, *n)?;
                Ok(dst)
            }
            ExprKind::Float(v) => {
                let bits = v.to_bits();
                if let Some(&reg) = self.hoisted_floats.get(&bits) {
                    return Ok(reg);
                }
                let dst = self.alloc_temp_s();
                self.load_float_into(dst, *v)?;
                Ok(dst)
            }
            ExprKind::Bool(b) => {
                let dst = self.alloc_temp_s();
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
                        let dst = self.alloc_temp_s();
                        self.load_int_into(dst, n.wrapping_neg())?;
                        return Ok(dst);
                    }
                    if let ExprKind::Float(v) = inner.kind {
                        let bits = (-v).to_bits();
                        if let Some(&reg) = self.hoisted_floats.get(&bits) {
                            return Ok(reg);
                        }
                        let dst = self.alloc_temp_s();
                        self.load_float_into(dst, -v)?;
                        return Ok(dst);
                    }
                    let src = self.lower_scalar(inner)?;
                    let dst = self.alloc_temp_s();
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
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::NotBool { dst, src });
                    Ok(dst)
                }
            },
            ExprKind::Binary { op, left, right } => self.lower_binary_scalar(*op, left, right, &expr.ty),
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
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::GetFieldScalar { dst_s, rec_obj, field_idx: f.slot_idx });
                Ok(dst_s)
            }
            ExprKind::Try(inner) => {
                if inner.ty.is_scalar() {
                    return self.lower_scalar(inner);
                }
                let src_obj = self.lower_obj(inner)?;
                let dst_s = self.alloc_temp_s();
                let ret_on_err = can_propagate_try(&inner.ty, &self.return_ty, self.mc.checked);
                self.ops.push(Op::TryUnwrapScalar {
                    dst_s,
                    src_obj,
                    ret_on_err,
                });
                Ok(dst_s)
            }
            ExprKind::Call { .. } => self.lower_call_scalar(expr),
            _ => Err(format!("Unsupported scalar expression {:?}", expr.kind)),
        }
    }

    pub(crate) fn lower_obj(&mut self, expr: &Expr) -> Result<u16, String> {
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
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::LoadConstObj { dst_obj, idx });
                Ok(dst_obj)
            }
            ExprKind::Nil => {
                let idx = self.mc.intern_empty()?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::LoadConstObj { dst_obj, idx });
                Ok(dst_obj)
            }
            ExprKind::Ident(name) => {
                if let Some((slot, _)) = self.lookup_local(name) {
                    if slot.is_scalar {
                        return Err(format!("Expected object '{}' but got scalar", name));
                    }
                    return Ok(slot.reg);
                }
                if let Some(&fid) = self.mc.checked.func_by_name.get(name) {
                    let dst_obj = self.alloc_temp_o();
                    self.ops.push(Op::LoadFuncObj { dst_obj, func_id: fid as u16 });
                    return Ok(dst_obj);
                }
                if self.mc.checked.val_direct_fns.get(name).map(String::as_str) == Some("email") {
                    return self.lower_email_fn_val();
                }
                Err(format!("Undefined object identifier '{}'", name))
            }
            ExprKind::Binary { op: BinaryOp::Add, left, right } if expr.ty == Type::String => {
                let a_obj = self.lower_obj(left)?;
                let b_obj = self.lower_obj(right)?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::StrConcat { dst_obj, a_obj, b_obj });
                Ok(dst_obj)
            }
            ExprKind::Binary { op: BinaryOp::Range, left, right } => {
                let start_s = self.lower_scalar(left)?;
                let end_s = self.lower_scalar(right)?;
                let dst_obj = self.alloc_temp_o();
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
                Type::List(inner) => {
                    let list_obj = self.lower_obj(target)?;
                    let idx_s = self.lower_scalar(index)?;
                    let elem_tag = elem_tag_for(inner);
                    let dst_obj = self.alloc_temp_o();
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
                    let dst_obj = self.alloc_temp_o();
                    self.ops.push(Op::ExtOp { kind: EXT_MAP_INDEX, aux, a: dst_obj, b: map_obj, c: key_reg });
                    Ok(dst_obj)
                }
                _ => Err("Unsupported index target".to_string()),
            },
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(alias) = &target.kind
                    && self.lookup_local(alias).is_none()
                    && self.mc.checked.val_aliases.contains(alias)
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
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::GetFieldObj { dst_obj, rec_obj, field_idx: f.slot_idx });
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
                self.ops.push(Op::TryCheckObj {
                    src_obj: obj_reg,
                    ret_on_err,
                    empty_is_err,
                });
                Ok(obj_reg)
            }
            ExprKind::Closure { params, return_type, body } => {
                self.lower_closure(params, return_type, body, &expr.ty)
            }
            ExprKind::Call { .. } => self.lower_call_obj(expr),
            _ => Err(format!("Unsupported object expression {:?}", expr.kind)),
        }
    }

    fn lower_binary_scalar(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        res_ty: &Type,
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
                    let def_s = self.lower_scalar(right)?;
                    let dst_s = self.alloc_temp_s();
                    self.ops.push(Op::MapGetIntIntOr { dst_s, map_obj, key_s, def_s });
                    return Ok(dst_s);
                }
                let lhs_o = self.lower_obj(left)?;
                let emp = self.alloc_temp_o();
                let emp_idx = self.mc.intern_empty()?;
                self.ops.push(Op::LoadConstObj { dst_obj: emp, idx: emp_idx });
                let is_nil = self.alloc_temp_s();
                self.ops.push(Op::EqObj { dst_s: is_nil, a_obj: lhs_o, b_obj: emp });
                let dst = self.alloc_temp_s();
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
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::FmaFloat { dst, a: ra, b: rb, c: rc });
                    return Ok(dst);
                }
                if let ExprKind::Binary { op: BinaryOp::Mul, left: ma, right: mb } = &right.kind {
                    let rc = self.lower_scalar(left)?;
                    let ra = self.lower_scalar(ma)?;
                    let rb = self.lower_scalar(mb)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::FmaFloat { dst, a: ra, b: rb, c: rc });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::AddFloat { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Add if res_ty.is_int() => {
                if let Some(imm) = as_i16_lit(right) {
                    let a = self.lower_scalar(left)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::AddIntImm { dst, a, imm });
                    return Ok(dst);
                }
                if let Some(imm) = as_i16_lit(left) {
                    let a = self.lower_scalar(right)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::AddIntImm { dst, a, imm });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::AddInt { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Sub if res_ty.is_int() => {
                if let Some(imm) = as_neg_i16_lit(right) {
                    let a = self.lower_scalar(left)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::AddIntImm { dst, a, imm });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::SubInt { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Mod if res_ty.is_int() => {
                if let Some(imm) = as_i16_lit(right) {
                    let a = self.lower_scalar(left)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::ModIntImm { dst, a, imm });
                    return Ok(dst);
                }
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::ModInt { dst, a, b });
                Ok(dst)
            }
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Sub => {
                let a = self.lower_scalar(left)?;
                let b = self.lower_scalar(right)?;
                let dst = self.alloc_temp_s();
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
            BinaryOp::Eq | BinaryOp::NotEq => {
                let is_neq = op == BinaryOp::NotEq;
                if (left.ty.is_int() && right.ty.is_int()) || (left.ty.is_bool() && right.ty.is_bool()) {
                    if let Some(imm) = as_i16_lit(right) && !is_neq {
                        let a = self.lower_scalar(left)?;
                        let dst = self.alloc_temp_s();
                        self.ops.push(Op::EqIntImm { dst, a, imm });
                        return Ok(dst);
                    }
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(if is_neq { Op::NeInt { dst, a, b } } else { Op::EqInt { dst, a, b } });
                    return Ok(dst);
                }
                if left.ty.is_float() && right.ty.is_float() {
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = self.alloc_temp_s();
                    self.ops.push(Op::EqFloat { dst, a, b });
                    if is_neq {
                        let nd = self.alloc_temp_s();
                        self.ops.push(Op::NotBool { dst: nd, src: dst });
                        return Ok(nd);
                    }
                    return Ok(dst);
                }
                let a_obj = self.lower_obj(left)?;
                let b_obj = self.lower_obj(right)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::EqObj { dst_s, a_obj, b_obj });
                if is_neq {
                    let nd = self.alloc_temp_s();
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
                        let dst = self.alloc_temp_s();
                        self.ops.push(Op::LtIntImm { dst, a, imm });
                        return Ok(dst);
                    }
                    let a = self.lower_scalar(left)?;
                    let b = self.lower_scalar(right)?;
                    let dst = self.alloc_temp_s();
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
                let dst = self.alloc_temp_s();
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

    fn lower_list_lit(&mut self, elems: &[Expr], list_ty: &Type) -> Result<u16, String> {
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
