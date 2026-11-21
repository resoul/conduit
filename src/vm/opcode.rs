/// Non-power-of-two divisors use a rounded-up fixed-point reciprocal.
pub(crate) fn remainder_magic(divisor: u32) -> Option<(i64, u8)> {
    if divisor < 3 || divisor.is_power_of_two() {
        return None;
    }
    let shift = divisor.ilog2();
    let scale = 1u128 << (64 + shift);
    let reciprocal = scale.div_ceil(u128::from(divisor));
    Some((reciprocal as u64 as i64, shift as u8))
}

#[inline]
pub(crate) fn remainder_by_constant(value: i64, divisor: u32, magic: i64, shift: u8) -> i64 {
    // The reciprocal is in (2^63, 2^64). Adding value restores the high
    // bit omitted by treating it as signed for the high-half multiply.
    let high = ((i128::from(value) * i128::from(magic)) >> 64) as i64;
    // Reciprocal rounding contributes less than 1/divisor to the quotient.
    // Negative exact multiples round down too, so add one for all negatives.
    let quotient = ((high + value) >> shift) - (value >> 63);
    value - quotient * i64::from(divisor)
}

/// Compact 3-address typed register instructions (<= 12 bytes, Copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    // --- Scalars & Arithmetic ---
    LoadImmInt { dst: u16, imm: i32 },
    LoadConstInt { dst: u16, idx: u16 },
    LoadConstFloat { dst: u16, idx: u16 },
    LoadBool { dst: u16, val: bool },
    MoveScalar { dst: u16, src: u16 },
    AddInt { dst: u16, a: u16, b: u16 },
    AddIntImm { dst: u16, a: u16, imm: i16 },
    SubInt { dst: u16, a: u16, b: u16 },
    MulAddIntImm { dst: u16, src: u16, mul: i16, add: i16 },
    MulAddInt { dst: u16, a: u16, b: u16, c: u16 },
    MulIntImm { dst: u16, a: u16, imm: i16 },
    MulInt { dst: u16, a: u16, b: u16 },
    DivInt { dst: u16, a: u16, b: u16 },
    ModInt { dst: u16, a: u16, b: u16 },
    ModIntConst { dst: u16, a: u16, divisor: u32, magic_idx: u16, shift: u8 },
    ModIntImm { dst: u16, a: u16, imm: i16 },
    BitAndInt { dst: u16, a: u16, b: u16 },
    BitOrInt { dst: u16, a: u16, b: u16 },
    BitXorInt { dst: u16, a: u16, b: u16 },
    ShiftLeftInt { dst: u16, a: u16, b: u16 },
    ShiftRightInt { dst: u16, a: u16, b: u16 },
    NegInt { dst: u16, src: u16 },
    AddFloat { dst: u16, a: u16, b: u16 },
    SubFloat { dst: u16, a: u16, b: u16 },
    MulFloat { dst: u16, a: u16, b: u16 },
    DivFloat { dst: u16, a: u16, b: u16 },
    FmaFloat { dst: u16, a: u16, b: u16, c: u16 },
    IntToFloat { dst: u16, src: u16 },
    FloatFloor { dst: u16, src: u16 },
    FloatRound { dst: u16, src: u16 },

    // --- Comparisons & Logic ---
    EqInt { dst: u16, a: u16, b: u16 },
    NeInt { dst: u16, a: u16, b: u16 },
    LtInt { dst: u16, a: u16, b: u16 },
    LeInt { dst: u16, a: u16, b: u16 },
    GtInt { dst: u16, a: u16, b: u16 },
    GeInt { dst: u16, a: u16, b: u16 },
    EqIntImm { dst: u16, a: u16, imm: i16 },
    LtIntImm { dst: u16, a: u16, imm: i16 },
    EqFloat { dst: u16, a: u16, b: u16 },
    LtFloat { dst: u16, a: u16, b: u16 },
    LeFloat { dst: u16, a: u16, b: u16 },
    NotBool { dst: u16, src: u16 },
    EqObj { dst_s: u16, a_obj: u16, b_obj: u16 },

    // --- Control Flow & Fast Loops ---
    Jump { target: u32 },
    JumpIfFalse { cond: u16, target: u32 },
    JumpIfTrue { cond: u16, target: u32 },
    JumpIfLtInt { a: u16, b: u16, target: u32, when_true: bool },
    JumpIfEqIntImm { a: u16, imm: i16, target: u32, when_true: bool },
    JumpIfLtIntImm { a: u16, imm: i16, target: u32, when_true: bool },
    ReturnScalarIfLtIntImm { a: u16, imm: i16, src: u16 },
    // Followed by the default Jump and one Jump per consecutive tag.
    SwitchIntImm { src: u16, first: i16, count: u16 },
    SwitchScalarTag { tag_s: u16, src_obj: u16, first_tag: u32, tag_count: u16 },
    MakeScalarEnum { dst_s: u16, schema_id: u32, a: u16, b: u16, len: u8 },
    BoxScalarEnum { dst_obj: u16, src_obj: u16, tag_s: u16, width: u8 },
    ReadScalarEnumPair { dst_a: u16, dst_b: u16, tag_s: u16, src_obj: u16, field_a: u8, field_b: u8 },
    ReadScalarEnumField { dst_s: u16, tag_s: u16, src_obj: u16, field_idx: u8 },
    SwitchTag { src_obj: u16, first_tag: u32, tag_count: u16 },
    AppendIntRangeTail { list_obj: u16, idx_s: u16, limit_s: u16 },
    ForRangeStep { idx_reg: u16, limit_reg: u16, target: u32 },

    // --- Calls & Closures ---
    CallSelfIntImm { arg_s: u16, arg_o: u16, src: u16, imm: i16 },
    CallDirect { func_id: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    CallCallable { callee_obj: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    MakeClosure { dst_obj: u16, proto_idx: u16 },
    LoadUpvalueScalar { dst: u16, idx: u16 },
    LoadUpvalueObj { dst: u16, idx: u16 },
    LoadFuncObj { dst_obj: u16, func_id: u16 },
    ReturnAddInt { a: u16, b: u16 },
    ReturnScalar { src: u16 },
    ReturnObj { src: u16 },
    ReturnVoid,

    // --- Collections & Records ---
    LoadConstObj { dst_obj: u16, idx: u16 },
    ToBytes { dst_obj: u16, src_obj: u16 },
    BytesToList { dst_obj: u16, src_obj: u16 },
    NewHttpRequest { dst_obj: u16, desc_idx: u16 },
    MoveObj { dst_obj: u16, src: u16 },
    MakeRangeList { dst_obj: u16, start_s: u16, end_s: u16 },
    NewListInt { dst_obj: u16, cap: u16 },
    NewListFloat { dst_obj: u16, cap: u16 },
    NewListObj { dst_obj: u16, cap: u16 },
    ListAppendInt { list_obj: u16, val_s: u16 },
    ListAppendFloat { list_obj: u16, val_s: u16 },
    ListAppendObj { list_obj: u16, val_obj: u16 },
    ListLen { dst_s: u16, list_obj: u16 },
    StrLen { dst_s: u16, str_obj: u16 },
    StrConcat { dst_obj: u16, a_obj: u16, b_obj: u16 },
    StrConcatInt { dst_obj: u16, prefix_obj: u16, src_s: u16 },
    ScalarToStr { dst_obj: u16, src_s: u16, tag: u8 },
    ObjToStr { dst_obj: u16, src_obj: u16 },
    StrMethod { dst_obj: u16, src_obj: u16, kind: u8 },
    StrReplace { dst_obj: u16, str_obj: u16, from_obj: u16, to_obj: u16 },
    ClampScalar { dst_s: u16, val_s: u16, lo_s: u16, hi_s: u16, is_float: bool },
    ListIndex { dst_obj: u16, list_obj: u16, idx_s: u16, elem_tag: u8 },
    ListGet { dst_obj: u16, list_obj: u16, idx_s: u16, elem_tag: u8 },
    ListContainsScalar { dst_s: u16, list_obj: u16, val_s: u16, elem_tag: u8 },
    ListContainsObj { dst_s: u16, list_obj: u16, val_obj: u16 },
    ListSlice { dst_obj: u16, list_obj: u16, start_s: u16, end_s: u16 },
    ListReverse { dst_obj: u16, list_obj: u16 },
    ListEnumerate { dst_obj: u16, list_obj: u16, elem_tag: u8 },
    ListJoin { dst_obj: u16, list_obj: u16, sep_obj: u16 },
    TupleGetScalar { dst_s: u16, tuple_obj: u16, idx: u16 },
    TupleGetObj { dst_obj: u16, tuple_obj: u16, idx: u16 },
    ForListInt { list_obj: u16, idx_s: u16, elem_s: u16, exit_target: u32 },
    SumListIntTail { list_obj: u16, idx_s: u16, elem_s: u16, acc_s: u16 },
    ForListIntStep { list_obj: u16, idx_s: u16, elem_s: u16, target: u32 },
    ForListObj { list_obj: u16, idx_s: u16, elem_obj: u16, exit_target: u32 },
    NewRecord { dst_obj: u16, desc_idx: u16 },
    NewRecordMove { dst_obj: u16, desc_idx: u16 },
    GetFieldScalar { dst_s: u16, rec_obj: u16, field_idx: u16 },
    GetFieldObj { dst_obj: u16, rec_obj: u16, field_idx: u16 },
    NewMap { dst_obj: u16 },
    MapSetIntInt { map_obj: u16, key_s: u16, val_s: u16 },
    MapGetIntIntOrImm { dst_s: u16, map_obj: u16, key_s: u16, default: i16 },
    AddIntMapGetOrImm { dst_s: u16, a: u16, map_obj: u16, key_s: u16, default: i16 },
    SumIntMapRangeTail { acc_s: u16, map_obj: u16, idx_s: u16, limit_s: u16, default: i16 },
    MapGetIntIntOr { dst_s: u16, map_obj: u16, key_s: u16, def_s: u16 },
    MapLen { dst_s: u16, map_obj: u16 },

    // --- Pipelines (map / filter / |>) ---
    RunPipeline { dst_obj: u16, src_obj: u16, plan_idx: u16 },

    // --- Native JSON ---
    JsonDecode { dst_obj: u16, src_obj: u16, plan_idx: u16 },
    JsonEncode { dst_obj: u16, src_obj: u16 },

    // --- Unions / ? / Type Matching ---
    BoxScalar { dst_obj: u16, src_s: u16, kind: u8 },
    UnboxScalar { dst_s: u16, src_obj: u16 },
    IsTypeObj { dst_s: u16, src_obj: u16, expected_tag: u32 },
    TryUnwrapScalar { dst_s: u16, src_obj: u16, ret_on_err: bool },
    TryCheckObj { src_obj: u16, ret_on_err: bool, empty_is_err: bool },

    // --- Native Time & Validation ---
    TimeOp { kind: u8, dst_obj: u16, arg_reg: u16 },
    ValidationOp { kind: u8, dst_obj: u16, a_reg: u16, b_reg: u16 },
    ValidationCheck { dst_obj: u16, src_obj: u16 },

    // --- Compact Extension Dispatch ---
    ExtOp { kind: u8, aux: u8, a: u16, b: u16, c: u16 },
}

pub const EXT_PRINT_SCALAR: u8 = 0;
pub const EXT_PRINT_OBJ: u8 = 1;
pub const EXT_FLOAT_CEIL: u8 = 2;
pub const EXT_STR_TO_INT: u8 = 3;
pub const EXT_STR_TO_FLOAT: u8 = 4;
pub const EXT_MAP_SET: u8 = 5;
pub const EXT_MAP_GET: u8 = 6;
pub const EXT_MAP_INDEX: u8 = 7;
pub const EXT_MAP_CONTAINS_KEY: u8 = 8;
pub const EXT_MAP_REMOVE: u8 = 9;
pub const EXT_MAP_KEYS: u8 = 10;
pub const EXT_MAP_VALUES: u8 = 11;
pub const EXT_MAP_ENTRIES: u8 = 12;
pub const EXT_MAP_FROM: u8 = 13;
pub const EXT_STR_IS_EMPTY: u8 = 14;
pub const EXT_STR_PRED: u8 = 15;
pub const EXT_STR_SPLIT: u8 = 16;
pub const EXT_INT_MATH: u8 = 17;
pub const EXT_FLOAT_MATH: u8 = 18;
pub const EXT_LIST_IS_EMPTY: u8 = 19;
pub const EXT_LIST_FIRST: u8 = 20;
pub const EXT_LIST_LAST: u8 = 21;
pub const EXT_LIST_SORT: u8 = 22;
pub const EXT_ENV_GET: u8 = 23;
pub const EXT_ENV_REQUIRE: u8 = 24;
pub const EXT_ENV_ARGS: u8 = 25;
pub const EXT_ENV_LOAD: u8 = 26;
pub const EXT_CLI_PARSE: u8 = 27;
pub const EXT_CLI_PARSE_ARGS: u8 = 28;
pub const EXT_CLI_HELP: u8 = 29;
pub const EXT_FS_UNARY: u8 = 30;
pub const EXT_FS_BINARY: u8 = 31;
pub const EXT_PATH_OP: u8 = 32;
pub const EXT_PROCESS_OP: u8 = 33;
pub const EXT_CRYPTO_OP: u8 = 34;
pub const EXT_PIPELINE_REDUCE: u8 = 35;
pub const EXT_URL_OP: u8 = 36;
pub const EXT_CSV_OP: u8 = 37;
pub const EXT_MATH_OP: u8 = 38;
pub const EXT_LOG_OP: u8 = 39;
pub const EXT_LIST_AGG: u8 = 40;
pub const EXT_HTTP_OP: u8 = 41;
pub const EXT_DB_OP: u8 = 42;
pub const EXT_CRON_OP: u8 = 43;
pub const EXT_ASSERT: u8 = 44;
pub const EXT_TIME_OP: u8 = 45;
pub const EXT_JWT_OP: u8 = 46;
pub const EXT_LOCK_OP: u8 = 47;
pub const EXT_CACHE_OP: u8 = 48;
pub const EXT_REDIS_OP: u8 = 49;
pub const EXT_SPAWN: u8 = 50;
pub const EXT_CHAN_OP: u8 = 51;
pub const EXT_APP_OP: u8 = 52;

#[cfg(test)]
mod tests {
    use super::Op;

    #[test]
    fn reciprocal_remainder_matches_signed_division() {
        use super::{remainder_by_constant, remainder_magic};
        for divisor in [3, 5, 6, 7, 97, 32767, 32769, 1_000_000_007, u32::MAX] {
            let (magic, shift) = remainder_magic(divisor).unwrap();
            let d = i64::from(divisor);
            let check = |n| assert_eq!(remainder_by_constant(n, divisor, magic, shift), n % d);
            for n in [i64::MIN, i64::MAX, -1, 0, 1] {
                check(n);
                let multiple = (n / d) * d;
                for offset in -2..=2 {
                    if let Some(value) = multiple.checked_add(offset) {
                        check(value);
                    }
                }
            }
            let mut bits = 1u64;
            for _ in 0..1024 {
                bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
                check(bits as i64);
            }
        }
    }

    #[test]
    fn op_size_is_compact() {
        assert!(
            std::mem::size_of::<Op>() <= 12,
            "Op size was {} bytes",
            std::mem::size_of::<Op>()
        );
    }
}
