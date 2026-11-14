use crate::ir::{
    ClosureProto, FunctionProto, Module, PipelinePlan, RecordDesc,
};
use crate::native::{JsonDecodePlan, StructSchema};
use crate::vm::opcode::Op;
use crate::vm::value::Obj;

pub struct FnBuilder {
    name: String,
    scalar_params: u16,
    obj_params: u16,
    num_scalars: u16,
    num_objs: u16,
    returns_obj: bool,
    ops: Vec<Op>,
}

impl FnBuilder {
    pub fn new(
        name: impl Into<String>,
        scalar_params: u16,
        obj_params: u16,
        returns_obj: bool,
    ) -> Self {
        Self {
            name: name.into(),
            scalar_params,
            obj_params,
            num_scalars: scalar_params,
            num_objs: obj_params,
            returns_obj,
            ops: Vec::new(),
        }
    }

    pub fn alloc_s(&mut self) -> u16 {
        let r = self.num_scalars;
        self.num_scalars += 1;
        r
    }

    pub fn alloc_s_block(&mut self, count: u16) -> u16 {
        let base = self.num_scalars;
        self.num_scalars += count;
        base
    }

    pub fn alloc_o(&mut self) -> u16 {
        let r = self.num_objs;
        self.num_objs += 1;
        r
    }

    pub fn alloc_o_block(&mut self, count: u16) -> u16 {
        let base = self.num_objs;
        self.num_objs += count;
        base
    }

    pub fn next_s(&self) -> u16 {
        self.num_scalars
    }

    pub fn next_o(&self) -> u16 {
        self.num_objs
    }

    pub fn pc(&self) -> u32 {
        self.ops.len() as u32
    }

    pub fn emit(&mut self, op: Op) -> usize {
        let idx = self.ops.len();
        self.ops.push(op);
        idx
    }

    pub fn patch_target(&mut self, op_idx: usize, new_target: u32) {
        match &mut self.ops[op_idx] {
            Op::Jump { target }
            | Op::JumpIfFalse { target, .. }
            | Op::JumpIfTrue { target, .. }
            | Op::ForRangeStep { target, .. } => *target = new_target,
            Op::ForListInt { exit_target, .. } | Op::ForListObj { exit_target, .. } => {
                *exit_target = new_target
            }
            _ => panic!("patch_target called on non-branch op"),
        }
    }

    pub fn finish(self) -> FunctionProto {
        FunctionProto {
            name: self.name,
            num_scalars: self.num_scalars,
            num_objs: self.num_objs,
            scalar_params: self.scalar_params,
            obj_params: self.obj_params,
            returns_obj: self.returns_obj,
            ops: self.ops.into_boxed_slice(),
        }
    }
}

pub trait ModuleExt {
    fn add_fn(&mut self, f: FunctionProto) -> u16;
    fn add_int(&mut self, v: i64) -> u16;
    fn add_float(&mut self, v: f64) -> u16;
    fn add_obj(&mut self, v: Obj) -> u16;
    fn add_closure(&mut self, cp: ClosureProto) -> u16;
    fn add_record(&mut self, desc: RecordDesc) -> u16;
    fn add_pipeline(&mut self, plan: PipelinePlan) -> u16;
    fn add_struct_schema(&mut self, schema: StructSchema) -> u16;
    fn add_json_plan(&mut self, plan: JsonDecodePlan) -> u16;
}

impl ModuleExt for Module {
    fn add_fn(&mut self, f: FunctionProto) -> u16 {
        let id = self.functions.len() as u16;
        self.functions.push(f);
        id
    }

    fn add_int(&mut self, v: i64) -> u16 {
        let id = self.int_consts.len() as u16;
        self.int_consts.push(v);
        id
    }

    fn add_float(&mut self, v: f64) -> u16 {
        let id = self.float_consts.len() as u16;
        self.float_consts.push(v);
        id
    }

    fn add_obj(&mut self, v: Obj) -> u16 {
        let id = self.obj_consts.len() as u16;
        self.obj_consts.push(v);
        id
    }

    fn add_closure(&mut self, cp: ClosureProto) -> u16 {
        let id = self.closures.len() as u16;
        self.closures.push(cp);
        id
    }

    fn add_record(&mut self, desc: RecordDesc) -> u16 {
        let id = self.records.len() as u16;
        self.records.push(desc);
        id
    }

    fn add_pipeline(&mut self, plan: PipelinePlan) -> u16 {
        let id = self.pipelines.len() as u16;
        self.pipelines.push(plan);
        id
    }

    fn add_struct_schema(&mut self, schema: StructSchema) -> u16 {
        let id = self.struct_schemas.len() as u16;
        self.struct_schemas.push(schema);
        id
    }

    fn add_json_plan(&mut self, plan: JsonDecodePlan) -> u16 {
        let id = self.json_plans.len() as u16;
        self.json_plans.push(plan);
        id
    }
}

/// Emits a benchmark driver function:
/// `func(iterations: Int, <extra params>) -> Int`
/// that loops `iter in 0..iterations`, runs `emit_body(fb, score_reg)`, accumulates `checksum += score`,
/// and returns `checksum`.
pub fn build_bench_loop<F>(
    name: &str,
    extra_scalar_params: u16,
    extra_obj_params: u16,
    emit_body: F,
) -> FunctionProto
where
    F: FnOnce(&mut FnBuilder, u16),
{
    let mut fb = FnBuilder::new(name, 1 + extra_scalar_params, extra_obj_params, false);
    let iterations = 0u16;
    let checksum = fb.alloc_s();
    let iter = fb.alloc_s();
    let cond = fb.alloc_s();
    let score = fb.alloc_s();

    fb.emit(Op::LoadImmInt {
        dst: checksum,
        imm: 0,
    });
    fb.emit(Op::LoadImmInt { dst: iter, imm: 0 });
    fb.emit(Op::LtInt {
        dst: cond,
        a: iter,
        b: iterations,
    });
    let exit_jmp = fb.emit(Op::JumpIfFalse { cond, target: 0 });

    let loop_top = fb.pc();
    emit_body(&mut fb, score);
    fb.emit(Op::AddInt {
        dst: checksum,
        a: checksum,
        b: score,
    });
    fb.emit(Op::ForRangeStep {
        idx_reg: iter,
        limit_reg: iterations,
        target: loop_top,
    });

    let done = fb.pc();
    fb.patch_target(exit_jmp, done);
    fb.emit(Op::ReturnScalar { src: checksum });
    fb.finish()
}
