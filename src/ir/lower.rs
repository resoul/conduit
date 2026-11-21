use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::types::Type;
use crate::ir::lower_loops::patch_jump;
use crate::ir::{ClosureProto, FunctionProto, Module, PipelinePlan, RecordDesc};
use crate::native::{JsonDecodePlan, JsonType};
use crate::vm::opcode::Op;
use crate::vm::value::Obj;

/// Compile a type-checked AST program into an IR `Module`.
pub fn compile_program(program: &Program, checked: &CheckedProgram) -> Result<Module, String> {
    let mut mc = ModuleCompiler::new(checked);
    mc.compile_all(program)?;
    inline_scalar_leaves(&mut mc.module.functions);
    Ok(mc.module)
}

// A single result-producing instruction can replace its direct call in place.
// Restrict inputs to parameters so a caller never depends on a callee's register file.
fn inline_scalar_leaves(functions: &mut [FunctionProto]) {
    let leaves: Vec<_> = functions.iter().map(|func| {
        if func.returns_obj || func.num_objs != 0 {
            return None;
        }
        let params = func.scalar_params;
        let leaf = match func.ops.as_ref() {
            [Op::ReturnScalar { src }] if *src < params => Op::MoveScalar { dst: 0, src: *src },
            [op, Op::ReturnScalar { src }] => {
                rebase_leaf(*op, params, *src, 0, 0)?
            }
            _ => return None,
        };
        Some((leaf, params))
    }).collect();

    for function in functions {
        for instruction in &mut function.ops {
            if let Op::CallDirect { func_id, arg_base_s, ret_reg, .. } = *instruction
                && let Some((leaf, params)) = leaves.get(func_id as usize).copied().flatten()
                && let Some(inlined) = rebase_leaf(leaf, params, 0, arg_base_s, ret_reg)
            {
                *instruction = inlined;
            }
        }
    }
}

fn rebase_leaf(op: Op, params: u16, return_src: u16, base: u16, dst: u16) -> Option<Op> {
    macro_rules! unary {
        ($name:ident, $from:ident $(, $other:ident)?) => {
            if let Op::$name { dst: result, $from, $($other,)? } = op
                && result == return_src && $from < params
            {
                return Some(Op::$name { dst, $from: base.checked_add($from)?, $($other,)? });
            }
        };
    }
    macro_rules! binary {
        ($name:ident) => {
            if let Op::$name { dst: result, a, b } = op
                && result == return_src && a < params && b < params
            {
                return Some(Op::$name { dst, a: base.checked_add(a)?, b: base.checked_add(b)? });
            }
        };
    }
    unary!(MoveScalar, src);
    unary!(NegInt, src);
    unary!(NotBool, src);
    unary!(AddIntImm, a, imm);
    unary!(MulIntImm, a, imm);
    unary!(ModIntImm, a, imm);
    unary!(LtIntImm, a, imm);
    unary!(EqIntImm, a, imm);
    binary!(AddInt);
    binary!(SubInt);
    binary!(MulInt);
    binary!(DivInt);
    binary!(ModInt);
    binary!(AddFloat);
    binary!(SubFloat);
    binary!(MulFloat);
    binary!(DivFloat);

    match op {
        Op::MulAddInt { dst: result, a, b, c } if result == return_src && a < params && b < params && c < params => {
            Some(Op::MulAddInt { dst, a: base.checked_add(a)?, b: base.checked_add(b)?, c: base.checked_add(c)? })
        }
        Op::ModIntConst { dst: result, a, divisor, magic_idx, shift } if result == return_src && a < params => {
            Some(Op::ModIntConst { dst, a: base.checked_add(a)?, divisor, magic_idx, shift })
        }
        Op::MulAddIntImm { dst: result, src, mul, add } if result == return_src && src < params => {
            Some(Op::MulAddIntImm { dst, src: base.checked_add(src)?, mul, add })
        }
        Op::LoadImmInt { dst: result, imm } if result == return_src => Some(Op::LoadImmInt { dst, imm }),
        Op::LoadConstInt { dst: result, idx } if result == return_src => Some(Op::LoadConstInt { dst, idx }),
        Op::LoadConstFloat { dst: result, idx } if result == return_src => Some(Op::LoadConstFloat { dst, idx }),
        Op::LoadBool { dst: result, val } if result == return_src => Some(Op::LoadBool { dst, val }),
        _ => None,
    }
}

fn jump_target(op: Op) -> Option<u32> {
    match op {
        Op::Jump { target }
        | Op::JumpIfFalse { target, .. }
        | Op::JumpIfTrue { target, .. }
        | Op::JumpIfLtInt { target, .. }
        | Op::JumpIfLtIntImm { target, .. }
        | Op::JumpIfEqIntImm { target, .. }
        | Op::ForRangeStep { target, .. }
        | Op::ForListInt { exit_target: target, .. }
        | Op::ForListIntStep { target, .. }
        | Op::ForListObj { exit_target: target, .. } => Some(target),
        _ => None,
    }
}

