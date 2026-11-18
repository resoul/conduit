use std::sync::Arc;

use crate::ir::{CallableRef, ElemKind, Module, PipelineStage};
use crate::vm::exec_cold::{err_div_zero, err_msg, err_overflow};
use crate::vm::opcode::Op;
use crate::vm::pipeline_leaf::{
    exec_obj_leaf, exec_scalar_leaf, is_o0_read_only, is_obj_leaf_ops, is_scalar_leaf_ops,
    try_run_record_bump_stage,
};
use crate::vm::value::{ClosureData, ListStorage, Obj};
use crate::vm::{ExecResult, RuntimeError, Vm};

#[cfg(test)]
thread_local! {
    pub(crate) static FAST_HITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn match_mod_eq_filter(ops: &[Op], int_consts: &[i64]) -> Option<(i64, i64)> {
    if ops.len() > 8 {
        return None;
    }
    let mut imm = [None; 16];
    let mut mod_of_x = [None; 16];
    let mut eq_res = [None; 16];
    for &op in ops {
        match op {
            Op::LoadImmInt { dst, imm: v } if (dst as usize) < 16 => {
                imm[dst as usize] = Some(i64::from(v));
            }
            Op::LoadConstInt { dst, idx } if (dst as usize) < 16 => {
                imm[dst as usize] = int_consts.get(idx as usize).copied();
            }
            Op::NegInt { dst, src } if (dst as usize) < 16 && (src as usize) < 16 => {
                imm[dst as usize] = imm[src as usize].and_then(i64::checked_neg);
            }
            Op::ModIntImm { dst, a, imm: m } if a == 0 && (dst as usize) < 16 => {
                mod_of_x[dst as usize] = Some(i64::from(m));
            }
            Op::ModInt { dst, a, b }
                if a == 0 && (dst as usize) < 16 && (b as usize) < 16 =>
            {
                mod_of_x[dst as usize] = imm[b as usize];
            }
            Op::EqIntImm { dst, a, imm: r }
                if (dst as usize) < 16 && (a as usize) < 16 =>
            {
                if let Some(m) = mod_of_x[a as usize] {
                    eq_res[dst as usize] = Some((m, i64::from(r)));
                }
            }
            Op::EqInt { dst, a, b }
                if (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                if let (Some(m), Some(r)) = (mod_of_x[a as usize], imm[b as usize]) {
                    eq_res[dst as usize] = Some((m, r));
                } else if let (Some(r), Some(m)) = (imm[a as usize], mod_of_x[b as usize]) {
                    eq_res[dst as usize] = Some((m, r));
                }
            }
            Op::ReturnScalar { src } if (src as usize) < 16 => {
                return eq_res[src as usize];
            }
            _ => return None,
        }
    }
    None
}

#[inline]
fn match_affine_map(ops: &[Op], int_consts: &[i64]) -> Option<(i64, i64)> {
    if ops.len() > 8 {
        return None;
    }
    let mut imm: [Option<i64>; 16] = [None; 16];
    let mut aff: [Option<(i64, i64)>; 16] = [None; 16];
    aff[0] = Some((1, 0));
    // stage 0: raw x only; stage 1: after at most one x*imm; stage 2: after at most one add/sub.
    let mut stage: u8 = 0;
    for &op in ops {
        match op {
            Op::LoadImmInt { dst, imm: v } if (dst as usize) < 16 => {
                imm[dst as usize] = Some(i64::from(v));
                aff[dst as usize] = None;
            }
            Op::LoadConstInt { dst, idx } if (dst as usize) < 16 => {
                imm[dst as usize] = int_consts.get(idx as usize).copied();
                aff[dst as usize] = None;
            }
            Op::NegInt { dst, src }
                if (dst as usize) < 16 && (src as usize) < 16 && imm[src as usize].is_some() =>
            {
                imm[dst as usize] = imm[src as usize].and_then(i64::checked_neg);
                aff[dst as usize] = None;
            }
            Op::MoveScalar { dst, src }
                if (dst as usize) < 16 && (src as usize) < 16 =>
            {
                imm[dst as usize] = imm[src as usize];
                aff[dst as usize] = aff[src as usize];
            }
            Op::MulInt { dst, a, b }
                if stage == 0 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let res = match (aff[a as usize], imm[b as usize], imm[a as usize], aff[b as usize]) {
                    (Some((k, c)), Some(m), _, _) | (_, _, Some(m), Some((k, c))) => {
                        Some((k.checked_mul(m)?, c.checked_mul(m)?))
                    }
                    _ => None,
                }?;
                aff[dst as usize] = Some(res);
                imm[dst as usize] = None;
                stage = 1;
            }
            Op::AddIntImm { dst, a, imm: d }
                if stage <= 1 && (dst as usize) < 16 && (a as usize) < 16 =>
            {
                let (k, c) = aff[a as usize]?;
                aff[dst as usize] = Some((k, c.checked_add(i64::from(d))?));
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::AddInt { dst, a, b }
                if stage <= 1 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let res = match (aff[a as usize], imm[b as usize], imm[a as usize], aff[b as usize]) {
                    (Some((k, c)), Some(d), _, _) | (_, _, Some(d), Some((k, c))) => {
                        Some((k, c.checked_add(d)?))
                    }
                    _ => None,
                }?;
                aff[dst as usize] = Some(res);
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::SubInt { dst, a, b }
                if stage <= 1 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let (k, c) = aff[a as usize]?;
                let d = imm[b as usize]?;
                aff[dst as usize] = Some((k, c.checked_sub(d)?));
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::ReturnScalar { src } if (src as usize) < 16 => {
                return aff[src as usize];
            }
            _ => return None,
        }
    }
    None
}

#[inline]
fn match_sum_reduce(ops: &[Op]) -> bool {
    matches!(
        ops,
        [
            Op::AddInt { dst, a: 0, b: 1 } | Op::AddInt { dst, a: 1, b: 0 },
            Op::ReturnScalar { src }
        ] if dst == src
    )
}

#[derive(Clone, Copy)]
enum IntCmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
enum IntFilter {
    ModEq(i64, i64),
    Cmp(IntCmp, i64),
}

#[derive(Clone, Copy)]
enum IntReduceOp {
    Sum,
    Min,
    Max,
}

#[inline]
fn flip_int_cmp(c: IntCmp) -> IntCmp {
    match c {
        IntCmp::Eq => IntCmp::Eq,
        IntCmp::Ne => IntCmp::Ne,
        IntCmp::Lt => IntCmp::Gt,
        IntCmp::Le => IntCmp::Ge,
        IntCmp::Gt => IntCmp::Lt,
        IntCmp::Ge => IntCmp::Le,
    }
}

#[inline]
fn match_cmp_int_filter(ops: &[Op], int_consts: &[i64]) -> Option<(IntCmp, i64)> {
    if ops.len() > 8 {
        return None;
    }
    let mut imm: [Option<i64>; 16] = [None; 16];
    let mut is_x = [false; 16];
    is_x[0] = true;
    let mut cmp_res: [Option<(IntCmp, i64)>; 16] = [None; 16];
    for &op in ops {
        match op {
            Op::LoadImmInt { dst, imm: v } if (dst as usize) < 16 => {
                imm[dst as usize] = Some(i64::from(v));
                is_x[dst as usize] = false;
            }
            Op::LoadConstInt { dst, idx } if (dst as usize) < 16 => {
                imm[dst as usize] = int_consts.get(idx as usize).copied();
                is_x[dst as usize] = false;
            }
            Op::NegInt { dst, src } if (dst as usize) < 16 && (src as usize) < 16 => {
                imm[dst as usize] = imm[src as usize].and_then(i64::checked_neg);
                is_x[dst as usize] = false;
            }
            Op::MoveScalar { dst, src } if (dst as usize) < 16 && (src as usize) < 16 => {
                imm[dst as usize] = imm[src as usize];
                is_x[dst as usize] = is_x[src as usize];
                cmp_res[dst as usize] = cmp_res[src as usize];
            }
            Op::EqIntImm { dst, a, imm: c } if (dst as usize) < 16 && (a as usize) < 16 && is_x[a as usize] => {
                cmp_res[dst as usize] = Some((IntCmp::Eq, i64::from(c)));
            }
            Op::LtIntImm { dst, a, imm: c } if (dst as usize) < 16 && (a as usize) < 16 && is_x[a as usize] => {
                cmp_res[dst as usize] = Some((IntCmp::Lt, i64::from(c)));
            }
            Op::EqInt { dst, a, b }
            | Op::NeInt { dst, a, b }
            | Op::LtInt { dst, a, b }
            | Op::LeInt { dst, a, b }
            | Op::GtInt { dst, a, b }
            | Op::GeInt { dst, a, b }
                if (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let base = match op {
                    Op::EqInt { .. } => IntCmp::Eq,
                    Op::NeInt { .. } => IntCmp::Ne,
                    Op::LtInt { .. } => IntCmp::Lt,
                    Op::LeInt { .. } => IntCmp::Le,
                    Op::GtInt { .. } => IntCmp::Gt,
                    _ => IntCmp::Ge,
                };
                if is_x[a as usize]
                    && let Some(c) = imm[b as usize]
                {
                    cmp_res[dst as usize] = Some((base, c));
                } else if is_x[b as usize]
                    && let Some(c) = imm[a as usize]
                {
                    cmp_res[dst as usize] = Some((flip_int_cmp(base), c));
                } else {
                    return None;
                }
            }
            Op::ReturnScalar { src } if (src as usize) < 16 => {
                return cmp_res[src as usize];
            }
            _ => return None,
        }
    }
    None
}

#[inline]
fn match_int_filter(ops: &[Op], int_consts: &[i64]) -> Option<IntFilter> {
    if let Some((m, r)) = match_mod_eq_filter(ops, int_consts) {
        return Some(IntFilter::ModEq(m, r));
    }
    match_cmp_int_filter(ops, int_consts).map(|(cmp, c)| IntFilter::Cmp(cmp, c))
}

#[inline]
fn extract_if_else_returns(ops: &[Op], cond_reg: u16, target: u32) -> Option<(u16, u16)> {
    // ops[0] is comparison, ops[1] is JumpIfFalse { cond, target }
    let t = target as usize;
    if t >= ops.len() || t < 3 {
        return None;
    }
    match (&ops[2..t], &ops[t..]) {
        ([Op::ReturnScalar { src: ts }], [Op::ReturnScalar { src: es }])
            if (*ts == 0 || *ts == 1) && (*es == 0 || *es == 1) && ts != es =>
        {
            let _ = cond_reg;
            Some((*ts, *es))
        }
        (
            [Op::ReturnScalar { src: ts }],
            [Op::MoveScalar { dst: d, src: es }, Op::ReturnScalar { src: r }],
        ) if d == r && (*ts == 0 || *ts == 1) && (*es == 0 || *es == 1) && ts != es => {
            Some((*ts, *es))
        }
        (
            [Op::MoveScalar { dst: d1, src: ts }, Op::Jump { target: j }],
            [Op::MoveScalar { dst: d2, src: es }, Op::ReturnScalar { src: r }],
        ) if *j as usize == ops.len() - 1
            && d1 == r
            && d2 == r
            && (*ts == 0 || *ts == 1)
            && (*es == 0 || *es == 1)
            && ts != es =>
        {
            Some((*ts, *es))
        }
        _ => None,
    }
}

#[inline]
fn match_int_reduce(ops: &[Op]) -> Option<IntReduceOp> {
    if match_sum_reduce(ops) {
        return Some(IntReduceOp::Sum);
    }
    if ops.len() < 4 || ops.len() > 6 {
        return None;
    }
    let (cmp, a, b, cond_reg) = match ops[0] {
        Op::LtInt { dst, a, b } => (IntCmp::Lt, a, b, dst),
        Op::LeInt { dst, a, b } => (IntCmp::Le, a, b, dst),
        Op::GtInt { dst, a, b } => (IntCmp::Gt, a, b, dst),
        Op::GeInt { dst, a, b } => (IntCmp::Ge, a, b, dst),
        _ => return None,
    };
    if !((a == 0 && b == 1) || (a == 1 && b == 0)) {
        return None;
    }
    let Op::JumpIfFalse { cond, target } = ops[1] else {
        return None;
    };
    if cond != cond_reg {
        return None;
    }
    let (then_s, _else_s) = extract_if_else_returns(ops, cond_reg, target)?;
    // Normalize so we ask: when `acc <cmp_norm> x` is true, does it return `acc` (0)?
    let cmp_norm = if a == 0 { cmp } else { flip_int_cmp(cmp) };
    let returns_acc_when_true = then_s == 0;
    match (cmp_norm, returns_acc_when_true) {
        (IntCmp::Lt | IntCmp::Le, true) | (IntCmp::Gt | IntCmp::Ge, false) => Some(IntReduceOp::Min),
        (IntCmp::Gt | IntCmp::Ge, true) | (IntCmp::Lt | IntCmp::Le, false) => Some(IntReduceOp::Max),
        _ => None,
    }
}

#[inline(always)]
fn mod_eq_const<const M: i64>(x: i64, rem: i64) -> (bool, bool) {
    let (r, ov) = x.overflowing_rem(M);
    (r == rem, ov)
}

#[inline(always)]
fn run_int_kernel(
    src: &[i64],
    init_acc: u64,
    filter: impl Fn(i64) -> (bool, bool),
    map: impl Fn(i64) -> (i64, bool),
    reduce: impl Fn(i64, i64) -> (i64, bool),
) -> Result<u64, RuntimeError> {
    let mut acc = init_acc as i64;
    let mut ov = false;
    for &x in src.iter() {
        let (keep, o1) = filter(x);
        ov |= o1;
        if keep {
            let (v, o2) = map(x);
            let (next, o3) = reduce(acc, v);
            acc = next;
            ov |= o2 | o3;
        }
    }
    if ov {
        Err(err_overflow())
    } else {
        Ok(acc as u64)
    }
}

#[inline(always)]
fn dispatch_int_reduce(
    src: &[i64],
    init_acc: u64,
    filter: impl Fn(i64) -> (bool, bool),
    map: impl Fn(i64) -> (i64, bool),
    red: IntReduceOp,
) -> Result<u64, RuntimeError> {
    match red {
        IntReduceOp::Sum => run_int_kernel(src, init_acc, filter, map, i64::overflowing_add),
        IntReduceOp::Min => run_int_kernel(src, init_acc, filter, map, |a, v| (a.min(v), false)),
        IntReduceOp::Max => run_int_kernel(src, init_acc, filter, map, |a, v| (a.max(v), false)),
    }
}

#[inline(always)]
fn dispatch_int_filter_map_reduce(
    src: &[i64],
    init_acc: u64,
    filter: Option<IntFilter>,
    map: Option<(i64, i64)>,
    red: IntReduceOp,
) -> Result<u64, RuntimeError> {
    let map_fn = move |x: i64| -> (i64, bool) {
        match map {
            Some((mul, add)) => {
                let (v, o1) = x.overflowing_mul(mul);
                let (res, o2) = v.overflowing_add(add);
                (res, o1 | o2)
            }
            None => (x, false),
        }
    };
    match filter {
        None => dispatch_int_reduce(src, init_acc, |_| (true, false), map_fn, red),
        Some(IntFilter::ModEq(m, rem)) => {
            if m == 0 {
                if src.is_empty() {
                    return Ok(init_acc);
                }
                return Err(err_div_zero());
            }
            match m {
                2 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<2>(x, rem), map_fn, red),
                3 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<3>(x, rem), map_fn, red),
                4 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<4>(x, rem), map_fn, red),
                5 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<5>(x, rem), map_fn, red),
                8 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<8>(x, rem), map_fn, red),
                10 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<10>(x, rem), map_fn, red),
                16 => dispatch_int_reduce(src, init_acc, |x| mod_eq_const::<16>(x, rem), map_fn, red),
                _ => dispatch_int_reduce(
                    src,
                    init_acc,
                    |x| {
                        let (r, ov) = x.overflowing_rem(m);
                        (r == rem, ov)
                    },
                    map_fn,
                    red,
                ),
            }
        }
        Some(IntFilter::Cmp(IntCmp::Eq, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x == c, false), map_fn, red)
        }
        Some(IntFilter::Cmp(IntCmp::Ne, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x != c, false), map_fn, red)
        }
        Some(IntFilter::Cmp(IntCmp::Lt, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x < c, false), map_fn, red)
        }
        Some(IntFilter::Cmp(IntCmp::Le, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x <= c, false), map_fn, red)
        }
        Some(IntFilter::Cmp(IntCmp::Gt, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x > c, false), map_fn, red)
        }
        Some(IntFilter::Cmp(IntCmp::Ge, c)) => {
            dispatch_int_reduce(src, init_acc, |x| (x >= c, false), map_fn, red)
        }
    }
}

