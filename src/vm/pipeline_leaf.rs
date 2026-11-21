use std::sync::Arc;

use crate::ir::{Module, RecordDesc};
use crate::vm::exec_cold::{
    build_record_from_regs, build_record_into_reg, err_div_zero, err_msg, err_overflow,
};
use crate::vm::opcode::Op;
use crate::vm::value::{ClosureData, Obj, StructData};
use crate::vm::{ExecResult, RuntimeError};

#[inline]
pub(crate) fn is_scalar_op(op: &Op) -> bool {
    matches!(
        op,
        Op::LoadImmInt { .. }
            | Op::LoadConstInt { .. }
            | Op::LoadConstFloat { .. }
            | Op::LoadBool { .. }
            | Op::MoveScalar { .. }
            | Op::AddInt { .. }
            | Op::AddIntImm { .. }
            | Op::SubInt { .. }
            | Op::MulInt { .. }
            | Op::MulIntImm { .. }
            | Op::MulAddInt { .. }
            | Op::MulAddIntImm { .. }
            | Op::DivInt { .. }
            | Op::ModInt { .. }
            | Op::ModIntConst { .. }
            | Op::ModIntImm { .. }
            | Op::NegInt { .. }
            | Op::AddFloat { .. }
            | Op::SubFloat { .. }
            | Op::MulFloat { .. }
            | Op::DivFloat { .. }
            | Op::FmaFloat { .. }
            | Op::IntToFloat { .. }
            | Op::FloatFloor { .. }
            | Op::FloatRound { .. }
            | Op::EqInt { .. }
            | Op::NeInt { .. }
            | Op::LtInt { .. }
            | Op::LeInt { .. }
            | Op::GtInt { .. }
            | Op::GeInt { .. }
            | Op::EqIntImm { .. }
            | Op::LtIntImm { .. }
            | Op::EqFloat { .. }
            | Op::LtFloat { .. }
            | Op::LeFloat { .. }
            | Op::NotBool { .. }
            | Op::Jump { .. }
            | Op::JumpIfFalse { .. }
            | Op::JumpIfTrue { .. }
            | Op::JumpIfLtInt { .. }
            | Op::JumpIfLtIntImm { .. }
            | Op::JumpIfEqIntImm { .. }
            | Op::SwitchIntImm { .. }
            | Op::ReturnScalarIfLtIntImm { .. }
            | Op::LoadUpvalueScalar { .. }
            | Op::ReturnScalar { .. }
    )
}

#[inline]
pub(crate) fn is_scalar_leaf_ops(ops: &[Op], num_objs: u16) -> bool {
    num_objs == 0 && ops.iter().all(is_scalar_op)
}

#[inline]
pub(crate) fn is_obj_leaf_ops(ops: &[Op]) -> bool {
    ops.iter().all(|op| {
        is_scalar_op(op)
            || matches!(
                op,
                Op::EqObj { .. }
                    | Op::LoadConstObj { .. }
                    | Op::MoveObj { .. }
                    | Op::ListLen { .. }
                    | Op::StrLen { .. }
                    | Op::MapLen { .. }
                    | Op::NewRecord { .. }
                    | Op::NewRecordMove { .. }
                    | Op::GetFieldScalar { .. }
                    | Op::GetFieldObj { .. }
                    | Op::LoadUpvalueObj { .. }
                    | Op::ReturnObj { .. }
            )
    })
}

#[inline]
pub(crate) fn is_o0_read_only(ops: &[Op], records: &[RecordDesc]) -> bool {
    ops.iter().all(|op| match *op {
        Op::LoadConstObj { dst_obj, .. }
        | Op::MoveObj { dst_obj, .. }
        | Op::NewRecord { dst_obj, .. }
        | Op::GetFieldObj { dst_obj, .. }
        | Op::LoadUpvalueObj { dst: dst_obj, .. } => dst_obj != 0,
        Op::NewRecordMove { dst_obj, desc_idx } => {
            dst_obj != 0 && !records[desc_idx as usize].obj_regs.contains(&0)
        }
        Op::ReturnObj { src } => src != 0,
        Op::EqObj { .. }
        | Op::ListLen { .. }
        | Op::StrLen { .. }
        | Op::MapLen { .. }
        | Op::GetFieldScalar { .. } => true,
        ref other => is_scalar_op(other),
    })
}