fn op_reads_scalar(op: Op, reg: u16, records: &[RecordDesc]) -> bool {
    match op {
        Op::MoveScalar { src, .. }
        | Op::NegInt { src, .. }
        | Op::IntToFloat { src, .. }
        | Op::FloatFloor { src, .. }
        | Op::FloatRound { src, .. }
        | Op::NotBool { src, .. }
        | Op::AddIntImm { a: src, .. }
        | Op::MulIntImm { a: src, .. }
        | Op::MulAddIntImm { src, .. }
        | Op::ModIntImm { a: src, .. }
        | Op::ModIntConst { a: src, .. }
        | Op::EqIntImm { a: src, .. }
        | Op::LtIntImm { a: src, .. }
        | Op::JumpIfFalse { cond: src, .. }
        | Op::JumpIfTrue { cond: src, .. }
        | Op::JumpIfEqIntImm { a: src, .. }
        | Op::JumpIfLtIntImm { a: src, .. }
        | Op::SwitchIntImm { src, .. }
        | Op::ListAppendInt { val_s: src, .. }
        | Op::ListAppendFloat { val_s: src, .. }
        | Op::StrConcatInt { src_s: src, .. }
        | Op::ScalarToStr { src_s: src, .. }
        | Op::BoxScalar { src_s: src, .. }
        | Op::ListContainsScalar { val_s: src, .. }
        | Op::ReturnScalar { src }
        | Op::CallSelfIntImm { src, .. } => src == reg,
        Op::AddInt { a, b, .. }
        | Op::SubInt { a, b, .. }
        | Op::MulInt { a, b, .. }
        | Op::DivInt { a, b, .. }
        | Op::ModInt { a, b, .. }
        | Op::BitAndInt { a, b, .. }
        | Op::BitOrInt { a, b, .. }
        | Op::BitXorInt { a, b, .. }
        | Op::ShiftLeftInt { a, b, .. }
        | Op::ShiftRightInt { a, b, .. }
        | Op::AddFloat { a, b, .. }
        | Op::SubFloat { a, b, .. }
        | Op::MulFloat { a, b, .. }
        | Op::DivFloat { a, b, .. }
        | Op::EqInt { a, b, .. }
        | Op::NeInt { a, b, .. }
        | Op::LtInt { a, b, .. }
        | Op::LeInt { a, b, .. }
        | Op::GtInt { a, b, .. }
        | Op::GeInt { a, b, .. }
        | Op::EqFloat { a, b, .. }
        | Op::LtFloat { a, b, .. }
        | Op::LeFloat { a, b, .. }
        | Op::JumpIfLtInt { a, b, .. }
        | Op::ReturnAddInt { a, b }
        | Op::ReturnScalarIfLtIntImm { a, src: b, .. }
        | Op::MakeRangeList { start_s: a, end_s: b, .. }
        | Op::ListSlice { start_s: a, end_s: b, .. }
        | Op::ForRangeStep { idx_reg: a, limit_reg: b, .. }
        | Op::MapSetIntInt { key_s: a, val_s: b, .. }
        | Op::MapGetIntIntOr { key_s: a, def_s: b, .. }
        | Op::AppendIntRangeTail { idx_s: a, limit_s: b, .. } => a == reg || b == reg,
        Op::MulAddInt { a, b, c, .. }
        | Op::FmaFloat { a, b, c, .. }
        | Op::ClampScalar { val_s: a, lo_s: b, hi_s: c, .. } => a == reg || b == reg || c == reg,
        Op::ListIndex { idx_s: a, .. }
        | Op::ListGet { idx_s: a, .. }
        | Op::MapGetIntIntOrImm { key_s: a, .. }
        | Op::SumListIntTail { idx_s: a, .. } => a == reg,
        Op::AddIntMapGetOrImm { a, key_s: b, .. } => a == reg || b == reg,
        Op::SumIntMapRangeTail { idx_s, limit_s, acc_s, .. } => {
            idx_s == reg || limit_s == reg || acc_s == reg
        }
        Op::SwitchScalarTag { tag_s, .. }
        | Op::BoxScalarEnum { tag_s, .. }
        | Op::ReadScalarEnumField { tag_s, .. }
        | Op::ReadScalarEnumPair { tag_s, .. } => tag_s == reg,
        Op::MakeScalarEnum { a, b, len, .. } => {
            (len >= 1 && a == reg) || (len >= 2 && b == reg)
        }
        Op::CallDirect { arg_base_s, .. } | Op::CallCallable { arg_base_s, .. } => reg >= arg_base_s,
        Op::NewRecord { desc_idx, .. } | Op::NewRecordMove { desc_idx, .. } => records
            .get(desc_idx as usize)
            .is_some_and(|desc| desc.scalar_regs.contains(&reg)),
        Op::ExtOp { a, b, c, .. } => a == reg || b == reg || c == reg,
        _ => false,
    }
}

fn tag_used_only_for_single_switch(ops: &[Op], tag_s: u16, records: &[RecordDesc]) -> bool {
    let mut switch_count = 0usize;
    for &op in ops {
        match op {
            Op::SwitchScalarTag { tag_s: t, .. } if t == tag_s => {
                switch_count += 1;
                if switch_count > 1 {
                    return false;
                }
            }
            Op::ReadScalarEnumPair { tag_s: t, dst_a, dst_b, .. }
                if t == tag_s && dst_a != tag_s && dst_b != tag_s => {}
            Op::BoxScalarEnum { tag_s: t, .. } | Op::ReadScalarEnumField { tag_s: t, .. }
                if t == tag_s =>
            {
                return false;
            }
            other if op_reads_scalar(other, tag_s, records) => return false,
            _ => {}
        }
    }
    switch_count == 1
}

