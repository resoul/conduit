use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

use crate::ir::{
    CallableRef, ClosureProto, ElemKind, FunctionProto, MigrationPlan, Module, PipelinePlan,
    PipelineStage, RecordDesc,
};
use crate::native::cli::{CliDefault, CliFieldPlan, CliKind, CliNodePlan, CliPlan, CliValKind};
use crate::native::env::{EnvDefault, EnvFieldKind, EnvFieldPlan, EnvLoadPlan};
use crate::native::validation::{
    CompiledValidationRule, NestedValidation, ValidationFieldPlan, ValidationStructPlan,
};
use crate::native::{JsonDecodePlan, JsonType, StructFieldDef, StructSchema};
use crate::vm::opcode::Op;
use crate::vm::value::{Obj, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT};

pub const BUNDLE_MAGIC: &[u8; 8] = b"CDTB0002";

pub const RET_VOID: u8 = 0;
pub const RET_INT: u8 = 1;
pub const RET_FLOAT: u8 = 2;
pub const RET_BOOL: u8 = 3;
pub const RET_OBJ: u8 = 4;

const MACHO_64_MAGIC: u32 = 0xfeed_facf;
const LC_SEGMENT_64: u32 = 0x19;
const LC_CODE_SIGNATURE: u32 = 0x1d;

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) { self.0.push(v); }
    fn bool(&mut self, v: bool) { self.u8(u8::from(v)); }
    fn u16(&mut self, v: u16) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn i16(&mut self, v: i16) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn i32(&mut self, v: i32) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn u64(&mut self, v: u64) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn i64(&mut self, v: i64) { self.0.extend_from_slice(&v.to_le_bytes()); }
    fn f64(&mut self, v: f64) { self.u64(v.to_bits()); }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn slice_u16(&mut self, s: &[u16]) {
        self.u32(s.len() as u32);
        for &v in s { self.u16(v); }
    }
    fn list<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T) -> Result<(), String>) -> Result<(), String> {
        self.u32(items.len() as u32);
        for item in items { f(self, item)?; }
        Ok(())
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self { Self { data, pos: 0 } }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or_else(|| "Bundle offset overflow".to_string())?;
        if end > self.data.len() { return Err("Unexpected end of bundle data".to_string()); }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, String> { Ok(self.take(1)?[0]) }
    fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(format!("Invalid bool tag {} in bundle", other)),
        }
    }
    fn take_arr<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "Unexpected EOF in bundle".to_string())
    }
    fn u16(&mut self) -> Result<u16, String> { Ok(u16::from_le_bytes(self.take_arr()?)) }
    fn i16(&mut self) -> Result<i16, String> { Ok(i16::from_le_bytes(self.take_arr()?)) }
    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take_arr()?)) }
    fn i32(&mut self) -> Result<i32, String> { Ok(i32::from_le_bytes(self.take_arr()?)) }
    fn u64(&mut self) -> Result<u64, String> { Ok(u64::from_le_bytes(self.take_arr()?)) }
    fn i64(&mut self) -> Result<i64, String> { Ok(i64::from_le_bytes(self.take_arr()?)) }
    fn f64(&mut self) -> Result<f64, String> { Ok(f64::from_bits(self.u64()?)) }
    fn str(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        std::str::from_utf8(self.take(len)?)
            .map(str::to_string)
            .map_err(|e| format!("Invalid UTF-8 in bundle: {}", e))
    }
    fn vec<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T, String>) -> Result<Vec<T>, String> {
        let len = self.u32()? as usize;
        if len > self.data.len() - self.pos {
            return Err("Bundle collection exceeds remaining data".to_owned());
        }
        let mut out = Vec::new();
        for _ in 0..len { out.push(f(self)?); }
        Ok(out)
    }
    fn box_u16(&mut self) -> Result<Box<[u16]>, String> { Ok(self.vec(Self::u16)?.into_boxed_slice()) }
}

macro_rules! op_codec {
    ($( $tag:literal => $name:ident { $( $field:ident : $ty:ident ),* } ),* $(,)?) => {
        fn encode_op(op: &Op, w: &mut Writer) {
            match *op { $( Op::$name { $( $field ),* } => { w.u8($tag); $( w.$ty($field); )* } )* }
        }
        fn decode_op(r: &mut Reader<'_>) -> Result<Op, String> {
            match r.u8()? {
                $( $tag => Ok(Op::$name { $( $field: r.$ty()? ),* }), )*
                tag => Err(format!("Unknown opcode tag {} in bundle", tag)),
            }
        }
    };
}