/// Executes a leaf scalar pipeline stage with direct unchecked access into validated register windows.
///
/// # Safety
/// `s_ptr` must point to at least `func.num_scalars` valid `u64` slots, all register indices
/// in `ops` must have passed `Module::validate()`, and `ops` must terminate via `ReturnScalar`.
#[inline(always)]
pub(crate) unsafe fn exec_scalar_leaf(
    s_ptr: *mut u64,
    ops: &[Op],
    int_consts: *const i64,
    float_consts: *const f64,
    closure: Option<&ClosureData>,
) -> Result<u64, RuntimeError> {
    let ops_ptr = ops.as_ptr();
    let mut ip = 0usize;

    macro_rules! s_get {
        ($r:expr) => {
            unsafe { *s_ptr.add($r as usize) }
        };
    }
    macro_rules! s_set {
        ($r:expr, $val:expr) => {{
            let val = $val;
            unsafe { *s_ptr.add($r as usize) = val };
        }};
    }

    loop {
        debug_assert!(ip < ops.len());
        let op = unsafe { *ops_ptr.add(ip) };
        ip += 1;
        match op {
            Op::LoadImmInt { dst, imm } => s_set!(dst, (imm as i64) as u64),
            Op::LoadConstInt { dst, idx } => {
                s_set!(dst, unsafe { *int_consts.add(idx as usize) } as u64);
            }
            Op::LoadConstFloat { dst, idx } => {
                s_set!(dst, unsafe { *float_consts.add(idx as usize) }.to_bits());
            }
            Op::LoadBool { dst, val } => s_set!(dst, val as u64),
            Op::MoveScalar { dst, src } => s_set!(dst, s_get!(src)),
            Op::AddInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_add(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::AddIntImm { dst, a, imm } => {
                let Some(res) = (s_get!(a) as i64).checked_add(imm as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::SubInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_sub(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::MulInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_mul(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::MulAddInt { dst, a, b, c } => {
                let product = (s_get!(a) as i64).checked_mul(s_get!(b) as i64).ok_or_else(err_overflow)?;
                let result = product.checked_add(s_get!(c) as i64).ok_or_else(err_overflow)?;
                s_set!(dst, result as u64);
            }
            Op::MulAddIntImm { dst, src, mul, add } => {
                let product = (s_get!(src) as i64).checked_mul(i64::from(mul)).ok_or_else(err_overflow)?;
                let value = product.checked_add(i64::from(add)).ok_or_else(err_overflow)?;
                s_set!(dst, value as u64);
            }
            Op::MulIntImm { dst, a, imm } => {
                let result = (s_get!(a) as i64).checked_mul(i64::from(imm)).ok_or_else(err_overflow)?;
                s_set!(dst, result as u64);
            }
            Op::DivInt { dst, a, b } => {
                let rhs = s_get!(b) as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_div(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::ModInt { dst, a, b } => {
                let rhs = s_get!(b) as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_rem(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::ModIntConst { dst, a, divisor, magic_idx, shift } => {
                // SAFETY: Module validation checks the reciprocal and its constant index.
                let magic = unsafe { *int_consts.add(usize::from(magic_idx)) };
                let rem = crate::vm::opcode::remainder_by_constant(s_get!(a) as i64, divisor, magic, shift);
                s_set!(dst, rem as u64);
            }
            Op::ModIntImm { dst, a, imm } => {
                let rhs = imm as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_rem(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::NegInt { dst, src } => {
                let Some(res) = (s_get!(src) as i64).checked_neg() else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::AddFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) + f64::from_bits(s_get!(b))).to_bits());
            }
            Op::SubFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) - f64::from_bits(s_get!(b))).to_bits());
            }
            Op::MulFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b))).to_bits());
            }
            Op::DivFloat { dst, a, b } => {
                let rhs = f64::from_bits(s_get!(b));
                if rhs == 0.0 {
                    return Err(err_div_zero());
                }
                s_set!(dst, (f64::from_bits(s_get!(a)) / rhs).to_bits());
            }
            Op::FmaFloat { dst, a, b, c } => {
                let res = f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b))
                    + f64::from_bits(s_get!(c));
                s_set!(dst, res.to_bits());
            }
            Op::IntToFloat { dst, src } => {
                s_set!(dst, ((s_get!(src) as i64) as f64).to_bits());
            }
            Op::FloatFloor { dst, src } => {
                s_set!(dst, f64::from_bits(s_get!(src)).floor().to_bits());
            }
            Op::FloatRound { dst, src } => {
                s_set!(dst, f64::from_bits(s_get!(src)).round().to_bits());
            }
            Op::EqInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) == (s_get!(b) as i64)) as u64);
            }
            Op::NeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) != (s_get!(b) as i64)) as u64);
            }
            Op::LtInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) < (s_get!(b) as i64)) as u64);
            }
            Op::LeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) <= (s_get!(b) as i64)) as u64);
            }
            Op::GtInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) > (s_get!(b) as i64)) as u64);
            }
            Op::GeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) >= (s_get!(b) as i64)) as u64);
            }
            Op::EqIntImm { dst, a, imm } => {
                s_set!(dst, ((s_get!(a) as i64) == imm as i64) as u64);
            }
            Op::LtIntImm { dst, a, imm } => {
                s_set!(dst, ((s_get!(a) as i64) < imm as i64) as u64);
            }
            Op::EqFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) == f64::from_bits(s_get!(b))) as u64);
            }
            Op::LtFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) < f64::from_bits(s_get!(b))) as u64);
            }
            Op::LeFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) <= f64::from_bits(s_get!(b))) as u64);
            }
            Op::NotBool { dst, src } => s_set!(dst, (s_get!(src) == 0) as u64),
            Op::Jump { target } => ip = target as usize,
            Op::JumpIfFalse { cond, target } => {
                if s_get!(cond) == 0 {
                    ip = target as usize;
                }
            }
            Op::JumpIfTrue { cond, target } => {
                if s_get!(cond) != 0 {
                    ip = target as usize;
                }
            }
            Op::JumpIfLtInt { a, b, target, when_true } => {
                if ((s_get!(a) as i64) < (s_get!(b) as i64)) == when_true {
                    ip = target as usize;
                }
            }
            Op::JumpIfEqIntImm { a, imm, target, when_true } => {
                if ((s_get!(a) as i64) == i64::from(imm)) == when_true {
                    ip = target as usize;
                }
            }
            Op::SwitchIntImm { src, first, count } => {
                let slot = s_get!(src).wrapping_sub(i64::from(first) as u64);
                let offset = if slot < u64::from(count) { slot as usize + 1 } else { 0 };
                if let Op::Jump { target } = ops[ip + offset] {
                    ip = target as usize;
                }
            }
            Op::JumpIfLtIntImm { a, imm, target, when_true } => {
                if ((s_get!(a) as i64) < i64::from(imm)) == when_true {
                    ip = target as usize;
                }
            }
            Op::LoadUpvalueScalar { dst, idx } => {
                let Some(c) = closure else {
                    return Err(err_msg("Missing active closure"));
                };
                s_set!(dst, c.get_scalar(idx as usize));
            }
            Op::ReturnScalarIfLtIntImm { a, imm, src } => {
                if (s_get!(a) as i64) < i64::from(imm) {
                    return Ok(s_get!(src));
                }
            }
            Op::ReturnScalar { src } => return Ok(s_get!(src)),
            _ => return Err(err_msg("Unexpected op in scalar leaf pipeline")),
        }
    }
}