#[inline]
fn try_run_int_reduce_fast(
    src: &[i64],
    init_acc: u64,
    stages: &[ResolvedStage<'_>],
    int_consts: &[i64],
) -> Option<Result<u64, RuntimeError>> {
    match stages {
        [st_f, st_m, st_r]
            if matches!(st_f.kind, StageKind::FilterScalar)
                && matches!(st_m.kind, StageKind::MapScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_f.closure_slot == usize::MAX
                && st_m.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let f = match_int_filter(st_f.ops, int_consts)?;
            let m = match_affine_map(st_m.ops, int_consts)?;
            let r = match_int_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_int_filter_map_reduce(src, init_acc, Some(f), Some(m), r))
        }
        [st_f, st_r]
            if matches!(st_f.kind, StageKind::FilterScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_f.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let f = match_int_filter(st_f.ops, int_consts)?;
            let r = match_int_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_int_filter_map_reduce(src, init_acc, Some(f), None, r))
        }
        [st_m, st_r]
            if matches!(st_m.kind, StageKind::MapScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_m.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let m = match_affine_map(st_m.ops, int_consts)?;
            let r = match_int_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_int_filter_map_reduce(src, init_acc, None, Some(m), r))
        }
        [st_r]
            if matches!(st_r.kind, StageKind::ReduceScalar)
                && st_r.closure_slot == usize::MAX =>
        {
            let r = match_int_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_int_filter_map_reduce(src, init_acc, None, None, r))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum FloatCmp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
