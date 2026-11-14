use std::sync::Arc;

use crate::ir::{CallableRef, ElemKind, Module, PipelineStage};
use crate::vm::exec_cold::err_msg;
use crate::vm::opcode::Op;
use crate::vm::pipeline_leaf::{
    exec_obj_leaf, exec_scalar_leaf, is_o0_read_only, is_obj_leaf_ops, is_scalar_leaf_ops,
    try_run_record_bump_stage,
};
use crate::vm::value::{ClosureData, ListStorage, Obj};
use crate::vm::{ExecResult, RuntimeError, Vm};

#[derive(Clone, Copy)]
enum StageKind {
    MapScalar,
    FilterScalar,
    MapObj,
    FilterObj,
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
}