// A jump immediately following a tag write already knows its switch arm,
// provided no branch can enter at the jump and bypass that write.
fn thread_scalar_enum_jumps(ops: &mut [Op], records: &[RecordDesc]) {
    if !ops.iter().any(|op| matches!(op, Op::SwitchScalarTag { .. })) {
        return;
    }
    let mut targets = vec![false; ops.len()];
    for &op in ops.iter() {
        if let Some(target) = jump_target(op)
            && let Some(entry) = targets.get_mut(target as usize)
        {
            *entry = true;
        }
    }
    for pc in 1..ops.len() {
        if targets[pc] {
            continue;
        }
        let Op::Jump { target } = ops[pc] else { continue };
        let switch = target as usize;
        let Some(Op::SwitchScalarTag { tag_s, first_tag, tag_count, .. }) = ops.get(switch).copied() else {
            continue;
        };
        let tag_write_pc = if pc >= 2
            && !targets[pc - 1]
            && matches!(ops[pc - 1], Op::SwitchIntImm { .. })
        {
            pc - 2
        } else {
            pc - 1
        };
        let tag = match ops[tag_write_pc] {
            Op::MakeScalarEnum { dst_s, schema_id, .. } if dst_s == tag_s => schema_id,
            Op::LoadImmInt { dst, imm } if dst == tag_s && imm >= 0 => imm as u32,
            _ => continue,
        };
        let slot = tag.wrapping_sub(first_tag);
        let offset = if slot < u32::from(tag_count) { slot as usize + 1 } else { 0 };
        if let Some(Op::Jump { target }) = ops.get(switch + 1 + offset).copied() {
            ops[pc] = Op::Jump { target };
            // A new edge can enter a later candidate, so include it in the guard.
            if let Some(entry) = targets.get_mut(target as usize) {
                *entry = true;
            }
        }
    }

    for pc in 2..ops.len() {
        if targets[pc] || targets[pc - 1] {
            continue;
        }
        let Op::Jump { target } = ops[pc] else { continue };
        let arm_pc = target as usize;
        let Op::MakeScalarEnum { dst_s, a, b, len: 2, .. } = ops[pc - 1] else { continue };
        let Op::LoadImmInt { dst: tmp, imm } = ops[pc - 2] else { continue };
        if tmp != b || tmp == a || tmp == dst_s {
            continue;
        }
        if !tag_used_only_for_single_switch(ops, dst_s, records) {
            continue;
        }
        let Some(Op::ReadScalarEnumPair {
            dst_a,
            dst_b,
            tag_s,
            field_a: 0,
            field_b: 1,
            ..
        }) = ops.get(arm_pc).copied()
        else {
            continue;
        };
        if tag_s != dst_s || dst_b == a || dst_a == dst_b || arm_pc + 1 >= ops.len() {
            continue;
        }
        ops[pc - 2] = Op::LoadImmInt { dst: dst_b, imm };
        ops[pc - 1] = Op::MoveScalar { dst: dst_a, src: a };
        ops[pc] = Op::Jump { target: (arm_pc + 1) as u32 };
        targets[arm_pc + 1] = true;
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VarSlot {
    pub is_scalar: bool,
    pub reg: u16,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompactEnum {
    pub tag_s: u16,
    pub width: u8,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopPrealloc {
    pub idx_s: u16,
    pub aux_s: u16,
    pub elem_reg: u16,
    pub list_o: u16,
}

pub(crate) struct LoopPatch {
    pub breaks: Vec<usize>,
    pub continues: Vec<usize>,
}

pub(crate) struct ModuleCompiler<'a> {
    pub checked: &'a CheckedProgram,
    pub module: Module,
    int_consts: HashMap<i64, u16>,
    float_consts: HashMap<u64, u16>,
    str_consts: HashMap<String, u16>,
    empty_const: Option<u16>,
    unit_record_consts: HashMap<u32, u16>,
    desc_cache: HashMap<(u32, Vec<u16>, Vec<u16>), u32>,
    closure_counter: usize,
    pub(crate) email_fn_id: Option<u32>,
}

impl<'a> ModuleCompiler<'a> {
    fn new(checked: &'a CheckedProgram) -> Self {
        let mut module = Module::new();
        module.struct_schemas = checked.struct_schemas.clone();
        module.validation_plans = checked.validation_plans.clone();
        module.env_plans = checked.env_plans.clone();
        module.cli_plans = checked.cli_plans.iter().map(|(_, p)| p.clone()).collect();
        // Reserve slots 0..N for top-level functions so forward calls keep stable indices.
        module.functions = checked
            .funcs
            .iter()
            .map(|sig| FunctionProto {
                name: sig.name.clone(),
                num_scalars: 0,
                num_objs: 0,
                scalar_params: sig.params.iter().filter(|t| t.is_scalar()).count() as u16,
                obj_params: sig.params.iter().filter(|t| t.is_obj()).count() as u16,
                returns_obj: sig.return_type.is_obj(),
                ops: Box::new([]),
            })
            .collect();

        Self {
            checked,
            module,
            int_consts: HashMap::new(),
            float_consts: HashMap::new(),
            str_consts: HashMap::new(),
            empty_const: None,
            unit_record_consts: HashMap::new(),
            desc_cache: HashMap::new(),
            closure_counter: 0,
            email_fn_id: None,
        }
    }

    fn compile_all(&mut self, program: &Program) -> Result<(), String> {
        let mut fn_idx = 0usize;
        for item in &program.items {
            if let ItemKind::Func {
                name, params, body, ..
            } = &item.kind
            {
                let sig = self.checked.funcs[fn_idx].clone();
                let proto = self.lower_function(
                    name,
                    Some(fn_idx as u32),
                    params,
                    &sig.params,
                    &sig.return_type,
                    body,
                    &[],
                    &[],
                )?;
                self.module.functions[fn_idx] = proto;
                fn_idx += 1;
            }
        }

        let db_conn_ty =
            Type::Custom(crate::frontend::types::TypeId::native("cdt.db", "Connection"));
        let db_err_ty =
            Type::Custom(crate::frontend::types::TypeId::native("cdt.db", "DbError"));
        let mig_ret_ty = Type::union(vec![Type::Void, db_err_ty]);
        let db_param = Param {
            name: "db".to_string(),
            ty: TypeExpr::Named("Connection".to_string()),
            default: None,
            span: crate::frontend::lexer::Span::new(0, 0),
        };
        for item in &program.items {
            if let ItemKind::Migration {
                version,
                name,
                up,
                down,
            } = &item.kind
            {
                let up_proto = self.lower_function(
                    &format!("__cdt_mig_{}_up", version),
                    None,
                    std::slice::from_ref(&db_param),
                    std::slice::from_ref(&db_conn_ty),
                    &mig_ret_ty,
                    up,
                    &[],
                    &[],
                )?;
                let up_func_id = self.module.functions.len() as u32;
                self.module.functions.push(up_proto);

                let down_proto = self.lower_function(
                    &format!("__cdt_mig_{}_down", version),
                    None,
                    std::slice::from_ref(&db_param),
                    std::slice::from_ref(&db_conn_ty),
                    &mig_ret_ty,
                    down,
                    &[],
                    &[],
                )?;
                let down_func_id = self.module.functions.len() as u32;
                self.module.functions.push(down_proto);

                self.module.migrations.push(crate::ir::MigrationPlan {
                    version: *version,
                    name: name.clone(),
                    up_func_id,
                    down_func_id,
                });
            }
        }
        self.module.migrations.sort_by_key(|m| m.version);
        let mut test_idx = 0usize;
        for item in &program.items {
            if let ItemKind::Test { name, body } = &item.kind {
                let test_proto = self.lower_function(
                    &format!("__cdt_test_{}", test_idx),
                    None,
                    &[],
                    &[],
                    &Type::Void,
                    body,
                    &[],
                    &[],
                )?;
                let func_id = self.module.functions.len() as u32;
                self.module.functions.push(test_proto);
                self.module.tests.push(crate::ir::TestSpec {
                    name: name.clone(),
                    func_id,
                    line: item.span.line,
                });
                test_idx += 1;
            }
        }
        self.module.db_models = self.checked.db_model_plans.clone();
        for (sched_name, entries) in &self.checked.cron_schedule_plans {
            let mut jobs = Vec::with_capacity(entries.len());
            for entry in entries {
                let func_id = self.checked.func_by_name[&entry.handler_name];
                jobs.push(crate::ir::CronJobSpec {
                    name: entry.name.clone(),
                    cron_expr: entry.cron_expr.clone(),
                    interval_ms: entry.interval_ms,
                    func_id,
                });
            }
            self.module.schedules.push(crate::ir::SchedulePlan {
                name: sched_name.clone(),
                jobs,
            });
        }
        self.module.populate_lifecycle_hooks();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_function(
        &mut self,
        name: &str,
        function_id: Option<u32>,
        params: &[Param],
        param_tys: &[Type],
        return_ty: &Type,
        body: &Block,
        scalar_upvalues: &[(String, Type)],
        obj_upvalues: &[(String, Type)],
    ) -> Result<FunctionProto, String> {
        let mut lowerer = FnLowerer::new(
            self,
            function_id,
            params,
            param_tys,
            return_ty.clone(),
            scalar_upvalues,
            obj_upvalues,
        );
        lowerer.prealloc_block(body);
        lowerer.finalize_prealloc();
        lowerer.emit_upvalue_prologue(scalar_upvalues, obj_upvalues);
        lowerer.flush_hoisted_constants_if_outer()?;
        lowerer.emit_defer_prologue()?;
        lowerer.lower_block_stmts(&body.stmts)?;
        lowerer.emit_defer_epilogue()?;
        lowerer.ensure_trailing_return(return_ty);
        let num_scalars = lowerer.max_s.max(lowerer.scalar_params_count).max(1);
        let num_objs = lowerer.max_o.max(lowerer.obj_params_count);
        let scalar_params = lowerer.scalar_params_count;
        let obj_params = lowerer.obj_params_count;
        let mut ops = lowerer.ops.into_boxed_slice();
        thread_scalar_enum_jumps(&mut ops, &self.module.records);

        Ok(FunctionProto {
            name: name.to_string(),
            num_scalars,
            num_objs,
            scalar_params,
            obj_params,
            returns_obj: return_ty.is_obj(),
            ops,
        })
    }

    pub(crate) fn compile_closure(
        &mut self,
        params: &[Param],
        param_tys: &[Type],
        return_ty: &Type,
        body: &Block,
        cap_scalars: &[(String, Type, u16)],
        cap_objs: &[(String, Type, u16)],
    ) -> Result<u16, String> {
        let cid = self.closure_counter;
        self.closure_counter += 1;
        let name = format!("__closure_{}", cid);
        let s_upv: Vec<(String, Type)> = cap_scalars.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect();
        let o_upv: Vec<(String, Type)> = cap_objs.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect();
        let proto = self.lower_function(&name, None, params, param_tys, return_ty, body, &s_upv, &o_upv)?;
        let fid = self.module.functions.len() as u32;
        self.module.functions.push(proto);

        let proto_idx = self.module.closures.len();
        self.module.closures.push(ClosureProto {
            func_id: fid,
            capture_scalars: cap_scalars.iter().map(|(_, _, r)| *r).collect(),
            capture_objs: cap_objs.iter().map(|(_, _, r)| *r).collect(),
        });
        u16::try_from(proto_idx).map_err(|_| "Too many closures in module".to_string())
    }

    pub(crate) fn intern_int(&mut self, val: i64) -> Result<u16, String> {
        if let Some(&idx) = self.int_consts.get(&val) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.module.int_consts.len())
            .map_err(|_| "Too many integer constants".to_string())?;
        self.module.int_consts.push(val);
        self.int_consts.insert(val, idx);
        Ok(idx)
    }

    pub(crate) fn intern_float(&mut self, bits: u64) -> Result<u16, String> {
        if let Some(&idx) = self.float_consts.get(&bits) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.module.float_consts.len())
            .map_err(|_| "Too many float constants".to_string())?;
        self.module.float_consts.push(f64::from_bits(bits));
        self.float_consts.insert(bits, idx);
        Ok(idx)
    }

    pub(crate) fn intern_empty(&mut self) -> Result<u16, String> {
        if let Some(idx) = self.empty_const {
            return Ok(idx);
        }
        let idx = u16::try_from(self.module.obj_consts.len())
            .map_err(|_| "Too many object constants".to_string())?;
        self.module.obj_consts.push(Obj::Empty);
        self.empty_const = Some(idx);
        Ok(idx)
    }

    pub(crate) fn intern_unit_record(&mut self, schema_id: u32) -> Result<u16, String> {
        if let Some(&idx) = self.unit_record_consts.get(&schema_id) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.module.obj_consts.len())
            .map_err(|_| "Too many object constants".to_string())?;
        let record = crate::vm::value::StructData::new(schema_id, 0, 0);
        self.module.obj_consts.push(Obj::Record(std::sync::Arc::new(record)));
        self.unit_record_consts.insert(schema_id, idx);
        Ok(idx)
    }

    pub(crate) fn intern_str(&mut self, s: &str) -> Result<u16, String> {
        if let Some(&idx) = self.str_consts.get(s) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.module.obj_consts.len())
            .map_err(|_| "Too many object constants".to_string())?;
        self.module.obj_consts.push(Obj::str(s));
        self.str_consts.insert(s.to_string(), idx);
        Ok(idx)
    }

    pub(crate) fn intern_record_desc(&mut self, desc: RecordDesc) -> Result<u16, String> {
        let key = (desc.schema_id, desc.scalar_regs.to_vec(), desc.obj_regs.to_vec());
        if let Some(&idx) = self.desc_cache.get(&key) {
            return Ok(idx as u16);
        }
        let idx = u16::try_from(self.module.records.len())
            .map_err(|_| "Too many record descriptors".to_string())?;
        self.module.records.push(desc);
        self.desc_cache.insert(key, idx as u32);
        Ok(idx)
    }

    pub(crate) fn add_pipeline_plan(&mut self, plan: PipelinePlan) -> Result<u16, String> {
        let idx = u16::try_from(self.module.pipelines.len())
            .map_err(|_| "Too many pipeline plans".to_string())?;
        self.module.pipelines.push(plan);
        Ok(idx)
    }

    pub(crate) fn add_json_decode_plan(&mut self, root_ty: &JsonType) -> Result<u16, String> {
        let plan = JsonDecodePlan::compile(root_ty, &self.module.struct_schemas);
        let idx = u16::try_from(self.module.json_plans.len())
            .map_err(|_| "Too many JSON decode plans".to_string())?;
        self.module.json_plans.push(plan);
        Ok(idx)
    }

    pub(crate) fn resolve_type(&self, te: &TypeExpr) -> Result<Type, String> {
        self.checked.resolve_type(te)
    }
}