enum FloatReduceOp {
    Sum,
    MinLt,
    MinLe,
    MaxGt,
    MaxGe,
}

#[inline]
fn flip_float_cmp(c: FloatCmp) -> FloatCmp {
    match c {
        FloatCmp::Eq => FloatCmp::Eq,
        FloatCmp::Lt => FloatCmp::Gt,
        FloatCmp::Le => FloatCmp::Ge,
        FloatCmp::Gt => FloatCmp::Lt,
        FloatCmp::Ge => FloatCmp::Le,
    }
}

#[inline]
fn match_float_filter(ops: &[Op], float_consts: &[f64]) -> Option<(FloatCmp, f64)> {
    if ops.len() > 8 {
        return None;
    }
    let mut imm: [Option<f64>; 16] = [None; 16];
    let mut is_x = [false; 16];
    is_x[0] = true;
    let mut cmp_res: [Option<(FloatCmp, f64)>; 16] = [None; 16];
    for &op in ops {
        match op {
            Op::LoadConstFloat { dst, idx } if (dst as usize) < 16 => {
                imm[dst as usize] = float_consts.get(idx as usize).copied();
                is_x[dst as usize] = false;
            }
            Op::MoveScalar { dst, src } if (dst as usize) < 16 && (src as usize) < 16 => {
                imm[dst as usize] = imm[src as usize];
                is_x[dst as usize] = is_x[src as usize];
                cmp_res[dst as usize] = cmp_res[src as usize];
            }
            Op::EqFloat { dst, a, b } | Op::LtFloat { dst, a, b } | Op::LeFloat { dst, a, b }
                if (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let base = match op {
                    Op::EqFloat { .. } => FloatCmp::Eq,
                    Op::LtFloat { .. } => FloatCmp::Lt,
                    _ => FloatCmp::Le,
                };
                if is_x[a as usize]
                    && let Some(c) = imm[b as usize]
                {
                    cmp_res[dst as usize] = Some((base, c));
                } else if is_x[b as usize]
                    && let Some(c) = imm[a as usize]
                {
                    cmp_res[dst as usize] = Some((flip_float_cmp(base), c));
                } else {
                    return None;
                }
            }
            Op::ReturnScalar { src } if (src as usize) < 16 => {
                return cmp_res[src as usize];
            }
            _ => return None,
        }
    }
    None
}

#[inline]
fn match_float_affine_map(ops: &[Op], float_consts: &[f64]) -> Option<(f64, f64)> {
    if ops.len() > 8 {
        return None;
    }
    let mut imm: [Option<f64>; 16] = [None; 16];
    let mut aff: [Option<(f64, f64)>; 16] = [None; 16];
    aff[0] = Some((1.0, 0.0));
    let mut stage: u8 = 0;
    for &op in ops {
        match op {
            Op::LoadConstFloat { dst, idx } if (dst as usize) < 16 => {
                imm[dst as usize] = float_consts.get(idx as usize).copied();
                aff[dst as usize] = None;
            }
            Op::MoveScalar { dst, src } if (dst as usize) < 16 && (src as usize) < 16 => {
                imm[dst as usize] = imm[src as usize];
                aff[dst as usize] = aff[src as usize];
            }
            Op::MulFloat { dst, a, b }
                if stage == 0 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let res = match (aff[a as usize], imm[b as usize], imm[a as usize], aff[b as usize]) {
                    (Some((k, c)), Some(m), _, _) | (_, _, Some(m), Some((k, c))) => {
                        Some((k * m, c * m))
                    }
                    _ => None,
                }?;
                aff[dst as usize] = Some(res);
                imm[dst as usize] = None;
                stage = 1;
            }
            Op::AddFloat { dst, a, b }
                if stage <= 1 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let res = match (aff[a as usize], imm[b as usize], imm[a as usize], aff[b as usize]) {
                    (Some((k, c)), Some(d), _, _) | (_, _, Some(d), Some((k, c))) => {
                        Some((k, c + d))
                    }
                    _ => None,
                }?;
                aff[dst as usize] = Some(res);
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::SubFloat { dst, a, b }
                if stage <= 1 && (dst as usize) < 16 && (a as usize) < 16 && (b as usize) < 16 =>
            {
                let (k, c) = aff[a as usize]?;
                let d = imm[b as usize]?;
                aff[dst as usize] = Some((k, c - d));
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::FmaFloat { dst, a, b, c }
                if stage == 0
                    && (dst as usize) < 16
                    && (a as usize) < 16
                    && (b as usize) < 16
                    && (c as usize) < 16 =>
            {
                let prod = match (aff[a as usize], imm[b as usize], imm[a as usize], aff[b as usize]) {
                    (Some((k, c0)), Some(m), _, _) | (_, _, Some(m), Some((k, c0))) => {
                        Some((k * m, c0 * m))
                    }
                    _ => None,
                }?;
                let d = imm[c as usize]?;
                aff[dst as usize] = Some((prod.0, prod.1 + d));
                imm[dst as usize] = None;
                stage = 2;
            }
            Op::ReturnScalar { src } if (src as usize) < 16 => {
                return aff[src as usize];
            }
            _ => return None,
        }
    }
    None
}

