use std::sync::Arc;

use crate::ir::Module;
use crate::vm::opcode::Op;
use crate::native::{json_decode::decode_json_with_plan, json_encode::encode_json_obj};
use crate::vm::value::{
    ClosureData, INLINE_CLOSURE_SCALARS, INLINE_STRUCT_OBJS, INLINE_STRUCT_SCALARS, ListStorage,
    MapKey, MapVal, Obj, OrderedMap, StructData,
};
use crate::vm::{CallFrame, RuntimeError, Vm};

#[cold]
#[inline(never)]
pub(crate) fn err_msg(msg: &str) -> RuntimeError {
    RuntimeError {
        message: msg.to_string(),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_overflow() -> RuntimeError {
    RuntimeError {
        message: "Integer overflow".to_string(),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_div_zero() -> RuntimeError {
    RuntimeError {
        message: "Division by zero".to_string(),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_call_depth() -> RuntimeError {
    RuntimeError {
        message: "Maximum call stack depth (256) exceeded".to_string(),
    }
}

#[cold]
#[inline(never)]
pub(crate) fn map_set_int_cow(arc: &mut Arc<OrderedMap>, key: i64, val: u64) {
    Arc::make_mut(arc).set(MapKey::Int(key), MapVal::Scalar(val));
}

#[inline(always)]
pub(crate) fn map_set_int_slot(slot: &mut Obj, key: i64, val: u64) -> Result<(), RuntimeError> {
    match slot {
        Obj::Map(arc) => {
            if let Some(m) = Arc::get_mut(arc) {
                m.set(MapKey::Int(key), MapVal::Scalar(val));
            } else {
                map_set_int_cow(arc, key, val);
            }
            Ok(())
        }
        _ => Err(err_msg("Expected Map in MapSetIntInt")),
    }
}

#[inline(always)]
pub(crate) fn map_get_int_or_slot(slot: &Obj, key: i64, def: u64) -> Result<u64, RuntimeError> {
    match slot {
        Obj::Map(arc) => Ok(match arc.get(&MapKey::Int(key)) {
            Some(MapVal::Scalar(v)) => *v,
            _ => def,
        }),
        _ => Err(err_msg("Expected Map in MapGetIntIntOr")),
    }
}

pub(crate) fn sum_int_map_range(
    slot: &Obj,
    keys: std::ops::Range<i64>,
    mut sum: i64,
    default: i64,
) -> Result<i64, RuntimeError> {
    let Obj::Map(map) = slot else {
        return Err(err_msg("Expected Map in MapGetIntIntOr"));
    };
    if map.dense_ints
        && let (Ok(start), Ok(end)) = (usize::try_from(keys.start), usize::try_from(keys.end))
        && let Some(entries) = map.entries().get(start..end)
    {
        for (_, value) in entries {
            let addend = match value {
                MapVal::Scalar(v) => *v as i64,
                _ => default,
            };
            sum = sum.checked_add(addend).ok_or_else(err_overflow)?;
        }
    } else {
        for key in keys {
            let addend = match map.get(&MapKey::Int(key)) {
                Some(MapVal::Scalar(v)) => *v as i64,
                _ => default,
            };
            sum = sum.checked_add(addend).ok_or_else(err_overflow)?;
        }
    }
    Ok(sum)
}

#[inline(always)]
unsafe fn take_or_clone_obj(o_ptr: *mut Obj, reg: u16, move_objs: bool) -> Obj {
    let slot = unsafe { &mut *o_ptr.add(reg as usize) };
    if move_objs {
        std::mem::take(slot)
    } else {
        slot.clone()
    }
}

#[cold]
#[inline(never)]
unsafe fn build_record_overflow(
    desc: &crate::ir::RecordDesc,
    s_ptr: *const u64,
    o_ptr: *mut Obj,
    move_objs: bool,
) -> Obj {
    let mut rec = StructData::new(
        desc.schema_id,
        desc.scalar_regs.len() as u16,
        desc.obj_regs.len() as u16,
    );
    for (i, &r) in desc.scalar_regs.iter().enumerate() {
        rec.set_scalar(i, unsafe { *s_ptr.add(r as usize) });
    }
    for (i, &r) in desc.obj_regs.iter().enumerate() {
        rec.set_obj(i, unsafe { take_or_clone_obj(o_ptr, r, move_objs) });
    }
    Obj::Record(Arc::new(rec))
}

#[inline(always)]
pub(crate) unsafe fn build_record_from_regs(
    desc: &crate::ir::RecordDesc,
    s_ptr: *const u64,
    o_ptr: *mut Obj,
    move_objs: bool,
) -> Obj {
    let s_len = desc.scalar_regs.len();
    let o_len = desc.obj_regs.len();
    if s_len <= INLINE_STRUCT_SCALARS && o_len <= INLINE_STRUCT_OBJS {
        let inline_scalars = match s_len {
            0 => [0, 0],
            1 => [
                unsafe { *s_ptr.add(*desc.scalar_regs.get_unchecked(0) as usize) },
                0,
            ],
            _ => [
                unsafe { *s_ptr.add(*desc.scalar_regs.get_unchecked(0) as usize) },
                unsafe { *s_ptr.add(*desc.scalar_regs.get_unchecked(1) as usize) },
            ],
        };
        let inline_objs = match o_len {
            0 => [Obj::Empty, Obj::Empty],
            1 => [
                unsafe { take_or_clone_obj(o_ptr, *desc.obj_regs.get_unchecked(0), move_objs) },
                Obj::Empty,
            ],
            _ => [
                unsafe { take_or_clone_obj(o_ptr, *desc.obj_regs.get_unchecked(0), move_objs) },
                unsafe { take_or_clone_obj(o_ptr, *desc.obj_regs.get_unchecked(1), move_objs) },
            ],
        };
        return Obj::Record(Arc::new(StructData {
            schema_id: desc.schema_id,
            scalar_len: s_len as u16,
            obj_len: o_len as u16,
            overflow: None,
            inline_scalars,
            inline_objs,
        }));
    }
    unsafe { build_record_overflow(desc, s_ptr, o_ptr, move_objs) }
}

#[inline(never)]
pub(crate) unsafe fn build_record_into_reg(
    desc: &crate::ir::RecordDesc,
    s_ptr: *const u64,
    o_ptr: *mut Obj,
    dst_obj: u16,
    move_objs: bool,
) {
    let rec = unsafe { build_record_from_regs(desc, s_ptr, o_ptr, move_objs) };
    unsafe { *o_ptr.add(dst_obj as usize) = rec };
}

// Keep the arithmetic kernels that meet the range and float benchmark gates.
#[cold]
pub(crate) fn finish_range_kernel(
    body: &[Op],
    scalars: &mut [u64],
    int_consts: &[i64],
    idx: u16,
    limit: u16,
    start: i64,
    end: i64,
) -> Result<bool, RuntimeError> {
    match *body {
        [Op::AddInt { dst, a, b }] if dst == a && b == idx && dst != idx && dst != limit => {
            let mut acc = scalars[usize::from(dst)] as i64;
            for k in start..end {
                acc = acc.checked_add(k).ok_or_else(err_overflow)?;
            }
            scalars[usize::from(dst)] = acc as u64;
        }
        [
            Op::MulAddInt { dst: t0, a: acc_in, b: mul_reg, c: add_reg },
            Op::AddInt { dst: t1, a: add_lhs, b: add_rhs },
            Op::ModIntConst { dst: acc_out, a: mod_src, divisor, magic_idx, shift },
        ] if acc_in == acc_out
            && mod_src == t1
            && ((add_lhs == t0 && add_rhs == idx) || (add_lhs == idx && add_rhs == t0))
            && t0 != t1
            && t0 != acc_out
            && t1 != acc_out
            && [acc_out, t0, t1].into_iter().all(|r| ![mul_reg, add_reg, idx, limit].contains(&r)) =>
        {
            let Some(&magic) = int_consts.get(usize::from(magic_idx)) else {
                return Ok(false);
            };
            let mut acc = scalars[usize::from(acc_out)] as i64;
            let mul_val = scalars[usize::from(mul_reg)] as i64;
            let add_val = scalars[usize::from(add_reg)] as i64;
            let mut last_t0 = scalars[usize::from(t0)] as i64;
            let mut last_t1 = scalars[usize::from(t1)] as i64;
            for k in start..end {
                last_t0 = acc
                    .checked_mul(mul_val)
                    .and_then(|v| v.checked_add(add_val))
                    .ok_or_else(err_overflow)?;
                last_t1 = last_t0.checked_add(k).ok_or_else(err_overflow)?;
                acc = crate::vm::opcode::remainder_by_constant(last_t1, divisor, magic, shift);
            }
            scalars[usize::from(t0)] = last_t0 as u64;
            scalars[usize::from(t1)] = last_t1 as u64;
            scalars[usize::from(acc_out)] = acc as u64;
        }
        [
            Op::IntToFloat { dst: x, src },
            Op::FmaFloat { dst: term, a: factor, b, c },
            Op::AddFloat { dst: acc, a, b: term_src },
        ] if src == idx && factor == x && a == acc && term_src == term
            && x != acc && term != acc
            && [b, c].into_iter().all(|reg| ![x, term, acc, idx].contains(&reg))
            && [x, term, acc].into_iter().all(|reg| reg != idx && reg != limit) =>
        {
            let multiplier = f64::from_bits(scalars[usize::from(b)]);
            let constant = f64::from_bits(scalars[usize::from(c)]);
            let mut sum = f64::from_bits(scalars[usize::from(acc)]);
            let mut last_x = 0.0;
            let mut last_term = 0.0;
            for k in start..end {
                last_x = k as f64;
                last_term = last_x * multiplier + constant;
                sum += last_term;
            }
            scalars[usize::from(x)] = last_x.to_bits();
            scalars[usize::from(term)] = last_term.to_bits();
            scalars[usize::from(acc)] = sum.to_bits();
        }
        _ => return Ok(false),
    }
    scalars[usize::from(idx)] = end as u64;
    Ok(true)
}

impl Vm {
    #[inline(never)]
    pub(crate) fn exec_make_closure(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        dst_obj: u16,
        proto_idx: u16,
    ) {
        let cp = &module.closures[proto_idx as usize];
        let mut inline_scalars = [0u64; INLINE_CLOSURE_SCALARS];
        let cap_s_len = cp.capture_scalars.len();
        for (i, &r) in cp
            .capture_scalars
            .iter()
            .enumerate()
            .take(INLINE_CLOSURE_SCALARS)
        {
            inline_scalars[i] = self.scalars[s_base + r as usize];
        }
        let extra_scalars = if cap_s_len > INLINE_CLOSURE_SCALARS {
            let mut extra = Vec::with_capacity(cap_s_len - INLINE_CLOSURE_SCALARS);
            for &r in &cp.capture_scalars[INLINE_CLOSURE_SCALARS..] {
                extra.push(self.scalars[s_base + r as usize]);
            }
            Some(extra.into_boxed_slice())
        } else {
            None
        };
        let obj_upvalues = if cp.capture_objs.is_empty() {
            Box::new([])
        } else {
            let mut cap_objs = Vec::with_capacity(cp.capture_objs.len());
            for &r in cp.capture_objs.iter() {
                cap_objs.push(self.objs[o_base + r as usize].clone());
            }
            cap_objs.into_boxed_slice()
        };
        let data = ClosureData {
            func_id: cp.func_id,
            scalar_len: cap_s_len as u16,
            inline_scalars,
            extra_scalars,
            obj_upvalues,
        };
        self.objs[o_base + dst_obj as usize] = Obj::Closure(Arc::new(data));
    }

    #[inline(never)]
    pub(crate) fn exec_make_range_list(
        &mut self,
        s_base: usize,
        o_base: usize,
        dst_obj: u16,
        start_s: u16,
        end_s: u16,
    ) {
        let start = self.scalars[s_base + start_s as usize] as i64;
        let end = self.scalars[s_base + end_s as usize] as i64;
        let data = if end <= start {
            Vec::new()
        } else {
            (start..end).collect()
        };
        self.objs[o_base + dst_obj as usize] = Obj::List(ListStorage::Int(Arc::new(data)));
    }

    #[inline(never)]
    pub(crate) fn exec_str_concat(
        &mut self,
        o_base: usize,
        dst_obj: u16,
        a_obj: u16,
        b_obj: u16,
    ) -> Result<(), RuntimeError> {
        let res = match (&self.objs[o_base + a_obj as usize], &self.objs[o_base + b_obj as usize]) {
            (Obj::Str(a), Obj::Str(b)) => {
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(a);
                s.push_str(b);
                Obj::str(s)
            }
            _ => return Err(err_msg("Expected Str operands in StrConcat")),
        };
        self.objs[o_base + dst_obj as usize] = res;
        Ok(())
    }

    #[inline(never)]
    pub(crate) fn exec_for_list_obj(
        &mut self,
        s_base: usize,
        o_base: usize,
        list_obj: u16,
        idx_s: u16,
        elem_obj: u16,
    ) -> Result<bool, RuntimeError> {
        let idx = self.scalars[s_base + idx_s as usize] as usize;
        let next_elem = match &self.objs[o_base + list_obj as usize] {
            Obj::List(ListStorage::Obj(arc)) => {
                if idx < arc.len() {
                    Some(arc[idx].clone())
                } else {
                    None
                }
            }
            _ => return Err(err_msg("Expected [Obj] in ForListObj")),
        };
        if let Some(val) = next_elem {
            self.objs[o_base + elem_obj as usize] = val;
            self.scalars[s_base + idx_s as usize] = (idx + 1) as u64;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[inline(never)]
    pub(crate) fn exec_call_callable(
        &mut self,
        module: &Module,
        caller_func_id: u32,
        caller_num_scalars: u16,
        caller_num_objs: u16,
        return_ip: usize,
        cur_s_base: usize,
        cur_o_base: usize,
        cur_closure_slot: usize,
        callee_obj: u16,
        arg_base_s: u16,
        arg_base_o: u16,
        ret_reg: u16,
    ) -> Result<(u32, usize, usize, usize), RuntimeError> {
        let slot = cur_o_base + callee_obj as usize;
        let (next_fid, next_cslot) = match &self.objs[slot] {
            Obj::Func(fid) => (*fid, usize::MAX),
            Obj::Closure(c) => (c.func_id, slot),
            _ => return Err(err_msg("Value is not callable")),
        };
        let next_func = module
            .functions
            .get(next_fid as usize)
            .ok_or_else(|| err_msg("Callable function id out of bounds"))?;
        let ret_ok = if next_func.returns_obj {
            ret_reg < caller_num_objs
        } else {
            ret_reg < caller_num_scalars
        };
        if !ret_ok
            || arg_base_s as usize + next_func.scalar_params as usize > caller_num_scalars as usize
            || arg_base_o as usize + next_func.obj_params as usize > caller_num_objs as usize
        {
            return Err(err_msg("Dynamic call register mismatch"));
        }
        self.push_frame(CallFrame {
            func_id: caller_func_id,
            return_ip: return_ip as u32,
            s_base: cur_s_base as u32,
            o_base: cur_o_base as u32,
            ret_reg,
            prev_closure_slot: CallFrame::pack_closure_slot(cur_closure_slot),
        })?;
        let next_s_base = cur_s_base + arg_base_s as usize;
        let next_o_base = cur_o_base + arg_base_o as usize;
        self.ensure_capacity(
            next_s_base + next_func.num_scalars as usize,
            next_o_base + next_func.num_objs as usize,
        );
        Ok((next_fid, next_s_base, next_o_base, next_cslot))
    }

    #[inline(never)]
    pub(crate) fn exec_json_decode(
        &mut self,
        module: &Module,
        o_base: usize,
        dst_obj: u16,
        src_obj: u16,
        plan_idx: u16,
    ) -> Result<(), RuntimeError> {
        let raw = match &self.objs[o_base + src_obj as usize] {
            Obj::Str(s) => Arc::clone(s),
            _ => return Err(err_msg("Expected String in JsonDecode")),
        };
        self.objs[o_base + dst_obj as usize] = Obj::Empty;
        let plan = &module.json_plans[plan_idx as usize];
        let decoded = decode_json_with_plan(&raw, plan).map_err(|e| err_msg(&e.to_string()))?;
        self.objs[o_base + dst_obj as usize] = decoded;
        Ok(())
    }

    #[inline(never)]
    pub(crate) fn exec_json_encode(
        &mut self,
        module: &Module,
        o_base: usize,
        dst_obj: u16,
        src_obj: u16,
    ) -> Result<(), RuntimeError> {
        let src_slot = o_base + src_obj as usize;
        let dst_slot = o_base + dst_obj as usize;
        let encoded = if dst_obj != src_obj {
            self.objs[dst_slot] = Obj::Empty;
            encode_json_obj(&self.objs[src_slot], &module.struct_schemas)
                .map_err(|e| err_msg(&e.to_string()))?
        } else {
            let src_val = std::mem::take(&mut self.objs[src_slot]);
            encode_json_obj(&src_val, &module.struct_schemas)
                .map_err(|e| err_msg(&e.to_string()))?
        };
        self.objs[dst_slot] = Obj::str(encoded);
        Ok(())
    }
}