op_codec! {
    0 => LoadImmInt { dst: u16, imm: i32 },
    1 => LoadConstInt { dst: u16, idx: u16 },
    2 => LoadConstFloat { dst: u16, idx: u16 },
    3 => LoadBool { dst: u16, val: bool },
    4 => MoveScalar { dst: u16, src: u16 },
    5 => AddInt { dst: u16, a: u16, b: u16 },
    6 => AddIntImm { dst: u16, a: u16, imm: i16 },
    7 => SubInt { dst: u16, a: u16, b: u16 },
    8 => MulInt { dst: u16, a: u16, b: u16 },
    9 => DivInt { dst: u16, a: u16, b: u16 },
    10 => ModInt { dst: u16, a: u16, b: u16 },
    11 => ModIntImm { dst: u16, a: u16, imm: i16 },
    12 => NegInt { dst: u16, src: u16 },
    13 => AddFloat { dst: u16, a: u16, b: u16 },
    14 => SubFloat { dst: u16, a: u16, b: u16 },
    15 => MulFloat { dst: u16, a: u16, b: u16 },
    16 => DivFloat { dst: u16, a: u16, b: u16 },
    17 => FmaFloat { dst: u16, a: u16, b: u16, c: u16 },
    18 => IntToFloat { dst: u16, src: u16 },
    19 => FloatFloor { dst: u16, src: u16 },
    20 => FloatRound { dst: u16, src: u16 },
    21 => EqInt { dst: u16, a: u16, b: u16 },
    22 => NeInt { dst: u16, a: u16, b: u16 },
    23 => LtInt { dst: u16, a: u16, b: u16 },
    24 => LeInt { dst: u16, a: u16, b: u16 },
    25 => GtInt { dst: u16, a: u16, b: u16 },
    26 => GeInt { dst: u16, a: u16, b: u16 },
    27 => EqIntImm { dst: u16, a: u16, imm: i16 },
    28 => LtIntImm { dst: u16, a: u16, imm: i16 },
    29 => EqFloat { dst: u16, a: u16, b: u16 },
    30 => LtFloat { dst: u16, a: u16, b: u16 },
    31 => LeFloat { dst: u16, a: u16, b: u16 },
    32 => NotBool { dst: u16, src: u16 },
    33 => EqObj { dst_s: u16, a_obj: u16, b_obj: u16 },
    34 => Jump { target: u32 },
    35 => JumpIfFalse { cond: u16, target: u32 },
    36 => JumpIfTrue { cond: u16, target: u32 },
    37 => ForRangeStep { idx_reg: u16, limit_reg: u16, target: u32 },
    38 => CallDirect { func_id: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    39 => CallCallable { callee_obj: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    40 => MakeClosure { dst_obj: u16, proto_idx: u16 },
    41 => LoadUpvalueScalar { dst: u16, idx: u16 },
    42 => LoadUpvalueObj { dst: u16, idx: u16 },
    43 => LoadFuncObj { dst_obj: u16, func_id: u16 },
    44 => ReturnScalar { src: u16 },
    45 => ReturnObj { src: u16 },
    46 => ReturnVoid {},
    47 => LoadConstObj { dst_obj: u16, idx: u16 },
    48 => MoveObj { dst_obj: u16, src: u16 },
    49 => MakeRangeList { dst_obj: u16, start_s: u16, end_s: u16 },
    50 => NewListInt { dst_obj: u16, cap: u16 },
    51 => NewListFloat { dst_obj: u16, cap: u16 },
    52 => NewListObj { dst_obj: u16, cap: u16 },
    53 => ListAppendInt { list_obj: u16, val_s: u16 },
    54 => ListAppendFloat { list_obj: u16, val_s: u16 },
    55 => ListAppendObj { list_obj: u16, val_obj: u16 },
    56 => ListLen { dst_s: u16, list_obj: u16 },
    57 => StrLen { dst_s: u16, str_obj: u16 },
    58 => StrConcat { dst_obj: u16, a_obj: u16, b_obj: u16 },
    59 => ScalarToStr { dst_obj: u16, src_s: u16, tag: u8 },
    60 => ObjToStr { dst_obj: u16, src_obj: u16 },
    61 => StrMethod { dst_obj: u16, src_obj: u16, kind: u8 },
    62 => StrReplace { dst_obj: u16, str_obj: u16, from_obj: u16, to_obj: u16 },
    63 => ClampScalar { dst_s: u16, val_s: u16, lo_s: u16, hi_s: u16, is_float: bool },
    64 => ListIndex { dst_obj: u16, list_obj: u16, idx_s: u16, elem_tag: u8 },
    65 => ListGet { dst_obj: u16, list_obj: u16, idx_s: u16, elem_tag: u8 },
    66 => ListContainsScalar { dst_s: u16, list_obj: u16, val_s: u16, elem_tag: u8 },
    67 => ListContainsObj { dst_s: u16, list_obj: u16, val_obj: u16 },
    68 => ListSlice { dst_obj: u16, list_obj: u16, start_s: u16, end_s: u16 },
    69 => ListReverse { dst_obj: u16, list_obj: u16 },
    70 => ListEnumerate { dst_obj: u16, list_obj: u16, elem_tag: u8 },
    71 => ListJoin { dst_obj: u16, list_obj: u16, sep_obj: u16 },
    72 => TupleGetScalar { dst_s: u16, tuple_obj: u16, idx: u16 },
    73 => TupleGetObj { dst_obj: u16, tuple_obj: u16, idx: u16 },
    74 => ForListInt { list_obj: u16, idx_s: u16, elem_s: u16, exit_target: u32 },
    75 => ForListIntStep { list_obj: u16, idx_s: u16, elem_s: u16, target: u32 },
    76 => ForListObj { list_obj: u16, idx_s: u16, elem_obj: u16, exit_target: u32 },
    77 => NewRecord { dst_obj: u16, desc_idx: u16 },
    78 => NewRecordMove { dst_obj: u16, desc_idx: u16 },
    79 => GetFieldScalar { dst_s: u16, rec_obj: u16, field_idx: u16 },
    80 => GetFieldObj { dst_obj: u16, rec_obj: u16, field_idx: u16 },
    81 => NewMap { dst_obj: u16 },
    82 => MapSetIntInt { map_obj: u16, key_s: u16, val_s: u16 },
    83 => MapGetIntIntOr { dst_s: u16, map_obj: u16, key_s: u16, def_s: u16 },
    84 => MapLen { dst_s: u16, map_obj: u16 },
    85 => RunPipeline { dst_obj: u16, src_obj: u16, plan_idx: u16 },
    86 => JsonDecode { dst_obj: u16, src_obj: u16, plan_idx: u16 },
    87 => JsonEncode { dst_obj: u16, src_obj: u16 },
    88 => BoxScalar { dst_obj: u16, src_s: u16, kind: u8 },
    89 => UnboxScalar { dst_s: u16, src_obj: u16 },
    90 => IsTypeObj { dst_s: u16, src_obj: u16, expected_tag: u32 },
    91 => TryUnwrapScalar { dst_s: u16, src_obj: u16, ret_on_err: bool },
    92 => TryCheckObj { src_obj: u16, ret_on_err: bool, empty_is_err: bool },
    93 => TimeOp { kind: u8, dst_obj: u16, arg_reg: u16 },
    94 => ValidationOp { kind: u8, dst_obj: u16, a_reg: u16, b_reg: u16 },
    95 => ValidationCheck { dst_obj: u16, src_obj: u16 },
    96 => ExtOp { kind: u8, aux: u8, a: u16, b: u16, c: u16 },
    97 => JumpIfLtInt { a: u16, b: u16, target: u32, when_true: bool },
    98 => JumpIfLtIntImm { a: u16, imm: i16, target: u32, when_true: bool },
    99 => ReturnScalarIfLtIntImm { a: u16, imm: i16, src: u16 },
    100 => SwitchTag { src_obj: u16, first_tag: u32, tag_count: u16 },
    101 => BitAndInt { dst: u16, a: u16, b: u16 },
    102 => BitOrInt { dst: u16, a: u16, b: u16 },
    103 => BitXorInt { dst: u16, a: u16, b: u16 },
    104 => ShiftLeftInt { dst: u16, a: u16, b: u16 },
    105 => ShiftRightInt { dst: u16, a: u16, b: u16 },
    106 => SwitchScalarTag { tag_s: u16, src_obj: u16, first_tag: u32, tag_count: u16 },
    107 => MakeScalarEnum { dst_s: u16, schema_id: u32, a: u16, b: u16, len: u8 },
    108 => BoxScalarEnum { dst_obj: u16, src_obj: u16, tag_s: u16, width: u8 },
    109 => ReadScalarEnumField { dst_s: u16, tag_s: u16, src_obj: u16, field_idx: u8 },
    110 => ToBytes { dst_obj: u16, src_obj: u16 },
    111 => BytesToList { dst_obj: u16, src_obj: u16 },
    112 => NewHttpRequest { dst_obj: u16, desc_idx: u16 },
    113 => SumListIntTail { list_obj: u16, idx_s: u16, elem_s: u16, acc_s: u16 },
    114 => AppendIntRangeTail { list_obj: u16, idx_s: u16, limit_s: u16 },
    115 => MulIntImm { dst: u16, a: u16, imm: i16 },
    116 => MapGetIntIntOrImm { dst_s: u16, map_obj: u16, key_s: u16, default: i16 },
    117 => AddIntMapGetOrImm { dst_s: u16, a: u16, map_obj: u16, key_s: u16, default: i16 },
    118 => SumIntMapRangeTail { acc_s: u16, map_obj: u16, idx_s: u16, limit_s: u16, default: i16 },
    119 => JumpIfEqIntImm { a: u16, imm: i16, target: u32, when_true: bool },
    120 => ReadScalarEnumPair { dst_a: u16, dst_b: u16, tag_s: u16, src_obj: u16, field_a: u8, field_b: u8 },
    121 => CallSelfIntImm { arg_s: u16, arg_o: u16, src: u16, imm: i16 },
    122 => ReturnAddInt { a: u16, b: u16 },
    123 => SwitchIntImm { src: u16, first: i16, count: u16 },
    124 => MulAddIntImm { dst: u16, src: u16, mul: i16, add: i16 },
    125 => StrConcatInt { dst_obj: u16, prefix_obj: u16, src_s: u16 },
    126 => MulAddInt { dst: u16, a: u16, b: u16, c: u16 },
    127 => ModIntConst { dst: u16, a: u16, divisor: u32, magic_idx: u16, shift: u8 },
}

fn encode_obj_const(w: &mut Writer, obj: &Obj) -> Result<(), String> {
    match obj {
        Obj::Empty => w.u8(0),
        Obj::Str(s) => { w.u8(1); w.str(s); }
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_INT => { w.u8(2); w.u64(rec.inline_scalars[0]); }
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_FLOAT => { w.u8(3); w.u64(rec.inline_scalars[0]); }
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_BOOL => { w.u8(4); w.bool(rec.inline_scalars[0] != 0); }
        Obj::Func(fid) => { w.u8(5); w.u32(*fid); }
        Obj::Record(rec) if rec.scalar_len == 0 && rec.obj_len == 0 => {
            w.u8(6);
            w.u32(rec.schema_id);
        }
        other => return Err(format!("Unsupported constant object {:?} in bundle", other)),
    }
    Ok(())
}

fn decode_obj_const(r: &mut Reader<'_>) -> Result<Obj, String> {
    match r.u8()? {
        0 => Ok(Obj::Empty),
        1 => Ok(Obj::str(r.str()?)),
        2 => Ok(Obj::boxed_int(r.u64()? as i64)),
        3 => Ok(Obj::boxed_float(r.u64()?)),
        4 => Ok(Obj::boxed_bool(r.bool()?)),
        5 => Ok(Obj::Func(r.u32()?)),
        6 => Ok(Obj::Record(std::sync::Arc::new(crate::vm::value::StructData::new(
            r.u32()?, 0, 0,
        )))),
        tag => Err(format!("Invalid object constant tag {} in bundle", tag)),
    }
}

fn encode_json_type(ty: &JsonType, w: &mut Writer) {
    match ty {
        JsonType::Int => w.u8(0),
        JsonType::Float => w.u8(1),
        JsonType::Bool => w.u8(2),
        JsonType::String => w.u8(3),
        JsonType::Optional(inner) => { w.u8(4); encode_json_type(inner, w); }
        JsonType::List(inner) => { w.u8(5); encode_json_type(inner, w); }
        JsonType::Map(val) => { w.u8(6); encode_json_type(val, w); }
        JsonType::Struct(id) => { w.u8(7); w.u32(*id); }
        JsonType::TaggedEnum { name, variants } => {
            w.u8(8);
            w.str(name);
            w.u32(variants.len() as u32);
            for (vname, sid) in variants {
                w.str(vname);
                w.u32(*sid);
            }
        }
    }
}

fn decode_json_type(r: &mut Reader<'_>) -> Result<JsonType, String> {
    match r.u8()? {
        0 => Ok(JsonType::Int),
        1 => Ok(JsonType::Float),
        2 => Ok(JsonType::Bool),
        3 => Ok(JsonType::String),
        4 => Ok(JsonType::Optional(Box::new(decode_json_type(r)?))),
        5 => Ok(JsonType::List(Box::new(decode_json_type(r)?))),
        6 => Ok(JsonType::Map(Box::new(decode_json_type(r)?))),
        7 => Ok(JsonType::Struct(r.u32()?)),
        8 => {
            let name = r.str()?;
            let variants = r.vec(|r| Ok((r.str()?, r.u32()?)))?;
            Ok(JsonType::TaggedEnum { name, variants })
        }
        tag => Err(format!("Invalid JsonType tag {} in bundle", tag)),
    }
}

fn encode_callable_ref(c: CallableRef, w: &mut Writer) {
    match c {
        CallableRef::DirectFunc(fid) => { w.u8(0); w.u32(fid); }
        CallableRef::RegObj(r) => { w.u8(1); w.u16(r); }
    }
}

fn decode_callable_ref(r: &mut Reader<'_>) -> Result<CallableRef, String> {
    match r.u8()? {
        0 => Ok(CallableRef::DirectFunc(r.u32()?)),
        1 => Ok(CallableRef::RegObj(r.u16()?)),
        tag => Err(format!("Invalid CallableRef tag {} in bundle", tag)),
    }
}

fn encode_nested_val(n: &NestedValidation, w: &mut Writer) {
    match n {
        NestedValidation::None => w.u8(0),
        NestedValidation::Struct(id) => { w.u8(1); w.u32(*id); }
        NestedValidation::List(inner) => { w.u8(2); encode_nested_val(inner, w); }
    }
}

fn decode_nested_val(r: &mut Reader<'_>) -> Result<NestedValidation, String> {
    match r.u8()? {
        0 => Ok(NestedValidation::None),
        1 => Ok(NestedValidation::Struct(r.u32()?)),
        2 => Ok(NestedValidation::List(Box::new(decode_nested_val(r)?))),
        tag => Err(format!("Invalid NestedValidation tag {} in bundle", tag)),
    }
}

fn encode_val_rule(w: &mut Writer, rule: &CompiledValidationRule) -> Result<(), String> {
    match rule {
        CompiledValidationRule::MinInt(n) => { w.u8(0); w.i64(*n); }
        CompiledValidationRule::MaxInt(n) => { w.u8(1); w.i64(*n); }
        CompiledValidationRule::MinFloat(x) => { w.u8(2); w.f64(*x); }
        CompiledValidationRule::MaxFloat(x) => { w.u8(3); w.f64(*x); }
        CompiledValidationRule::MinObj(n) => { w.u8(4); w.i64(*n); }
        CompiledValidationRule::MaxObj(n) => { w.u8(5); w.i64(*n); }
        CompiledValidationRule::Email => w.u8(6),
        CompiledValidationRule::UserFunc {
            func_name, func_id, first_param_is_scalar, first_param_kind, extra_scalars, extra_objs,
        } => {
            w.u8(7);
            w.str(func_name);
            w.u32(*func_id);
            w.bool(*first_param_is_scalar);
            w.u8(*first_param_kind);
            w.list(extra_scalars, |w, &s| { w.u64(s); Ok(()) })?;
            w.list(extra_objs, encode_obj_const)?;
        }
        CompiledValidationRule::NotEmpty => w.u8(8),
        CompiledValidationRule::Url => w.u8(9),
        CompiledValidationRule::Uuid => w.u8(10),
        CompiledValidationRule::OneOf(vals) => {
            w.u8(11);
            w.list(vals, |w, s| { w.str(s); Ok(()) })?;
        }
        CompiledValidationRule::RangeInt(lo, hi) => { w.u8(12); w.i64(*lo); w.i64(*hi); }
        CompiledValidationRule::RangeFloat(lo, hi) => { w.u8(13); w.f64(*lo); w.f64(*hi); }
    }
    Ok(())
}

fn decode_val_rule(r: &mut Reader<'_>) -> Result<CompiledValidationRule, String> {
    Ok(match r.u8()? {
        0 => CompiledValidationRule::MinInt(r.i64()?),
        1 => CompiledValidationRule::MaxInt(r.i64()?),
        2 => CompiledValidationRule::MinFloat(r.f64()?),
        3 => CompiledValidationRule::MaxFloat(r.f64()?),
        4 => CompiledValidationRule::MinObj(r.i64()?),
        5 => CompiledValidationRule::MaxObj(r.i64()?),
        6 => CompiledValidationRule::Email,
        7 => CompiledValidationRule::UserFunc {
            func_name: r.str()?,
            func_id: r.u32()?,
            first_param_is_scalar: r.bool()?,
            first_param_kind: r.u8()?,
            extra_scalars: r.vec(Reader::u64)?,
            extra_objs: r.vec(decode_obj_const)?,
        },
        8 => CompiledValidationRule::NotEmpty,
        9 => CompiledValidationRule::Url,
        10 => CompiledValidationRule::Uuid,
        11 => CompiledValidationRule::OneOf(r.vec(Reader::str)?),
        12 => CompiledValidationRule::RangeInt(r.i64()?, r.i64()?),
        13 => CompiledValidationRule::RangeFloat(r.f64()?, r.f64()?),
        tag => return Err(format!("Invalid ValidationRule tag {} in bundle", tag)),
    })
}

pub fn encode_bundle(module: &Module, main_fid: u32, main_ret_tag: u8) -> Result<Vec<u8>, String> {
    let mut w = Writer(Vec::with_capacity(4096));
    w.0.extend_from_slice(BUNDLE_MAGIC);
    w.u32(main_fid);
    w.u8(main_ret_tag);

    w.list(&module.functions, |w, f| {
        w.str(&f.name);
        w.u16(f.num_scalars);
        w.u16(f.num_objs);
        w.u16(f.scalar_params);
        w.u16(f.obj_params);
        w.bool(f.returns_obj);
        w.list(&f.ops, |w, op| { encode_op(op, w); Ok(()) })
    })?;
    w.list(&module.int_consts, |w, &v| { w.i64(v); Ok(()) })?;
    w.list(&module.float_consts, |w, &v| { w.f64(v); Ok(()) })?;
    w.list(&module.obj_consts, encode_obj_const)?;
    w.list(&module.closures, |w, c| {
        w.u32(c.func_id);
        w.slice_u16(&c.capture_scalars);
        w.slice_u16(&c.capture_objs);
        Ok(())
    })?;
    w.list(&module.records, |w, r| {
        w.u32(r.schema_id);
        w.slice_u16(&r.scalar_regs);
        w.slice_u16(&r.obj_regs);
        Ok(())
    })?;
    w.list(&module.pipelines, |w, p| {
        w.u8(match p.elem_kind { ElemKind::Int => 0, ElemKind::Float => 1, ElemKind::Obj => 2 });
        w.u8(match p.out_elem_kind { ElemKind::Int => 0, ElemKind::Float => 1, ElemKind::Obj => 2 });
        w.list(&p.stages, |w, &st| {
            let (tag, cr) = match st {
                PipelineStage::MapScalarToScalar(c) => (0, c),
                PipelineStage::FilterScalar(c) => (1, c),
                PipelineStage::MapObjToObj(c) => (2, c),
                PipelineStage::FilterObj(c) => (3, c),
                PipelineStage::ReduceScalar(c) => (4, c),
                PipelineStage::MapObjToScalar(c) => (5, c),
                PipelineStage::MapScalarToObj(c) => (6, c),
            };
            w.u8(tag);
            encode_callable_ref(cr, w);
            Ok(())
        })
    })?;
    w.list(&module.struct_schemas, |w, s| {
        w.u32(s.schema_id);
        w.str(&s.type_name);
        w.bool(s.is_error);
        w.u16(s.http_status.unwrap_or(0));
        w.bool(s.enum_tag.is_some());
        if let Some(tag) = &s.enum_tag {
            w.str(tag);
        }
        w.list(&s.fields, |w, f| {
            w.str(&f.name);
            w.str(&f.json_name);
            encode_json_type(&f.ty, w);
            Ok(())
        })
    })?;
    w.list(&module.json_plans, |w, jp| { encode_json_type(&jp.root_ty, w); Ok(()) })?;
    w.list(&module.validation_plans, |w, vp| {
        w.u32(vp.schema_id);
        w.list(&vp.fields, |w, f| {
            w.str(&f.name);
            w.bool(f.is_scalar);
            w.u8(f.scalar_kind);
            w.bool(f.is_optional);
            w.u16(f.slot_idx);
            w.list(&f.rules, encode_val_rule)?;
            encode_nested_val(&f.nested, w);
            Ok(())
        })
    })?;
    w.list(&module.env_plans, |w, ep| {
        w.u32(ep.schema_id);
        w.u16(ep.scalar_len);
        w.u16(ep.obj_len);
        w.list(&ep.fields, |w, f| {
            w.str(&f.env_name);
            w.u8(match f.kind {
                EnvFieldKind::String => 0, EnvFieldKind::Int => 1,
                EnvFieldKind::Float => 2, EnvFieldKind::Bool => 3,
            });
            w.bool(f.is_optional);
            w.bool(f.is_scalar);
            w.u16(f.slot_idx);
            match &f.default {
                None => w.u8(0),
                Some(EnvDefault::Int(n)) => { w.u8(1); w.i64(*n); }
                Some(EnvDefault::Float(b)) => { w.u8(2); w.u64(*b); }
                Some(EnvDefault::Bool(b)) => { w.u8(3); w.bool(*b); }
                Some(EnvDefault::Str(s)) => { w.u8(4); w.str(s); }
                Some(EnvDefault::Nil) => w.u8(5),
            }
            Ok(())
        })
    })?;
    w.list(&module.cli_plans, |w, cp| {
        w.list(&cp.nodes, |w, n| {
            w.str(&n.name); w.str(&n.about); w.u32(n.schema_id); w.u16(n.scalar_len); w.u16(n.obj_len);
            w.list(&n.fields, |w, f| {
                w.str(&f.field_name); w.str(&f.cli_name);
                match f.short { Some(c) => { w.u8(1); w.u32(c as u32); } None => w.u8(0), }
                w.str(&f.help);
                w.u8(match f.kind { CliKind::Flag => 0, CliKind::Opt => 1, CliKind::Arg => 2, CliKind::Rest => 3, CliKind::Cmd => 4, CliKind::Group => 5 });
                w.u8(match f.val_kind { CliValKind::String => 0, CliValKind::Int => 1, CliValKind::Float => 2, CliValKind::Bool => 3, CliValKind::ListStr => 4, CliValKind::SubStruct => 5 });
                w.bool(f.is_optional); w.bool(f.is_scalar); w.u16(f.slot_idx);
                match &f.default {
                    None => w.u8(0),
                    Some(CliDefault::Int(v)) => { w.u8(1); w.i64(*v); }
                    Some(CliDefault::Float(b)) => { w.u8(2); w.u64(*b); }
                    Some(CliDefault::Bool(b)) => { w.u8(3); w.bool(*b); }
                    Some(CliDefault::Str(s)) => { w.u8(4); w.str(s); }
                    Some(CliDefault::Nil) => w.u8(5),
                    Some(CliDefault::EmptyList) => w.u8(6),
                }
                match f.sub_node { Some(s) => { w.u8(1); w.u16(s); } None => w.u8(0), }
                Ok(())
            })
        })
    })?;
    w.list(&module.migrations, |w, m| {
        w.i64(m.version);
        w.str(&m.name);
        w.u32(m.up_func_id);
        w.u32(m.down_func_id);
        Ok(())
    })?;
    w.list(&module.db_models, |w, m| {
        w.u32(m.schema_id);
        w.str(&m.table_name);
        w.list(&m.columns, |w, c| {
            w.str(&c.field_name);
            w.str(&c.col_name);
            w.bool(c.is_pk);
            w.bool(c.is_auto);
            w.bool(c.is_unique);
            w.bool(c.is_index);
            match &c.default_sql {
                Some(s) => { w.u8(1); w.str(s); }
                None => w.u8(0),
            }
            match &c.fk {
                Some((t, col)) => { w.u8(1); w.str(t); w.str(col); }
                None => w.u8(0),
            }
            Ok(())
        })
    })?;
    w.list(&module.schedules, |w, s| {
        w.str(&s.name);
        w.list(&s.jobs, |w, j| {
            w.str(&j.name);
            match &j.cron_expr {
                Some(e) => { w.u8(1); w.str(e); }
                None => w.u8(0),
            }
            match j.interval_ms {
                Some(ms) => { w.u8(1); w.u64(ms); }
                None => w.u8(0),
            }
            w.u32(j.func_id);
            Ok(())
        })
    })?;
    w.list(&module.apps, |w, a| {
        w.str(&a.name);
        match a.env_plan_idx {
            Some(idx) => { w.u8(1); w.u16(idx); }
            None => w.u8(0),
        }
        match &a.http_addr {
            Some(addr) => { w.u8(1); w.str(addr); }
            None => w.u8(0),
        }
        match a.schedule_idx {
            Some(idx) => { w.u8(1); w.u16(idx); }
            None => w.u8(0),
        }
        w.u64(a.shutdown_ms);
        w.list(&a.on_start_funcs, |w, &fid| { w.u32(fid); Ok(()) })?;
        w.list(&a.on_stop_funcs, |w, &fid| { w.u32(fid); Ok(()) })?;
        Ok(())
    })?;
    w.list(&module.http_query_plans, |w, qp| {
        w.u32(qp.schema_id);
        w.u16(qp.scalar_len);
        w.u16(qp.obj_len);
        w.list(&qp.fields, |w, f| {
            w.str(&f.name);
            w.str(&f.param_name);
            w.u8(f.kind);
            w.bool(f.is_optional);
            w.u16(f.slot);
            match &f.default_val {
                None => w.u8(0),
                Some(crate::ir::HttpQueryDefault::Str(s)) => { w.u8(1); w.str(s); }
                Some(crate::ir::HttpQueryDefault::Int(n)) => { w.u8(2); w.i64(*n); }
                Some(crate::ir::HttpQueryDefault::Float(v)) => { w.u8(3); w.u64(v.to_bits()); }
                Some(crate::ir::HttpQueryDefault::Bool(b)) => { w.u8(4); w.bool(*b); }
                Some(crate::ir::HttpQueryDefault::Nil) => w.u8(5),
            }
            Ok(())
        })
    })?;

    let digest = crate::native::crypto::sha256_bytes(&w.0);
    w.0.extend_from_slice(&digest);
    Ok(w.0)
}

pub fn decode_bundle(bytes: &[u8]) -> Result<(Module, u32, u8), String> {
    if bytes.len() < 40 {
        return Err("Invalid or truncated bundle".to_string());
    }
    if &bytes[..8] != BUNDLE_MAGIC {
        return Err("Invalid bundle magic header".to_string());
    }
    let (payload, checksum) = bytes.split_at(bytes.len() - 32);
    let expected = crate::native::crypto::sha256_bytes(payload);
    if !crate::native::crypto::constant_time_eq(&expected, checksum) {
        return Err("Bundle SHA-256 checksum mismatch (corrupted or tampered bundle)".to_string());
    }
    let mut r = Reader::new(payload);
    let _ = r.take(8)?;
    let main_fid = r.u32()?;
    let main_ret_tag = r.u8()?;

    let mut module = Module::new();
    module.functions = r.vec(|r| {
        Ok(FunctionProto {
            name: r.str()?,
            num_scalars: r.u16()?,
            num_objs: r.u16()?,
            scalar_params: r.u16()?,
            obj_params: r.u16()?,
            returns_obj: r.bool()?,
            ops: r.vec(decode_op)?.into_boxed_slice(),
        })
    })?;
    module.populate_lifecycle_hooks();
    module.int_consts = r.vec(Reader::i64)?;
    module.float_consts = r.vec(Reader::f64)?;
    module.obj_consts = r.vec(decode_obj_const)?;
    module.closures = r.vec(|r| Ok(ClosureProto {
        func_id: r.u32()?,
        capture_scalars: r.box_u16()?,
        capture_objs: r.box_u16()?,
    }))?;
    module.records = r.vec(|r| Ok(RecordDesc {
        schema_id: r.u32()?,
        scalar_regs: r.box_u16()?,
        obj_regs: r.box_u16()?,
    }))?;
    module.pipelines = r.vec(|r| {
        let elem_kind = match r.u8()? {
            0 => ElemKind::Int,
            1 => ElemKind::Float,
            2 => ElemKind::Obj,
            tag => return Err(format!("Invalid ElemKind tag {} in bundle", tag)),
        };
        let out_elem_kind = match r.u8()? {
            0 => ElemKind::Int,
            1 => ElemKind::Float,
            2 => ElemKind::Obj,
            tag => return Err(format!("Invalid out ElemKind tag {} in bundle", tag)),
        };
        let stages = r.vec(|r| {
            Ok(match r.u8()? {
                0 => PipelineStage::MapScalarToScalar(decode_callable_ref(r)?),
                1 => PipelineStage::FilterScalar(decode_callable_ref(r)?),
                2 => PipelineStage::MapObjToObj(decode_callable_ref(r)?),
                3 => PipelineStage::FilterObj(decode_callable_ref(r)?),
                4 => PipelineStage::ReduceScalar(decode_callable_ref(r)?),
                5 => PipelineStage::MapObjToScalar(decode_callable_ref(r)?),
                6 => PipelineStage::MapScalarToObj(decode_callable_ref(r)?),
                tag => return Err(format!("Invalid PipelineStage tag {} in bundle", tag)),
            })
        })?.into_boxed_slice();
        Ok(PipelinePlan { stages, elem_kind, out_elem_kind })
    })?;
    module.struct_schemas = r.vec(|r| {
        let schema_id = r.u32()?;
        let type_name = r.str()?;
        let is_error = r.bool()?;
        let raw_http_status = r.u16()?;
        let http_status = if raw_http_status == 0 { None } else { Some(raw_http_status) };
        let enum_tag = if r.bool()? { Some(r.str()?) } else { None };
        let field_defs = r.vec(|r| Ok(StructFieldDef {
            name: r.str()?,
            json_name: r.str()?,
            ty: decode_json_type(r)?,
        }))?;
        let mut schema = StructSchema::with_flags(schema_id, type_name, field_defs, is_error, enum_tag);
        schema.http_status = http_status;
        Ok(schema)
    })?;
    let json_roots = r.vec(decode_json_type)?;
    module.json_plans = json_roots
        .iter()
        .map(|ty| JsonDecodePlan::compile(ty, &module.struct_schemas))
        .collect();
    module.validation_plans = r.vec(|r| {
        Ok(ValidationStructPlan {
            schema_id: r.u32()?,
            fields: r.vec(|r| Ok(ValidationFieldPlan {
                name: r.str()?,
                is_scalar: r.bool()?,
                scalar_kind: r.u8()?,
                is_optional: r.bool()?,
                slot_idx: r.u16()?,
                rules: r.vec(decode_val_rule)?,
                nested: decode_nested_val(r)?,
            }))?,
        })
    })?;
    module.env_plans = r.vec(|r| {
        Ok(EnvLoadPlan {
            schema_id: r.u32()?,
            scalar_len: r.u16()?,
            obj_len: r.u16()?,
            fields: r.vec(|r| {
                Ok(EnvFieldPlan {
                    env_name: r.str()?,
                    kind: match r.u8()? {
                        0 => EnvFieldKind::String, 1 => EnvFieldKind::Int,
                        2 => EnvFieldKind::Float, 3 => EnvFieldKind::Bool,
                        tag => return Err(format!("Invalid EnvFieldKind tag {} in bundle", tag)),
                    },
                    is_optional: r.bool()?,
                    is_scalar: r.bool()?,
                    slot_idx: r.u16()?,
                    default: match r.u8()? {
                        0 => None,
                        1 => Some(EnvDefault::Int(r.i64()?)),
                        2 => Some(EnvDefault::Float(r.u64()?)),
                        3 => Some(EnvDefault::Bool(r.bool()?)),
                        4 => Some(EnvDefault::Str(r.str()?)),
                        5 => Some(EnvDefault::Nil),
                        tag => return Err(format!("Invalid EnvDefault tag {} in bundle", tag)),
                    },
                })
            })?.into_boxed_slice(),
        })
    })?;
    module.cli_plans = r.vec(|r| {
        let nodes = r.vec(|r| {
            let name = r.str()?; let about = r.str()?; let schema_id = r.u32()?; let scalar_len = r.u16()?; let obj_len = r.u16()?;
            let fields = r.vec(|r| {
                let field_name = r.str()?; let cli_name = r.str()?;
                let short = if r.u8()? == 1 { Some(char::from_u32(r.u32()?).ok_or_else(|| "Invalid char in bundle".to_string())?) } else { None };
                let help = r.str()?;
                let kind = match r.u8()? { 0 => CliKind::Flag, 1 => CliKind::Opt, 2 => CliKind::Arg, 3 => CliKind::Rest, 4 => CliKind::Cmd, 5 => CliKind::Group, t => return Err(format!("Invalid CliKind {}", t)) };
                let val_kind = match r.u8()? { 0 => CliValKind::String, 1 => CliValKind::Int, 2 => CliValKind::Float, 3 => CliValKind::Bool, 4 => CliValKind::ListStr, 5 => CliValKind::SubStruct, t => return Err(format!("Invalid CliValKind {}", t)) };
                let is_optional = r.bool()?; let is_scalar = r.bool()?; let slot_idx = r.u16()?;
                let default = match r.u8()? { 0 => None, 1 => Some(CliDefault::Int(r.i64()?)), 2 => Some(CliDefault::Float(r.u64()?)), 3 => Some(CliDefault::Bool(r.bool()?)), 4 => Some(CliDefault::Str(r.str()?)), 5 => Some(CliDefault::Nil), 6 => Some(CliDefault::EmptyList), t => return Err(format!("Invalid CliDefault {}", t)) };
                let sub_node = if r.u8()? == 1 { Some(r.u16()?) } else { None };
                Ok(CliFieldPlan { field_name, cli_name, short, help, kind, val_kind, is_optional, is_scalar, slot_idx, default, sub_node })
            })?.into_boxed_slice();
            Ok(CliNodePlan { name, about, schema_id, scalar_len, obj_len, fields })
        })?.into_boxed_slice();
        Ok(CliPlan { nodes })
    })?;
    module.migrations = r.vec(|r| {
        let version = r.i64()?;
        let name = r.str()?;
        let up_func_id = r.u32()?;
        let down_func_id = r.u32()?;
        Ok(MigrationPlan {
            version,
            name,
            up_func_id,
            down_func_id,
        })
    })?;
    module.db_models = r.vec(|r| {
        let schema_id = r.u32()?;
        let table_name = r.str()?;
        let columns = r.vec(|r| {
            let field_name = r.str()?;
            let col_name = r.str()?;
            let is_pk = r.bool()?;
            let is_auto = r.bool()?;
            let is_unique = r.bool()?;
            let is_index = r.bool()?;
            let default_sql = if r.u8()? == 1 { Some(r.str()?) } else { None };
            let fk = if r.u8()? == 1 { Some((r.str()?, r.str()?)) } else { None };
            Ok(crate::ir::DbColumnMeta {
                field_name,
                col_name,
                is_pk,
                is_auto,
                is_unique,
                is_index,
                default_sql,
                fk,
            })
        })?;
        Ok(crate::ir::DbModelMeta {
            schema_id,
            table_name,
            columns,
        })
    })?;
    module.schedules = r.vec(|r| {
        let name = r.str()?;
        let jobs = r.vec(|r| {
            let name = r.str()?;
            let cron_expr = if r.u8()? == 1 { Some(r.str()?) } else { None };
            let interval_ms = if r.u8()? == 1 { Some(r.u64()?) } else { None };
            let func_id = r.u32()?;
            Ok(crate::ir::CronJobSpec {
                name,
                cron_expr,
                interval_ms,
                func_id,
            })
        })?;
        Ok(crate::ir::SchedulePlan { name, jobs })
    })?;
    module.apps = r.vec(|r| {
        let name = r.str()?;
        let env_plan_idx = if r.u8()? == 1 { Some(r.u16()?) } else { None };
        let http_addr = if r.u8()? == 1 { Some(r.str()?) } else { None };
        let schedule_idx = if r.u8()? == 1 { Some(r.u16()?) } else { None };
        let shutdown_ms = r.u64()?;
        let on_start_funcs = r.vec(Reader::u32)?;
        let on_stop_funcs = r.vec(Reader::u32)?;
        Ok(crate::ir::AppPlan {
            name,
            env_plan_idx,
            http_addr,
            schedule_idx,
            shutdown_ms,
            on_start_funcs,
            on_stop_funcs,
        })
    })?;
    module.http_query_plans = r.vec(|r| {
        Ok(crate::ir::HttpQueryPlan {
            schema_id: r.u32()?,
            scalar_len: r.u16()?,
            obj_len: r.u16()?,
            fields: r.vec(|r| {
                let name = r.str()?;
                let param_name = r.str()?;
                let kind = r.u8()?;
                let is_optional = r.bool()?;
                let slot = r.u16()?;
                let default_val = match r.u8()? {
                    0 => None,
                    1 => Some(crate::ir::HttpQueryDefault::Str(r.str()?)),
                    2 => Some(crate::ir::HttpQueryDefault::Int(r.i64()?)),
                    3 => Some(crate::ir::HttpQueryDefault::Float(f64::from_bits(r.u64()?))),
                    4 => Some(crate::ir::HttpQueryDefault::Bool(r.bool()?)),
                    5 => Some(crate::ir::HttpQueryDefault::Nil),
                    tag => return Err(format!("Invalid HttpQueryDefault tag {} in bundle", tag)),
                };
                Ok(crate::ir::HttpQueryFieldPlan {
                    name,
                    param_name,
                    kind,
                    is_optional,
                    slot,
                    default_val,
                })
            })?,
        })
    })?;

    if r.pos != payload.len() {
        return Err("Unexpected trailing bytes in bundle".to_string());
    }
    module.validate()?;
    if (main_fid as usize) >= module.functions.len() {
        return Err(format!("Invalid main_fid {} in bundle", main_fid));
    }
    Ok((module, main_fid, main_ret_tag))
}

fn find_macho_load_cmd(buf: &[u8], target_cmd: u32) -> Option<usize> {
    if buf.len() < 32 || u32::from_le_bytes(buf[0..4].try_into().ok()?) != MACHO_64_MAGIC {
        return None;
    }
    let ncmds = u32::from_le_bytes(buf[16..20].try_into().ok()?);
    let mut off = 32usize;
    for _ in 0..ncmds {
        if off + 8 > buf.len() { break; }
        let cmd = u32::from_le_bytes(buf[off..off + 4].try_into().ok()?);
        let cmdsize = u32::from_le_bytes(buf[off + 4..off + 8].try_into().ok()?) as usize;
        if cmdsize < 8 || off + cmdsize > buf.len() { break; }
        if cmd == target_cmd {
            if target_cmd == LC_SEGMENT_64 {
                if off + 56 <= buf.len() && buf[off + 8..off + 24].starts_with(b"__LINKEDIT\0") {
                    return Some(off);
                }
            } else {
                return Some(off);
            }
        }
        off += cmdsize;
    }
    None
}

pub fn extract_bundle_from_exe(exe_path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut file = File::open(exe_path).map_err(|e| format!("Failed to open executable: {}", e))?;
    let file_len = file.metadata().map_err(|e| format!("Failed to stat executable: {}", e))?.len();
    let mut hdr = vec![0u8; file_len.min(4096) as usize];
    file.read_exact(&mut hdr).map_err(|e| format!("Failed to read header: {}", e))?;

    let end_pos = find_macho_load_cmd(&hdr, LC_CODE_SIGNATURE)
        .and_then(|off| hdr.get(off + 8..off + 12))
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as u64)
        .unwrap_or(file_len);
    if end_pos < 16 { return Ok(None); }

    file.seek(SeekFrom::Start(end_pos - 16)).map_err(|e| format!("Failed to seek trailer: {}", e))?;
    let mut footer = [0u8; 16];
    file.read_exact(&mut footer).map_err(|e| format!("Failed to read trailer footer: {}", e))?;
    if &footer[8..16] != BUNDLE_MAGIC { return Ok(None); }

    let raw_len = u64::from_le_bytes(footer[0..8].try_into().unwrap()) as usize;
    let padded_len = (raw_len + 15) & !15;
    if 16 + padded_len > end_pos as usize {
        return Err("Corrupt embedded bytecode bundle length".to_string());
    }
    file.seek(SeekFrom::Start(end_pos - 16 - padded_len as u64))
        .map_err(|e| format!("Failed to seek embedded bundle: {}", e))?;
    let mut buf = vec![0u8; raw_len];
    file.read_exact(&mut buf).map_err(|e| format!("Failed to read embedded bundle: {}", e))?;
    Ok(Some(buf))
}

pub fn write_standalone_binary(base_exe: &Path, out_path: &Path, bundle_bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = out_path.parent() && !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create output directory: {}", e))?;
    }
    fs::copy(base_exe, out_path)
        .map_err(|e| format!("Failed to copy runtime binary to '{}': {}", out_path.display(), e))?;

    if cfg!(target_os = "macos") {
        let _ = Command::new("codesign").args(["--remove-signature"]).arg(out_path).output();
    }

    let mut data = fs::read(out_path)
        .map_err(|e| format!("Failed to read '{}': {}", out_path.display(), e))?;
    if data.len() >= 16 && &data[data.len() - 8..] == BUNDLE_MAGIC {
        let raw_len = u64::from_le_bytes(data[data.len() - 16..data.len() - 8].try_into().unwrap()) as usize;
        let padded_len = (raw_len + 15) & !15;
        if 16 + padded_len <= data.len() {
            data.truncate(data.len() - 16 - padded_len);
        }
    }
    while !data.len().is_multiple_of(16) { data.push(0); }

    let mut trailer = Vec::with_capacity(((bundle_bytes.len() + 15) & !15) + 16);
    trailer.extend_from_slice(bundle_bytes);
    while !trailer.len().is_multiple_of(16) { trailer.push(0); }
    trailer.extend_from_slice(&(bundle_bytes.len() as u64).to_le_bytes());
    trailer.extend_from_slice(BUNDLE_MAGIC);

    let is_macho = data.len() >= 32 && u32::from_le_bytes(data[0..4].try_into().unwrap()) == MACHO_64_MAGIC;
    if let Some(off) = find_macho_load_cmd(&data, LC_SEGMENT_64) {
        let vmsize = u64::from_le_bytes(data[off + 32..off + 40].try_into().unwrap());
        let fileoff = u64::from_le_bytes(data[off + 40..off + 48].try_into().unwrap());
        let new_filesize = (data.len() as u64 - fileoff) + trailer.len() as u64;
        let new_vmsize = vmsize.max((new_filesize + 0x3fff) & !0x3fff);
        data[off + 32..off + 40].copy_from_slice(&new_vmsize.to_le_bytes());
        data[off + 48..off + 56].copy_from_slice(&new_filesize.to_le_bytes());
    }

    data.extend_from_slice(&trailer);
    fs::write(out_path, &data)
        .map_err(|e| format!("Failed to write '{}': {}", out_path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(out_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to mark '{}' executable: {}", out_path.display(), e))?;
    }

    if is_macho && cfg!(target_os = "macos") {
        let out = Command::new("codesign").args(["-f", "-s", "-"]).arg(out_path).output()
            .map_err(|e| format!("Failed to run codesign: {}", e))?;
        if !out.status.success() {
            return Err(format!("codesign failed for '{}': {}", out_path.display(), String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
    Ok(())
}
