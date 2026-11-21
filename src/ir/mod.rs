pub mod bundle;
pub mod lower;
pub(crate) mod lower_call;
pub(crate) mod lower_expr;
pub(crate) mod lower_ext;
pub(crate) mod lower_loops;
pub(crate) mod lower_native;
pub(crate) mod lower_prealloc;

use std::fmt::Write;

use crate::vm::opcode::{
    EXT_ASSERT, EXT_CACHE_OP, EXT_CHAN_OP, EXT_CLI_HELP, EXT_CLI_PARSE, EXT_CLI_PARSE_ARGS,
    EXT_ENV_ARGS, EXT_ENV_GET, EXT_ENV_LOAD, EXT_ENV_REQUIRE, EXT_FLOAT_CEIL, EXT_FLOAT_MATH,
    EXT_FS_BINARY, EXT_FS_UNARY, EXT_INT_MATH, EXT_JWT_OP, EXT_LIST_FIRST, EXT_LIST_IS_EMPTY,
    EXT_LIST_LAST, EXT_LIST_SORT, EXT_LOCK_OP, EXT_MAP_CONTAINS_KEY, EXT_MAP_ENTRIES,
    EXT_MAP_FROM, EXT_MAP_GET, EXT_MAP_INDEX, EXT_MAP_KEYS, EXT_MAP_REMOVE, EXT_MAP_SET,
    EXT_MAP_VALUES, EXT_PRINT_OBJ, EXT_PRINT_SCALAR, EXT_REDIS_OP, EXT_STR_IS_EMPTY,
    EXT_STR_PRED, EXT_STR_SPLIT, EXT_STR_TO_FLOAT, EXT_STR_TO_INT, EXT_TIME_OP, Op,
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
    ReduceScalar(CallableRef),
    MapObjToScalar(CallableRef),
    MapScalarToObj(CallableRef),
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
    pub out_elem_kind: ElemKind,
}

