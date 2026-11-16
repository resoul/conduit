pub mod bundle;
pub mod lower;
pub(crate) mod lower_call;
pub(crate) mod lower_expr;
pub(crate) mod lower_loops;
pub(crate) mod lower_native;
pub(crate) mod lower_prealloc;

use std::fmt::Write;

use crate::vm::opcode::{
    EXT_CLI_HELP, EXT_CLI_PARSE, EXT_CLI_PARSE_ARGS, EXT_ENV_ARGS, EXT_ENV_GET, EXT_ENV_LOAD,
    EXT_ENV_REQUIRE, EXT_FLOAT_CEIL, EXT_FLOAT_MATH, EXT_FS_BINARY, EXT_FS_UNARY, EXT_INT_MATH, EXT_LIST_FIRST,
    EXT_LIST_IS_EMPTY, EXT_LIST_LAST, EXT_LIST_SORT, EXT_MAP_CONTAINS_KEY, EXT_MAP_ENTRIES,
    EXT_MAP_FROM, EXT_MAP_GET, EXT_MAP_INDEX, EXT_MAP_KEYS, EXT_MAP_REMOVE, EXT_MAP_SET,
    EXT_MAP_VALUES, EXT_PRINT_OBJ, EXT_PRINT_SCALAR, EXT_STR_IS_EMPTY, EXT_STR_PRED, EXT_STR_SPLIT,
    EXT_STR_TO_FLOAT, EXT_STR_TO_INT, Op,
};
use crate::vm::value::Obj;

#[derive(Debug, Clone)]
pub struct FunctionProto {
    pub name: String,
    pub num_scalars: u16,
    pub num_objs: u16,
    pub scalar_params: u16,
    pub obj_params: u16,
    pub returns_obj: bool,
    pub ops: Box<[Op]>,
}

#[derive(Debug, Clone)]
pub struct ClosureProto {
    pub func_id: u32,
    pub capture_scalars: Box<[u16]>,
    pub capture_objs: Box<[u16]>,
}