pub(crate) struct FnLowerer<'a, 'b> {
    pub mc: &'b mut ModuleCompiler<'a>,
    pub function_id: Option<u32>,
    pub return_ty: Type,
    pub ops: Vec<Op>,
    pub scalar_params_count: u16,
    pub obj_params_count: u16,
    next_local_s: u16,
    next_local_o: u16,
    pub(crate) local_s_end: u16,
    local_o_end: u16,
    pub temp_s: u16,
    pub temp_o: u16,
    pub max_s: u16,
    pub max_o: u16,
    pub compact_enums: HashMap<u16, CompactEnum>,
    pub stmt_slots: HashMap<usize, VarSlot>,
    pub pat_slots: HashMap<(usize, String), VarSlot>,
    pub loop_slots: HashMap<usize, LoopPrealloc>,
    pub(crate) hoisted_ints: HashMap<i64, u16>,
    pub(crate) hoisted_floats: HashMap<u64, u16>,
    pub(crate) hoisted_strs: HashMap<String, u16>,
    pub(crate) pending_hoist_ints: Vec<i64>,
    pub(crate) pending_hoist_floats: Vec<u64>,
    pub(crate) pending_hoist_strs: Vec<String>,
    pub scopes: Vec<HashMap<String, (VarSlot, Type)>>,
    pub loops: Vec<LoopPatch>,
    pub loop_depth: usize,
    pub(crate) has_defer: bool,
    pub(crate) defer_list_o: u16,
    pub(crate) defer_ret_slot: Option<VarSlot>,
    pub(crate) defer_rev_o: u16,
    pub(crate) defer_idx_s: u16,
    pub(crate) defer_id_s: u16,
    pub(crate) defer_epilogue_jumps: Vec<usize>,
    pub(crate) defer_sites: Vec<DeferSite>,
}