/// Executes a leaf object pipeline stage without clearing `o_ptr` on every return.
/// Caller clears `o_win[..max_o]` once after the pipeline loop completes.
///
/// # Safety
/// `s_ptr` and `o_ptr` must cover `func.num_scalars` / `func.num_objs` slots,
/// and `ops` must have passed `Module::validate()`.
#[inline]
pub(crate) unsafe fn exec_obj_leaf(
    s_ptr: *mut u64,
    o_ptr: *mut Obj,
    ops: &[Op],
    module: &Module,
    closure: Option<&ClosureData>,
) -> Result<ExecResult, RuntimeError> {
    let ops_ptr = ops.as_ptr();
    let int_consts = module.int_consts.as_ptr();
    let float_consts = module.float_consts.as_ptr();
    let mut ip = 0usize;

    macro_rules! s_get {
        ($r:expr) => {
            unsafe { *s_ptr.add($r as usize) }
        };
    }
    macro_rules! s_set {
        ($r:expr, $val:expr) => {{
            let val = $val;
            unsafe { *s_ptr.add($r as usize) = val };
        }};
    }
    macro_rules! o_ref {
        ($r:expr) => {
            unsafe { &*o_ptr.add($r as usize) }
        };
    }
    macro_rules! o_mut {
        ($r:expr) => {
            unsafe { &mut *o_ptr.add($r as usize) }
        };
    }

    loop {
        debug_assert!(ip < ops.len());
        let op = unsafe { *ops_ptr.add(ip) };
        ip += 1;
        match op {
            Op::LoadImmInt { dst, imm } => s_set!(dst, (imm as i64) as u64),
            Op::LoadConstInt { dst, idx } => {
                s_set!(dst, unsafe { *int_consts.add(idx as usize) } as u64);
            }
            Op::LoadConstFloat { dst, idx } => {
                s_set!(dst, unsafe { *float_consts.add(idx as usize) }.to_bits());
            }
            Op::LoadBool { dst, val } => s_set!(dst, val as u64),
            Op::MoveScalar { dst, src } => s_set!(dst, s_get!(src)),
            Op::AddInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_add(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::AddIntImm { dst, a, imm } => {
                let Some(res) = (s_get!(a) as i64).checked_add(imm as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::SubInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_sub(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::MulInt { dst, a, b } => {
                let Some(res) = (s_get!(a) as i64).checked_mul(s_get!(b) as i64) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::MulAddInt { dst, a, b, c } => {
                let product = (s_get!(a) as i64).checked_mul(s_get!(b) as i64).ok_or_else(err_overflow)?;
                let result = product.checked_add(s_get!(c) as i64).ok_or_else(err_overflow)?;
                s_set!(dst, result as u64);
            }
            Op::MulAddIntImm { dst, src, mul, add } => {
                let product = (s_get!(src) as i64).checked_mul(i64::from(mul)).ok_or_else(err_overflow)?;
                let value = product.checked_add(i64::from(add)).ok_or_else(err_overflow)?;
                s_set!(dst, value as u64);
            }
            Op::MulIntImm { dst, a, imm } => {
                let result = (s_get!(a) as i64).checked_mul(i64::from(imm)).ok_or_else(err_overflow)?;
                s_set!(dst, result as u64);
            }
            Op::DivInt { dst, a, b } => {
                let rhs = s_get!(b) as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_div(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::ModInt { dst, a, b } => {
                let rhs = s_get!(b) as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_rem(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::ModIntConst { dst, a, divisor, magic_idx, shift } => {
                // SAFETY: Module validation checks the reciprocal and its constant index.
                let magic = unsafe { *int_consts.add(usize::from(magic_idx)) };
                let rem = crate::vm::opcode::remainder_by_constant(s_get!(a) as i64, divisor, magic, shift);
                s_set!(dst, rem as u64);
            }
            Op::ModIntImm { dst, a, imm } => {
                let rhs = imm as i64;
                if rhs == 0 {
                    return Err(err_div_zero());
                }
                let Some(res) = (s_get!(a) as i64).checked_rem(rhs) else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::NegInt { dst, src } => {
                let Some(res) = (s_get!(src) as i64).checked_neg() else {
                    return Err(err_overflow());
                };
                s_set!(dst, res as u64);
            }
            Op::AddFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) + f64::from_bits(s_get!(b))).to_bits());
            }
            Op::SubFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) - f64::from_bits(s_get!(b))).to_bits());
            }
            Op::MulFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b))).to_bits());
            }
            Op::DivFloat { dst, a, b } => {
                let rhs = f64::from_bits(s_get!(b));
                if rhs == 0.0 {
                    return Err(err_div_zero());
                }
                s_set!(dst, (f64::from_bits(s_get!(a)) / rhs).to_bits());
            }
            Op::FmaFloat { dst, a, b, c } => {
                let res = f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b))
                    + f64::from_bits(s_get!(c));
                s_set!(dst, res.to_bits());
            }
            Op::IntToFloat { dst, src } => {
                s_set!(dst, ((s_get!(src) as i64) as f64).to_bits());
            }
            Op::FloatFloor { dst, src } => {
                s_set!(dst, f64::from_bits(s_get!(src)).floor().to_bits());
            }
            Op::FloatRound { dst, src } => {
                s_set!(dst, f64::from_bits(s_get!(src)).round().to_bits());
            }
            Op::EqInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) == (s_get!(b) as i64)) as u64);
            }
            Op::NeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) != (s_get!(b) as i64)) as u64);
            }
            Op::LtInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) < (s_get!(b) as i64)) as u64);
            }
            Op::LeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) <= (s_get!(b) as i64)) as u64);
            }
            Op::GtInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) > (s_get!(b) as i64)) as u64);
            }
            Op::GeInt { dst, a, b } => {
                s_set!(dst, ((s_get!(a) as i64) >= (s_get!(b) as i64)) as u64);
            }
            Op::EqIntImm { dst, a, imm } => {
                s_set!(dst, ((s_get!(a) as i64) == imm as i64) as u64);
            }
            Op::LtIntImm { dst, a, imm } => {
                s_set!(dst, ((s_get!(a) as i64) < imm as i64) as u64);
            }
            Op::EqFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) == f64::from_bits(s_get!(b))) as u64);
            }
            Op::LtFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) < f64::from_bits(s_get!(b))) as u64);
            }
            Op::LeFloat { dst, a, b } => {
                s_set!(dst, (f64::from_bits(s_get!(a)) <= f64::from_bits(s_get!(b))) as u64);
            }
            Op::NotBool { dst, src } => s_set!(dst, (s_get!(src) == 0) as u64),
            Op::Jump { target } => ip = target as usize,
            Op::JumpIfFalse { cond, target } => {
                if s_get!(cond) == 0 {
                    ip = target as usize;
                }
            }
            Op::JumpIfTrue { cond, target } => {
                if s_get!(cond) != 0 {
                    ip = target as usize;
                }
            }
            Op::JumpIfLtInt { a, b, target, when_true } => {
                if ((s_get!(a) as i64) < (s_get!(b) as i64)) == when_true {
                    ip = target as usize;
                }
            }
            Op::JumpIfEqIntImm { a, imm, target, when_true } => {
                if ((s_get!(a) as i64) == i64::from(imm)) == when_true {
                    ip = target as usize;
                }
            }
            Op::SwitchIntImm { src, first, count } => {
                let slot = s_get!(src).wrapping_sub(i64::from(first) as u64);
                let offset = if slot < u64::from(count) { slot as usize + 1 } else { 0 };
                if let Op::Jump { target } = ops[ip + offset] {
                    ip = target as usize;
                }
            }
            Op::JumpIfLtIntImm { a, imm, target, when_true } => {
                if ((s_get!(a) as i64) < i64::from(imm)) == when_true {
                    ip = target as usize;
                }
            }
            Op::LoadUpvalueScalar { dst, idx } => {
                let Some(c) = closure else {
                    return Err(err_msg("Missing active closure"));
                };
                s_set!(dst, c.get_scalar(idx as usize));
            }
            Op::EqObj {
                dst_s,
                a_obj,
                b_obj,
            } => s_set!(dst_s, (o_ref!(a_obj) == o_ref!(b_obj)) as u64),
            Op::LoadConstObj { dst_obj, idx } => {
                *o_mut!(dst_obj) = unsafe { module.obj_consts.get_unchecked(idx as usize) }.clone();
            }
            Op::MoveObj { dst_obj, src } => *o_mut!(dst_obj) = o_ref!(src).clone(),
            Op::ListLen { dst_s, list_obj } => {
                let len = match o_ref!(list_obj) {
                    Obj::List(l) => l.len() as u64,
                    Obj::Bytes(bytes) => bytes.len() as u64,
                    _ => return Err(err_msg("Expected List in ListLen")),
                };
                s_set!(dst_s, len);
            }
            Op::StrLen { dst_s, str_obj } => {
                let len = match o_ref!(str_obj) {
                    Obj::Str(st) => st.len() as u64,
                    _ => return Err(err_msg("Expected Str in StrLen")),
                };
                s_set!(dst_s, len);
            }
            Op::MapLen { dst_s, map_obj } => {
                let len = match o_ref!(map_obj) {
                    Obj::Map(m) => m.len() as u64,
                    Obj::DeferredField(df) => {
                        if df.as_single_map_entry().is_some() {
                            1
                        } else if let Obj::Map(m) = df.value() {
                            m.len() as u64
                        } else {
                            return Err(err_msg("Expected Map in MapLen"));
                        }
                    }
                    _ => return Err(err_msg("Expected Map in MapLen")),
                };
                s_set!(dst_s, len);
            }
            Op::NewRecord { dst_obj, desc_idx } => {
                let desc = unsafe { module.records.get_unchecked(desc_idx as usize) };
                unsafe { build_record_into_reg(desc, s_ptr, o_ptr, dst_obj, false) };
            }
            Op::NewRecordMove { dst_obj, desc_idx } => {
                let desc = unsafe { module.records.get_unchecked(desc_idx as usize) };
                let rec = unsafe { build_record_from_regs(desc, s_ptr, o_ptr, true) };
                if ip < ops.len()
                    && matches!(unsafe { *ops_ptr.add(ip) }, Op::ReturnObj { src } if src == dst_obj)
                {
                    return Ok(ExecResult::Obj(rec));
                }
                *o_mut!(dst_obj) = rec;
            }
            Op::GetFieldScalar {
                dst_s,
                rec_obj,
                field_idx,
            } => {
                let val = match o_ref!(rec_obj) {
                    Obj::Record(rec) => rec.get_scalar(field_idx as usize),
                    _ => return Err(err_msg("Expected Record in GetFieldScalar")),
                };
                s_set!(dst_s, val);
            }
            Op::GetFieldObj {
                dst_obj,
                rec_obj,
                field_idx,
            } => {
                let val = match o_ref!(rec_obj) {
                    Obj::Record(rec) => rec.get_obj_keep_single_map(field_idx as usize).clone(),
                    _ => return Err(err_msg("Expected Record in GetFieldObj")),
                };
                *o_mut!(dst_obj) = val;
            }
            Op::LoadUpvalueObj { dst, idx } => {
                let Some(c) = closure else {
                    return Err(err_msg("Missing active closure"));
                };
                *o_mut!(dst) = c.get_obj(idx as usize).clone();
            }
            Op::ReturnObj { src } => {
                return Ok(ExecResult::Obj(std::mem::take(o_mut!(src))));
            }
            Op::ReturnScalarIfLtIntImm { a, imm, src } => {
                if (s_get!(a) as i64) < i64::from(imm) {
                    return Ok(ExecResult::Scalar(s_get!(src)));
                }
            }
            Op::ReturnScalar { src } => return Ok(ExecResult::Scalar(s_get!(src))),
            _ => return Err(err_msg("Unexpected op in obj leaf pipeline")),
        }
    }
}

