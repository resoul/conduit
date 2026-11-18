/// Compact 3-address typed register instructions (<= 16 bytes, Copy).
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
    MulInt { dst: u16, a: u16, b: u16 },
    DivInt { dst: u16, a: u16, b: u16 },
    ModInt { dst: u16, a: u16, b: u16 },
    ModIntImm { dst: u16, a: u16, imm: i16 },
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
    ForRangeStep { idx_reg: u16, limit_reg: u16, target: u32 },

    // --- Calls & Closures ---
    CallDirect { func_id: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    CallCallable { callee_obj: u16, arg_base_s: u16, arg_base_o: u16, ret_reg: u16 },
    MakeClosure { dst_obj: u16, proto_idx: u16 },
    LoadUpvalueScalar { dst: u16, idx: u16 },
    LoadUpvalueObj { dst: u16, idx: u16 },
    LoadFuncObj { dst_obj: u16, func_id: u16 },
    ReturnScalar { src: u16 },
    ReturnObj { src: u16 },
    ReturnVoid,

    // --- Collections & Records ---
    LoadConstObj { dst_obj: u16, idx: u16 },
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
    ForListIntStep { list_obj: u16, idx_s: u16, elem_s: u16, target: u32 },
    ForListObj { list_obj: u16, idx_s: u16, elem_obj: u16, exit_target: u32 },
    NewRecord { dst_obj: u16, desc_idx: u16 },
    NewRecordMove { dst_obj: u16, desc_idx: u16 },
    GetFieldScalar { dst_s: u16, rec_obj: u16, field_idx: u16 },
    GetFieldObj { dst_obj: u16, rec_obj: u16, field_idx: u16 },
    NewMap { dst_obj: u16 },
    MapSetIntInt { map_obj: u16, key_s: u16, val_s: u16 },
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

#[cfg(test)]
mod tests {
    use super::Op;

    #[test]
    fn op_size_is_compact() {
        assert!(
            std::mem::size_of::<Op>() <= 16,
            "Op size was {} bytes",
            std::mem::size_of::<Op>()
        );
    }
}