pub(crate) type DeferSite = (Block, Vec<HashMap<String, (VarSlot, Type)>>);

impl<'a, 'b> FnLowerer<'a, 'b> {
    fn new(
        mc: &'b mut ModuleCompiler<'a>,
        function_id: Option<u32>,
        params: &[Param],
        param_tys: &[Type],
        return_ty: Type,
        scalar_upvalues: &[(String, Type)],
        obj_upvalues: &[(String, Type)],
    ) -> Self {
        let mut init_scope = HashMap::new();
        let mut s_idx = 0u16;
        let mut o_idx = 0u16;
        for (p, ty) in params.iter().zip(param_tys.iter()) {
            let slot = if ty.is_scalar() {
                let r = s_idx;
                s_idx += 1;
                VarSlot { is_scalar: true, reg: r }
            } else {
                let r = o_idx;
                o_idx += 1;
                VarSlot { is_scalar: false, reg: r }
            };
            init_scope.insert(p.name.clone(), (slot, ty.clone()));
        }
        let scalar_params_count = s_idx;
        let obj_params_count = o_idx;

        for (name, ty) in scalar_upvalues {
            let r = s_idx;
            s_idx += 1;
            init_scope.insert(name.clone(), (VarSlot { is_scalar: true, reg: r }, ty.clone()));
        }
        for (name, ty) in obj_upvalues {
            let r = o_idx;
            o_idx += 1;
            init_scope.insert(name.clone(), (VarSlot { is_scalar: false, reg: r }, ty.clone()));
        }

        Self {
            mc,
            function_id,
            return_ty,
            ops: Vec::new(),
            scalar_params_count,
            obj_params_count,
            next_local_s: s_idx,
            next_local_o: o_idx,
            local_s_end: s_idx,
            local_o_end: o_idx,
            temp_s: s_idx,
            temp_o: o_idx,
            max_s: s_idx,
            max_o: o_idx,
            compact_enums: HashMap::new(),
            stmt_slots: HashMap::new(),
            pat_slots: HashMap::new(),
            loop_slots: HashMap::new(),
            hoisted_ints: HashMap::new(),
            hoisted_floats: HashMap::new(),
            hoisted_strs: HashMap::new(),
            pending_hoist_ints: Vec::new(),
            pending_hoist_floats: Vec::new(),
            pending_hoist_strs: Vec::new(),
            scopes: vec![init_scope],
            loops: Vec::new(),
            loop_depth: 0,
            has_defer: false,
            defer_list_o: 0,
            defer_ret_slot: None,
            defer_rev_o: 0,
            defer_idx_s: 0,
            defer_id_s: 0,
            defer_epilogue_jumps: Vec::new(),
            defer_sites: Vec::new(),
        }
    }