/// Fast path for single-stage read-only record field bump maps.
#[inline(never)]
pub(crate) fn try_run_record_bump_stage(
    src: &[Obj],
    ops: &[Op],
    records: &[RecordDesc],
    out: &mut Vec<Obj>,
) -> Option<Result<(), RuntimeError>> {
    let [
        Op::GetFieldScalar {
            dst_s,
            rec_obj: 0,
            field_idx: 0,
        },
        Op::AddIntImm { dst: add_dst, a, imm },
        Op::GetFieldObj {
            dst_obj: o1,
            rec_obj: 0,
            field_idx: f_o0,
        },
        Op::GetFieldObj {
            dst_obj: o2,
            rec_obj: 0,
            field_idx: f_o1,
        },
        Op::NewRecord {
            dst_obj: rec_dst,
            desc_idx,
        }
        | Op::NewRecordMove {
            dst_obj: rec_dst,
            desc_idx,
        },
        Op::ReturnObj { src: ret_src },
    ] = *ops
    else {
        return None;
    };
    if a != dst_s || ret_src != rec_dst || o1 == 0 || o2 == 0 || o1 == o2 || f_o0 > 1 || f_o1 > 1 {
        return None;
    }
    let desc = records.get(desc_idx as usize)?;
    if desc.scalar_regs.as_ref() != [add_dst] || desc.obj_regs.as_ref() != [o1, o2] {
        return None;
    }

    let start_len = out.len();
    let schema_id = desc.schema_id;
    let delta = imm as i64;
    let i0 = f_o0 as usize;
    let i1 = f_o1 as usize;

    for item in src {
        let Obj::Record(rec) = item else {
            return Some(Err(err_msg("Expected Record in GetFieldScalar")));
        };
        if rec.scalar_len < 1 || rec.obj_len < 2 || rec.overflow.is_some() {
            out.truncate(start_len);
            return None;
        }
        let Some(next_val) = (rec.inline_scalars[0] as i64).checked_add(delta) else {
            return Some(Err(err_overflow()));
        };
        let obj0 = rec.inline_objs[i0].clone();
        let obj1 = rec.inline_objs[i1].clone();
        out.push(Obj::Record(Arc::new(StructData {
            schema_id,
            scalar_len: 1,
            obj_len: 2,
            overflow: None,
            inline_scalars: [next_val as u64, 0],
            inline_objs: [obj0, obj1],
        })));
    }
    Some(Ok(()))
}