#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub version: i64,
    pub name: String,
    pub up_func_id: u32,
    pub down_func_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbColumnMeta {
    pub field_name: String,
    pub col_name: String,
    pub is_pk: bool,
    pub is_auto: bool,
    pub is_unique: bool,
    pub is_index: bool,
    pub default_sql: Option<String>,
    pub fk: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbModelMeta {
    pub schema_id: u32,
    pub table_name: String,
    pub columns: Vec<DbColumnMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronJobSpec {
    pub name: String,
    pub cron_expr: Option<String>,
    pub interval_ms: Option<u64>,
    pub func_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulePlan {
    pub name: String,
    pub jobs: Vec<CronJobSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSpec {
    pub name: String,
    pub func_id: u32,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HttpQueryDefault {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Nil,
}

#[derive(Debug, Clone)]
pub struct HttpQueryFieldPlan {
    pub name: String,
    pub param_name: String,
    pub kind: u8, // 0=String, 1=Int, 2=Float, 3=Bool
    pub is_optional: bool,
    pub slot: u16,
    pub default_val: Option<HttpQueryDefault>,
}

#[derive(Debug, Clone)]
pub struct HttpQueryPlan {
    pub schema_id: u32,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub fields: Vec<HttpQueryFieldPlan>,
}

pub fn is_on_start_hook_name(name: &str) -> bool {
    name.starts_with("__cdt_on_start_") || name.contains("___cdt_on_start_")
}

pub fn is_on_stop_hook_name(name: &str) -> bool {
    name.starts_with("__cdt_on_stop_") || name.contains("___cdt_on_stop_")
}

pub fn is_lifecycle_hook_name(name: &str) -> bool {
    is_on_start_hook_name(name) || is_on_stop_hook_name(name)
}

#[derive(Debug, Default, Clone)]
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
    pub migrations: Vec<MigrationPlan>,
    pub db_models: Vec<DbModelMeta>,
    pub schedules: Vec<SchedulePlan>,
    pub tests: Vec<TestSpec>,
    pub http_query_plans: Vec<HttpQueryPlan>,
    pub on_start_funcs: Vec<u32>,
    pub on_stop_funcs: Vec<u32>,
}

impl Module {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn populate_lifecycle_hooks(&mut self) {
        self.on_start_funcs.clear();
        self.on_stop_funcs.clear();
        for (fid, f) in self.functions.iter().enumerate() {
            if is_on_start_hook_name(&f.name) {
                self.on_start_funcs.push(fid as u32);
            } else if is_on_stop_hook_name(&f.name) {
                self.on_stop_funcs.push(fid as u32);
            }
        }
    }

    pub fn func_id(&self, name: &str) -> Option<u32> {
        self.functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    /// Validates register indices, constants, function IDs, jump targets, and return register banks.
    pub fn validate(&self) -> Result<(), String> {
        for (fid, func) in self.functions.iter().enumerate() {
            let Some(&last_op) = func.ops.last() else {
                return Err(format!("func {} ({}): ops cannot be empty", fid, func.name));
            };
            if !matches!(
                last_op,
                Op::ReturnScalar { .. }
                    | Op::ReturnAddInt { .. }
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

            let check_switch_table = |ip: usize, count: u16, ctx: &str| -> Result<(), String> {
                let end = ip + 2 + usize::from(count);
                let table = func.ops.get(ip + 1..end);
                if count == 0
                    || !table.is_some_and(|entries| entries.iter().all(|op| matches!(op, Op::Jump { .. })))
                {
                    return Err(format!("{}: invalid switch jump table", ctx));
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
                    Op::ModIntConst { dst, a, divisor, magic_idx, shift } => {
                        check_s(dst, &ctx)?;
                        check_s(a, &ctx)?;
                        let actual = self.int_consts.get(usize::from(magic_idx)).map(|&magic| (magic, shift));
                        if actual.is_none() || actual != crate::vm::opcode::remainder_magic(divisor) {
                            return Err(format!("{}: invalid constant remainder operands", ctx));
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
                    | Op::MulIntImm { dst, a: src, .. }
                    | Op::MulAddIntImm { dst, src, .. }
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
                    | Op::BitAndInt { dst, a, b }
                    | Op::BitOrInt { dst, a, b }
                    | Op::BitXorInt { dst, a, b }
                    | Op::ShiftLeftInt { dst, a, b }
                    | Op::ShiftRightInt { dst, a, b }
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
                    Op::MulAddInt { dst, a, b, c } | Op::FmaFloat { dst, a, b, c } => {
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
                    Op::JumpIfLtInt { a, b, target, .. } => {
                        check_s(a, &ctx)?;
                        check_s(b, &ctx)?;
                        check_jmp(target, &ctx)?;
                    }
                    Op::JumpIfLtIntImm { a, target, .. }
                    | Op::JumpIfEqIntImm { a, target, .. } => {
                        check_s(a, &ctx)?;
                        check_jmp(target, &ctx)?;
                    }
                    Op::ReturnScalarIfLtIntImm { a, src, .. } => {
                        check_s(a, &ctx)?;
                        check_s(src, &ctx)?;
                        if func.returns_obj {
                            return Err(format!("{}: conditional scalar return in object function", ctx));
                        }
                    }
                    Op::MakeScalarEnum { dst_s, a, b, len, .. } => {
                        if len > 2 || usize::from(dst_s) + usize::from(len) >= usize::from(func.num_scalars) {
                            return Err(format!("{}: invalid scalar enum payload registers", ctx));
                        }
                        if len >= 1 { check_s(a, &ctx)?; }
                        if len == 2 { check_s(b, &ctx)?; }
                    }
                    Op::BoxScalarEnum { dst_obj, src_obj, tag_s, width } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
                        if width > 2 || usize::from(tag_s) + usize::from(width) >= usize::from(func.num_scalars) {
                            return Err(format!("{}: invalid scalar enum payload registers", ctx));
                        }
                    }
                    Op::ReadScalarEnumPair { dst_a, dst_b, tag_s, src_obj, field_a, field_b } => {
                        check_s(dst_a, &ctx)?;
                        check_s(dst_b, &ctx)?;
                        check_o(src_obj, &ctx)?;
                        let field = field_a.max(field_b);
                        if field >= 2 || usize::from(tag_s) + 1 + usize::from(field) >= usize::from(func.num_scalars) {
                            return Err(format!("{}: invalid scalar enum field register", ctx));
                        }
                    }
                    Op::ReadScalarEnumField { dst_s, tag_s, src_obj, field_idx } => {
                        check_s(dst_s, &ctx)?;
                        check_o(src_obj, &ctx)?;
                        if field_idx >= 2 || usize::from(tag_s) + 1 + usize::from(field_idx) >= usize::from(func.num_scalars) {
                            return Err(format!("{}: invalid scalar enum field register", ctx));
                        }
                    }
                    Op::SwitchIntImm { src, count, .. } => {
                        check_s(src, &ctx)?;
                        check_switch_table(ip, count, &ctx)?;
                    }
                    Op::SwitchTag { src_obj, first_tag, tag_count }
                    | Op::SwitchScalarTag { src_obj, first_tag, tag_count, .. } => {
                        check_o(src_obj, &ctx)?;
                        if let Op::SwitchScalarTag { tag_s, .. } = op {
                            check_s(tag_s, &ctx)?;
                        }
                        check_switch_table(ip, tag_count, &ctx)?;
                        if first_tag.checked_add(u32::from(tag_count) - 1).is_none() {
                            return Err(format!("{}: invalid switch tag range", ctx));
                        }
                    }
                    Op::AppendIntRangeTail { list_obj, idx_s, limit_s } => {
                        check_o(list_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                        check_s(limit_s, &ctx)?;
                    }
                    Op::ForRangeStep { idx_reg, limit_reg, target } => {
                        check_s(idx_reg, &ctx)?;
                        check_s(limit_reg, &ctx)?;
                        check_jmp(target, &ctx)?;
                    }
                    Op::CallSelfIntImm { arg_s, arg_o, src, .. } => {
                        check_s(src, &ctx)?;
                        check_s(arg_s, &ctx)?;
                        if func.returns_obj || func.scalar_params != 1 || func.obj_params != 0
                            || arg_o > func.num_objs || arg_s == 0
                        {
                            return Err(format!("{}: invalid scalar self-call", ctx));
                        }
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
                    Op::ReturnScalar { .. } | Op::ReturnAddInt { .. } if func.returns_obj => {
                        return Err(format!(
                            "func {} ({}): {} returns a scalar but the function declares an object result",
                            fid, func.name, ctx
                        ));
                    }
                    Op::ReturnObj { .. }
                    | Op::TryUnwrapScalar { ret_on_err: true, .. }
                    | Op::TryCheckObj { ret_on_err: true, .. }
                        if !func.returns_obj =>
                    {
                        return Err(format!(
                            "func {} ({}): {} can return an object but the function declares no object result",
                            fid, func.name, ctx
                        ));
                    }
                    Op::ReturnScalar { src } => check_s(src, &ctx)?,
                    Op::ReturnAddInt { a, b } => {
                        check_s(a, &ctx)?;
                        check_s(b, &ctx)?;
                    }
                    Op::ReturnObj { src } => check_o(src, &ctx)?,
                    // Also used as an unreachable end-of-function terminator.
                    Op::ReturnVoid => {}
                    Op::LoadConstObj { dst_obj, idx } => {
                        check_o(dst_obj, &ctx)?;
                        if (idx as usize) >= self.obj_consts.len() {
                            return Err(format!("{}: obj_const {} out of bounds", ctx, idx));
                        }
                    }
                    Op::ToBytes { dst_obj, src_obj } | Op::BytesToList { dst_obj, src_obj } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(src_obj, &ctx)?;
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
                    Op::StrConcatInt { dst_obj, prefix_obj, src_s } => {
                        check_o(dst_obj, &ctx)?;
                        check_o(prefix_obj, &ctx)?;
                        check_s(src_s, &ctx)?;
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
                    Op::SumListIntTail { list_obj, idx_s, elem_s, acc_s } => {
                        check_o(list_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                        check_s(elem_s, &ctx)?;
                        check_s(acc_s, &ctx)?;
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
                    Op::NewHttpRequest { dst_obj, desc_idx } => {
                        check_o(dst_obj, &ctx)?;
                        let desc = self.records.get(desc_idx as usize)
                            .ok_or_else(|| format!("{}: missing HTTP request descriptor", ctx))?;
                        if desc.schema_id != crate::vm::value::SCHEMA_HTTP_REQUEST
                            || desc.obj_regs.len() != 6 || !desc.scalar_regs.is_empty()
                        {
                            return Err(format!("{}: invalid HTTP request descriptor", ctx));
                        }
                        for &reg in desc.obj_regs.iter() { check_o(reg, &ctx)?; }
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
                    Op::MapGetIntIntOrImm { dst_s, map_obj, key_s, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_o(map_obj, &ctx)?;
                        check_s(key_s, &ctx)?;
                    }
                    Op::AddIntMapGetOrImm { dst_s, a, map_obj, key_s, .. } => {
                        check_s(dst_s, &ctx)?;
                        check_s(a, &ctx)?;
                        check_o(map_obj, &ctx)?;
                        check_s(key_s, &ctx)?;
                    }
                    Op::SumIntMapRangeTail { acc_s, map_obj, idx_s, limit_s, .. } => {
                        check_s(acc_s, &ctx)?;
                        check_o(map_obj, &ctx)?;
                        check_s(idx_s, &ctx)?;
                        check_s(limit_s, &ctx)?;
                        if acc_s == idx_s || acc_s == limit_s {
                            return Err(format!("{}: map sum overlaps the range registers", ctx));
                        }
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
                                | PipelineStage::FilterObj(c)
                                | PipelineStage::ReduceScalar(c)
                                | PipelineStage::MapObjToScalar(c)
                                | PipelineStage::MapScalarToObj(c) => c,
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
                                | crate::native::time::TimeOpKind::Milliseconds
                                | crate::native::time::TimeOpKind::Sleep
                                | crate::native::time::TimeOpKind::FromUnix
                                | crate::native::time::TimeOpKind::FromUnixMillis,
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
                            Some(
                                crate::native::validation::ValOpKind::Email
                                | crate::native::validation::ValOpKind::NotEmpty
                                | crate::native::validation::ValOpKind::Url
                                | crate::native::validation::ValOpKind::Uuid,
                            ) => {
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
                                crate::native::validation::ValOpKind::RangeInt
                                | crate::native::validation::ValOpKind::RangeFloat,
                            ) => {
                                check_s(a_reg, &ctx)?;
                                check_o(b_reg, &ctx)?;
                            }
                            Some(
                                crate::native::validation::ValOpKind::MinDyn
                                | crate::native::validation::ValOpKind::MaxDyn
                                | crate::native::validation::ValOpKind::OneOf,
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
                                    3 | 6 => {
                                        check_o(a, &ctx)?;
                                        check_o(b, &ctx)?;
                                    }
                                    4 | 5 => check_s(a, &ctx)?,
                                    _ => return Err(format!("{}: invalid process sub-op {}", ctx, aux)),
                                }
                            }
                            crate::vm::opcode::EXT_CRYPTO_OP => match aux {
                                0 | 2..=5 | 9 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                1 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                6 => check_o(a, &ctx)?,
                                7 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                }
                                8 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                10 | 11 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                _ => return Err(format!("{}: invalid crypto sub-op {}", ctx, aux)),
                            },
                            crate::vm::opcode::EXT_PIPELINE_REDUCE => {
                                check_s(a, &ctx)?;
                                check_o(b, &ctx)?;
                                let plan = self.pipelines.get(c as usize).ok_or_else(|| {
                                    format!("{}: pipeline plan {} out of bounds", ctx, c)
                                })?;
                                for stage in plan.stages.iter() {
                                    let cref = match stage {
                                        PipelineStage::MapScalarToScalar(cr)
                                        | PipelineStage::FilterScalar(cr)
                                        | PipelineStage::MapObjToObj(cr)
                                        | PipelineStage::FilterObj(cr)
                                        | PipelineStage::ReduceScalar(cr)
                                        | PipelineStage::MapObjToScalar(cr)
                                        | PipelineStage::MapScalarToObj(cr) => cr,
                                    };
                                    match *cref {
                                        CallableRef::DirectFunc(id) => {
                                            if (id as usize) >= self.functions.len() {
                                                return Err(format!(
                                                    "{}: pipeline func {} out of bounds",
                                                    ctx, id
                                                ));
                                            }
                                        }
                                        CallableRef::RegObj(r) => check_o(r, &ctx)?,
                                    }
                                }
                            }
                            crate::vm::opcode::EXT_URL_OP => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                            }
                            crate::vm::opcode::EXT_CSV_OP => {
                                check_o(a, &ctx)?;
                                check_o(b, &ctx)?;
                                if aux == 1 || aux == 4 {
                                    check_o(c, &ctx)?;
                                }
                            }
                            crate::vm::opcode::EXT_MATH_OP => {
                                check_s(a, &ctx)?;
                                if aux >= 3 {
                                    check_s(b, &ctx)?;
                                }
                                if aux >= 17 {
                                    check_s(c, &ctx)?;
                                }
                            }
                            crate::vm::opcode::EXT_LOG_OP => match aux {
                                0..=4 => check_o(b, &ctx)?,
                                5 => check_s(b, &ctx)?,
                                6 => check_o(a, &ctx)?,
                                _ => return Err(format!("Invalid log aux {}", aux)),
                            },
                            crate::vm::opcode::EXT_LIST_AGG => {
                                check_o(b, &ctx)?;
                                match aux {
                                    0 => check_s(a, &ctx)?,
                                    1 | 2 => check_o(a, &ctx)?,
                                    _ => return Err(format!("Invalid list agg aux {}", aux)),
                                }
                            }
                            crate::vm::opcode::EXT_HTTP_OP => match aux {
                                0..=2 | 11 | 13 | 19..=23 | 50 | 51 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                3 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                14 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                4 => check_o(a, &ctx)?,
                                5..=10 | 12 | 15..=18 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                24 => {
                                    check_o(a, &ctx)?;
                                    if (c as usize) >= self.json_plans.len() {
                                        return Err(format!("http body json_plan {} out of bounds", c));
                                    }
                                }
                                25..=27 | 33..=35 | 37 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                28 | 29 | 36 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                30 | 31 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                32 => {
                                    check_o(a, &ctx)?;
                                }
                                38 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                39 => {
                                    check_o(a, &ctx)?;
                                    if (c as usize) >= self.http_query_plans.len() {
                                        return Err(format!("HTTP query plan {} out of bounds", c));
                                    }
                                }
                                40 => {
                                    check_o(a, &ctx)?;
                                }
                                41 | 48 | 49 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                42 | 44..=47 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                43 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                _ => return Err(format!("Invalid http aux {}", aux)),
                            },
                            crate::vm::opcode::EXT_DB_OP => match aux {
                                0 | 5 | 6 | 9..=12 | 14 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                1 => check_o(a, &ctx)?,
                                2..=4 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                }
                                7 | 8 | 15 | 26 | 28..=33 | 36 | 37 | 40 | 43 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                38 | 39 | 41 | 42 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                34 | 35 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    if c > 5 {
                                        return Err(format!("Invalid db query mode {}", c));
                                    }
                                }
                                13 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                16 | 17 | 19 | 24 | 27 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    if (c as usize) >= self.db_models.len() {
                                        return Err(format!("db model_idx {} out of bounds", c));
                                    }
                                }
                                18 | 20 | 21 | 23 | 25 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                22 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    if (c as usize) >= self.db_models.len() {
                                        return Err(format!("db model_idx {} out of bounds", c));
                                    }
                                }
                                _ => return Err(format!("Invalid db aux {}", aux)),
                            },
                            crate::vm::opcode::EXT_CRON_OP => match aux {
                                0 | 1 => {
                                    check_o(a, &ctx)?;
                                    if (b as usize) >= self.schedules.len() {
                                        return Err(format!("cron schedule_idx {} out of bounds", b));
                                    }
                                }
                                2 => {
                                    check_o(a, &ctx)?;
                                    if (b as usize) >= self.schedules.len() {
                                        return Err(format!("cron schedule_idx {} out of bounds", b));
                                    }
                                    check_o(c, &ctx)?;
                                }
                                3 | 5 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                4 => {
                                    check_o(b, &ctx)?;
                                }
                                _ => return Err(format!("Invalid cron aux {}", aux)),
                            },
                            EXT_ASSERT => {
                                let op_kind = aux >> 3;
                                let val_tag = aux & 0x07;
                                match op_kind {
                                    0 => check_s(a, &ctx)?,
                                    1 => {
                                        check_s(a, &ctx)?;
                                        check_o(b, &ctx)?;
                                    }
                                    2 => check_o(a, &ctx)?,
                                    3 | 5 => {
                                        check_tag(val_tag, a)?;
                                        check_tag(val_tag, b)?;
                                    }
                                    4 | 6 => {
                                        check_tag(val_tag, a)?;
                                        check_tag(val_tag, b)?;
                                        check_o(c, &ctx)?;
                                    }
                                    _ => return Err(format!("Invalid assert aux {}", aux)),
                                }
                            }
                            EXT_TIME_OP => match aux {
                                0..=2 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                3..=9 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                _ => return Err(format!("Invalid time aux {}", aux)),
                            },
                            EXT_JWT_OP => match aux {
                                0 | 1 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                2 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    if (c as usize) >= self.json_plans.len() {
                                        return Err(format!("jwt json_plan {} out of bounds", c));
                                    }
                                }
                                _ => return Err(format!("Invalid jwt aux {}", aux)),
                            },
                            EXT_LOCK_OP => match aux {
                                0..=7 | 9 | 10 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                8 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                11 => check_o(b, &ctx)?,
                                12 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                _ => return Err(format!("Invalid lock aux {}", aux)),
                            },
                            EXT_CACHE_OP => match aux {
                                0 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                1 | 14 | 18 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                2 | 12 | 15 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                3..=9 | 11 | 13 | 16 | 17 | 19 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                10 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                _ => return Err(format!("Invalid cache aux {}", aux)),
                            },
                            EXT_REDIS_OP => match aux {
                                0 | 1 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                3 => check_o(b, &ctx)?,
                                2 | 4..=34 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                _ => return Err(format!("Invalid redis aux {}", aux)),
                            },
                            crate::vm::opcode::EXT_SPAWN => {
                                check_o(a, &ctx)?;
                            }
                            EXT_CHAN_OP => match aux {
                                0 => {
                                    check_o(a, &ctx)?;
                                    check_s(b, &ctx)?;
                                }
                                1 | 2 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_o(c, &ctx)?;
                                }
                                3 | 4 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                5 => {
                                    check_o(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                    check_s(c, &ctx)?;
                                }
                                6 => {
                                    check_o(b, &ctx)?;
                                }
                                7..=10 => {
                                    check_s(a, &ctx)?;
                                    check_o(b, &ctx)?;
                                }
                                _ => return Err(format!("Invalid chan aux {}", aux)),
                            },
                            _ => return Err(format!("Invalid ExtOp kind {}", kind)),
                        }
                    }
                }
            }
        }
        for mig in &self.migrations {
            if (mig.up_func_id as usize) >= self.functions.len()
                || (mig.down_func_id as usize) >= self.functions.len()
            {
                return Err(format!("Migration {} '{}' func_id out of bounds", mig.version, mig.name));
            }
        }
        for sched in &self.schedules {
            for job in &sched.jobs {
                if (job.func_id as usize) >= self.functions.len() {
                    return Err(format!(
                        "Schedule '{}' job '{}' func_id out of bounds",
                        sched.name, job.name
                    ));
                }
            }
        }
        for test in &self.tests {
            if (test.func_id as usize) >= self.functions.len() {
                return Err(format!("Test '{}' func_id out of bounds", test.name));
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

#[cfg(test)]
mod tests {
    use super::{FunctionProto, Module, Op};

    #[test]
    fn validates_fused_branch_registers_and_switch_entries() {
        let mut module = Module::new();
        module.functions.push(FunctionProto {
            name: "branch".into(), num_scalars: 1, num_objs: 1,
            scalar_params: 0, obj_params: 0, returns_obj: false,
            ops: Box::new([
                Op::JumpIfLtIntImm { a: 0, imm: 2, target: 3, when_true: true },
                Op::SwitchTag { src_obj: 0, first_tag: 5, tag_count: 1 },
                Op::Jump { target: 4 },
                Op::Jump { target: 4 },
                Op::ReturnVoid,
            ]),
        });
        assert!(module.validate().is_ok());
        module.functions[0].ops[3] = Op::Jump { target: 5 };
        assert!(module.validate().unwrap_err().contains("jump target"));
        module.functions[0].ops[3] = Op::ReturnVoid;
        assert!(module.validate().unwrap_err().contains("switch jump table"));
        module.functions[0].ops[0] = Op::JumpIfLtInt { a: 0, b: 1, target: 4, when_true: false };
        assert!(module.validate().unwrap_err().contains("scalar reg"));
    }

    #[test]
    fn validates_register_bank_for_explicit_and_early_returns() {
        for (op, required_returns_obj) in [
            (Op::ReturnScalar { src: 0 }, Some(false)),
            (Op::ReturnObj { src: 0 }, Some(true)),
            (
                Op::TryUnwrapScalar { dst_s: 0, src_obj: 0, ret_on_err: true },
                Some(true),
            ),
            (
                Op::TryCheckObj { src_obj: 0, ret_on_err: true, empty_is_err: false },
                Some(true),
            ),
            (
                Op::TryUnwrapScalar { dst_s: 0, src_obj: 0, ret_on_err: false },
                None,
            ),
            (
                Op::TryCheckObj { src_obj: 0, ret_on_err: false, empty_is_err: false },
                None,
            ),
            (Op::ReturnVoid, None),
        ] {
            let mut module = Module::new();
            module.functions.push(FunctionProto {
                name: "test_return".into(),
                num_scalars: 1,
                num_objs: 1,
                scalar_params: 0,
                obj_params: 0,
                returns_obj: false,
                ops: Box::new([op, Op::ReturnVoid]),
            });

            for returns_obj in [false, true] {
                module.functions[0].returns_obj = returns_obj;
                let result = module.validate();
                if required_returns_obj.is_some_and(|required| required != returns_obj) {
                    let err = result.expect_err("incompatible return bank should be rejected");
                    assert!(err.contains("test_return): op@0"), "{op:?}: {err}");
                } else {
                    assert_eq!(result, Ok(()), "{op:?} with returns_obj={returns_obj}");
                }
            }
        }
    }
}
