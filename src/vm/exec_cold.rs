use std::sync::Arc;

use crate::ir::Module;
use crate::native::{json_decode::decode_json_with_plan, json_encode::encode_json_obj};
use crate::vm::opcode::Op;
use crate::vm::value::{
    ClosureData, INLINE_CLOSURE_SCALARS, INLINE_STRUCT_OBJS, INLINE_STRUCT_SCALARS, ListStorage,
    MapKey, MapVal, Obj, OrderedMap, StructData,
};
use crate::vm::{CallFrame, MAX_CALL_DEPTH, RuntimeError, Vm};

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

#[inline]
fn is_branchless_scalar_body_op(op: Op, idx_reg: u16, limit_reg: u16) -> bool {
    let dst = match op {
        Op::LoadImmInt { dst, .. }
        | Op::LoadConstInt { dst, .. }
        | Op::LoadConstFloat { dst, .. }
        | Op::LoadBool { dst, .. }
        | Op::MoveScalar { dst, .. }
        | Op::AddInt { dst, .. }
        | Op::AddIntImm { dst, .. }
        | Op::SubInt { dst, .. }
        | Op::MulInt { dst, .. }
        | Op::DivInt { dst, .. }
        | Op::ModInt { dst, .. }
        | Op::ModIntImm { dst, .. }
        | Op::NegInt { dst, .. }
        | Op::AddFloat { dst, .. }
        | Op::SubFloat { dst, .. }
        | Op::MulFloat { dst, .. }
        | Op::DivFloat { dst, .. }
        | Op::FmaFloat { dst, .. }
        | Op::IntToFloat { dst, .. }
        | Op::FloatFloor { dst, .. }
        | Op::FloatRound { dst, .. }
        | Op::EqInt { dst, .. }
        | Op::NeInt { dst, .. }
        | Op::LtInt { dst, .. }
        | Op::LeInt { dst, .. }
        | Op::GtInt { dst, .. }
        | Op::GeInt { dst, .. }
        | Op::EqIntImm { dst, .. }
        | Op::LtIntImm { dst, .. }
        | Op::EqFloat { dst, .. }
        | Op::LtFloat { dst, .. }
        | Op::LeFloat { dst, .. }
        | Op::NotBool { dst, .. } => dst,
        _ => return false,
    };
    dst != idx_reg && dst != limit_reg
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
pub(crate) unsafe fn exec_for_range_fast(
    body: &[Op],
    s_ptr: *mut u64,
    o_ptr: *mut Obj,
    int_consts: *const i64,
    float_consts: *const f64,
    idx_reg: u16,
    limit_reg: u16,
    cur: i64,
    limit: i64,
) -> Result<bool, RuntimeError> {
    match *body {
        [Op::AddInt { dst, a, b }]
            if dst == a && b == idx_reg && dst != idx_reg && dst != limit_reg =>
        {
            let mut acc = unsafe { *s_ptr.add(dst as usize) } as i64;
            for k in (cur + 1)..limit {
                let Some(next_acc) = acc.checked_add(k) else {
                    return Err(err_overflow());
                };
                acc = next_acc;
            }
            unsafe {
                *s_ptr.add(dst as usize) = acc as u64;
                *s_ptr.add(idx_reg as usize) = limit as u64;
            }
            Ok(true)
        }
        [Op::ListAppendInt { list_obj, val_s }] if val_s == idx_reg => {
            match unsafe { &mut *o_ptr.add(list_obj as usize) } {
                Obj::List(ListStorage::Int(arc)) => {
                    let start = cur + 1;
                    if start < limit {
                        let vec = Arc::make_mut(arc);
                        vec.reserve((limit - start) as usize);
                        for k in start..limit {
                            vec.push(k);
                        }
                    }
                    unsafe { *s_ptr.add(idx_reg as usize) = limit as u64 };
                    Ok(true)
                }
                _ => Err(err_msg("Expected [Int] list in ListAppendInt")),
            }
        }
        [
            Op::IntToFloat { dst: fi, src },
            Op::FmaFloat {
                dst: tmp,
                a: fa,
                b: cm,
                c: ca,
            },
            Op::AddFloat {
                dst: acc,
                a: aa,
                b: ab,
            },
        ] if src == idx_reg
            && fa == fi
            && aa == acc
            && ab == tmp
            && fi != acc
            && tmp != acc
            && cm != fi
            && ca != fi
            && cm != tmp
            && ca != tmp
            && cm != acc
            && ca != acc
            && fi != idx_reg
            && tmp != idx_reg
            && acc != idx_reg
            && acc != limit_reg =>
        {
            let cm_f = f64::from_bits(unsafe { *s_ptr.add(cm as usize) });
            let ca_f = f64::from_bits(unsafe { *s_ptr.add(ca as usize) });
            let mut acc_f = f64::from_bits(unsafe { *s_ptr.add(acc as usize) });
            let mut last_fi = 0.0f64;
            let mut last_tmp = 0.0f64;
            for k in (cur + 1)..limit {
                last_fi = k as f64;
                last_tmp = last_fi * cm_f + ca_f;
                acc_f += last_tmp;
            }
            unsafe {
                *s_ptr.add(fi as usize) = last_fi.to_bits();
                *s_ptr.add(tmp as usize) = last_tmp.to_bits();
                *s_ptr.add(acc as usize) = acc_f.to_bits();
                *s_ptr.add(idx_reg as usize) = limit as u64;
            }
            Ok(true)
        }
        _ if !body.is_empty()
            && body
                .iter()
                .copied()
                .all(|op| is_branchless_scalar_body_op(op, idx_reg, limit_reg)) =>
        {
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

            for k in (cur + 1)..limit {
                s_set!(idx_reg, k as u64);
                for &op in body {
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
                        Op::AddFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) + f64::from_bits(s_get!(b))).to_bits()),
                        Op::SubFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) - f64::from_bits(s_get!(b))).to_bits()),
                        Op::MulFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) * f64::from_bits(s_get!(b))).to_bits()),
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
                        Op::EqInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) == (s_get!(b) as i64)) as u64),
                        Op::NeInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) != (s_get!(b) as i64)) as u64),
                        Op::LtInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) < (s_get!(b) as i64)) as u64),
                        Op::LeInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) <= (s_get!(b) as i64)) as u64),
                        Op::GtInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) > (s_get!(b) as i64)) as u64),
                        Op::GeInt { dst, a, b } => s_set!(dst, ((s_get!(a) as i64) >= (s_get!(b) as i64)) as u64),
                        Op::EqIntImm { dst, a, imm } => s_set!(dst, ((s_get!(a) as i64) == imm as i64) as u64),
                        Op::LtIntImm { dst, a, imm } => s_set!(dst, ((s_get!(a) as i64) < imm as i64) as u64),
                        Op::EqFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) == f64::from_bits(s_get!(b))) as u64),
                        Op::LtFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) < f64::from_bits(s_get!(b))) as u64),
                        Op::LeFloat { dst, a, b } => s_set!(dst, (f64::from_bits(s_get!(a)) <= f64::from_bits(s_get!(b))) as u64),
                        Op::NotBool { dst, src } => s_set!(dst, (s_get!(src) == 0) as u64),
                        _ => return Ok(false),
                    }
                }
            }
            s_set!(idx_reg, limit as u64);
            Ok(true)
        }
        _ => Ok(false),
    }
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
                Obj::Str(Arc::new(s))
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
    pub(crate) fn exec_map_set_int(
        &mut self,
        s_base: usize,
        o_base: usize,
        map_obj: u16,
        key_s: u16,
        val_s: u16,
    ) -> Result<(), RuntimeError> {
        let key = self.scalars[s_base + key_s as usize] as i64;
        let val = self.scalars[s_base + val_s as usize];
        match &mut self.objs[o_base + map_obj as usize] {
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

    #[inline(never)]
    pub(crate) fn exec_map_get_int_or(
        &mut self,
        s_base: usize,
        o_base: usize,
        dst_s: u16,
        map_obj: u16,
        key_s: u16,
        def_s: u16,
    ) -> Result<(), RuntimeError> {
        let key = MapKey::Int(self.scalars[s_base + key_s as usize] as i64);
        let def = self.scalars[s_base + def_s as usize];
        let val = match &self.objs[o_base + map_obj as usize] {
            Obj::Map(arc) => match arc.get(&key) {
                Some(MapVal::Scalar(v)) => *v,
                _ => def,
            },
            _ => return Err(err_msg("Expected Map in MapGetIntIntOr")),
        };
        self.scalars[s_base + dst_s as usize] = val;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
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
        if self.frames.len() >= MAX_CALL_DEPTH {
            return Err(err_call_depth());
        }
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
        self.frames.push(CallFrame {
            func_id: caller_func_id,
            return_ip,
            s_base: cur_s_base,
            o_base: cur_o_base,
            ret_reg,
            prev_closure_slot: cur_closure_slot,
        });
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
        self.objs[dst_slot] = Obj::Str(Arc::new(encoded));
        Ok(())
    }
}