#[derive(Debug, Clone)]
pub struct RecordDesc {
    pub schema_id: u32,
    pub scalar_regs: Box<[u16]>,
    pub obj_regs: Box<[u16]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableRef {
    DirectFunc(u32),
    RegObj(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    MapScalarToScalar(CallableRef),
    FilterScalar(CallableRef),
    MapObjToObj(CallableRef),
    FilterObj(CallableRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElemKind {
    Int,
    Float,
    Obj,
}

#[derive(Debug, Clone)]
pub struct PipelinePlan {
    pub stages: Box<[PipelineStage]>,
    pub elem_kind: ElemKind,
}

#[derive(Debug, Default)]
pub struct Module {
    pub functions: Vec<FunctionProto>,
    pub int_consts: Vec<i64>,
    pub float_consts: Vec<f64>,
    pub obj_consts: Vec<Obj>,
    pub closures: Vec<ClosureProto>,
    pub records: Vec<RecordDesc>,
    pub pipelines: Vec<PipelinePlan>,
    pub struct_schemas: Vec<crate::native::StructSchema>,
    pub json_plans: Vec<crate::native::JsonDecodePlan>,
    pub validation_plans: Vec<crate::native::validation::ValidationStructPlan>,
    pub env_plans: Vec<crate::native::env::EnvLoadPlan>,
    pub cli_plans: Vec<crate::native::cli::CliPlan>,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn func_id(&self, name: &str) -> Option<u32> {
        self.functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    /// Validates all register indices, constant indices, function IDs, and jump targets once
    /// so the VM execution loop can rely on these invariants.
    pub fn validate(&self) -> Result<(), String> {
        for (fid, func) in self.functions.iter().enumerate() {
            let Some(&last_op) = func.ops.last() else {
                return Err(format!("func {} ({}): ops cannot be empty", fid, func.name));
            };
            if !matches!(
                last_op,
                Op::ReturnScalar { .. }
                    | Op::ReturnObj { .. }
                    | Op::ReturnVoid
                    | Op::Jump { .. }
            ) {
                return Err(format!(
                    "func {} ({}): must end with Return* or Jump",
                    fid, func.name
                ));
            }
            if func.scalar_params > func.num_scalars {
                return Err(format!(
                    "func {} ({}): scalar_params {} > num_scalars {}",
                    fid, func.name, func.scalar_params, func.num_scalars
                ));
            }
            if func.obj_params > func.num_objs {
                return Err(format!(
                    "func {} ({}): obj_params {} > num_objs {}",
                    fid, func.name, func.obj_params, func.num_objs
                ));
            }
            let op_len = func.ops.len();
            let check_s = |r: u16, ctx: &str| -> Result<(), String> {
                if r >= func.num_scalars {
                    return Err(format!(
                        "func {} ({}): scalar reg {} out of bounds ({}) in {}",
                        fid, func.name, r, func.num_scalars, ctx
                    ));
                }
                Ok(())
            };
            let check_o = |r: u16, ctx: &str| -> Result<(), String> {
                if r >= func.num_objs {
                    return Err(format!(
                        "func {} ({}): obj reg {} out of bounds ({}) in {}",
                        fid, func.name, r, func.num_objs, ctx
                    ));
                }
                Ok(())
            };
            let check_jmp = |t: u32, ctx: &str| -> Result<(), String> {
                if (t as usize) >= op_len {
                    return Err(format!(
                        "func {} ({}): jump target {} out of bounds ({}) in {}",
                        fid, func.name, t, op_len, ctx
                    ));
                }
                Ok(())
            };

            for (ip, op) in func.ops.iter().copied().enumerate() {
                let ctx = format!("op@{}", ip);
                match op {
                    Op::LoadImmInt { dst, .. } | Op::LoadBool { dst, .. } => {
                        check_s(dst, &ctx)?;
                    }
                    Op::LoadConstInt { dst, idx } => {
                        check_s(dst, &ctx)?;
                        if (idx as usize) >= self.int_consts.len() {
                            return Err(format!("{}: int_const {} out of bounds", ctx, idx));
                        }
                    }
                    Op::LoadConstFloat { dst, idx } => {
                        check_s(dst, &ctx)?;
                        if (idx as usize) >= self.float_consts.len() {
                            return Err(format!("{}: float_const {} out of bounds", ctx, idx));
                        }
                    }
                    Op::MoveScalar { dst, src }
                    | Op::NegInt { dst, src }
                    | Op::IntToFloat { dst, src }
                    | Op::FloatFloor { dst, src }
                    | Op::FloatRound { dst, src }
                    | Op::NotBool { dst, src }
                    | Op::AddIntImm { dst, a: src, .. }
                    | Op::ModIntImm { dst, a: src, .. }
                    | Op::EqIntImm { dst, a: src, .. }
                    | Op::LtIntImm { dst, a: src, .. } => {
                        check_s(dst, &ctx)?;
                        check_s(src, &ctx)?;
                    }
                    Op::AddInt { dst, a, b }
                    | Op::SubInt { dst, a, b }
                    | Op::MulInt { dst, a, b }
                    | Op::DivInt { dst, a, b }
                    | Op::ModInt { dst, a, b }
                    | Op::AddFloat { dst, a, b }
                    | Op::SubFloat { dst, a, b }
                    | Op::MulFloat { dst, a, b }
                    | Op::DivFloat { dst, a, b }
                    | Op::EqInt { dst, a, b }
                    | Op::NeInt { dst, a, b }
                    | Op::LtInt { dst, a, b }
                    | Op::LeInt { dst, a, b }
                    | Op::GtInt { dst, a, b }
                    | Op::GeInt { dst, a, b }
                    | Op::EqFloat { dst, a, b }
                    | Op::LtFloat { dst, a, b }
                    | Op::LeFloat { dst, a, b } => {
                        check_s(dst, &ctx)?;
                        check_s(a, &ctx)?;
                        check_s(b, &ctx)?;
                    }
                    Op::FmaFloat { dst, a, b, c } => {
                        check_s(dst, &ctx)?;
                        check_s(a, &ctx)?;
                        check_s(b, &ctx)?;
                        check_s(c, &ctx)?;
                    }
                    Op::EqObj { dst_s, a_obj, b_obj } => {
                        check_s(dst_s, &ctx)?;
                        check_o(a_obj, &ctx)?;
                        check_o(b_obj, &ctx)?;
                    }
                    Op::Jump { target } => check_jmp(target, &ctx)?,
                    Op::JumpIfFalse { cond, target } | Op::JumpIfTrue { cond, target } => {
                        check_s(cond, &ctx)?;
                        check_jmp(target, &ctx)?;
                    }
                    Op::ForRangeStep { idx_reg, limit_reg, target } => {
                        check_s(idx_reg, &ctx)?;
                        check_s(limit_reg, &ctx)?;
                        check_jmp(target, &ctx)?;
                    }
                    Op::CallDirect {
                        func_id,
                        arg_base_s,
                        arg_base_o,
                        ret_reg,
                    } => {
                        let callee = self.functions.get(func_id as usize).ok_or_else(|| {
                            format!("{}: CallDirect func_id {} out of bounds", ctx, func_id)
                        })?;
                        if arg_base_s as usize + callee.scalar_params as usize
                            > func.num_scalars as usize
                        {
                            return Err(format!("{}: CallDirect arg_base_s out of bounds", ctx));
                        }
                        if arg_base_o as usize + callee.obj_params as usize > func.num_objs as usize
                        {
                            return Err(format!("{}: CallDirect arg_base_o out of bounds", ctx));
                        }
                        if callee.returns_obj {
                            check_o(ret_reg, &ctx)?;
                        } else {
                            check_s(ret_reg, &ctx)?;
                        }
                    }
                    Op::CallCallable {
                        callee_obj,
                        arg_base_s,
                        arg_base_o,
                        ret_reg,
                    } => {
                        check_o(callee_obj, &ctx)?;
                        if callee_obj >= arg_base_o {
                            return Err(format!(
                                "{}: CallCallable callee_obj {} must be < arg_base_o {}",
                                ctx, callee_obj, arg_base_o
                            ));
                        }
                        if arg_base_s > func.num_scalars || arg_base_o > func.num_objs {
                            return Err(format!("{}: CallCallable arg_base out of bounds", ctx));
                        }
                        // ret_reg can be either scalar or obj depending on dynamic callee; check max when non-zero
                        if ret_reg >= func.num_scalars && ret_reg >= func.num_objs {
                            return Err(format!("{}: CallCallable ret_reg {} out of bounds", ctx, ret_reg));
                        }
                    }
                    Op::MakeClosure { dst_obj, proto_idx } => {
                        check_o(dst_obj, &ctx)?;
                        let cp = self.closures.get(proto_idx as usize).ok_or_else(|| {
                            format!("{}: closure proto {} out of bounds", ctx, proto_idx)
                        })?;
                        if (cp.func_id as usize) >= self.functions.len() {
                            return Err(format!("{}: closure func_id out of bounds", ctx));
                        }
                        for &r in cp.capture_scalars.iter() {
                            check_s(r, &ctx)?;
                        }
                        for &r in cp.capture_objs.iter() {
                            check_o(r, &ctx)?;
                        }
                    }
                    Op::LoadUpvalueScalar { dst, .. } => check_s(dst, &ctx)?,
                    Op::LoadUpvalueObj { dst, .. } => check_o(dst, &ctx)?,
                    Op::LoadFuncObj { dst_obj, func_id } => {
                        check_o(dst_obj, &ctx)?;
                        if (func_id as usize) >= self.functions.len() {
                            return Err(format!("{}: LoadFuncObj func_id out of bounds", ctx));
                        }
                    }
                    Op::ReturnScalar { src } => check_s(src, &ctx)?,
                    Op::ReturnObj { src } => check_o(src, &ctx)?,
                    Op::ReturnVoid => {}
                    Op::LoadConstObj { dst_obj, idx } => {
                        check_o(dst_obj, &ctx)?;
                        if (idx as usize) >= self.obj_consts.len() {
                            return Err(format!("{}: obj_const {} out of bounds", ctx, idx));
                        }
                    }
                    Op::MoveObj { dst_obj, src } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src, &ctx)?;
                    }
                    Op::MakeRangeList { dst_obj, start_s, end_s } => {
                        check_o(dst_obj, &ctx)?;
                        check_s(start_s, &ctx)?;
                        check_s(end_s, &ctx)?;
                    }
                    Op::NewListInt { dst_obj, .. }
                    | Op::NewListFloat { dst_obj, .. }
                    | Op::NewListObj { dst_obj, .. }
                    | Op::NewMap { dst_obj } => check_o(dst_obj, &ctx)?,
                    Op::ListAppendInt { list_obj, val_s }
                    | Op::ListAppendFloat { list_obj, val_s } => {
                        check_o(list_obj, &ctx)?;
                        check_s(val_s, &ctx)?;
                    }
                    Op::ListAppendObj { list_obj, val_obj } => {
                        check_o(list_obj, &ctx)?;
                        check_o(val_obj, &ctx)?;
                    }
                    Op::ListLen { dst_s, list_obj }
                    | Op::StrLen { dst_s, str_obj: list_obj }
                    | Op::MapLen { dst_s, map_obj: list_obj } => {
                        check_s(dst_s, &ctx)?;
                        check_o(list_obj, &ctx)?;
                    }
                    Op::StrConcat { dst_obj, a_obj, b_obj }
                    | Op::ListJoin { dst_obj, list_obj: a_obj, sep_obj: b_obj } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(a_obj, &ctx)?;
                        check_o(b_obj, &ctx)?;
                    }
                    Op::ScalarToStr { dst_obj, src_s, .. } => {
                        check_o(dst_obj, &ctx)?;
                        check_s(src_s, &ctx)?;
                    }
                    Op::ObjToStr { dst_obj, src_obj }
                    | Op::StrMethod { dst_obj, src_obj, .. }
                    | Op::ListReverse { dst_obj, list_obj: src_obj }
                    | Op::ListEnumerate { dst_obj, list_obj: src_obj, .. }
                    | Op::TupleGetObj { dst_obj, tuple_obj: src_obj, .. } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::StrReplace { dst_obj, str_obj, from_obj, to_obj } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(str_obj, &ctx)?;
                        check_o(from_obj, &ctx)?;
                        check_o(to_obj, &ctx)?;
                    }
                    Op::ClampScalar { dst_s, val_s, lo_s, hi_s, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_s(val_s, &ctx)?;
                        check_s(lo_s, &ctx)?;
                        check_s(hi_s, &ctx)?;
                    }
                    Op::ListIndex { dst_obj, list_obj, idx_s, .. }
                    | Op::ListGet { dst_obj, list_obj, idx_s, .. } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(list_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                    }
                    Op::ListContainsScalar { dst_s, list_obj, val_s, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(list_obj, &ctx)?;
                        check_s(val_s, &ctx)?;
                    }
                    Op::ListContainsObj { dst_s, list_obj, val_obj } => {
                        check_s(dst_s, &ctx)?;
                        check_o(list_obj, &ctx)?;
                        check_o(val_obj, &ctx)?;
                    }
                    Op::ListSlice { dst_obj, list_obj, start_s, end_s } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(list_obj, &ctx)?;
                        check_s(start_s, &ctx)?;
                        check_s(end_s, &ctx)?;
                    }
                    Op::TupleGetScalar { dst_s, tuple_obj, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(tuple_obj, &ctx)?;
                    }
                    Op::ForListInt {
                        list_obj,
                        idx_s,
                        elem_s,
                        exit_target,
                    }
                    | Op::ForListIntStep {
                        list_obj,
                        idx_s,
                        elem_s,
                        target: exit_target,
                    } => {
                        check_o(list_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                        check_s(elem_s, &ctx)?;
                        check_jmp(exit_target, &ctx)?;
                    }
                    Op::ForListObj {
                        list_obj,
                        idx_s,
                        elem_obj,
                        exit_target,
                    } => {
                        check_o(list_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                        check_o(elem_obj, &ctx)?;
                        check_jmp(exit_target, &ctx)?;
                    }
                    Op::NewRecord { dst_obj, desc_idx }
                    | Op::NewRecordMove { dst_obj, desc_idx } => {
                        check_o(dst_obj, &ctx)?;
                        let desc = self.records.get(desc_idx as usize).ok_or_else(|| {
                            format!("{}: record desc {} out of bounds", ctx, desc_idx)
                        })?;
                        for &r in desc.scalar_regs.iter() {
                            check_s(r, &ctx)?;
                        }
                        for &r in desc.obj_regs.iter() {
                            check_o(r, &ctx)?;
                        }
                    }
                    Op::GetFieldScalar { dst_s, rec_obj, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(rec_obj, &ctx)?;
                    }
                    Op::GetFieldObj { dst_obj, rec_obj, .. } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(rec_obj, &ctx)?;
                    }
                    Op::MapSetIntInt { map_obj, key_s, val_s } => {
                        check_o(map_obj, &ctx)?;
                        check_s(key_s, &ctx)?;
                        check_s(val_s, &ctx)?;
                    }
                    Op::MapGetIntIntOr {
                        dst_s,
                        map_obj,
                        key_s,
                        def_s,
                    } => {
                        check_s(dst_s, &ctx)?;
                        check_o(map_obj, &ctx)?;
                        check_s(key_s, &ctx)?;
                        check_s(def_s, &ctx)?;
                    }
                    Op::RunPipeline {
                        dst_obj,
                        src_obj,
                        plan_idx,
                    } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                        let plan = self.pipelines.get(plan_idx as usize).ok_or_else(|| {
                            format!("{}: pipeline plan {} out of bounds", ctx, plan_idx)
                        })?;
                        for stage in plan.stages.iter() {
                            let cref = match stage {
                                PipelineStage::MapScalarToScalar(c)
                                | PipelineStage::FilterScalar(c)
                                | PipelineStage::MapObjToObj(c)
                                | PipelineStage::FilterObj(c) => c,
                            };
                            match *cref {
                                CallableRef::DirectFunc(id) => {
                                    if (id as usize) >= self.functions.len() {
                                        return Err(format!("{}: pipeline func {} out of bounds", ctx, id));
                                    }
                                }
                                CallableRef::RegObj(r) => check_o(r, &ctx)?,
                            }
                        }
                    }
                    Op::JsonDecode {
                        dst_obj,
                        src_obj,
                        plan_idx,
                    } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                        if (plan_idx as usize) >= self.json_plans.len() {
                            return Err(format!("{}: json_plan {} out of bounds", ctx, plan_idx));
                        }
                    }
                    Op::JsonEncode { dst_obj, src_obj } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::BoxScalar { dst_obj, src_s, .. } => {
                        check_o(dst_obj, &ctx)?;
                        check_s(src_s, &ctx)?;
                    }
                    Op::UnboxScalar { dst_s, src_obj } => {
                        check_s(dst_s, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::IsTypeObj { dst_s, src_obj, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::TryUnwrapScalar { dst_s, src_obj, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::TryCheckObj { src_obj, .. } => {
                        check_o(src_obj, &ctx)?;
                    }
                    Op::TimeOp {
                        kind,
                        dst_obj,
                        arg_reg,
                    } => {
                        check_o(dst_obj, &ctx)?;
                        match crate::native::time::TimeOpKind::from_u8(kind) {
                            Some(
                                crate::native::time::TimeOpKind::Now
                                | crate::native::time::TimeOpKind::Monotonic,
                            ) => {}
                            Some(
                                crate::native::time::TimeOpKind::Seconds
                                | crate::native::time::TimeOpKind::Milliseconds,
                            ) => check_s(arg_reg, &ctx)?,
                            Some(_) => check_o(arg_reg, &ctx)?,
                            None => return Err(format!("{}: invalid TimeOp kind {}", ctx, kind)),
                        }
                    }
                    Op::ValidationOp {
                        kind,
                        dst_obj,
                        a_reg,
                        b_reg,
                    } => {
                        check_o(dst_obj, &ctx)?;
                        match crate::native::validation::ValOpKind::from_u8(kind) {
                            Some(crate::native::validation::ValOpKind::Email) => {
                                check_o(a_reg, &ctx)?;
                            }
                            Some(
                                crate::native::validation::ValOpKind::MinInt
                                | crate::native::validation::ValOpKind::MaxInt
                                | crate::native::validation::ValOpKind::MinFloat
                                | crate::native::validation::ValOpKind::MaxFloat,
                            ) => {
                                check_s(a_reg, &ctx)?;
                                check_s(b_reg, &ctx)?;
                            }
                            Some(
                                crate::native::validation::ValOpKind::MinObj
                                | crate::native::validation::ValOpKind::MaxObj,
                            ) => {
                                check_o(a_reg, &ctx)?;
                                check_s(b_reg, &ctx)?;
                            }
                            Some(
                                crate::native::validation::ValOpKind::MinDyn
                                | crate::native::validation::ValOpKind::MaxDyn,
                            ) => {
                                check_o(a_reg, &ctx)?;
                                check_o(b_reg, &ctx)?;
                            }
                            None => return Err(format!("{}: invalid ValidationOp kind {}", ctx, kind)),
                        }
                    }
                    Op::ValidationCheck { dst_obj, src_obj } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                    }
                    Op::ExtOp { kind, aux, a, b, c } => {
                        let check_tag = |tag: u8, reg: u16| -> Result<(), String> {
                            if tag == 0 { check_o(reg, &ctx) } else { check_s(reg, &ctx) }
                        };
                        match kind {
                            EXT_PRINT_SCALAR => check_s(a, &ctx)?,
                            EXT_PRINT_OBJ => check_o(a, &ctx)?,
                            EXT_FLOAT_CEIL => {
                                check_s(a, &ctx)?;
                                check_s(b, &ctx)?;
                            }
                            EXT_STR_TO_INT | EXT_STR_TO_FLOAT => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_MAP_SET => {
                                check_o(a, &ctx)?;
                                check_tag(aux >> 4, b)?;
                                check_tag(aux & 0x0F, c)?;
                            }
                            EXT_MAP_GET | EXT_MAP_INDEX | EXT_MAP_REMOVE => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                                check_tag(aux >> 4, c)?;
                            }
                            EXT_MAP_CONTAINS_KEY => {
                                check_s(a, &ctx)?;
                                check_o(b, &ctx)?;
                                check_tag(aux, c)?;
                            }
                            EXT_MAP_KEYS | EXT_MAP_VALUES | EXT_MAP_ENTRIES | EXT_MAP_FROM => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_STR_IS_EMPTY => {
                                check_s(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_STR_PRED => {
                                check_s(a, &ctx)?;
                                check_o(b, &ctx)?;
                                check_o(c, &ctx)?;
                            }
                            EXT_STR_SPLIT => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                                check_o(c, &ctx)?;
                            }
                            EXT_INT_MATH | EXT_FLOAT_MATH => {
                                check_s(a, &ctx)?;
                                check_s(b, &ctx)?;
                                if aux != 0 {
                                    check_s(c, &ctx)?;
                                }
                            }
                            EXT_LIST_IS_EMPTY => {
                                check_s(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_LIST_FIRST | EXT_LIST_LAST | EXT_LIST_SORT => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_ENV_GET | EXT_ENV_REQUIRE => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            EXT_ENV_ARGS => {
                                check_o(a, &ctx)?;
                            }
                            EXT_ENV_LOAD => {
                                check_o(a, &ctx)?;
                                if (b as usize) >= self.env_plans.len() {
                                    return Err(format!("{}: env_plan {} out of bounds", ctx, b));
                                }
                            }
                            EXT_CLI_PARSE | EXT_CLI_HELP => {
                                check_o(a, &ctx)?;
                                if (b as usize) >= self.cli_plans.len() {
                                    return Err(format!("{}: cli_plan {} out of bounds", ctx, b));
                                }
                            }
                            EXT_CLI_PARSE_ARGS => {
                                check_o(a, &ctx)?;
                                check_o(c, &ctx)?;
                                if (b as usize) >= self.cli_plans.len() {
                                    return Err(format!("{}: cli_plan {} out of bounds", ctx, b));
                                }
                            }
                            EXT_FS_UNARY => {
                                if matches!(aux, 2..=4) {
                                    check_s(a, &ctx)?;
                                } else {
                                    check_o(a, &ctx)?;
                                }
                                check_o(b, &ctx)?;
                            }
                            EXT_FS_BINARY => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                                if aux == 7 {
                                    check_s(c, &ctx)?;
                                } else {
                                    check_o(c, &ctx)?;
                                }
                            }
                            crate::vm::opcode::EXT_PATH_OP => {
                                if aux == 7 {
                                    check_s(a, &ctx)?;
                                } else {
                                    check_o(a, &ctx)?;
                                }
                                check_o(b, &ctx)?;
                                if matches!(aux, 0 | 8) {
                                    check_o(c, &ctx)?;
                                }
                            }
                            crate::vm::opcode::EXT_PROCESS_OP => {
                                match aux {
                                    0 | 1 => {
                                        check_o(a, &ctx)?;
                                        check_o(b, &ctx)?;
                                        check_o(c, &ctx)?;
                                    }
                                    2 => check_o(a, &ctx)?,
                                    3 => {
                                        check_o(a, &ctx)?;
                                        check_o(b, &ctx)?;
                                    }
                                    4 | 5 => check_s(a, &ctx)?,
                                    _ => return Err(format!("{}: invalid process sub-op {}", ctx, aux)),
                                }
                            }
                            crate::vm::opcode::EXT_CRYPTO_OP => {
                                check_o(a, &ctx)?;
                                match aux {
                                    0 | 2..=5 => check_o(b, &ctx)?,
                                    1 => {
                                        check_o(b, &ctx)?;
                                        check_o(c, &ctx)?;
                                    }
                                    6 => {}
                                    7 => check_s(b, &ctx)?,
                                    8 => {
                                        check_s(b, &ctx)?;
                                        check_s(c, &ctx)?;
                                    }
                                    _ => return Err(format!("{}: invalid crypto sub-op {}", ctx, aux)),
                                }
                            }
                            _ => return Err(format!("Invalid ExtOp kind {}", kind)),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        for (fid, func) in self.functions.iter().enumerate() {
            let _ = writeln!(
                out,
                "fn #{} {} (s_params={}, o_params={}, s_regs={}, o_regs={}):",
                fid,
                func.name,
                func.scalar_params,
                func.obj_params,
                func.num_scalars,
                func.num_objs
            );
            for (ip, op) in func.ops.iter().enumerate() {
                let _ = writeln!(out, "  {:04}: {:?}", ip, op);
            }
        }
        out
    }
}
