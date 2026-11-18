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
    Ok(mc.module)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VarSlot {
    pub is_scalar: bool,
    pub reg: u16,
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
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_function(
        &mut self,
        name: &str,
        params: &[Param],
        param_tys: &[Type],
        return_ty: &Type,
        body: &Block,
        scalar_upvalues: &[(String, Type)],
        obj_upvalues: &[(String, Type)],
    ) -> Result<FunctionProto, String> {
        let mut lowerer = FnLowerer::new(
            self,
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
        lowerer.lower_block_stmts(&body.stmts)?;
        lowerer.ensure_trailing_return(return_ty);

        Ok(FunctionProto {
            name: name.to_string(),
            num_scalars: lowerer.max_s.max(lowerer.scalar_params_count).max(1),
            num_objs: lowerer.max_o.max(lowerer.obj_params_count),
            scalar_params: lowerer.scalar_params_count,
            obj_params: lowerer.obj_params_count,
            returns_obj: return_ty.is_obj(),
            ops: lowerer.ops.into_boxed_slice(),
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
        let proto = self.lower_function(&name, params, param_tys, return_ty, body, &s_upv, &o_upv)?;
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
}

impl<'a, 'b> FnLowerer<'a, 'b> {
    fn new(
        mc: &'b mut ModuleCompiler<'a>,
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
        self.local_s_end = self.next_local_s;
        self.local_o_end = self.next_local_o;
        self.temp_s = self.local_s_end;
        self.temp_o = self.local_o_end;
        self.max_s = self.local_s_end;
        self.max_o = self.local_o_end;
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
        let end = self.ops.len() as u32;
        self.ops.iter().any(|op| match *op {
            Op::Jump { target }
            | Op::JumpIfFalse { target, .. }
            | Op::JumpIfTrue { target, .. }
            | Op::ForRangeStep { target, .. }
            | Op::ForListInt { exit_target: target, .. }
            | Op::ForListIntStep { target, .. }
            | Op::ForListObj { exit_target: target, .. } => target == end,
            _ => false,
        })
    }

    fn ensure_trailing_return(&mut self, return_ty: &Type) {
        let ends_ret = matches!(
            self.ops.last(),
            Some(Op::ReturnScalar { .. } | Op::ReturnObj { .. } | Op::ReturnVoid)
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
                if slot.is_scalar {
                    let r = self.lower_scalar(value)?;
                    if r != slot.reg {
                        self.ops.push(Op::MoveScalar { dst: slot.reg, src: r });
                    }
                } else {
                    let r = self.lower_obj(value)?;
                    if r != slot.reg {
                        self.ops.push(Op::MoveObj { dst_obj: slot.reg, src: r });
                    }
                }
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
                    if slot.is_scalar {
                        let r = self.lower_scalar(init_expr)?;
                        if r != slot.reg {
                            self.ops.push(Op::MoveScalar { dst: slot.reg, src: r });
                        }
                    } else {
                        let r = self.lower_obj(init_expr)?;
                        if r != slot.reg {
                            self.ops.push(Op::MoveObj { dst_obj: slot.reg, src: r });
                        }
                    }
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
                if slot.is_scalar {
                    let r = self.lower_scalar(value)?;
                    if r != slot.reg {
                        self.ops.push(Op::MoveScalar { dst: slot.reg, src: r });
                    }
                } else {
                    let r = self.lower_obj(value)?;
                    if r != slot.reg {
                        self.ops.push(Op::MoveObj { dst_obj: slot.reg, src: r });
                    }
                }
            }
            StmtKind::Return(opt) => {
                if let Some(expr) = opt {
                    if self.return_ty.is_scalar() && expr.ty.is_scalar() {
                        let r = self.lower_scalar(expr)?;
                        self.ops.push(Op::ReturnScalar { src: r });
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
                let cond_r = self.lower_scalar(condition)?;
                let jf_pc = self.ops.len();
                self.ops.push(Op::JumpIfFalse { cond: cond_r, target: 0 });
                self.lower_block_stmts(&then_block.stmts)?;
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
                let cond_r = self.lower_scalar(condition)?;
                let jt_pc = self.ops.len();
                self.ops.push(Op::JumpIfTrue { cond: cond_r, target: 0 });
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