    pub(crate) fn alloc_local_s(&mut self) -> u16 {
        let r = self.next_local_s;
        self.next_local_s += 1;
        r
    }

    pub(crate) fn alloc_local_o(&mut self) -> u16 {
        let r = self.next_local_o;
        self.next_local_o += 1;
        r
    }

    pub(crate) fn prealloc_slot_for_ty(&mut self, ty: &Type) -> VarSlot {
        if ty.is_scalar() {
            VarSlot { is_scalar: true, reg: self.alloc_local_s() }
        } else {
            VarSlot { is_scalar: false, reg: self.alloc_local_o() }
        }
    }

    pub(crate) fn patch_here(&mut self, pc: usize) {
        let target = self.ops.len() as u32;
        patch_jump(&mut self.ops[pc], target);
    }

    fn finalize_prealloc(&mut self) {
        if self.has_defer {
            self.defer_list_o = self.alloc_local_o();
            self.defer_rev_o = self.alloc_local_o();
            self.defer_idx_s = self.alloc_local_s();
            self.defer_id_s = self.alloc_local_s();
            if self.return_ty != Type::Void {
                let rty = self.return_ty.clone();
                self.defer_ret_slot = Some(self.prealloc_slot_for_ty(&rty));
            }
        }
        self.local_s_end = self.next_local_s;
        self.local_o_end = self.next_local_o;
        self.temp_s = self.local_s_end;
        self.temp_o = self.local_o_end;
        self.max_s = self.local_s_end;
        self.max_o = self.local_o_end;
    }

    fn emit_defer_prologue(&mut self) -> Result<(), String> {
        if !self.has_defer {
            return Ok(());
        }
        self.ops.push(Op::NewListInt {
            dst_obj: self.defer_list_o,
            cap: 0,
        });
        if let Some(ret_slot) = self.defer_ret_slot
            && !ret_slot.is_scalar
            && Type::Void.is_assignable_to(&self.return_ty)
        {
            let idx = self.mc.intern_empty()?;
            self.ops.push(Op::LoadConstObj {
                dst_obj: ret_slot.reg,
                idx,
            });
        }
        Ok(())
    }

    fn emit_defer_epilogue(&mut self) -> Result<(), String> {
        if !self.has_defer {
            return Ok(());
        }
        let epilogue_pc = self.ops.len() as u32;
        let jumps = std::mem::take(&mut self.defer_epilogue_jumps);
        for j_pc in jumps {
            patch_jump(&mut self.ops[j_pc], epilogue_pc);
        }

        self.has_defer = false;
        self.ops.push(Op::ListReverse {
            dst_obj: self.defer_rev_o,
            list_obj: self.defer_list_o,
        });
        self.ops.push(Op::LoadImmInt {
            dst: self.defer_idx_s,
            imm: 0,
        });
        let loop_head = self.ops.len() as u32;
        let exit_jump = self.ops.len();
        self.ops.push(Op::ForListInt {
            list_obj: self.defer_rev_o,
            idx_s: self.defer_idx_s,
            elem_s: self.defer_id_s,
            exit_target: 0,
        });

        let saved_current_scopes = std::mem::take(&mut self.scopes);
        let sites = std::mem::take(&mut self.defer_sites);
        for (i, (block, site_scopes)) in sites.into_iter().enumerate() {
            let cmp_s = self.alloc_temp_s();
            self.ops.push(Op::EqIntImm {
                dst: cmp_s,
                a: self.defer_id_s,
                imm: i as i16,
            });
            let next_j = self.ops.len();
            self.ops.push(Op::JumpIfFalse {
                cond: cmp_s,
                target: 0,
            });
            self.scopes = site_scopes;
            self.lower_block_stmts(&block.stmts)?;
            self.reset_temps();
            self.ops.push(Op::Jump { target: loop_head });
            self.patch_here(next_j);
        }
        self.scopes = saved_current_scopes;
        self.ops.push(Op::Jump { target: loop_head });
        self.patch_here(exit_jump);

        match self.defer_ret_slot {
            None => self.ops.push(Op::ReturnVoid),
            Some(slot) if slot.is_scalar => {
                self.ops.push(Op::ReturnScalar { src: slot.reg });
            }
            Some(slot) => {
                self.ops.push(Op::ReturnObj { src: slot.reg });
            }
        }
        self.has_defer = true;
        Ok(())
    }

    fn emit_upvalue_prologue(
        &mut self,
        scalar_upvalues: &[(String, Type)],
        obj_upvalues: &[(String, Type)],
    ) {
        for (i, (name, _)) in scalar_upvalues.iter().enumerate() {
            let slot = self.scopes[0][name].0;
            self.ops.push(Op::LoadUpvalueScalar { dst: slot.reg, idx: i as u16 });
        }
        for (i, (name, _)) in obj_upvalues.iter().enumerate() {
            let slot = self.scopes[0][name].0;
            self.ops.push(Op::LoadUpvalueObj { dst: slot.reg, idx: i as u16 });
        }
    }

    fn has_jump_to_end(&self) -> bool {
        let end = Some(self.ops.len() as u32);
        self.ops.iter().any(|op| jump_target(*op) == end)
    }

