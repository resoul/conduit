use std::sync::Arc;

use crate::ir::Module;
use crate::native::validation::{box_scalar_val, is_err_or_nil_obj, is_type_tag_match};
use crate::vm::exec_cold::{
    build_record_into_reg, err_div_zero, err_msg, err_overflow, finish_range_kernel,
    map_get_int_or_slot, map_set_int_slot, sum_int_map_range,
};
use crate::vm::exec_union::{format_try_error, is_try_fail_obj, unbox_scalar_val};
use crate::vm::opcode::Op;
use crate::vm::value::{ListStorage, Obj, OrderedMap, append_int_cow, append_obj_cow};
use crate::vm::{CallFrame, ExecResult, RuntimeError, Vm};

impl Vm {
    pub(crate) fn exec_from(
        &mut self,
        module: &Module,
        entry_func_id: u32,
        entry_s_base: usize,
        entry_o_base: usize,
        entry_closure_slot: usize,
    ) -> Result<ExecResult, RuntimeError> {
        let base_frame_depth = self.frame_depth;
        let mut func_id = entry_func_id;
        let mut func = &module.functions[func_id as usize];
        let mut ops_ptr = func.ops.as_ptr();
        let mut ip: usize = 0;
        let mut s_base = entry_s_base;
        let mut o_base = entry_o_base;
        let mut cur_closure_slot = entry_closure_slot;

        self.ensure_capacity(
            s_base + func.num_scalars as usize,
            o_base + func.num_objs as usize,
        );
        // SAFETY: `ensure_capacity` sized `scalars` and `objs` to cover `[s_base .. s_base + num_scalars]`
        // and `[o_base .. o_base + num_objs]`; `Module::validate` guarantees all register operands stay
        // within each function's declared frame window.
        let mut s_ptr = unsafe { self.scalars.as_mut_ptr().add(s_base) };
        let mut o_ptr = unsafe { self.objs.as_mut_ptr().add(o_base) };

        let int_consts = module.int_consts.as_ptr();
        let float_consts = module.float_consts.as_ptr();
        let obj_consts = &module.obj_consts;

        macro_rules! s_get {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_scalars as usize);
                // SAFETY: `idx < func.num_scalars` is verified by `Module::validate` and capacity by `ensure_capacity`.
                unsafe { *s_ptr.add(idx) }
            }};
        }
        macro_rules! s_set {
            ($r:expr, $val:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_scalars as usize);
                let val = $val;
                // SAFETY: `idx < func.num_scalars` is verified by `Module::validate` and capacity by `ensure_capacity`.
                unsafe { *s_ptr.add(idx) = val };
            }};
        }
        macro_rules! o_ref {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_objs as usize);
                // SAFETY: `idx < func.num_objs` is verified by `Module::validate` and capacity by `ensure_capacity`.
                unsafe { &*o_ptr.add(idx) }
            }};
        }
        macro_rules! o_mut {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_objs as usize);
                // SAFETY: `idx < func.num_objs` is verified by `Module::validate` and capacity by `ensure_capacity`.
                unsafe { &mut *o_ptr.add(idx) }
            }};
        }
        macro_rules! switch_jump {
            ($slot:expr, $count:expr) => {{
                let slot = $slot;
                let offset = if slot < u64::from($count) { slot as usize + 1 } else { 0 };
                // SAFETY: `Module::validate` verifies every switch entry is an in-bounds `Op::Jump`.
                if let Op::Jump { target } = unsafe { *ops_ptr.add(ip + offset) } {
                    ip = target as usize;
                }
            }};
        }
        macro_rules! sync_frame_ptrs {
            () => {
                // SAFETY: Frame pointers are recomputed from live `Vec` allocations after any growth.
                s_ptr = unsafe { self.scalars.as_mut_ptr().add(s_base) };
                o_ptr = unsafe { self.objs.as_mut_ptr().add(o_base) };
            };
        }
        macro_rules! return_scalar_val {
            ($val:expr) => {{
                let val = $val;
                if func.num_objs > 0 {
                    self.clear_obj_range(o_base, func.num_objs as usize);
                }
                let Some(frame) = self.pop_frame(base_frame_depth) else {
                    return Ok(ExecResult::Scalar(val));
                };
                ip = frame.return_ip as usize;
                cur_closure_slot = frame.unpack_closure_slot();
                let caller_s_base = frame.s_base as usize;
                if frame.func_id == func_id && func.num_objs == 0 {
                    // SAFETY: `s_base >= caller_s_base` and both frames lie within `self.scalars`.
                    s_ptr = unsafe { s_ptr.sub(s_base - caller_s_base) };
                    s_base = caller_s_base;
                } else {
                    func_id = frame.func_id;
                    // SAFETY: `frame.func_id` came from a validated `module.functions` entry.
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    s_base = caller_s_base;
                    o_base = frame.o_base as usize;
                    sync_frame_ptrs!();
                }
                s_set!(frame.ret_reg, val);
            }};
        }
        macro_rules! return_obj_val {
            ($val:expr) => {{
                let val = $val;
                let Some(frame) = self.finish_return_frame(o_base, func.num_objs, base_frame_depth) else {
                    return Ok(ExecResult::Obj(val));
                };
                func_id = frame.func_id;
                // SAFETY: `frame.func_id` came from a validated `module.functions` entry.
                func = unsafe { module.functions.get_unchecked(func_id as usize) };
                ops_ptr = func.ops.as_ptr();
                ip = frame.return_ip as usize;
                s_base = frame.s_base as usize;
                o_base = frame.o_base as usize;
                cur_closure_slot = frame.unpack_closure_slot();
                sync_frame_ptrs!();
                *o_mut!(frame.ret_reg) = val;
            }};
        }

        loop {
            debug_assert!(ip < func.ops.len());
            // SAFETY: `Module::validate` guarantees `ip` and jump targets remain inside `func.ops`.
            let op = unsafe { *ops_ptr.add(ip) };
            ip += 1;

            match op {
                Op::LoadImmInt { dst, imm } => s_set!(dst, (imm as i64) as u64),
                Op::LoadConstInt { dst, idx } => {
                    // SAFETY: `idx` is bounds-checked against `module.int_consts` by `Module::validate`.
                    s_set!(dst, unsafe { *int_consts.add(idx as usize) } as u64);
                }
                Op::LoadConstFloat { dst, idx } => {
                    // SAFETY: `idx` is bounds-checked against `module.float_consts` by `Module::validate`.
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
                    let value = (s_get!(a) as i64).checked_mul(i64::from(imm)).ok_or_else(err_overflow)?;
                    s_set!(dst, value as u64);
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
                    let lhs = s_get!(a) as i64;
                    let res = if rhs > 0 && (rhs & (rhs - 1)) == 0 {
                        let mask = rhs - 1;
                        if lhs >= 0 {
                            lhs & mask
                        } else {
                            -(lhs.wrapping_neg() & mask)
                        }
                    } else {
                        let Some(r) = lhs.checked_rem(rhs) else {
                            return Err(err_overflow());
                        };
                        r
                    };
                    s_set!(dst, res as u64);
                }
                Op::BitAndInt { dst, a, b } => s_set!(dst, s_get!(a) & s_get!(b)),
                Op::BitOrInt { dst, a, b } => s_set!(dst, s_get!(a) | s_get!(b)),
                Op::BitXorInt { dst, a, b } => s_set!(dst, s_get!(a) ^ s_get!(b)),
                Op::ShiftLeftInt { dst, a, b } | Op::ShiftRightInt { dst, a, b } => {
                    let shift = s_get!(b);
                    if shift >= u64::from(i64::BITS) {
                        return Err(err_msg("Shift count must be in 0..64"));
                    }
                    let value = s_get!(a) as i64;
                    let result = match op {
                        Op::ShiftLeftInt { .. } => value << shift,
                        _ => value >> shift,
                    };
                    s_set!(dst, result as u64);
                }
                Op::NegInt { dst, src } => {
                    let Some(res) = (s_get!(src) as i64).checked_neg() else {
                        return Err(err_overflow());
                    };
                    s_set!(dst, res as u64);
                }
                Op::AddFloat { dst, a, b } => {
                    let res = f64::from_bits(s_get!(a)) + f64::from_bits(s_get!(b));
                    s_set!(dst, res.to_bits());
                }
                Op::SubFloat { dst, a, b } => {
                    let res = f64::from_bits(s_get!(a)) - f64::from_bits(s_get!(b));
                    s_set!(dst, res.to_bits());
                }
                Op::MulFloat { dst, a, b } => {
                    let res = f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b));
                    s_set!(dst, res.to_bits());
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
                    let eq = f64::from_bits(s_get!(a)) == f64::from_bits(s_get!(b));
                    s_set!(dst, eq as u64);
                }
                Op::LtFloat { dst, a, b } => {
                    let lt = f64::from_bits(s_get!(a)) < f64::from_bits(s_get!(b));
                    s_set!(dst, lt as u64);
                }
                Op::LeFloat { dst, a, b } => {
                    let le = f64::from_bits(s_get!(a)) <= f64::from_bits(s_get!(b));
                    s_set!(dst, le as u64);
                }
                Op::NotBool { dst, src } => s_set!(dst, (s_get!(src) == 0) as u64),
                Op::EqObj { dst_s, a_obj, b_obj } => {
                    let eq = o_ref!(a_obj) == o_ref!(b_obj);
                    s_set!(dst_s, eq as u64);
                }
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
                Op::JumpIfLtIntImm { a, imm, target, when_true } => {
                    if ((s_get!(a) as i64) < imm as i64) == when_true {
                        ip = target as usize;
                    }
                }
                Op::JumpIfEqIntImm { a, imm, target, when_true } => {
                    if ((s_get!(a) as i64) == i64::from(imm)) == when_true {
                        ip = target as usize;
                    }
                }
                Op::MakeScalarEnum { dst_s, schema_id, a, b, len } => {
                    // The low half holds the schema ID. The high half holds the payload length.
                    let first = if len >= 1 { s_get!(a) } else { 0 };
                    let second = if len == 2 { s_get!(b) } else { 0 };
                    if len >= 1 { s_set!(dst_s + 1, first); }
                    if len == 2 { s_set!(dst_s + 2, second); }
                    s_set!(dst_s, u64::from(schema_id) | (u64::from(len) << 32));
                }
                Op::BoxScalarEnum { dst_obj, src_obj, tag_s, width } => {
                    let tag = s_get!(tag_s);
                    let value = if tag == u64::MAX {
                        o_ref!(src_obj).clone()
                    } else {
                        let len = tag >> 32;
                        if len > u64::from(width) {
                            return Err(err_msg("Invalid scalar enum length"));
                        }
                        let mut record = crate::vm::value::StructData::new(tag as u32, len as u16, 0);
                        for field in 0..len as u16 {
                            record.set_scalar(usize::from(field), s_get!(tag_s + 1 + field));
                        }
                        Obj::Record(Arc::new(record))
                    };
                    *o_mut!(dst_obj) = value;
                }
                Op::ReadScalarEnumField { dst_s, tag_s, src_obj, field_idx } => {
                    let tag = s_get!(tag_s);
                    let value = if tag == u64::MAX {
                        match o_ref!(src_obj) {
                            Obj::Record(rec) => rec.get_scalar(usize::from(field_idx)),
                            _ => return Err(err_msg("Expected enum record")),
                        }
                    } else {
                        if u64::from(field_idx) >= tag >> 32 {
                            return Err(err_msg("Scalar enum field out of bounds"));
                        }
                        s_get!(tag_s + 1 + u16::from(field_idx))
                    };
                    s_set!(dst_s, value);
                }
                Op::ReadScalarEnumPair { dst_a, dst_b, tag_s, src_obj, field_a, field_b } => {
                    let tag = s_get!(tag_s);
                    let (a, b) = if tag == u64::MAX {
                        match o_ref!(src_obj) {
                            Obj::Record(rec) => (
                                rec.get_scalar(usize::from(field_a)),
                                rec.get_scalar(usize::from(field_b)),
                            ),
                            _ => return Err(err_msg("Expected enum record")),
                        }
                    } else {
                        if u64::from(field_a.max(field_b)) >= tag >> 32 {
                            return Err(err_msg("Scalar enum field out of bounds"));
                        }
                        (
                            s_get!(tag_s + 1 + u16::from(field_a)),
                            s_get!(tag_s + 1 + u16::from(field_b)),
                        )
                    };
                    s_set!(dst_a, a);
                    s_set!(dst_b, b);
                }
                Op::SwitchIntImm { src, first, count } => {
                    switch_jump!(s_get!(src).wrapping_sub(i64::from(first) as u64), count);
                }
                Op::SwitchTag { src_obj, first_tag, tag_count }
                | Op::SwitchScalarTag { src_obj, first_tag, tag_count, .. } => {
                    let tag = match op {
                        Op::SwitchScalarTag { tag_s, .. } => s_get!(tag_s),
                        _ => u64::MAX,
                    };
                    let slot = if tag != u64::MAX {
                        (tag as u32).wrapping_sub(first_tag)
                    } else {
                        match o_ref!(src_obj) {
                            Obj::Record(rec) => rec.schema_id.wrapping_sub(first_tag),
                            _ => u32::MAX,
                        }
                    };
                    switch_jump!(u64::from(slot), tag_count);
                }
                Op::AppendIntRangeTail { list_obj, idx_s, limit_s } => {
                    let start = (s_get!(idx_s) as i64).checked_add(1).ok_or_else(err_overflow)?;
                    let end = s_get!(limit_s) as i64;
                    if start < end {
                        let Obj::List(ListStorage::Int(items)) = o_mut!(list_obj) else {
                            return Err(err_msg("Expected [Int] list in AppendIntRangeTail"));
                        };
                        let items = Arc::make_mut(items);
                        let count = usize::try_from(start.abs_diff(end)).map_err(|_| err_overflow())?;
                        items.try_reserve(count).map_err(|_| err_msg("Cannot reserve list capacity"))?;
                        items.extend(start..end);
                    }
                    s_set!(idx_s, start.max(end) as u64);
                }
                Op::ForRangeStep {
                    idx_reg,
                    limit_reg,
                    target,
                } => {
                    let cur = s_get!(idx_reg) as i64;
                    let Some(next) = cur.checked_add(1) else {
                        return Err(err_overflow());
                    };
                    let limit = s_get!(limit_reg) as i64;
                    if next < limit {
                        let target = target as usize;
                        let step = ip - 1;
                        let may_use_arithmetic_kernel = match step.checked_sub(target) {
                            Some(1) => matches!(func.ops[target], Op::AddInt { .. }),
                            Some(3) => matches!(
                                func.ops[target],
                                Op::IntToFloat { .. } | Op::MulAddInt { .. }
                            ),
                            _ => false,
                        };
                        if may_use_arithmetic_kernel {
                            let finished = finish_range_kernel(
                                &func.ops[target..step],
                                &mut self.scalars[s_base..s_base + usize::from(func.num_scalars)],
                                &module.int_consts,
                                idx_reg, limit_reg, next, limit,
                            )?;
                            sync_frame_ptrs!();
                            if finished {
                                continue;
                            }
                        }
                        ip = target;
                    }
                    s_set!(idx_reg, next as u64);
                }
                Op::CallSelfIntImm { arg_s, arg_o, src, imm } => {
                    let arg = (s_get!(src) as i64).checked_add(i64::from(imm)).ok_or_else(err_overflow)?;
                    s_set!(arg_s, arg as u64);
                    let start_ip = if func.num_objs == 0
                        && let Some(Op::ReturnScalarIfLtIntImm { a: 0, src: 0, imm: limit }) = func.ops.first()
                    {
                        if arg < i64::from(*limit) {
                            // This still counts as a call at the language's depth limit.
                            if self.frame_depth >= crate::vm::MAX_CALL_DEPTH {
                                return Err(crate::vm::exec_cold::err_call_depth());
                            }
                            continue;
                        }
                        1
                    } else {
                        0
                    };
                    self.push_frame(CallFrame {
                        func_id,
                        return_ip: ip as u32,
                        s_base: s_base as u32,
                        o_base: o_base as u32,
                        ret_reg: arg_s,
                        prev_closure_slot: CallFrame::pack_closure_slot(cur_closure_slot),
                    })?;
                    ip = start_ip;
                    s_base += usize::from(arg_s);
                    o_base += usize::from(arg_o);
                    cur_closure_slot = usize::MAX;
                    let s_needed = s_base + usize::from(func.num_scalars);
                    let o_needed = o_base + usize::from(func.num_objs);
                    if s_needed > self.scalars.len() || o_needed > self.objs.len() {
                        self.ensure_capacity(s_needed, o_needed);
                        sync_frame_ptrs!();
                    } else {
                        // SAFETY: Capacity check above ensures the new self-call frame fits within live buffers.
                        s_ptr = unsafe { s_ptr.add(usize::from(arg_s)) };
                        o_ptr = unsafe { o_ptr.add(usize::from(arg_o)) };
                    }
                }
                Op::CallDirect {
                    func_id: callee_id,
                    arg_base_s,
                    arg_base_o,
                    ret_reg,
                } => {
                    self.push_frame(CallFrame {
                        func_id,
                        return_ip: ip as u32,
                        s_base: s_base as u32,
                        o_base: o_base as u32,
                        ret_reg,
                        prev_closure_slot: CallFrame::pack_closure_slot(cur_closure_slot),
                    })?;
                    func_id = callee_id as u32;
                    // SAFETY: `callee_id` is bounds-checked against `module.functions` by `Module::validate`.
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = 0;
                    s_base += arg_base_s as usize;
                    o_base += arg_base_o as usize;
                    cur_closure_slot = usize::MAX;
                    let s_needed = s_base + func.num_scalars as usize;
                    let o_needed = o_base + func.num_objs as usize;
                    if s_needed > self.scalars.len() || o_needed > self.objs.len() {
                        self.ensure_capacity(s_needed, o_needed);
                        sync_frame_ptrs!();
                    } else {
                        // SAFETY: Capacity check above ensures the callee frame fits within live buffers.
                        s_ptr = unsafe { s_ptr.add(arg_base_s as usize) };
                        o_ptr = unsafe { o_ptr.add(arg_base_o as usize) };
                    }
                }
                Op::CallCallable {
                    callee_obj,
                    arg_base_s,
                    arg_base_o,
                    ret_reg,
                } => {
                    let (nf, nsb, nob, ncs) = self.exec_call_callable(
                        module,
                        func_id,
                        func.num_scalars,
                        func.num_objs,
                        ip,
                        s_base,
                        o_base,
                        cur_closure_slot,
                        callee_obj,
                        arg_base_s,
                        arg_base_o,
                        ret_reg,
                    )?;
                    func_id = nf;
                    // SAFETY: `exec_call_callable` validates `nf < module.functions.len()`.
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = 0;
                    s_base = nsb;
                    o_base = nob;
                    cur_closure_slot = ncs;
                    sync_frame_ptrs!();
                }
                Op::MakeClosure { dst_obj, proto_idx } => {
                    self.exec_make_closure(module, s_base, o_base, dst_obj, proto_idx);
                    sync_frame_ptrs!();
                }
                Op::LoadUpvalueScalar { dst, idx } => {
                    let val = match self.objs.get(cur_closure_slot) {
                        Some(Obj::Closure(c)) => c.get_scalar(idx as usize),
                        _ => return Err(err_msg("Missing active closure for scalar upvalue")),
                    };
                    s_set!(dst, val);
                }
                Op::LoadUpvalueObj { dst, idx } => {
                    let val = match self.objs.get(cur_closure_slot) {
                        Some(Obj::Closure(c)) => c.get_obj(idx as usize).clone(),
                        _ => return Err(err_msg("Missing active closure for obj upvalue")),
                    };
                    *o_mut!(dst) = val;
                }
                Op::LoadFuncObj {
                    dst_obj,
                    func_id: fid,
                } => {
                    *o_mut!(dst_obj) = Obj::Func(fid as u32);
                }
                Op::ReturnScalar { src } => {
                    let val = s_get!(src);
                    return_scalar_val!(val);
                }
                Op::ReturnAddInt { a, b } => {
                    let sum = (s_get!(a) as i64).checked_add(s_get!(b) as i64).ok_or_else(err_overflow)?;
                    return_scalar_val!(sum as u64);
                }
                Op::ReturnScalarIfLtIntImm { a, imm, src } => {
                    if (s_get!(a) as i64) < imm as i64 {
                        let val = s_get!(src);
                        return_scalar_val!(val);
                    }
                }
                Op::ReturnObj { src } => {
                    let val = std::mem::take(o_mut!(src));
                    return_obj_val!(val);
                }
                Op::ReturnVoid => {
                    if func.num_objs > 0 {
                        self.clear_obj_range(o_base, func.num_objs as usize);
                    }
                    let Some(frame) = self.pop_frame(base_frame_depth) else {
                        return Ok(ExecResult::Void);
                    };
                    func_id = frame.func_id;
                    // SAFETY: `frame.func_id` came from a validated `module.functions` entry.
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = frame.return_ip as usize;
                    s_base = frame.s_base as usize;
                    o_base = frame.o_base as usize;
                    cur_closure_slot = frame.unpack_closure_slot();
                    sync_frame_ptrs!();
                }
                Op::ToBytes { dst_obj, src_obj } => {
                    let bytes = match o_ref!(src_obj) {
                        Obj::Bytes(bytes) => Arc::clone(bytes),
                        Obj::Str(text) => Arc::new(text.as_bytes().to_vec()),
                        Obj::List(ListStorage::Int(ints)) => Arc::new(
                            ints.iter().copied().map(u8::try_from).collect::<Result<Vec<_>, _>>()
                                .map_err(|_| err_msg("Bytes values must be in 0..=255"))?,
                        ),
                        _ => return Err(err_msg("Expected String or [Int] in Bytes()")),
                    };
                    *o_mut!(dst_obj) = Obj::Bytes(bytes);
                }
                Op::BytesToList { dst_obj, src_obj } => {
                    let ints = match o_ref!(src_obj) {
                        Obj::Bytes(bytes) => Arc::new(bytes.iter().copied().map(i64::from).collect()),
                        Obj::List(ListStorage::Int(ints)) => Arc::clone(ints),
                        _ => return Err(err_msg("Expected Bytes in to_list()")),
                    };
                    *o_mut!(dst_obj) = Obj::List(ListStorage::Int(ints));
                }
                Op::NewHttpRequest { dst_obj, desc_idx } => {
                    let desc = &module.records[desc_idx as usize];
                    let fields = std::array::from_fn(|idx| o_ref!(desc.obj_regs[idx]).clone());
                    *o_mut!(dst_obj) = crate::native::http::request_from_fields(fields);
                }
                Op::LoadConstObj { dst_obj, idx } => {
                    *o_mut!(dst_obj) = obj_consts[idx as usize].clone();
                }
                Op::MoveObj { dst_obj, src } => {
                    let v = o_ref!(src).clone();
                    *o_mut!(dst_obj) = v;
                }
                Op::MakeRangeList { dst_obj, start_s, end_s } => {
                    self.exec_make_range_list(s_base, o_base, dst_obj, start_s, end_s);
                    sync_frame_ptrs!();
                }
                Op::NewListInt { dst_obj, cap } => {
                    *o_mut!(dst_obj) = Obj::List(ListStorage::new_int(cap as usize));
                }
                Op::NewListObj { dst_obj, cap } => {
                    *o_mut!(dst_obj) = Obj::List(ListStorage::new_obj(cap as usize));
                }
                Op::ListAppendInt { list_obj, val_s } => {
                    let val = s_get!(val_s) as i64;
                    match o_mut!(list_obj) {
                        Obj::List(ListStorage::Int(arc)) => {
                            if let Some(vec) = Arc::get_mut(arc) {
                                vec.push(val);
                            } else {
                                append_int_cow(arc, val);
                            }
                        }
                        _ => return Err(err_msg("Expected [Int] list in ListAppendInt")),
                    }
                }
                Op::ListAppendObj { list_obj, val_obj } => {
                    let val = o_ref!(val_obj).clone();
                    match o_mut!(list_obj) {
                        Obj::List(ListStorage::Obj(arc)) => {
                            if let Some(vec) = Arc::get_mut(arc) {
                                vec.push(val);
                            } else {
                                append_obj_cow(arc, val);
                            }
                        }
                        _ => return Err(err_msg("Expected [Obj] list in ListAppendObj")),
                    }
                }
                Op::ListLen { dst_s, list_obj } => {
                    let len = match o_ref!(list_obj) {
                        Obj::List(l) => l.len() as u64,
                        Obj::Bytes(bytes) => bytes.len() as u64,
                        _ => return Err(err_msg("Expected List or Bytes in ListLen")),
                    };
                    s_set!(dst_s, len);
                }
                Op::StrLen { dst_s, str_obj } => {
                    let len = match o_ref!(str_obj) {
                        Obj::Str(s) => s.len() as u64,
                        _ => return Err(err_msg("Expected Str in StrLen")),
                    };
                    s_set!(dst_s, len);
                }
                Op::StrConcatInt { dst_obj, prefix_obj, src_s } => {
                    let Obj::Str(prefix) = o_ref!(prefix_obj) else {
                        return Err(err_msg("Expected string prefix in StrConcatInt"));
                    };
                    let result = crate::vm::string::VmString::concat_int(prefix, s_get!(src_s) as i64);
                    *o_mut!(dst_obj) = Obj::Str(Arc::new(result));
                }
                Op::StrConcat { dst_obj, a_obj, b_obj } => {
                    self.exec_str_concat(o_base, dst_obj, a_obj, b_obj)?;
                    sync_frame_ptrs!();
                }
                Op::ForListInt { list_obj, idx_s, elem_s, exit_target } => {
                    let idx = s_get!(idx_s) as usize;
                    match o_ref!(list_obj) {
                        Obj::List(ListStorage::Int(arc)) => {
                            if idx < arc.len() {
                                // SAFETY: `idx < arc.len()` was just checked.
                                s_set!(elem_s, unsafe { *arc.get_unchecked(idx) } as u64);
                                s_set!(idx_s, (idx + 1) as u64);
                            } else {
                                ip = exit_target as usize;
                            }
                        }
                        _ => {
                            if !self.exec_for_list_float_next(s_base, o_base, list_obj, idx_s, elem_s)? {
                                ip = exit_target as usize;
                            }
                        }
                    }
                }
                Op::SumListIntTail { list_obj, idx_s, elem_s, acc_s } => {
                    let Obj::List(ListStorage::Int(items)) = o_ref!(list_obj) else {
                        return Err(err_msg("Expected [Int] in SumListIntTail"));
                    };
                    let idx = s_get!(idx_s) as usize;
                    if let Some(tail) = items.get(idx..)
                        && let Some(&last) = tail.last()
                    {
                        let mut acc = s_get!(acc_s) as i64;
                        for &value in tail {
                            acc = acc.checked_add(value).ok_or_else(err_overflow)?;
                        }
                        s_set!(acc_s, acc as u64);
                        s_set!(elem_s, last as u64);
                        s_set!(idx_s, items.len() as u64);
                    }
                }
                Op::ForListIntStep { list_obj, idx_s, elem_s, target } => {
                    let idx = s_get!(idx_s) as usize;
                    match o_ref!(list_obj) {
                        Obj::List(ListStorage::Int(arc)) => {
                            if idx < arc.len() {
                                // SAFETY: `idx < arc.len()` was just checked.
                                s_set!(elem_s, unsafe { *arc.get_unchecked(idx) } as u64);
                                s_set!(idx_s, (idx + 1) as u64);
                                ip = target as usize;
                            }
                        }
                        _ => {
                            if self.exec_for_list_float_next(s_base, o_base, list_obj, idx_s, elem_s)? {
                                ip = target as usize;
                            }
                        }
                    }
                }
                Op::ForListObj { list_obj, idx_s, elem_obj, exit_target } => {
                    if !self.exec_for_list_obj(s_base, o_base, list_obj, idx_s, elem_obj)? {
                        ip = exit_target as usize;
                    }
                    sync_frame_ptrs!();
                }
                Op::NewRecord { dst_obj, desc_idx } => {
                    let desc = &module.records[desc_idx as usize];
                    // SAFETY: `Module::validate` verifies `desc` register indices fit within the frame window.
                    unsafe { build_record_into_reg(desc, s_ptr, o_ptr, dst_obj, false) };
                }
                Op::NewRecordMove { dst_obj, desc_idx } => {
                    let desc = &module.records[desc_idx as usize];
                    // SAFETY: `Module::validate` verifies `desc` register indices fit within the frame window.
                    unsafe { build_record_into_reg(desc, s_ptr, o_ptr, dst_obj, true) };
                }
                Op::GetFieldScalar { dst_s, rec_obj, field_idx } => {
                    let val = match o_ref!(rec_obj) {
                        Obj::Record(rec) => rec.get_scalar(field_idx as usize),
                        _ => return Err(err_msg("Expected Record in GetFieldScalar")),
                    };
                    s_set!(dst_s, val);
                }
                Op::GetFieldObj { dst_obj, rec_obj, field_idx } => {
                    let val = match o_ref!(rec_obj) {
                        Obj::Record(rec) => rec.get_obj_keep_single_map(field_idx as usize).clone(),
                        _ => return Err(err_msg("Expected Record in GetFieldObj")),
                    };
                    *o_mut!(dst_obj) = val;
                }
                Op::NewMap { dst_obj } => *o_mut!(dst_obj) = Obj::Map(Arc::new(OrderedMap::new())),
                Op::MapSetIntInt { map_obj, key_s, val_s } => {
                    map_set_int_slot(o_mut!(map_obj), s_get!(key_s) as i64, s_get!(val_s))?;
                }
                Op::MapGetIntIntOr { dst_s, map_obj, key_s, def_s } => {
                    let value = map_get_int_or_slot(o_ref!(map_obj), s_get!(key_s) as i64, s_get!(def_s))?;
                    s_set!(dst_s, value);
                }
                Op::MapGetIntIntOrImm { dst_s, map_obj, key_s, default } => {
                    let value = map_get_int_or_slot(o_ref!(map_obj), s_get!(key_s) as i64, i64::from(default) as u64)?;
                    s_set!(dst_s, value);
                }
                Op::AddIntMapGetOrImm { dst_s, a, map_obj, key_s, default } => {
                    let value = map_get_int_or_slot(
                        o_ref!(map_obj),
                        s_get!(key_s) as i64,
                        i64::from(default) as u64,
                    )?;
                    let sum = (s_get!(a) as i64).checked_add(value as i64).ok_or_else(err_overflow)?;
                    s_set!(dst_s, sum as u64);
                }
                Op::SumIntMapRangeTail { acc_s, map_obj, idx_s, limit_s, default } => {
                    let start = (s_get!(idx_s) as i64).checked_add(1).ok_or_else(err_overflow)?;
                    let end = s_get!(limit_s) as i64;
                    if start < end {
                        let sum = sum_int_map_range(
                            o_ref!(map_obj), start..end, s_get!(acc_s) as i64, i64::from(default),
                        )?;
                        s_set!(acc_s, sum as u64);
                    }
                    s_set!(idx_s, start.max(end) as u64);
                }
                Op::MapLen { dst_s, map_obj } => {
                    let len = match o_ref!(map_obj) {
                        Obj::Map(arc) => arc.len() as u64,
                        Obj::DeferredField(df) => {
                            if df.as_single_map_entry().is_some() {
                                1
                            } else if let Obj::Map(arc) = df.value() {
                                arc.len() as u64
                            } else {
                                return Err(err_msg("Expected Map in MapLen"));
                            }
                        }
                        _ => return Err(err_msg("Expected Map in MapLen")),
                    };
                    s_set!(dst_s, len);
                }
                Op::RunPipeline { dst_obj, src_obj, plan_idx } => {
                    let sb = s_base + func.num_scalars as usize;
                    let ob = o_base + func.num_objs as usize;
                    self.run_pipeline(module, o_base, dst_obj, src_obj, plan_idx, sb, ob)?;
                    sync_frame_ptrs!();
                }
                Op::JsonDecode { dst_obj, src_obj, plan_idx } => {
                    self.exec_json_decode(module, o_base, dst_obj, src_obj, plan_idx)?;
                    sync_frame_ptrs!();
                }
                Op::JsonEncode { dst_obj, src_obj } => {
                    self.exec_json_encode(module, o_base, dst_obj, src_obj)?;
                    sync_frame_ptrs!();
                }
                Op::BoxScalar { dst_obj, src_s, kind } => {
                    *o_mut!(dst_obj) = box_scalar_val(s_get!(src_s), kind);
                }
                Op::UnboxScalar { dst_s, src_obj } => {
                    s_set!(dst_s, unbox_scalar_val(o_ref!(src_obj))?);
                }
                Op::IsTypeObj { dst_s, src_obj, expected_tag } => {
                    s_set!(dst_s, u64::from(is_type_tag_match(o_ref!(src_obj), expected_tag)));
                }
                Op::TryUnwrapScalar { dst_s, src_obj, ret_on_err } => {
                    if is_err_or_nil_obj(module, o_ref!(src_obj)) {
                        if ret_on_err {
                            return_obj_val!(std::mem::take(o_mut!(src_obj)));
                        } else {
                            return Err(format_try_error(o_ref!(src_obj)));
                        }
                    } else {
                        s_set!(dst_s, unbox_scalar_val(o_ref!(src_obj))?);
                    }
                }
                Op::TryCheckObj { src_obj, ret_on_err, empty_is_err } => {
                    if is_try_fail_obj(module, o_ref!(src_obj), empty_is_err) {
                        if ret_on_err {
                            return_obj_val!(std::mem::take(o_mut!(src_obj)));
                        } else {
                            return Err(format_try_error(o_ref!(src_obj)));
                        }
                    }
                }
                Op::TimeOp { kind, dst_obj, arg_reg } => {
                    self.exec_time_op(s_base, o_base, kind, dst_obj, arg_reg)?;
                    sync_frame_ptrs!();
                }
                Op::ValidationOp { kind, dst_obj, a_reg, b_reg } => {
                    self.exec_validation_op(s_base, o_base, kind, dst_obj, a_reg, b_reg)?;
                    sync_frame_ptrs!();
                }
                Op::ValidationCheck { dst_obj, src_obj } => {
                    let (sb, ob) = (s_base + func.num_scalars as usize, o_base + func.num_objs as usize);
                    self.exec_validation_check(module, sb, ob, o_base, dst_obj, src_obj)?;
                    sync_frame_ptrs!();
                }
                op @ (Op::NewListFloat { .. }
                | Op::ListAppendFloat { .. }
                | Op::ScalarToStr { .. }
                | Op::ObjToStr { .. }
                | Op::StrMethod { .. }
                | Op::StrReplace { .. }
                | Op::ClampScalar { .. }
                | Op::ListIndex { .. }
                | Op::ListGet { .. }
                | Op::ListContainsScalar { .. }
                | Op::ListContainsObj { .. }
                | Op::ListSlice { .. }
                | Op::ListReverse { .. }
                | Op::ListEnumerate { .. }
                | Op::ListJoin { .. }
                | Op::TupleGetScalar { .. }
                | Op::TupleGetObj { .. }) => self.exec_ext_op(s_base, o_base, op)?,
                Op::ExtOp { kind, aux, a, b, c } => {
                    self.exec_ext_dispatch(
                        module,
                        s_base,
                        o_base,
                        s_base + func.num_scalars as usize,
                        o_base + func.num_objs as usize,
                        kind,
                        aux,
                        a,
                        b,
                        c,
                    )?;
                    sync_frame_ptrs!();
                }
            }
        }
    }
}