#[inline]
fn match_float_reduce(ops: &[Op]) -> Option<FloatReduceOp> {
    if matches!(
        ops,
        [
            Op::AddFloat { dst, a: 0, b: 1 } | Op::AddFloat { dst, a: 1, b: 0 },
            Op::ReturnScalar { src }
        ] if dst == src
    ) {
        return Some(FloatReduceOp::Sum);
    }
    if ops.len() < 4 || ops.len() > 6 {
        return None;
    }
    let (cmp, a, b, cond_reg) = match ops[0] {
        Op::LtFloat { dst, a, b } => (FloatCmp::Lt, a, b, dst),
        Op::LeFloat { dst, a, b } => (FloatCmp::Le, a, b, dst),
        _ => return None,
    };
    if !((a == 0 && b == 1) || (a == 1 && b == 0)) {
        return None;
    }
    let Op::JumpIfFalse { cond, target } = ops[1] else {
        return None;
    };
    if cond != cond_reg {
        return None;
    }
    let (then_s, _else_s) = extract_if_else_returns(ops, cond_reg, target)?;
    let cmp_norm = if a == 0 { cmp } else { flip_float_cmp(cmp) };
    let returns_acc_when_true = then_s == 0;
    match (cmp_norm, returns_acc_when_true) {
        (FloatCmp::Lt, true) => Some(FloatReduceOp::MinLt),
        (FloatCmp::Le, true) => Some(FloatReduceOp::MinLe),
        (FloatCmp::Gt, true) => Some(FloatReduceOp::MaxGt),
        (FloatCmp::Ge, true) => Some(FloatReduceOp::MaxGe),
        // Note: when NaN is involved, `if acc < x { x } else { acc }` differs from `if acc > x { acc } else { x }`,
        // so only match direct `returns_acc_when_true` forms or handle `else` when non-NaN equivalence isn't needed;
        // actually we can preserve the exact comparison: if `then_s == 1`, `acc` is returned in `else`.
        _ => None,
    }
}

#[inline(always)]
fn run_float_kernel(
    src: &[f64],
    init_acc: u64,
    filter: impl Fn(f64) -> bool,
    map: impl Fn(f64) -> f64,
    reduce: impl Fn(f64, f64) -> f64,
) -> u64 {
    let mut acc = f64::from_bits(init_acc);
    for &x in src.iter() {
        if filter(x) {
            let v = map(x);
            acc = reduce(acc, v);
        }
    }
    acc.to_bits()
}

#[inline(always)]
fn run_float_sum_kernel(
    src: &[f64],
    init_acc: u64,
    filter: impl Fn(f64) -> bool,
    map: impl Fn(f64) -> f64,
) -> u64 {
    // `acc + (-0.0) == acc` bit-for-bit for every non-signaling `acc` (incl. ±0.0, ±inf, NaN),
    // so adding `-0.0` for rejected elements matches the interpreter's conditional add exactly
    // while keeping the select off the `acc` dependency chain (one fadd latency per element).
    let mut acc = f64::from_bits(init_acc);
    for &x in src.iter() {
        let v = if filter(x) { map(x) } else { -0.0 };
        acc += v;
    }
    acc.to_bits()
}

#[inline(always)]
fn dispatch_float_reduce(
    src: &[f64],
    init_acc: u64,
    filter: impl Fn(f64) -> bool,
    map: impl Fn(f64) -> f64,
    red: FloatReduceOp,
) -> u64 {
    match red {
        FloatReduceOp::Sum => run_float_sum_kernel(src, init_acc, filter, map),
        FloatReduceOp::MinLt => {
            run_float_kernel(src, init_acc, filter, map, |a, v| if a < v { a } else { v })
        }
        FloatReduceOp::MinLe => {
            run_float_kernel(src, init_acc, filter, map, |a, v| if a <= v { a } else { v })
        }
        FloatReduceOp::MaxGt => {
            run_float_kernel(src, init_acc, filter, map, |a, v| if a > v { a } else { v })
        }
        FloatReduceOp::MaxGe => {
            run_float_kernel(src, init_acc, filter, map, |a, v| if a >= v { a } else { v })
        }
    }
}

#[inline(always)]
fn dispatch_float_filter_map_reduce(
    src: &[f64],
    init_acc: u64,
    filter: Option<(FloatCmp, f64)>,
    map: Option<(f64, f64)>,
    red: FloatReduceOp,
) -> u64 {
    let map_fn = move |x: f64| -> f64 {
        match map {
            Some((mul, add)) => x * mul + add,
            None => x,
        }
    };
    match filter {
        None => dispatch_float_reduce(src, init_acc, |_| true, map_fn, red),
        Some((FloatCmp::Eq, c)) => dispatch_float_reduce(src, init_acc, |x| x == c, map_fn, red),
        Some((FloatCmp::Lt, c)) => dispatch_float_reduce(src, init_acc, |x| x < c, map_fn, red),
        Some((FloatCmp::Le, c)) => dispatch_float_reduce(src, init_acc, |x| x <= c, map_fn, red),
        Some((FloatCmp::Gt, c)) => dispatch_float_reduce(src, init_acc, |x| x > c, map_fn, red),
        Some((FloatCmp::Ge, c)) => dispatch_float_reduce(src, init_acc, |x| x >= c, map_fn, red),
    }
}

#[inline]
fn try_run_float_reduce_fast(
    src: &[f64],
    init_acc: u64,
    stages: &[ResolvedStage<'_>],
    float_consts: &[f64],
) -> Option<u64> {
    match stages {
        [st_f, st_m, st_r]
            if matches!(st_f.kind, StageKind::FilterScalar)
                && matches!(st_m.kind, StageKind::MapScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_f.closure_slot == usize::MAX
                && st_m.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let f = match_float_filter(st_f.ops, float_consts)?;
            let m = match_float_affine_map(st_m.ops, float_consts)?;
            let r = match_float_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_float_filter_map_reduce(src, init_acc, Some(f), Some(m), r))
        }
        [st_f, st_r]
            if matches!(st_f.kind, StageKind::FilterScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_f.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let f = match_float_filter(st_f.ops, float_consts)?;
            let r = match_float_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_float_filter_map_reduce(src, init_acc, Some(f), None, r))
        }
        [st_m, st_r]
            if matches!(st_m.kind, StageKind::MapScalar)
                && matches!(st_r.kind, StageKind::ReduceScalar)
                && st_m.closure_slot == usize::MAX
                && st_r.closure_slot == usize::MAX =>
        {
            let m = match_float_affine_map(st_m.ops, float_consts)?;
            let r = match_float_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_float_filter_map_reduce(src, init_acc, None, Some(m), r))
        }
        [st_r]
            if matches!(st_r.kind, StageKind::ReduceScalar)
                && st_r.closure_slot == usize::MAX =>
        {
            let r = match_float_reduce(st_r.ops)?;
            #[cfg(test)]
            FAST_HITS.with(|c| c.set(c.get() + 1));
            Some(dispatch_float_filter_map_reduce(src, init_acc, None, None, r))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum StageKind {
    MapScalar,
    FilterScalar,
    MapObj,
    FilterObj,
    ReduceScalar,
    MapObjToScalar,
    MapScalarToObj,
}

#[derive(Clone, Copy)]
struct ResolvedStage<'a> {
    kind: StageKind,
    func_id: u32,
    closure_slot: usize,
    ops: &'a [Op],
    is_scalar_leaf: bool,
    is_obj_leaf: bool,
    o0_read_only: bool,
    num_scalars: u16,
    num_objs: u16,
}

