use std::sync::Arc;

use crate::ir::Module;
use crate::native::validation::{box_scalar_val, is_err_or_nil_obj, is_type_tag_match};
use crate::vm::exec_cold::{
    build_record_into_reg, err_call_depth, err_div_zero, err_msg, err_overflow,
    exec_for_range_fast,
};
use crate::vm::exec_union::{format_try_error, is_try_fail_obj, unbox_scalar_val};
use crate::vm::opcode::Op;
use crate::vm::value::{ListStorage, Obj, OrderedMap, append_int_cow, append_obj_cow};
use crate::vm::{CallFrame, ExecResult, MAX_CALL_DEPTH, RuntimeError, Vm};

impl Vm {
    pub(crate) fn exec_from(
        &mut self,
        module: &Module,
        entry_func_id: u32,
        entry_s_base: usize,
        entry_o_base: usize,
        entry_closure_slot: usize,
    ) -> Result<ExecResult, RuntimeError> {
        let base_frame_depth = self.frames.len();
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
        let mut s_ptr = unsafe { self.scalars.as_mut_ptr().add(s_base) };
        let mut o_ptr = unsafe { self.objs.as_mut_ptr().add(o_base) };

        let int_consts = module.int_consts.as_ptr();
        let float_consts = module.float_consts.as_ptr();
        let obj_consts = &module.obj_consts;

        macro_rules! s_get {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_scalars as usize);
                unsafe { *s_ptr.add(idx) }
            }};
        }
        macro_rules! s_set {
            ($r:expr, $val:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_scalars as usize);
                let val = $val;
                unsafe { *s_ptr.add(idx) = val };
            }};
        }
        macro_rules! o_ref {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_objs as usize);
                unsafe { &*o_ptr.add(idx) }
            }};
        }
        macro_rules! o_mut {
            ($r:expr) => {{
                let idx = $r as usize;
                debug_assert!(idx < func.num_objs as usize);
                unsafe { &mut *o_ptr.add(idx) }
            }};
        }
        macro_rules! sync_frame_ptrs {
            () => {
                s_ptr = unsafe { self.scalars.as_mut_ptr().add(s_base) };
                o_ptr = unsafe { self.objs.as_mut_ptr().add(o_base) };
            };
        }
        macro_rules! return_obj_val {
            ($val:expr) => {{
                let val = $val;
                let Some(frame) = self.finish_return_frame(o_base, func.num_objs, base_frame_depth) else {
                    return Ok(ExecResult::Obj(val));
                };
                func_id = frame.func_id;
                func = unsafe { module.functions.get_unchecked(func_id as usize) };
                ops_ptr = func.ops.as_ptr();
                ip = frame.return_ip;
                s_base = frame.s_base;
                o_base = frame.o_base;
                cur_closure_slot = frame.prev_closure_slot;
                sync_frame_ptrs!();
                *o_mut!(frame.ret_reg) = val;
            }};
        }

        loop {
            debug_assert!(ip < func.ops.len());
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
                Op::ForRangeStep {
                    idx_reg,
                    limit_reg,
                    target,
                } => {
                    let cur = s_get!(idx_reg) as i64;
                    let limit = s_get!(limit_reg) as i64;
                    if cur < limit {
                        let next = cur + 1;
                        if next < limit {
                            let step_ip = ip - 1;
                            let t = target as usize;
                            if t < step_ip
                                && step_ip - t <= 8
                                && !matches!(
                                    unsafe { *ops_ptr.add(t) },
                                    Op::CallDirect { .. }
                                        | Op::CallCallable { .. }
                                        | Op::MakeClosure { .. }
                                        | Op::LoadConstObj { .. }
                                        | Op::MoveObj { .. }
                                        | Op::RunPipeline { .. }
                                        | Op::JsonDecode { .. }
                                        | Op::JsonEncode { .. }
                                )
                            {
                                let body =
                                    unsafe { std::slice::from_raw_parts(ops_ptr.add(t), step_ip - t) };
                                if unsafe {
                                    exec_for_range_fast(
                                        body,
                                        s_ptr,
                                        o_ptr,
                                        int_consts,
                                        float_consts,
                                        idx_reg,
                                        limit_reg,
                                        cur,
                                        limit,
                                    )?
                                } {
                                    continue;
                                }
                            }
                            s_set!(idx_reg, next as u64);
                            ip = t;
                        } else {
                            s_set!(idx_reg, next as u64);
                        }
                    } else {
                        let Some(next) = cur.checked_add(1) else {
                            return Err(err_overflow());
                        };
                        s_set!(idx_reg, next as u64);
                    }
                }
                Op::CallDirect {
                    func_id: callee_id,
                    arg_base_s,
                    arg_base_o,
                    ret_reg,
                } => {
                    if self.frames.len() >= MAX_CALL_DEPTH {
                        return Err(err_call_depth());
                    }
                    self.frames.push(CallFrame {
                        func_id,
                        return_ip: ip,
                        s_base,
                        o_base,
                        ret_reg,
                        prev_closure_slot: cur_closure_slot,
                    });
                    func_id = callee_id as u32;
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = 0;
                    s_base += arg_base_s as usize;
                    o_base += arg_base_o as usize;
                    cur_closure_slot = usize::MAX;
                    self.ensure_capacity(
                        s_base + func.num_scalars as usize,
                        o_base + func.num_objs as usize,
                    );
                    sync_frame_ptrs!();
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
                    let Some(frame) = self.finish_return_frame(o_base, func.num_objs, base_frame_depth) else {
                        return Ok(ExecResult::Scalar(val));
                    };
                    func_id = frame.func_id;
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = frame.return_ip;
                    s_base = frame.s_base;
                    o_base = frame.o_base;
                    cur_closure_slot = frame.prev_closure_slot;
                    sync_frame_ptrs!();
                    s_set!(frame.ret_reg, val);
                }
                Op::ReturnObj { src } => {
                    let val = std::mem::take(o_mut!(src));
                    return_obj_val!(val);
                }
                Op::ReturnVoid => {
                    let Some(frame) = self.finish_return_frame(o_base, func.num_objs, base_frame_depth) else {
                        return Ok(ExecResult::Void);
                    };
                    func_id = frame.func_id;
                    func = unsafe { module.functions.get_unchecked(func_id as usize) };
                    ops_ptr = func.ops.as_ptr();
                    ip = frame.return_ip;
                    s_base = frame.s_base;
                    o_base = frame.o_base;
                    cur_closure_slot = frame.prev_closure_slot;
                    sync_frame_ptrs!();
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
                        _ => return Err(err_msg("Expected List in ListLen")),
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
                Op::StrConcat { dst_obj, a_obj, b_obj } => {
                    self.exec_str_concat(o_base, dst_obj, a_obj, b_obj)?;
                    sync_frame_ptrs!();
                }
                Op::ForListInt { list_obj, idx_s, elem_s, exit_target } => {
                    let idx = s_get!(idx_s) as usize;
                    match o_ref!(list_obj) {
                        Obj::List(ListStorage::Int(arc)) => {
                            if idx < arc.len() {
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
                Op::ForListIntStep { list_obj, idx_s, elem_s, target } => {
                    let idx = s_get!(idx_s) as usize;
                    match o_ref!(list_obj) {
                        Obj::List(ListStorage::Int(arc)) => {
                            if idx < arc.len() {
                                if target as usize + 1 == ip - 1
                                    && let Op::AddInt { dst, a, b } =
                                        unsafe { *ops_ptr.add(target as usize) }
                                    && dst == a
                                    && b == elem_s
                                    && dst != idx_s
                                    && dst != elem_s
                                {
                                    let mut acc = s_get!(dst) as i64;
                                    let slice = &arc[idx..];
                                    for &v in slice {
                                        let Some(next_acc) = acc.checked_add(v) else {
                                            return Err(err_overflow());
                                        };
                                        acc = next_acc;
                                    }
                                    s_set!(dst, acc as u64);
                                    s_set!(elem_s, *slice.last().unwrap() as u64);
                                    s_set!(idx_s, arc.len() as u64);
                                    continue;
                                }
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
                    unsafe { build_record_into_reg(desc, s_ptr, o_ptr, dst_obj, false) };
                }
                Op::NewRecordMove { dst_obj, desc_idx } => {
                    let desc = &module.records[desc_idx as usize];
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
                        Obj::Record(rec) => rec.get_obj(field_idx as usize).clone(),
                        _ => return Err(err_msg("Expected Record in GetFieldObj")),
                    };
                    *o_mut!(dst_obj) = val;
                }
                Op::NewMap { dst_obj } => *o_mut!(dst_obj) = Obj::Map(Arc::new(OrderedMap::new())),
                Op::MapSetIntInt { map_obj, key_s, val_s } => {
                    self.exec_map_set_int(s_base, o_base, map_obj, key_s, val_s)?;
                    sync_frame_ptrs!();
                }
                Op::MapGetIntIntOr { dst_s, map_obj, key_s, def_s } => {
                    self.exec_map_get_int_or(s_base, o_base, dst_s, map_obj, key_s, def_s)?;
                    sync_frame_ptrs!();
                }
                Op::MapLen { dst_s, map_obj } => {
                    let len = match o_ref!(map_obj) {
                        Obj::Map(arc) => arc.len() as u64,
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
                | Op::TupleGetObj { .. }
                | Op::ExtOp { .. }) => self.exec_ext_op(module, s_base, o_base, op)?,
            }
        }
    }
}