    fn ensure_trailing_return(&mut self, return_ty: &Type) {
        let ends_ret = matches!(
            self.ops.last(),
            Some(Op::ReturnScalar { .. } | Op::ReturnAddInt { .. } | Op::ReturnObj { .. } | Op::ReturnVoid)
        );
        if !ends_ret || self.has_jump_to_end() {
            if matches!(return_ty, Type::Void) {
                self.ops.push(Op::ReturnVoid);
            } else if return_ty.is_obj()
                && Type::Void.is_assignable_to(return_ty)
                && let Ok(idx) = self.mc.intern_empty()
            {
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::LoadConstObj { dst_obj, idx });
                self.ops.push(Op::ReturnObj { src: dst_obj });
            } else if self.has_jump_to_end() {
                // Exhaustive branches/switches that all return can still leave a fallthrough jump
                // pointing to `ops.len()`; emit an unreachable terminator so jump targets stay in bounds.
                self.ops.push(Op::ReturnVoid);
            }
        }
    }

    pub(crate) fn reset_temps(&mut self) {
        self.temp_s = self.local_s_end;
        self.temp_o = self.local_o_end;
    }

    pub(crate) fn alloc_temp_s(&mut self) -> u16 {
        let r = self.temp_s;
        self.temp_s += 1;
        self.max_s = self.max_s.max(self.temp_s);
        r
    }

    pub(crate) fn alloc_temp_o(&mut self) -> u16 {
        let r = self.temp_o;
        self.temp_o += 1;
        self.max_o = self.max_o.max(self.temp_o);
        r
    }

    pub(crate) fn alloc_window_s(&mut self, count: u16) -> u16 {
        let base = self.temp_s;
        self.temp_s += count;
        self.max_s = self.max_s.max(self.temp_s);
        base
    }

    pub(crate) fn alloc_window_o(&mut self, count: u16) -> u16 {
        let base = self.temp_o;
        self.temp_o += count;
        self.max_o = self.max_o.max(self.temp_o);
        base
    }

    pub(crate) fn lookup_local(&self, name: &str) -> Option<(VarSlot, Type)> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        None
    }

    pub(crate) fn resolve_type(&self, te: &TypeExpr) -> Result<Type, String> {
        self.mc.resolve_type(te)
    }

    pub(crate) fn bind_var(&mut self, name: String, slot: VarSlot, ty: Type) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, (slot, ty));
        }
    }

    fn flush_hoisted_constants_if_outer(&mut self) -> Result<(), String> {
        if self.loop_depth != 0 {
            return Ok(());
        }
        for val in std::mem::take(&mut self.pending_hoist_ints) {
            let reg = self.hoisted_ints[&val];
            self.load_int_into(reg, val)?;
        }
        for bits in std::mem::take(&mut self.pending_hoist_floats) {
            let reg = self.hoisted_floats[&bits];
            let idx = self.mc.intern_float(bits)?;
            self.ops.push(Op::LoadConstFloat { dst: reg, idx });
        }
        for s in std::mem::take(&mut self.pending_hoist_strs) {
            let reg = self.hoisted_strs[&s];
            let idx = self.mc.intern_str(&s)?;
            self.ops.push(Op::LoadConstObj { dst_obj: reg, idx });
        }
        Ok(())
    }

    pub(crate) fn lower_block_stmts(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        self.scopes.push(HashMap::new());
        for stmt in stmts {
            self.lower_stmt(stmt)?;
        }
        self.scopes.pop();
        Ok(())
    }

    pub(crate) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        self.reset_temps();
        let skey = stmt as *const Stmt as usize;
        match &stmt.kind {
            StmtKind::Define { name, value } => {
                let slot = self.stmt_slots[&skey];
                self.lower_into_slot(value, slot)?;
                self.bind_var(name.clone(), slot, value.ty.clone());
            }
            StmtKind::VarDecl { name, ty, init } => {
                let slot = self.stmt_slots[&skey];
                let var_ty = if let Some(te) = ty {
                    self.resolve_type(te)?
                } else if let Some(e) = init {
                    e.ty.clone()
                } else {
                    Type::Any
                };
                if let Some(init_expr) = init {
                    self.lower_into_slot(init_expr, slot)?;
                }
                self.bind_var(name.clone(), slot, var_ty);
            }
            StmtKind::DestructureDefine { pattern, value } => {
                let is_scalar = value.ty.is_scalar();
                let reg = if is_scalar {
                    self.lower_scalar(value)?
                } else {
                    self.lower_obj(value)?
                };
                self.lower_pattern_bind(pattern, &value.ty, reg, is_scalar)?;
            }
            StmtKind::Assign { target, value } => {
                let ExprKind::Ident(name) = &target.kind else {
                    return Err("Only variable assignment is supported in Stage M4".to_string());
                };
                let (slot, _) = self
                    .lookup_local(name)
                    .ok_or_else(|| format!("Undefined variable '{}' in assign", name))?;
                if slot.is_scalar && self.try_lower_inplace_scalar_assign(name, slot.reg, value)? {
                    return Ok(());
                }
                self.lower_into_slot(value, slot)?;
            }
            StmtKind::Return(opt) => {
                if self.has_defer {
                    if let Some(ret_slot) = self.defer_ret_slot {
                        if let Some(expr) = opt {
                            if ret_slot.is_scalar && expr.ty.is_scalar() {
                                let r = self.lower_scalar(expr)?;
                                if r != ret_slot.reg {
                                    self.ops.push(Op::MoveScalar {
                                        dst: ret_slot.reg,
                                        src: r,
                                    });
                                }
                            } else if !ret_slot.is_scalar {
                                if expr.ty == Type::Void {
                                    self.lower_expr_discard(expr)?;
                                    let idx = self.mc.intern_empty()?;
                                    self.ops.push(Op::LoadConstObj {
                                        dst_obj: ret_slot.reg,
                                        idx,
                                    });
                                } else {
                                    let r = self.lower_obj(expr)?;
                                    if r != ret_slot.reg {
                                        self.ops.push(Op::MoveObj {
                                            dst_obj: ret_slot.reg,
                                            src: r,
                                        });
                                    }
                                }
                            } else {
                                return Err("Return register kind mismatch in defer".to_string());
                            }
                        } else if !ret_slot.is_scalar {
                            let idx = self.mc.intern_empty()?;
                            self.ops.push(Op::LoadConstObj {
                                dst_obj: ret_slot.reg,
                                idx,
                            });
                        }
                    } else if let Some(expr) = opt {
                        self.lower_expr_discard(expr)?;
                    }
                    let j = self.ops.len();
                    self.ops.push(Op::Jump { target: 0 });
                    self.defer_epilogue_jumps.push(j);
                    return Ok(());
                }
                if let Some(expr) = opt {
                    if self.return_ty.is_scalar() && expr.ty.is_scalar() {
                        let first = self.ops.len();
                        let r = self.lower_scalar(expr)?;
                        if let Some(Op::AddInt { dst, a, b }) = self.ops.last().copied()
                            && dst == r
                            && self.ops[first..].iter().any(|op| matches!(op, Op::CallSelfIntImm { .. }))
                        {
                            self.ops.pop();
                            self.ops.push(Op::ReturnAddInt { a, b });
                        } else {
                            self.ops.push(Op::ReturnScalar { src: r });
                        }
                    } else if self.return_ty.is_obj() {
                        if expr.ty == Type::Void {
                            self.lower_expr_discard(expr)?;
                            let idx = self.mc.intern_empty()?;
                            let r = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
                            self.ops.push(Op::ReturnObj { src: r });
                        } else {
                            let r = self.lower_obj(expr)?;
                            self.ops.push(Op::ReturnObj { src: r });
                        }
                    } else if expr.ty.is_scalar() {
                        let r = self.lower_scalar(expr)?;
                        self.ops.push(Op::ReturnScalar { src: r });
                    } else if expr.ty.is_obj() {
                        let r = self.lower_obj(expr)?;
                        self.ops.push(Op::ReturnObj { src: r });
                    } else {
                        self.lower_expr_discard(expr)?;
                        self.ops.push(Op::ReturnVoid);
                    }
                } else if self.return_ty.is_obj() {
                    let idx = self.mc.intern_empty()?;
                    let r = self.alloc_temp_o();
                    self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
                    self.ops.push(Op::ReturnObj { src: r });
                } else {
                    self.ops.push(Op::ReturnVoid);
                }
            }
            StmtKind::If { condition, then_block, else_block } => {
                if self.try_lower_int_if_chain(condition, then_block, else_block.as_ref())? {
                    return Ok(());
                }
                let jf_pc = self.lower_conditional_jump(condition, false)?;
                self.lower_block_stmts(&then_block.stmts)?;
                if self.ops.len() == jf_pc + 2
                    && let Op::JumpIfLtIntImm { a, imm, .. } = self.ops[jf_pc]
                    && let Op::ReturnScalar { src } = self.ops[jf_pc + 1]
                {
                    self.ops[jf_pc] = Op::ReturnScalarIfLtIntImm { a, imm, src };
                    self.ops.pop();
                    if let Some(eb) = else_block {
                        self.lower_block_stmts(&eb.stmts)?;
                    }
                    return Ok(());
                }
                if let Some(eb) = else_block {
                    let j_end_pc = self.ops.len();
                    self.ops.push(Op::Jump { target: 0 });
                    self.patch_here(jf_pc);
                    self.lower_block_stmts(&eb.stmts)?;
                    self.patch_here(j_end_pc);
                } else {
                    self.patch_here(jf_pc);
                }
            }
            StmtKind::Guard { condition, else_block } => {
                let jt_pc = self.lower_conditional_jump(condition, true)?;
                self.lower_block_stmts(&else_block.stmts)?;
                self.patch_here(jt_pc);
            }
            StmtKind::ForIn { pattern, iterable, body } => {
                self.flush_hoisted_constants_if_outer()?;
                self.lower_for_in(skey, pattern, iterable, body)?;
            }
            StmtKind::While { condition, body } => {
                self.flush_hoisted_constants_if_outer()?;
                if !self.try_lower_counting_while(skey, condition, body)? {
                    self.lower_while_generic(condition, body)?;
                }
            }
            StmtKind::Switch { expr, cases } => self.lower_switch(expr, cases)?,
            StmtKind::Defer(block) => {
                let site_id = self.defer_sites.len();
                self.defer_sites.push((block.clone(), self.scopes.clone()));
                let id_s = self.alloc_temp_s();
                self.ops.push(Op::LoadImmInt {
                    dst: id_s,
                    imm: site_id as i32,
                });
                self.ops.push(Op::ListAppendInt {
                    list_obj: self.defer_list_o,
                    val_s: id_s,
                });
            }
            StmtKind::Spawn(block) => {
                let closure_ty = Type::Function {
                    params: Vec::new(),
                    return_type: Box::new(Type::Void),
                };
                let closure_obj = self.lower_closure(&[], &None, block, &closure_ty)?;
                self.ops.push(Op::ExtOp {
                    kind: crate::vm::opcode::EXT_SPAWN,
                    aux: 0,
                    a: closure_obj,
                    b: 0,
                    c: 0,
                });
            }
            StmtKind::Break => {
                let pc = self.ops.len();
                self.ops.push(Op::Jump { target: 0 });
                let cur = self.loops.last_mut().ok_or("break outside loop")?;
                cur.breaks.push(pc);
            }
            StmtKind::Continue => {
                let pc = self.ops.len();
                self.ops.push(Op::Jump { target: 0 });
                let cur = self.loops.last_mut().ok_or("continue outside loop")?;
                cur.continues.push(pc);
            }
            StmtKind::Expr(e) => self.lower_expr_discard(e)?,
        }
        Ok(())
    }
}