#[derive(Clone, Copy)]
struct LeafStage<'m, 'c> {
    kind: StageKind,
    ops: &'m [Op],
    closure: Option<&'c ClosureData>,
    o0_read_only: bool,
}

#[inline]
fn resolve_leaf_stages<'m, 'c>(
    stages: &[ResolvedStage<'m>],
    objs: &'c [Obj],
) -> Vec<LeafStage<'m, 'c>> {
    let mut out = Vec::with_capacity(stages.len());
    for st in stages {
        let closure = if st.closure_slot == usize::MAX {
            None
        } else {
            match objs.get(st.closure_slot) {
                Some(Obj::Closure(c)) => Some(c.as_ref()),
                _ => None,
            }
        };
        out.push(LeafStage {
            kind: st.kind,
            ops: st.ops,
            closure,
            o0_read_only: st.o0_read_only,
        });
    }
    out
}

impl Vm {
    fn resolve_stage<'a>(
        &self,
        module: &'a Module,
        caller_o_base: usize,
        stage: PipelineStage,
    ) -> Result<ResolvedStage<'a>, RuntimeError> {
        let (kind, cref) = match stage {
            PipelineStage::MapScalarToScalar(c) => (StageKind::MapScalar, c),
            PipelineStage::FilterScalar(c) => (StageKind::FilterScalar, c),
            PipelineStage::MapObjToObj(c) => (StageKind::MapObj, c),
            PipelineStage::FilterObj(c) => (StageKind::FilterObj, c),
            PipelineStage::ReduceScalar(c) => (StageKind::ReduceScalar, c),
            PipelineStage::MapObjToScalar(c) => (StageKind::MapObjToScalar, c),
            PipelineStage::MapScalarToObj(c) => (StageKind::MapScalarToObj, c),
        };
        let (func_id, closure_slot) = match cref {
            CallableRef::DirectFunc(fid) => (fid, usize::MAX),
            CallableRef::RegObj(reg) => {
                let slot = caller_o_base + reg as usize;
                match &self.objs[slot] {
                    Obj::Func(fid) => (*fid, usize::MAX),
                    Obj::Closure(c) => (c.func_id, slot),
                    _ => return Err(err_msg("Pipeline callback is not callable")),
                }
            }
        };
        let func = &module.functions[func_id as usize];
        Ok(ResolvedStage {
            kind,
            func_id,
            closure_slot,
            ops: &func.ops,
            is_scalar_leaf: is_scalar_leaf_ops(&func.ops, func.num_objs),
            is_obj_leaf: is_obj_leaf_ops(&func.ops),
            o0_read_only: is_o0_read_only(&func.ops, &module.records),
            num_scalars: func.num_scalars,
            num_objs: func.num_objs,
        })
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn run_scalar_slice<T: Copy, R>(
        &mut self,
        module: &Module,
        src: &[T],
        stages: &[ResolvedStage<'_>],
        has_filter: bool,
        all_scalar_leaf: bool,
        stage_s_base: usize,
        stage_o_base: usize,
        max_s: usize,
        to_u64: impl Fn(T) -> u64,
        from_u64: impl Fn(u64) -> R,
    ) -> Result<Vec<R>, RuntimeError>
    where
        R: From<T>,
    {
        let out_cap = if has_filter { src.len() / 2 } else { src.len() };
        let mut out = Vec::with_capacity(out_cap);

        if all_scalar_leaf {
            let leaf_stages = resolve_leaf_stages(stages, &self.objs);
            let s_win = &mut self.scalars[stage_s_base..stage_s_base + max_s];
            let s_ptr = s_win.as_mut_ptr();
            let int_consts = module.int_consts.as_ptr();
            let float_consts = module.float_consts.as_ptr();

            match leaf_stages.as_slice() {
                [LeafStage {
                    kind: StageKind::MapScalar,
                    ops,
                    closure,
                    ..
                }] => {
                    for &item in src {
                        unsafe {
                            *s_ptr = to_u64(item);
                            let res =
                                exec_scalar_leaf(s_ptr, ops, int_consts, float_consts, *closure)?;
                            out.push(from_u64(res));
                        }
                    }
                }
                [LeafStage {
                    kind: StageKind::FilterScalar,
                    ops,
                    closure,
                    ..
                }] => {
                    for &item in src {
                        unsafe {
                            *s_ptr = to_u64(item);
                            if exec_scalar_leaf(s_ptr, ops, int_consts, float_consts, *closure)?
                                != 0
                            {
                                out.push(R::from(item));
                            }
                        }
                    }
                }
                [
                    LeafStage {
                        kind: StageKind::MapScalar,
                        ops: ops1,
                        closure: c1,
                        ..
                    },
                    LeafStage {
                        kind: StageKind::FilterScalar,
                        ops: ops2,
                        closure: c2,
                        ..
                    },
                ] => {
                    for &item in src {
                        unsafe {
                            *s_ptr = to_u64(item);
                            let mapped =
                                exec_scalar_leaf(s_ptr, ops1, int_consts, float_consts, *c1)?;
                            *s_ptr = mapped;
                            if exec_scalar_leaf(s_ptr, ops2, int_consts, float_consts, *c2)? != 0 {
                                out.push(from_u64(mapped));
                            }
                        }
                    }
                }
                _ => {
                    for &item in src {
                        let mut cur = to_u64(item);
                        let mut keep = true;
                        for st in &leaf_stages {
                            let res = unsafe {
                                *s_ptr = cur;
                                exec_scalar_leaf(
                                    s_ptr,
                                    st.ops,
                                    int_consts,
                                    float_consts,
                                    st.closure,
                                )?
                            };
                            match st.kind {
                                StageKind::MapScalar => cur = res,
                                StageKind::FilterScalar => {
                                    if res == 0 {
                                        keep = false;
                                        break;
                                    }
                                }
                                _ => {
                                    return Err(err_msg(
                                        "Mismatched stage kind for scalar pipeline",
                                    ));
                                }
                            }
                        }
                        if keep {
                            out.push(from_u64(cur));
                        }
                    }
                }
            }
        } else {
            for &item in src {
                let mut cur = to_u64(item);
                let mut keep = true;
                for st in stages {
                    self.scalars[stage_s_base] = cur;
                    let res = match self.exec_from(
                        module,
                        st.func_id,
                        stage_s_base,
                        stage_o_base,
                        st.closure_slot,
                    )? {
                        ExecResult::Scalar(v) => v,
                        _ => return Err(err_msg("Expected scalar stage result")),
                    };
                    match st.kind {
                        StageKind::MapScalar => cur = res,
                        StageKind::FilterScalar => {
                            if res == 0 {
                                keep = false;
                                break;
                            }
                        }
                        _ => return Err(err_msg("Mismatched stage kind for scalar pipeline")),
                    }
                }
                if keep {
                    out.push(from_u64(cur));
                }
            }
        }

        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_pipeline(
        &mut self,
        module: &Module,
        caller_o_base: usize,
        dst_obj: u16,
        src_obj: u16,
        plan_idx: u16,
        stage_s_base: usize,
        stage_o_base: usize,
    ) -> Result<(), RuntimeError> {
        let plan = &module.pipelines[plan_idx as usize];
        let mut stages = Vec::with_capacity(plan.stages.len());
        let mut max_s = 1usize;
        let mut max_o = 1usize;
        let mut has_filter = false;
        let mut all_scalar_leaf = true;
        let mut all_obj_leaf = true;

        for &stage in plan.stages.iter() {
            let resolved = self.resolve_stage(module, caller_o_base, stage)?;
            max_s = max_s.max(resolved.num_scalars as usize);
            max_o = max_o.max(resolved.num_objs as usize);
            if matches!(resolved.kind, StageKind::FilterScalar | StageKind::FilterObj) {
                has_filter = true;
            }
            if !resolved.is_scalar_leaf {
                all_scalar_leaf = false;
            }
            if !resolved.is_obj_leaf {
                all_obj_leaf = false;
            }
            stages.push(resolved);
        }

        self.ensure_capacity(stage_s_base + max_s, stage_o_base + max_o);

        let dst_slot = caller_o_base + dst_obj as usize;
        let clear_dst = !stages.iter().any(|s| s.closure_slot == dst_slot);

        if stages
            .iter()
            .any(|s| matches!(s.kind, StageKind::MapObjToScalar | StageKind::MapScalarToObj))
        {
            return self.run_mixed_pipeline(
                module,
                plan.elem_kind,
                plan.out_elem_kind,
                &stages,
                caller_o_base,
                src_obj,
                dst_obj,
                stage_s_base,
                stage_o_base,
                clear_dst,
            );
        }

        match plan.elem_kind {
            ElemKind::Int => {
                let src_arc = match &self.objs[caller_o_base + src_obj as usize] {
                    Obj::List(ListStorage::Int(arc)) => Arc::clone(arc),
                    _ => return Err(err_msg("Expected [Int] input in pipeline")),
                };
                if clear_dst {
                    self.objs[dst_slot] = Obj::Empty;
                }
                let out = self.run_scalar_slice(
                    module,
                    &src_arc,
                    &stages,
                    has_filter,
                    all_scalar_leaf,
                    stage_s_base,
                    stage_o_base,
                    max_s,
                    |v| v as u64,
                    |bits| bits as i64,
                )?;
                self.objs[caller_o_base + dst_obj as usize] =
                    Obj::List(ListStorage::Int(Arc::new(out)));
            }
            ElemKind::Float => {
                let src_arc = match &self.objs[caller_o_base + src_obj as usize] {
                    Obj::List(ListStorage::Float(arc)) => Arc::clone(arc),
                    _ => return Err(err_msg("Expected [Float] input in pipeline")),
                };
                if clear_dst {
                    self.objs[dst_slot] = Obj::Empty;
                }
                let out = self.run_scalar_slice(
                    module,
                    &src_arc,
                    &stages,
                    has_filter,
                    all_scalar_leaf,
                    stage_s_base,
                    stage_o_base,
                    max_s,
                    f64::to_bits,
                    f64::from_bits,
                )?;
                self.objs[caller_o_base + dst_obj as usize] =
                    Obj::List(ListStorage::Float(Arc::new(out)));
            }
            ElemKind::Obj => {
                let src_arc = match &self.objs[caller_o_base + src_obj as usize] {
                    Obj::List(ListStorage::Obj(arc)) => Arc::clone(arc),
                    _ => return Err(err_msg("Expected [Obj] input in pipeline")),
                };
                if clear_dst {
                    self.objs[dst_slot] = Obj::Empty;
                }
                let out_cap = if has_filter {
                    src_arc.len() / 2
                } else {
                    src_arc.len()
                };
                let mut out = Vec::with_capacity(out_cap);

                if all_obj_leaf {
                    let s_win = &mut self.scalars[stage_s_base..stage_s_base + max_s];
                    let (before, after) = self.objs.split_at_mut(stage_o_base);
                    let o_win = &mut after[..max_o];
                    let s_ptr = s_win.as_mut_ptr();
                    let leaf_stages = resolve_leaf_stages(&stages, before);

                    match leaf_stages.as_slice() {
                        [LeafStage {
                            kind: StageKind::MapObj,
                            ops,
                            closure,
                            o0_read_only: true,
                        }] => {
                            let fast_res = if closure.is_none() {
                                try_run_record_bump_stage(&src_arc, ops, &module.records, &mut out)
                            } else {
                                None
                            };
                            if let Some(res) = fast_res {
                                res?;
                            } else {
                                if !matches!(o_win[0], Obj::Empty) {
                                    o_win[0] = Obj::Empty;
                                }
                                let o_ptr = o_win.as_mut_ptr();
                                let mut res = Ok(());
                                for item in src_arc.iter() {
                                    unsafe {
                                        std::ptr::copy_nonoverlapping(
                                            item as *const Obj,
                                            o_ptr,
                                            1,
                                        );
                                    }
                                    match unsafe {
                                        exec_obj_leaf(s_ptr, o_ptr, ops, module, *closure)
                                    } {
                                        Ok(ExecResult::Obj(next_obj)) => out.push(next_obj),
                                        Ok(_) => {
                                            res = Err(err_msg(
                                                "Mismatched stage kind/result in Obj pipeline",
                                            ));
                                            break;
                                        }
                                        Err(e) => {
                                            res = Err(e);
                                            break;
                                        }
                                    }
                                }
                                unsafe { std::ptr::write(o_ptr, Obj::Empty) };
                                for slot in &mut o_win[1..] {
                                    if !matches!(slot, Obj::Empty) {
                                        *slot = Obj::Empty;
                                    }
                                }
                                res?;
                            }
                        }
                        [LeafStage {
                            kind: StageKind::FilterObj,
                            ops,
                            closure,
                            o0_read_only: true,
                        }] => {
                            if !matches!(o_win[0], Obj::Empty) {
                                o_win[0] = Obj::Empty;
                            }
                            let o_ptr = o_win.as_mut_ptr();
                            let mut res = Ok(());
                            for item in src_arc.iter() {
                                unsafe {
                                    std::ptr::copy_nonoverlapping(item as *const Obj, o_ptr, 1);
                                }
                                match unsafe { exec_obj_leaf(s_ptr, o_ptr, ops, module, *closure) }
                                {
                                    Ok(ExecResult::Scalar(pass)) => {
                                        if pass != 0 {
                                            out.push(item.clone());
                                        }
                                    }
                                    Ok(_) => {
                                        res = Err(err_msg(
                                            "Mismatched stage kind/result in Obj pipeline",
                                        ));
                                        break;
                                    }
                                    Err(e) => {
                                        res = Err(e);
                                        break;
                                    }
                                }
                            }
                            unsafe { std::ptr::write(o_ptr, Obj::Empty) };
                            for slot in &mut o_win[1..] {
                                if !matches!(slot, Obj::Empty) {
                                    *slot = Obj::Empty;
                                }
                            }
                            res?;
                        }
                        _ => {
                            let o_ptr = o_win.as_mut_ptr();
                            let loop_res: Result<(), RuntimeError> = (|| {
                                if let [LeafStage {
                                    kind: StageKind::MapObj,
                                    ops,
                                    closure,
                                    ..
                                }] = leaf_stages.as_slice()
                                {
                                    for item in src_arc.iter() {
                                        unsafe { *o_ptr = item.clone() };
                                        match unsafe {
                                            exec_obj_leaf(s_ptr, o_ptr, ops, module, *closure)
                                        }? {
                                            ExecResult::Obj(next_obj) => out.push(next_obj),
                                            _ => {
                                                return Err(err_msg(
                                                    "Mismatched stage kind/result in Obj pipeline",
                                                ));
                                            }
                                        }
                                    }
                                } else {
                                    if !matches!(unsafe { &*o_ptr }, Obj::Empty) {
                                        unsafe { *o_ptr = Obj::Empty };
                                    }
                                    for item in src_arc.iter() {
                                        let mut owned_cur: Option<Obj> = None;
                                        let mut keep = true;
                                        for st in &leaf_stages {
                                            let borrowed_item =
                                                st.o0_read_only && owned_cur.is_none();
                                            if borrowed_item {
                                                unsafe {
                                                    std::ptr::copy_nonoverlapping(
                                                        item as *const Obj,
                                                        o_ptr,
                                                        1,
                                                    );
                                                }
                                            } else {
                                                unsafe {
                                                    *o_ptr = match st.kind {
                                                        StageKind::MapObj => owned_cur
                                                            .take()
                                                            .unwrap_or_else(|| item.clone()),
                                                        StageKind::FilterObj => owned_cur
                                                            .as_ref()
                                                            .unwrap_or(item)
                                                            .clone(),
                                                        _ => {
                                                            return Err(err_msg(
                                                                "Mismatched stage kind in Obj pipeline",
                                                            ));
                                                        }
                                                    };
                                                }
                                            }
                                            let res = unsafe {
                                                exec_obj_leaf(
                                                    s_ptr, o_ptr, st.ops, module, st.closure,
                                                )
                                            };
                                            if borrowed_item {
                                                unsafe { std::ptr::write(o_ptr, Obj::Empty) };
                                            }
                                            let res = res?;
                                            match (st.kind, res) {
                                                (StageKind::MapObj, ExecResult::Obj(next_obj)) => {
                                                    owned_cur = Some(next_obj);
                                                }
                                                (StageKind::FilterObj, ExecResult::Scalar(pass)) => {
                                                    if pass == 0 {
                                                        keep = false;
                                                        break;
                                                    }
                                                }
                                                _ => {
                                                    return Err(err_msg(
                                                        "Mismatched stage kind/result in Obj pipeline",
                                                    ));
                                                }
                                            }
                                        }
                                        if keep {
                                            out.push(owned_cur.unwrap_or_else(|| item.clone()));
                                        }
                                    }
                                }
                                Ok(())
                            })();

                            for slot in o_win.iter_mut() {
                                if !matches!(slot, Obj::Empty) {
                                    *slot = Obj::Empty;
                                }
                            }
                            loop_res?;
                        }
                    }
                } else {
                    for item in src_arc.iter() {
                        let mut owned_cur: Option<Obj> = None;
                        let mut keep = true;
                        for st in &stages {
                            self.objs[stage_o_base] = match st.kind {
                                StageKind::MapObj => {
                                    owned_cur.take().unwrap_or_else(|| item.clone())
                                }
                                StageKind::FilterObj => {
                                    owned_cur.as_ref().unwrap_or(item).clone()
                                }
                                _ => {
                                    return Err(err_msg(
                                        "Mismatched stage kind in Obj pipeline",
                                    ));
                                }
                            };
                            let res = self.exec_from(
                                module,
                                st.func_id,
                                stage_s_base,
                                stage_o_base,
                                st.closure_slot,
                            )?;
                            match (st.kind, res) {
                                (StageKind::MapObj, ExecResult::Obj(next_obj)) => {
                                    owned_cur = Some(next_obj);
                                }
                                (StageKind::FilterObj, ExecResult::Scalar(pass)) => {
                                    if pass == 0 {
                                        keep = false;
                                        break;
                                    }
                                }
                                _ => {
                                    return Err(err_msg(
                                        "Mismatched stage kind/result in Obj pipeline",
                                    ));
                                }
                            }
                        }
                        if keep {
                            out.push(owned_cur.unwrap_or_else(|| item.clone()));
                        }
                    }
                }

                self.objs[caller_o_base + dst_obj as usize] =
                    Obj::List(ListStorage::Obj(Arc::new(out)));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_scalar_slice_reduce<T: Copy>(
        &mut self,
        module: &Module,
        stages: &[ResolvedStage<'_>],
        src: &[T],
        mut acc: u64,
        to_u64: fn(T) -> u64,
        stage_s_base: usize,
        stage_o_base: usize,
    ) -> Result<u64, RuntimeError> {
        let all_scalar_leaf = stages.iter().all(|st| st.is_scalar_leaf);
        if all_scalar_leaf {
            let max_s = stages.iter().map(|s| s.num_scalars as usize).max().unwrap_or(2).max(2);
            self.ensure_capacity(stage_s_base + max_s, stage_o_base);
            let leaf_stages = resolve_leaf_stages(stages, &self.objs);
            let s_ptr = unsafe { self.scalars.as_mut_ptr().add(stage_s_base) };
            let int_consts = module.int_consts.as_ptr();
            let float_consts = module.float_consts.as_ptr();

            match leaf_stages.as_slice() {
                [LeafStage { kind: StageKind::ReduceScalar, ops, closure, .. }] => {
                    for &item in src {
                        unsafe {
                            *s_ptr = acc;
                            *s_ptr.add(1) = to_u64(item);
                            acc = exec_scalar_leaf(s_ptr, ops, int_consts, float_consts, *closure)?;
                        }
                    }
                }
                [
                    LeafStage { kind: StageKind::FilterScalar, ops: ops_f, closure: cf, .. },
                    LeafStage { kind: StageKind::MapScalar, ops: ops_m, closure: cm, .. },
                    LeafStage { kind: StageKind::ReduceScalar, ops: ops_r, closure: cr, .. },
                ] => {
                    for &item in src {
                        unsafe {
                            let x = to_u64(item);
                            *s_ptr = x;
                            if exec_scalar_leaf(s_ptr, ops_f, int_consts, float_consts, *cf)? != 0 {
                                *s_ptr = x;
                                let mapped = exec_scalar_leaf(s_ptr, ops_m, int_consts, float_consts, *cm)?;
                                *s_ptr = acc;
                                *s_ptr.add(1) = mapped;
                                acc = exec_scalar_leaf(s_ptr, ops_r, int_consts, float_consts, *cr)?;
                            }
                        }
                    }
                }
                [
                    LeafStage { kind: StageKind::FilterScalar, ops: ops_f, closure: cf, .. },
                    LeafStage { kind: StageKind::ReduceScalar, ops: ops_r, closure: cr, .. },
                ] => {
                    for &item in src {
                        unsafe {
                            let x = to_u64(item);
                            *s_ptr = x;
                            if exec_scalar_leaf(s_ptr, ops_f, int_consts, float_consts, *cf)? != 0 {
                                *s_ptr = acc;
                                *s_ptr.add(1) = x;
                                acc = exec_scalar_leaf(s_ptr, ops_r, int_consts, float_consts, *cr)?;
                            }
                        }
                    }
                }
                [
                    LeafStage { kind: StageKind::MapScalar, ops: ops_m, closure: cm, .. },
                    LeafStage { kind: StageKind::ReduceScalar, ops: ops_r, closure: cr, .. },
                ] => {
                    for &item in src {
                        unsafe {
                            *s_ptr = to_u64(item);
                            let mapped = exec_scalar_leaf(s_ptr, ops_m, int_consts, float_consts, *cm)?;
                            *s_ptr = acc;
                            *s_ptr.add(1) = mapped;
                            acc = exec_scalar_leaf(s_ptr, ops_r, int_consts, float_consts, *cr)?;
                        }
                    }
                }
                _ => {
                    for &item in src {
                        let mut cur = to_u64(item);
                        for st in &leaf_stages {
                            match st.kind {
                                StageKind::MapScalar => {
                                    cur = unsafe {
                                        *s_ptr = cur;
                                        exec_scalar_leaf(s_ptr, st.ops, int_consts, float_consts, st.closure)?
                                    };
                                }
                                StageKind::FilterScalar => {
                                    let pass = unsafe {
                                        *s_ptr = cur;
                                        exec_scalar_leaf(s_ptr, st.ops, int_consts, float_consts, st.closure)?
                                    };
                                    if pass == 0 {
                                        break;
                                    }
                                }
                                StageKind::ReduceScalar => {
                                    acc = unsafe {
                                        *s_ptr = acc;
                                        *s_ptr.add(1) = cur;
                                        exec_scalar_leaf(s_ptr, st.ops, int_consts, float_consts, st.closure)?
                                    };
                                }
                                _ => return Err(err_msg("Mismatched stage kind in scalar reduce pipeline")),
                            }
                        }
                    }
                }
            }
            return Ok(acc);
        }

        self.ensure_capacity(stage_s_base + 2, stage_o_base);
        for &item in src {
            let mut cur = to_u64(item);
            for st in stages {
                match st.kind {
                    StageKind::MapScalar => {
                        self.scalars[stage_s_base] = cur;
                        let ExecResult::Scalar(v) = self.exec_from(module, st.func_id, stage_s_base, stage_o_base, st.closure_slot)? else {
                            return Err(err_msg("Expected scalar stage result"));
                        };
                        cur = v;
                    }
                    StageKind::FilterScalar => {
                        self.scalars[stage_s_base] = cur;
                        let ExecResult::Scalar(v) = self.exec_from(module, st.func_id, stage_s_base, stage_o_base, st.closure_slot)? else {
                            return Err(err_msg("Expected scalar stage result"));
                        };
                        if v == 0 {
                            break;
                        }
                    }
                    StageKind::ReduceScalar => {
                        self.scalars[stage_s_base] = acc;
                        self.scalars[stage_s_base + 1] = cur;
                        let ExecResult::Scalar(v) = self.exec_from(module, st.func_id, stage_s_base, stage_o_base, st.closure_slot)? else {
                            return Err(err_msg("Expected scalar reduce result"));
                        };
                        acc = v;
                    }
                    _ => return Err(err_msg("Mismatched stage kind in scalar reduce pipeline")),
                }
            }
        }
        Ok(acc)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_pipeline_reduce(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        dst_s: u16,
        src_obj: u16,
        plan_idx: u16,
    ) -> Result<(), RuntimeError> {
        let plan = &module.pipelines[plan_idx as usize];
        let mut stages = Vec::with_capacity(plan.stages.len());
        for &st in plan.stages.iter() {
            stages.push(self.resolve_stage(module, o_base, st)?);
        }
        let stage_s_base = s_top;
        let stage_o_base = o_top;
        let init_acc = self.scalars[s_base + dst_s as usize];

        let res = match &self.objs[o_base + src_obj as usize] {
            Obj::List(ListStorage::Int(v)) => {
                let src = Arc::clone(v);
                if let Some(fast_res) = try_run_int_reduce_fast(&src, init_acc, &stages, &module.int_consts) {
                    fast_res
                } else {
                    self.run_scalar_slice_reduce(module, &stages, &src, init_acc, |x| x as u64, stage_s_base, stage_o_base)
                }
            }
            Obj::List(ListStorage::Float(v)) => {
                let src = Arc::clone(v);
                if let Some(fast_val) = try_run_float_reduce_fast(&src, init_acc, &stages, &module.float_consts) {
                    Ok(fast_val)
                } else {
                    self.run_scalar_slice_reduce(module, &stages, &src, init_acc, f64::to_bits, stage_s_base, stage_o_base)
                }
            }
            _ => return Err(err_msg("Expected scalar list in pipeline reduce")),
        };
        self.scalars[s_base + dst_s as usize] = res?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_mixed_pipeline(
        &mut self,
        module: &Module,
        in_kind: ElemKind,
        out_kind: ElemKind,
        stages: &[ResolvedStage<'_>],
        caller_o_base: usize,
        src_obj: u16,
        dst_obj: u16,
        stage_s_base: usize,
        stage_o_base: usize,
        clear_dst: bool,
    ) -> Result<(), RuntimeError> {
        enum Cur {
            Scalar(u64),
            Obj(Obj),
        }

        let src_clone = self.objs[caller_o_base + src_obj as usize].clone();
        if clear_dst {
            self.objs[caller_o_base + dst_obj as usize] = Obj::Empty;
        }
        let mut out_ints: Vec<i64> = Vec::new();
        let mut out_floats: Vec<f64> = Vec::new();
        let mut out_objs: Vec<Obj> = Vec::new();

        let len = match (&src_clone, in_kind) {
            (Obj::List(ListStorage::Int(v)), ElemKind::Int) => v.len(),
            (Obj::List(ListStorage::Float(v)), ElemKind::Float) => v.len(),
            (Obj::List(ListStorage::Obj(v)), ElemKind::Obj) => v.len(),
            _ => return Err(err_msg("Mismatched list element type in mixed pipeline")),
        };

        for idx in 0..len {
            let mut cur = match &src_clone {
                Obj::List(ListStorage::Int(v)) => Cur::Scalar(v[idx] as u64),
                Obj::List(ListStorage::Float(v)) => Cur::Scalar(v[idx].to_bits()),
                Obj::List(ListStorage::Obj(v)) => Cur::Obj(v[idx].clone()),
                _ => unreachable!(),
            };
            let mut keep = true;
            for st in stages {
                match (st.kind, &cur) {
                    (StageKind::MapScalar | StageKind::MapScalarToObj, Cur::Scalar(val)) => {
                        self.scalars[stage_s_base] = *val;
                        let res = self.exec_from(
                            module,
                            st.func_id,
                            stage_s_base,
                            stage_o_base,
                            st.closure_slot,
                        )?;
                        cur = match (st.kind, res) {
                            (StageKind::MapScalar, ExecResult::Scalar(v)) => Cur::Scalar(v),
                            (StageKind::MapScalarToObj, ExecResult::Obj(o)) => Cur::Obj(o),
                            _ => return Err(err_msg("Mismatched stage result in mixed pipeline")),
                        };
                    }
                    (StageKind::FilterScalar, Cur::Scalar(val)) => {
                        self.scalars[stage_s_base] = *val;
                        let ExecResult::Scalar(b) = self.exec_from(
                            module,
                            st.func_id,
                            stage_s_base,
                            stage_o_base,
                            st.closure_slot,
                        )? else {
                            return Err(err_msg("Expected scalar bool from filter"));
                        };
                        if b == 0 {
                            keep = false;
                            break;
                        }
                    }
                    (StageKind::MapObj | StageKind::MapObjToScalar, Cur::Obj(o)) => {
                        self.objs[stage_o_base] = o.clone();
                        let res = self.exec_from(
                            module,
                            st.func_id,
                            stage_s_base,
                            stage_o_base,
                            st.closure_slot,
                        )?;
                        cur = match (st.kind, res) {
                            (StageKind::MapObj, ExecResult::Obj(next)) => Cur::Obj(next),
                            (StageKind::MapObjToScalar, ExecResult::Scalar(v)) => Cur::Scalar(v),
                            _ => return Err(err_msg("Mismatched stage result in mixed pipeline")),
                        };
                    }
                    (StageKind::FilterObj, Cur::Obj(o)) => {
                        self.objs[stage_o_base] = o.clone();
                        let ExecResult::Scalar(b) = self.exec_from(
                            module,
                            st.func_id,
                            stage_s_base,
                            stage_o_base,
                            st.closure_slot,
                        )? else {
                            return Err(err_msg("Expected scalar bool from filter"));
                        };
                        if b == 0 {
                            keep = false;
                            break;
                        }
                    }
                    _ => return Err(err_msg("Mismatched stage kind in mixed pipeline")),
                }
            }
            if keep {
                match (out_kind, cur) {
                    (ElemKind::Int, Cur::Scalar(v)) => out_ints.push(v as i64),
                    (ElemKind::Float, Cur::Scalar(v)) => out_floats.push(f64::from_bits(v)),
                    (ElemKind::Obj, Cur::Obj(o)) => out_objs.push(o),
                    _ => return Err(err_msg("Mismatched final element kind in mixed pipeline")),
                }
            }
        }

        self.objs[caller_o_base + dst_obj as usize] = match out_kind {
            ElemKind::Int => Obj::List(ListStorage::Int(Arc::new(out_ints))),
            ElemKind::Float => Obj::List(ListStorage::Float(Arc::new(out_floats))),
            ElemKind::Obj => Obj::List(ListStorage::Obj(Arc::new(out_objs))),
        };
        Ok(())
    }
}
