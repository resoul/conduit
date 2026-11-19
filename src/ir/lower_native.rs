use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::{native_member_field, scalar_kind};
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::ir::lower::FnLowerer;
use crate::ir::{FunctionProto, RecordDesc};
use crate::native::crypto::{
    CRYPTO_B64_DEC, CRYPTO_B64_ENC, CRYPTO_CONST_TIME_EQ, CRYPTO_HASH_PASSWORD, CRYPTO_HEX_DEC,
    CRYPTO_HEX_ENC, CRYPTO_HMAC_SHA256, CRYPTO_RAND_HEX, CRYPTO_RAND_INT, CRYPTO_SHA256,
    CRYPTO_UUID_V4, CRYPTO_VERIFY_PASSWORD,
};
use crate::native::fs::{
    FS_APPEND_TEXT, FS_COPY, FS_EACH_LINE, FS_EXISTS, FS_IS_DIR, FS_IS_FILE, FS_LIST_DIR,
    FS_MKDIR_ALL, FS_OPEN_APPEND, FS_OPEN_READ, FS_OPEN_WRITE, FS_READ_LINES, FS_READ_TEXT,
    FS_READER_CLOSE, FS_READER_READ_CHUNK, FS_READER_READ_LINE, FS_REMOVE, FS_RENAME, FS_SIZE,
    FS_WRITE_ATOMIC, FS_WRITE_TEXT, FS_WRITER_CLOSE, FS_WRITER_FLUSH, FS_WRITER_WRITE,
    FS_WRITER_WRITE_LINE, FS_WRITER_WRITE_LINES,
};
use crate::native::path::{
    PATH_BASENAME, PATH_DIRNAME, PATH_EXT, PATH_IS_ABS, PATH_JOIN, PATH_JOIN_ALL, PATH_NORMALIZE,
    PATH_STEM, PATH_WITH_EXT,
};
use crate::native::process::{
    PROC_CWD, PROC_EXIT, PROC_PID, PROC_RUN, PROC_SET_CWD, PROC_STATUS,
};
use crate::native::time::{TIME_ADD, TIME_DIFF, TIME_SUB, TimeOpKind};
use crate::native::url::{URL_DECODE, URL_DECODE_QUERY, URL_ENCODE, URL_ENCODE_QUERY, URL_PARSE};
use crate::native::validation::ValOpKind;
use crate::vm::opcode::{
    EXT_CLI_HELP, EXT_CLI_PARSE, EXT_CLI_PARSE_ARGS, EXT_CRYPTO_OP, EXT_ENV_ARGS, EXT_ENV_GET,
    EXT_ENV_LOAD, EXT_ENV_REQUIRE, EXT_FS_BINARY, EXT_FS_UNARY, EXT_LIST_FIRST, EXT_LIST_LAST,
    EXT_LIST_SORT, EXT_MAP_ENTRIES, EXT_MAP_GET, EXT_MAP_KEYS, EXT_MAP_REMOVE, EXT_MAP_VALUES,
    EXT_PATH_OP, EXT_PROCESS_OP, EXT_STR_SPLIT, EXT_STR_TO_FLOAT, EXT_STR_TO_INT, EXT_TIME_OP,
    EXT_URL_OP, EXT_CSV_OP, EXT_DB_OP, EXT_CRON_OP, EXT_MATH_OP, EXT_LOG_OP, EXT_HTTP_OP, Op,
};
use crate::vm::value::{
    SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT, SCHEMA_CACHE_ERROR,
    SCHEMA_CACHE_HANDLE, SCHEMA_CLI_ERROR, SCHEMA_CRON_ERROR, SCHEMA_CRON_SCHEDULER,
    SCHEMA_CRYPTO_ERROR, SCHEMA_CSV_ERROR, SCHEMA_DB_CONN, SCHEMA_DB_ERROR, SCHEMA_DB_PARAM,
    SCHEMA_EMPTY_TAG, SCHEMA_ENV_ERROR, SCHEMA_FS_ERROR, SCHEMA_FS_READER, SCHEMA_FS_WRITER,
    SCHEMA_HTTP_BINARY_RESPONSE, SCHEMA_HTTP_ERROR, SCHEMA_HTTP_REQUEST, SCHEMA_HTTP_RESPONSE,
    SCHEMA_HTTP_ROUTER, SCHEMA_INDEX_ERROR, SCHEMA_JSON_DECODE_ERROR, SCHEMA_JSON_ENCODE_ERROR,
    SCHEMA_JWT_ERROR, SCHEMA_KEY_ERROR, SCHEMA_LIST_TAG, SCHEMA_LOCK_ERROR, SCHEMA_LOCK_GUARD,
    SCHEMA_MAP_TAG, SCHEMA_PARSE_ERROR, SCHEMA_PROCESS_ERROR, SCHEMA_PROCESS_OUTPUT,
    SCHEMA_REDIS_CLIENT, SCHEMA_REDIS_ERROR, SCHEMA_REDIS_LOCK, SCHEMA_STR_TAG,
    SCHEMA_TIME_DURATION, SCHEMA_TIME_ERROR, SCHEMA_TIME_INSTANT, SCHEMA_TIME_TIMESTAMP,
    SCHEMA_URL, SCHEMA_URL_ERROR, SCHEMA_VALIDATION_ERROR, SCHEMA_VALIDATION_ISSUE,
    SCHEMA_VALIDATION_VIOLATION,
};

#[inline]
pub(crate) fn elem_tag_for(ty: &Type) -> u8 {
    match ty {
        Type::Int | Type::Enum(_) => 1,
        Type::Float => 2,
        Type::Bool => 3,
        _ => 0,
    }
}

pub(crate) fn type_tag_for_switch(ty: &Type, checked: &CheckedProgram) -> Result<u32, String> {
    match ty {
        Type::Void | Type::Nil => Ok(SCHEMA_EMPTY_TAG),
        Type::Int | Type::Enum(_) => Ok(SCHEMA_BOXED_INT),
        Type::Float => Ok(SCHEMA_BOXED_FLOAT),
        Type::Bool => Ok(SCHEMA_BOXED_BOOL),
        Type::String => Ok(SCHEMA_STR_TAG),
        Type::List(_) => Ok(SCHEMA_LIST_TAG),
        Type::Map(_, _) => Ok(SCHEMA_MAP_TAG),
        Type::Custom(tid) => match &tid.module {
            ModuleId::Builtin if tid.name == "IndexError" => Ok(SCHEMA_INDEX_ERROR),
            ModuleId::Builtin if tid.name == "KeyError" => Ok(SCHEMA_KEY_ERROR),
            ModuleId::Builtin if tid.name == "ParseError" => Ok(SCHEMA_PARSE_ERROR),
            ModuleId::Program => checked
                .struct_by_name
                .get(&tid.name)
                .map(|&id| checked.structs[id].schema_id)
                .ok_or_else(|| format!("Unknown struct '{}' in switch pattern", tid.name)),
            ModuleId::Native(m) => match (m.as_str(), tid.name.as_str()) {
                ("cdt.time", "Timestamp") => Ok(SCHEMA_TIME_TIMESTAMP),
                ("cdt.time", "Instant") => Ok(SCHEMA_TIME_INSTANT),
                ("cdt.time", "Duration") => Ok(SCHEMA_TIME_DURATION),
                ("cdt.time", "TimeError") => Ok(SCHEMA_TIME_ERROR),
                ("cdt.validation", "Issue") => Ok(SCHEMA_VALIDATION_ISSUE),
                ("cdt.validation", "Violation") => Ok(SCHEMA_VALIDATION_VIOLATION),
                ("cdt.validation", "ValidationError") => Ok(SCHEMA_VALIDATION_ERROR),
                ("cdt.json", "DecodeError") => Ok(SCHEMA_JSON_DECODE_ERROR),
                ("cdt.json", "EncodeError") => Ok(SCHEMA_JSON_ENCODE_ERROR),
                ("cdt.env", "EnvError") => Ok(SCHEMA_ENV_ERROR),
                ("cdt.cli", "CliError") => Ok(SCHEMA_CLI_ERROR),
                ("cdt.fs", "FsError") => Ok(SCHEMA_FS_ERROR),
                ("cdt.fs", "Reader") => Ok(SCHEMA_FS_READER),
                ("cdt.fs", "Writer") => Ok(SCHEMA_FS_WRITER),
                ("cdt.process", "ProcessError") => Ok(SCHEMA_PROCESS_ERROR),
                ("cdt.process", "Output") => Ok(SCHEMA_PROCESS_OUTPUT),
                ("cdt.crypto", "CryptoError") => Ok(SCHEMA_CRYPTO_ERROR),
                ("cdt.url", "UrlError") => Ok(SCHEMA_URL_ERROR),
                ("cdt.url", "Url") => Ok(SCHEMA_URL),
                ("cdt.csv", "CsvError") => Ok(SCHEMA_CSV_ERROR),
                ("cdt.http", "HttpError") => Ok(SCHEMA_HTTP_ERROR),
                ("cdt.http", "Request") => Ok(SCHEMA_HTTP_REQUEST),
                ("cdt.http", "Response") => Ok(SCHEMA_HTTP_RESPONSE),
                ("cdt.http", "BinaryResponse") => Ok(SCHEMA_HTTP_BINARY_RESPONSE),
                ("cdt.http", "Router") => Ok(SCHEMA_HTTP_ROUTER),
                ("cdt.db", "DbError") => Ok(SCHEMA_DB_ERROR),
                ("cdt.db", "Connection") => Ok(SCHEMA_DB_CONN),
                ("cdt.db", "Param") => Ok(SCHEMA_DB_PARAM),
                ("cdt.cron", "CronError") => Ok(SCHEMA_CRON_ERROR),
                ("cdt.cron", "Scheduler") => Ok(SCHEMA_CRON_SCHEDULER),
                ("cdt.jwt", "JwtError") => Ok(SCHEMA_JWT_ERROR),
                ("cdt.lock", "LockError") => Ok(SCHEMA_LOCK_ERROR),
                ("cdt.lock", "Guard") => Ok(SCHEMA_LOCK_GUARD),
                ("cdt.cache", "CacheError") => Ok(SCHEMA_CACHE_ERROR),
                ("cdt.cache", "Cache") => Ok(SCHEMA_CACHE_HANDLE),
                ("cdt.redis", "RedisError") => Ok(SCHEMA_REDIS_ERROR),
                ("cdt.redis", "Client") => Ok(SCHEMA_REDIS_CLIENT),
                ("cdt.redis", "Lock") => Ok(SCHEMA_REDIS_LOCK),
                _ => Err(format!("Unsupported native type '{}' in switch pattern", tid)),
            },
            _ => Err(format!("Unsupported type '{}' in switch pattern", tid)),
        },
        other => Err(format!("Unsupported type '{}' in switch pattern", other)),
    }
}

impl<'a, 'b> FnLowerer<'a, 'b> {
    pub(crate) fn lower_time_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "now" => {
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: TimeOpKind::Now as u8,
                    dst_obj,
                    arg_reg: 0,
                });
                Ok(dst_obj)
            }
            "monotonic" => {
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: TimeOpKind::Monotonic as u8,
                    dst_obj,
                    arg_reg: 0,
                });
                Ok(dst_obj)
            }
            "elapsed" => {
                let arg_reg = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: TimeOpKind::Elapsed as u8,
                    dst_obj,
                    arg_reg,
                });
                Ok(dst_obj)
            }
            "seconds" => {
                let arg_reg = self.lower_scalar(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: TimeOpKind::Seconds as u8,
                    dst_obj,
                    arg_reg,
                });
                Ok(dst_obj)
            }
            "milliseconds" | "sleep" | "from_unix" | "from_unix_millis" => {
                let kind = match sym {
                    "sleep" => TimeOpKind::Sleep,
                    "from_unix" => TimeOpKind::FromUnix,
                    "from_unix_millis" => TimeOpKind::FromUnixMillis,
                    _ => TimeOpKind::Milliseconds,
                };
                let arg_reg = self.lower_scalar(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: kind as u8,
                    dst_obj,
                    arg_reg,
                });
                Ok(dst_obj)
            }
            "parse_iso" | "format_iso" => {
                let kind = if sym == "parse_iso" {
                    TimeOpKind::ParseIso
                } else {
                    TimeOpKind::FormatIso
                };
                let arg_reg = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: kind as u8,
                    dst_obj,
                    arg_reg,
                });
                Ok(dst_obj)
            }
            "TimeError" => self.lower_native_record(SCHEMA_TIME_ERROR, &["message"], args, named_args),
            other => Err(format!("Unsupported cdt.time symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_validation_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "email" | "url" | "uuid" | "not_empty" => {
                let kind = match sym {
                    "email" => ValOpKind::Email,
                    "url" => ValOpKind::Url,
                    "uuid" => ValOpKind::Uuid,
                    _ => ValOpKind::NotEmpty,
                };
                let a_reg = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: kind as u8,
                    dst_obj,
                    a_reg,
                    b_reg: 0,
                });
                Ok(dst_obj)
            }
            "min_len" | "max_len" => {
                let kind = if sym == "min_len" {
                    ValOpKind::MinObj
                } else {
                    ValOpKind::MaxObj
                };
                let a_reg = self.lower_obj(&args[0])?;
                let b_reg = self.lower_scalar(&args[1])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: kind as u8,
                    dst_obj,
                    a_reg,
                    b_reg,
                });
                Ok(dst_obj)
            }
            "one_of" => {
                let a_reg = self.lower_obj(&args[0])?;
                let b_reg = self.lower_obj(&args[1])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: ValOpKind::OneOf as u8,
                    dst_obj,
                    a_reg,
                    b_reg,
                });
                Ok(dst_obj)
            }
            "range" => {
                let a_reg = self.lower_scalar(&args[0])?;
                let lo_reg = self.lower_scalar(&args[1])?;
                let hi_reg = self.lower_scalar(&args[2])?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_VALIDATION_ISSUE,
                    scalar_regs: Box::new([lo_reg, hi_reg]),
                    obj_regs: Box::new([]),
                })?;
                let b_reg = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj: b_reg, desc_idx });
                let kind = if args[0].ty.is_float() {
                    ValOpKind::RangeFloat
                } else {
                    ValOpKind::RangeInt
                };
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: kind as u8,
                    dst_obj,
                    a_reg,
                    b_reg,
                });
                Ok(dst_obj)
            }
            "min" | "max" => {
                let is_min = sym == "min";
                let val = &args[0];
                let bound = &args[1];
                let (kind, a_reg, b_reg) = match &val.ty {
                    Type::Int => (
                        if is_min { ValOpKind::MinInt } else { ValOpKind::MaxInt },
                        self.lower_scalar(val)?,
                        self.lower_scalar(bound)?,
                    ),
                    Type::Float => (
                        if is_min { ValOpKind::MinFloat } else { ValOpKind::MaxFloat },
                        self.lower_scalar(val)?,
                        self.lower_scalar(bound)?,
                    ),
                    Type::String | Type::List(_) | Type::Map(_, _) => (
                        if is_min { ValOpKind::MinObj } else { ValOpKind::MaxObj },
                        self.lower_obj(val)?,
                        self.lower_scalar(bound)?,
                    ),
                    _ => (
                        if is_min { ValOpKind::MinDyn } else { ValOpKind::MaxDyn },
                        self.lower_obj(val)?,
                        self.lower_obj(bound)?,
                    ),
                };
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: kind as u8,
                    dst_obj,
                    a_reg,
                    b_reg,
                });
                Ok(dst_obj)
            }
            "check" => {
                let src_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationCheck { dst_obj, src_obj });
                Ok(dst_obj)
            }
            "Issue" => self.lower_native_record(SCHEMA_VALIDATION_ISSUE, &["code", "message"], args, named_args),
            "Violation" => self.lower_native_record(
                SCHEMA_VALIDATION_VIOLATION,
                &["path", "code", "message"],
                args,
                named_args,
            ),
            "ValidationError" => {
                self.lower_native_record(SCHEMA_VALIDATION_ERROR, &["issues"], args, named_args)
            }
            other => Err(format!("Unsupported cdt.validation symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_env_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "EnvError" => self.lower_native_record(SCHEMA_ENV_ERROR, &["name", "message"], args, named_args),
            "get" | "require" => {
                let name_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                let kind = if sym == "get" { EXT_ENV_GET } else { EXT_ENV_REQUIRE };
                self.ops.push(Op::ExtOp {
                    kind,
                    aux: 0,
                    a: dst_obj,
                    b: name_obj,
                    c: 0,
                });
                Ok(dst_obj)
            }
            "args" => {
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_ENV_ARGS,
                    aux: 0,
                    a: dst_obj,
                    b: 0,
                    c: 0,
                });
                Ok(dst_obj)
            }
            "load" => {
                let ty = self.mc.checked.resolve_type(&type_args[0])?;
                let Type::Custom(tid) = ty else {
                    return Err("env.load<T>() requires a struct type".to_string());
                };
                let sid = self.mc.checked.struct_by_name[&tid.name];
                let schema_id = self.mc.checked.structs[sid].schema_id;
                let plan_idx = self
                    .mc
                    .checked
                    .env_plans
                    .iter()
                    .position(|p| p.schema_id == schema_id)
                    .ok_or_else(|| format!("Missing EnvLoadPlan for '{}'", tid.name))?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_ENV_LOAD,
                    aux: 0,
                    a: dst_obj,
                    b: plan_idx as u16,
                    c: 0,
                });
                Ok(dst_obj)
            }
            other => Err(format!("Unsupported cdt.env symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_cli_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "CliError" => self.lower_native_record(SCHEMA_CLI_ERROR, &["kind", "message"], args, named_args),
            "parse" | "parse_args" | "help" => {
                let ty = self.mc.checked.resolve_type(&type_args[0])?;
                let Type::Custom(tid) = ty else {
                    return Err(format!("cli.{}<T>() requires a struct type", sym));
                };
                let plan_idx = self
                    .mc
                    .checked
                    .cli_plans
                    .iter()
                    .position(|(name, _)| name == &tid.name)
                    .ok_or_else(|| format!("Missing CliPlan for '{}'", tid.name))?;
                let (kind, c_reg) = match sym {
                    "parse" => (EXT_CLI_PARSE, 0),
                    "help" => (EXT_CLI_HELP, 0),
                    _ => (EXT_CLI_PARSE_ARGS, self.lower_obj(&args[0])?),
                };
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind,
                    aux: 0,
                    a: dst_obj,
                    b: plan_idx as u16,
                    c: c_reg,
                });
                Ok(dst_obj)
            }
            "dispatch" | "dispatch_args" => {
                let ty = self.mc.checked.resolve_type(&type_args[0])?;
                let Type::Custom(tid) = ty else {
                    return Err(format!("cli.{}<T>() requires a struct type", sym));
                };
                let plan_idx = self
                    .mc
                    .checked
                    .cli_plans
                    .iter()
                    .position(|(name, _)| name == &tid.name)
                    .ok_or_else(|| format!("Missing CliPlan for '{}'", tid.name))?;
                let routes = self
                    .mc
                    .checked
                    .cli_dispatches
                    .iter()
                    .find(|(name, _)| name == &tid.name)
                    .map(|(_, r)| r.clone())
                    .ok_or_else(|| format!("Missing CliDispatch for '{}'", tid.name))?;

                let (kind, c_reg) = if sym == "dispatch" {
                    (EXT_CLI_PARSE, 0)
                } else {
                    (EXT_CLI_PARSE_ARGS, self.lower_obj(&args[0])?)
                };
                let parsed_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind,
                    aux: 0,
                    a: parsed_o,
                    b: plan_idx as u16,
                    c: c_reg,
                });

                let dst_obj = self.alloc_temp_o();
                let is_err_s = self.alloc_temp_s();
                self.ops.push(Op::IsTypeObj {
                    dst_s: is_err_s,
                    src_obj: parsed_o,
                    expected_tag: SCHEMA_CLI_ERROR,
                });
                let err_jump = self.ops.len();
                self.ops.push(Op::JumpIfFalse { cond: is_err_s, target: 0 });
                self.ops.push(Op::MoveObj { dst_obj, src: parsed_o });
                let mut end_jumps = Vec::new();
                end_jumps.push(self.ops.len());
                self.ops.push(Op::Jump { target: 0 });
                let start_dispatch = self.ops.len() as u32;
                if let Op::JumpIfFalse { target, .. } = &mut self.ops[err_jump] {
                    *target = start_dispatch;
                }

                let is_empty_s = self.alloc_temp_s();
                for route in &routes {
                    let mut cur_o = parsed_o;
                    let mut skip_jumps = Vec::new();
                    for &slot in &route.field_slot_path {
                        let next_o = self.alloc_temp_o();
                        self.ops.push(Op::GetFieldObj {
                            dst_obj: next_o,
                            rec_obj: cur_o,
                            field_idx: slot,
                        });
                        self.ops.push(Op::IsTypeObj {
                            dst_s: is_empty_s,
                            src_obj: next_o,
                            expected_tag: SCHEMA_EMPTY_TAG,
                        });
                        skip_jumps.push(self.ops.len());
                        self.ops.push(Op::JumpIfTrue { cond: is_empty_s, target: 0 });
                        cur_o = next_o;
                    }

                    let arg_base_s = self.alloc_window_s(0);
                    let arg_base_o = self.alloc_window_o(if route.takes_parent { 2 } else { 1 });
                    if route.takes_parent {
                        self.ops.push(Op::MoveObj { dst_obj: arg_base_o, src: parsed_o });
                        self.ops.push(Op::MoveObj { dst_obj: arg_base_o + 1, src: cur_o });
                    } else {
                        self.ops.push(Op::MoveObj { dst_obj: arg_base_o, src: cur_o });
                    }

                    let fid = *self
                        .mc
                        .checked
                        .func_by_name
                        .get(&route.handler_name)
                        .ok_or_else(|| format!("Unknown CLI handler '{}'", route.handler_name))?;
                    if route.ret_type == Type::Void {
                        let dummy_s = self.alloc_temp_s();
                        self.ops.push(Op::CallDirect {
                            func_id: fid as u16,
                            arg_base_s,
                            arg_base_o,
                            ret_reg: dummy_s,
                        });
                        let empty_idx = self.mc.intern_empty()?;
                        self.ops.push(Op::LoadConstObj { dst_obj, idx: empty_idx });
                    } else if route.ret_type.is_scalar() {
                        let ret_s = self.alloc_temp_s();
                        self.ops.push(Op::CallDirect {
                            func_id: fid as u16,
                            arg_base_s,
                            arg_base_o,
                            ret_reg: ret_s,
                        });
                        let box_kind = match route.ret_type {
                            Type::Int => 0,
                            Type::Float => 1,
                            _ => 2,
                        };
                        self.ops.push(Op::BoxScalar {
                            dst_obj,
                            src_s: ret_s,
                            kind: box_kind,
                        });
                    } else {
                        self.ops.push(Op::CallDirect {
                            func_id: fid as u16,
                            arg_base_s,
                            arg_base_o,
                            ret_reg: dst_obj,
                        });
                    }

                    end_jumps.push(self.ops.len());
                    self.ops.push(Op::Jump { target: 0 });
                    let next_pc = self.ops.len() as u32;
                    for sj in skip_jumps {
                        if let Op::JumpIfTrue { target, .. } = &mut self.ops[sj] {
                            *target = next_pc;
                        }
                    }
                }

                let empty_idx = self.mc.intern_empty()?;
                self.ops.push(Op::LoadConstObj { dst_obj, idx: empty_idx });
                let end_pc = self.ops.len() as u32;
                for ej in end_jumps {
                    if let Op::Jump { target } = &mut self.ops[ej] {
                        *target = end_pc;
                    }
                }
                Ok(dst_obj)
            }
            other => Err(format!("Unsupported cdt.cli symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_native_record(
        &mut self,
        schema_id: u32,
        fields: &[&str],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        let mut o_regs = Vec::with_capacity(fields.len());
        if !named_args.is_empty() {
            for fname in fields {
                let na = named_args
                    .iter()
                    .find(|n| n.name == *fname)
                    .ok_or_else(|| format!("Missing field '{}' in constructor", fname))?;
                o_regs.push(self.lower_obj(&na.value)?);
            }
        } else {
            for arg in args {
                o_regs.push(self.lower_obj(arg)?);
            }
        }
        let desc_idx = self.mc.intern_record_desc(RecordDesc {
            schema_id,
            scalar_regs: Box::new([]),
            obj_regs: o_regs.into_boxed_slice(),
        })?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::NewRecord { dst_obj, desc_idx });
        Ok(dst_obj)
    }

    pub(crate) fn lower_fs_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "FsError" => self.lower_native_record(
                SCHEMA_FS_ERROR,
                &["kind", "path", "message"],
                args,
                named_args,
            ),
            "exists" | "is_file" | "is_dir" => {
                let aux = match sym {
                    "exists" => FS_EXISTS,
                    "is_file" => FS_IS_FILE,
                    "is_dir" => FS_IS_DIR,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux,
                    a: dst_s,
                    b,
                    c: 0,
                });
                Ok(dst_s)
            }
            "read_text" | "read_lines" | "size" | "list_dir" | "mkdir_all" | "remove"
            | "open_read" | "open_write" | "open_append" => {
                let aux = match sym {
                    "read_text" => FS_READ_TEXT,
                    "read_lines" => FS_READ_LINES,
                    "size" => FS_SIZE,
                    "list_dir" => FS_LIST_DIR,
                    "mkdir_all" => FS_MKDIR_ALL,
                    "remove" => FS_REMOVE,
                    "open_read" => FS_OPEN_READ,
                    "open_write" => FS_OPEN_WRITE,
                    "open_append" => FS_OPEN_APPEND,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let dst_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux,
                    a: dst_o,
                    b,
                    c: 0,
                });
                Ok(dst_o)
            }
            "write_text" | "append_text" | "write_atomic" | "copy" | "rename" | "each_line" => {
                let aux = match sym {
                    "write_text" => FS_WRITE_TEXT,
                    "append_text" => FS_APPEND_TEXT,
                    "write_atomic" => FS_WRITE_ATOMIC,
                    "copy" => FS_COPY,
                    "rename" => FS_RENAME,
                    "each_line" => FS_EACH_LINE,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let dst_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_BINARY,
                    aux,
                    a: dst_o,
                    b,
                    c,
                });
                Ok(dst_o)
            }
            other => Err(format!("Unsupported cdt.fs symbol '{}'", other)),
        }
    }

    fn lower_fs_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<u16, String> {
        let b = self.lower_obj(target)?;
        let dst_o = self.alloc_temp_o();
        match (type_name, method) {
            ("Reader", "read_line") => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux: FS_READER_READ_LINE,
                    a: dst_o,
                    b,
                    c: 0,
                });
            }
            ("Reader", "close") => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux: FS_READER_CLOSE,
                    a: dst_o,
                    b,
                    c: 0,
                });
            }
            ("Writer", "flush") => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux: FS_WRITER_FLUSH,
                    a: dst_o,
                    b,
                    c: 0,
                });
            }
            ("Writer", "close") => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_UNARY,
                    aux: FS_WRITER_CLOSE,
                    a: dst_o,
                    b,
                    c: 0,
                });
            }
            ("Reader", "read_chunk") => {
                let c = self.lower_scalar(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_BINARY,
                    aux: FS_READER_READ_CHUNK,
                    a: dst_o,
                    b,
                    c,
                });
            }
            ("Writer", "write") => {
                let c = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_BINARY,
                    aux: FS_WRITER_WRITE,
                    a: dst_o,
                    b,
                    c,
                });
            }
            ("Writer", "write_line" | "write_lines") => {
                let c = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_FS_BINARY,
                    aux: if method == "write_lines" {
                        FS_WRITER_WRITE_LINES
                    } else {
                        FS_WRITER_WRITE_LINE
                    },
                    a: dst_o,
                    b,
                    c,
                });
            }
            _ => return Err(format!("Unsupported cdt.fs method '{}.{}()'", type_name, method)),
        }
        Ok(dst_o)
    }

    pub(crate) fn lower_path_call(&mut self, sym: &str, args: &[Expr]) -> Result<u16, String> {
        match sym {
            "join" | "with_ext" => {
                let aux = if sym == "join" { PATH_JOIN } else { PATH_WITH_EXT };
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_PATH_OP, aux, a, b, c });
                Ok(a)
            }
            "is_abs" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_s();
                self.ops.push(Op::ExtOp { kind: EXT_PATH_OP, aux: PATH_IS_ABS, a, b, c: 0 });
                Ok(a)
            }
            "join_all" | "dirname" | "basename" | "ext" | "stem" | "normalize" => {
                let aux = match sym {
                    "join_all" => PATH_JOIN_ALL,
                    "dirname" => PATH_DIRNAME,
                    "basename" => PATH_BASENAME,
                    "ext" => PATH_EXT,
                    "stem" => PATH_STEM,
                    "normalize" => PATH_NORMALIZE,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_PATH_OP, aux, a, b, c: 0 });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.path symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_process_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "ProcessError" => {
                self.lower_native_record(SCHEMA_PROCESS_ERROR, &["kind", "message"], args, named_args)
            }
            "Output" => {
                let (code_expr, out_expr, err_expr) = if !named_args.is_empty() {
                    let c = &named_args.iter().find(|n| n.name == "code").unwrap().value;
                    let o = &named_args.iter().find(|n| n.name == "stdout").unwrap().value;
                    let e = &named_args.iter().find(|n| n.name == "stderr").unwrap().value;
                    (c, o, e)
                } else {
                    (&args[0], &args[1], &args[2])
                };
                let code_s = self.lower_scalar(code_expr)?;
                let out_o = self.lower_obj(out_expr)?;
                let err_o = self.lower_obj(err_expr)?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_PROCESS_OUTPUT,
                    scalar_regs: Box::new([code_s]),
                    obj_regs: Box::new([out_o, err_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "run" | "status" => {
                let aux = if sym == "run" { PROC_RUN } else { PROC_STATUS };
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_PROCESS_OP, aux, a, b, c });
                Ok(a)
            }
            "cwd" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_PROCESS_OP, aux: PROC_CWD, a, b: 0, c: 0 });
                Ok(a)
            }
            "set_cwd" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_PROCESS_OP, aux: PROC_SET_CWD, a, b, c: 0 });
                Ok(a)
            }
            "pid" => {
                let a = self.alloc_temp_s();
                self.ops.push(Op::ExtOp { kind: EXT_PROCESS_OP, aux: PROC_PID, a, b: 0, c: 0 });
                Ok(a)
            }
            "exit" => {
                let a = self.lower_scalar(&args[0])?;
                self.ops.push(Op::ExtOp { kind: EXT_PROCESS_OP, aux: PROC_EXIT, a, b: 0, c: 0 });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.process symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_crypto_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "CryptoError" => {
                self.lower_native_record(SCHEMA_CRYPTO_ERROR, &["kind", "message"], args, named_args)
            }
            "sha256" | "base64_encode" | "base64_decode" | "hex_encode" | "hex_decode" => {
                let aux = match sym {
                    "sha256" => CRYPTO_SHA256,
                    "base64_encode" => CRYPTO_B64_ENC,
                    "base64_decode" => CRYPTO_B64_DEC,
                    "hex_encode" => CRYPTO_HEX_ENC,
                    "hex_decode" => CRYPTO_HEX_DEC,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_CRYPTO_OP, aux, a, b, c: 0 });
                Ok(a)
            }
            "hmac_sha256" => {
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRYPTO_OP,
                    aux: CRYPTO_HMAC_SHA256,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            "uuid_v4" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_CRYPTO_OP, aux: CRYPTO_UUID_V4, a, b: 0, c: 0 });
                Ok(a)
            }
            "random_bytes_hex" => {
                let b = self.lower_scalar(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRYPTO_OP,
                    aux: CRYPTO_RAND_HEX,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "random_int" => {
                let b = self.lower_scalar(&args[0])?;
                let c = self.lower_scalar(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRYPTO_OP,
                    aux: CRYPTO_RAND_INT,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            "hash_password" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRYPTO_OP,
                    aux: CRYPTO_HASH_PASSWORD,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "verify_password" | "constant_time_eq" => {
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_s();
                let aux = if sym == "verify_password" {
                    CRYPTO_VERIFY_PASSWORD
                } else {
                    CRYPTO_CONST_TIME_EQ
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRYPTO_OP,
                    aux,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.crypto symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_url_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "UrlError" => {
                self.lower_native_record(SCHEMA_URL_ERROR, &["kind", "message"], args, named_args)
            }
            "Url" => {
                let get_expr = |idx: usize, name: &str| -> &Expr {
                    if !named_args.is_empty() {
                        &named_args.iter().find(|n| n.name == name).unwrap().value
                    } else {
                        &args[idx]
                    }
                };
                let scheme_o = self.lower_obj(get_expr(0, "scheme"))?;
                let host_o = self.lower_obj(get_expr(1, "host"))?;
                let port_s = self.lower_scalar(get_expr(2, "port"))?;
                let path_o = self.lower_obj(get_expr(3, "path"))?;
                let query_o = self.lower_obj(get_expr(4, "query"))?;
                let frag_o = self.lower_obj(get_expr(5, "fragment"))?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_URL,
                    scalar_regs: Box::new([port_s]),
                    obj_regs: Box::new([scheme_o, host_o, path_o, query_o, frag_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "parse" | "encode" | "decode" | "encode_query" | "decode_query" => {
                let aux = match sym {
                    "parse" => URL_PARSE,
                    "encode" => URL_ENCODE,
                    "decode" => URL_DECODE,
                    "encode_query" => URL_ENCODE_QUERY,
                    "decode_query" => URL_DECODE_QUERY,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_URL_OP, aux, a, b, c: 0 });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.url symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_csv_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "CsvError" => {
                let get_expr = |idx: usize, name: &str| -> &Expr {
                    if !named_args.is_empty() {
                        &named_args.iter().find(|n| n.name == name).unwrap().value
                    } else {
                        &args[idx]
                    }
                };
                let kind_o = self.lower_obj(get_expr(0, "kind"))?;
                let line_s = self.lower_scalar(get_expr(1, "line"))?;
                let msg_o = self.lower_obj(get_expr(2, "message"))?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_CSV_ERROR,
                    scalar_regs: Box::new([line_s]),
                    obj_regs: Box::new([kind_o, msg_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "parse" | "parse_records" | "stringify" => {
                let aux = match sym {
                    "parse" => 0,
                    "parse_records" => 2,
                    "stringify" => 3,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_CSV_OP, aux, a, b, c: 0 });
                Ok(a)
            }
            "parse_with" | "stringify_with" => {
                let aux = if sym == "parse_with" { 1 } else { 4 };
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_CSV_OP, aux, a, b, c });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.csv symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_math_call(&mut self, sym: &str, args: &[Expr]) -> Result<u16, String> {
        use crate::native::math::*;
        let aux = match sym {
            "pi" => MATH_PI,
            "e" => MATH_E,
            "tau" => MATH_TAU,
            "sqrt" => MATH_SQRT,
            "sin" => MATH_SIN,
            "cos" => MATH_COS,
            "tan" => MATH_TAN,
            "asin" => MATH_ASIN,
            "acos" => MATH_ACOS,
            "atan" => MATH_ATAN,
            "exp" => MATH_EXP,
            "ln" => MATH_LN,
            "log10" => MATH_LOG10,
            "floor" => MATH_FLOOR,
            "ceil" => MATH_CEIL,
            "round" => MATH_ROUND,
            "trunc" => MATH_TRUNC,
            "pow" => MATH_POW,
            "atan2" => MATH_ATAN2,
            "hypot" => MATH_HYPOT,
            other => return Err(format!("Unsupported cdt.math symbol '{}'", other)),
        };
        let b = if aux >= 3 { self.lower_scalar(&args[0])? } else { 0 };
        let c = if aux >= 17 { self.lower_scalar(&args[1])? } else { 0 };
        let a = self.alloc_temp_s();
        self.ops.push(Op::ExtOp { kind: EXT_MATH_OP, aux, a, b, c });
        Ok(a)
    }

    pub(crate) fn lower_log_call(&mut self, sym: &str, args: &[Expr]) -> Result<u16, String> {
        use crate::native::log::*;
        match sym {
            "debug" | "info" | "warn" | "error" | "set_level" => {
                let aux = match sym {
                    "debug" => LOG_DEBUG,
                    "info" => LOG_INFO,
                    "warn" => LOG_WARN,
                    "error" => LOG_ERROR,
                    "set_level" => LOG_SET_LEVEL,
                    _ => unreachable!(),
                };
                let b = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp { kind: EXT_LOG_OP, aux, a: 0, b, c: 0 });
                Ok(0)
            }
            "set_json" => {
                let b = self.lower_scalar(&args[0])?;
                self.ops.push(Op::ExtOp { kind: EXT_LOG_OP, aux: LOG_SET_JSON, a: 0, b, c: 0 });
                Ok(0)
            }
            "get_level" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_LOG_OP, aux: LOG_GET_LEVEL, a, b: 0, c: 0 });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.log symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_http_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        use crate::vm::exec_http::*;
        let get_expr = |idx: usize, name: &str| -> &Expr {
            if !named_args.is_empty() {
                &named_args.iter().find(|n| n.name == name).unwrap().value
            } else {
                &args[idx]
            }
        };
        match sym {
            "HttpError" => {
                self.lower_native_record(SCHEMA_HTTP_ERROR, &["kind", "message"], args, named_args)
            }
            "Request" => self.lower_native_record(
                SCHEMA_HTTP_REQUEST,
                &["method", "path", "body", "headers", "query", "params"],
                args,
                named_args,
            ),
            "Response" | "BinaryResponse" => {
                let status_s = self.lower_scalar(get_expr(0, "status"))?;
                let body_o = self.lower_obj(get_expr(1, "body"))?;
                let headers_o = self.lower_obj(get_expr(2, "headers"))?;
                let schema_id = if sym == "BinaryResponse" {
                    SCHEMA_HTTP_BINARY_RESPONSE
                } else {
                    SCHEMA_HTTP_RESPONSE
                };
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id,
                    scalar_regs: Box::new([status_s]),
                    obj_regs: Box::new([body_o, headers_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "text" | "html" | "json" => {
                let aux = match sym {
                    "text" => HTTP_TEXT,
                    "html" => HTTP_HTML,
                    _ => HTTP_JSON,
                };
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux, a, b, c: 0 });
                Ok(a)
            }
            "response" => {
                let b = self.lower_scalar(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux: HTTP_RESPONSE, a, b, c });
                Ok(a)
            }
            "response_with" => {
                let status_s = self.lower_scalar(&args[0])?;
                let body_o = self.lower_obj(&args[1])?;
                let headers_o = self.lower_obj(&args[2])?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_HTTP_RESPONSE,
                    scalar_regs: Box::new([status_s]),
                    obj_regs: Box::new([body_o, headers_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "router" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux: HTTP_ROUTER, a, b: 0, c: 0 });
                Ok(a)
            }
            "get" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux: HTTP_GET, a, b, c: 0 });
                Ok(a)
            }
            "post" => {
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux: HTTP_POST, a, b, c });
                Ok(a)
            }
            "get_bytes" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux: HTTP_GET_BYTES, a, b, c: 0 });
                Ok(a)
            }
            "request" | "request_timeout" | "request_bytes" => {
                let method_o = self.lower_obj(&args[0])?;
                let url_o = self.lower_obj(&args[1])?;
                let body_o = self.lower_obj(&args[2])?;
                let headers_o = self.lower_obj(&args[3])?;
                let empty_m = self.alloc_temp_o();
                self.ops.push(Op::NewMap { dst_obj: empty_m });
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_HTTP_REQUEST,
                    scalar_regs: Box::new([]),
                    obj_regs: Box::new([method_o, url_o, body_o, headers_o, empty_m, empty_m]),
                })?;
                let req_o = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj: req_o, desc_idx });
                let (aux, c) = match sym {
                    "request_timeout" => (HTTP_REQUEST_TIMEOUT, self.lower_scalar(&args[4])?),
                    "request_bytes" => (HTTP_REQUEST_BYTES, 0),
                    _ => (HTTP_REQUEST, 0),
                };
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind: EXT_HTTP_OP, aux, a, b: req_o, c });
                Ok(a)
            }
            "build" | "serve" => {
                use crate::frontend::check::HttpRouterDeclKind;
                let ty = self.mc.checked.resolve_type(&type_args[0])?;
                let Type::Custom(tid) = ty else {
                    return Err(format!("http.{}<T>() requires a router type", sym));
                };
                let entries = self
                    .mc
                    .checked
                    .http_router_plans
                    .iter()
                    .find(|(name, _)| name == &tid.name)
                    .map(|(_, e)| e.clone())
                    .ok_or_else(|| format!("Missing router plan for '{}'", tid.name))?;
                let router_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_ROUTER,
                    a: router_o,
                    b: 0,
                    c: 0,
                });
                for entry in entries {
                    match entry.kind {
                        HttpRouterDeclKind::Static(url_prefix, dir_path) => {
                            let p_idx = self.mc.intern_str(&url_prefix)?;
                            let p_o = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: p_o, idx: p_idx });
                            let d_idx = self.mc.intern_str(&dir_path)?;
                            let d_o = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: d_o, idx: d_idx });
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_STATIC,
                                a: router_o,
                                b: p_o,
                                c: d_o,
                            });
                            continue;
                        }
                        HttpRouterDeclKind::Cors(origin) => {
                            let o_idx = self.mc.intern_str(&origin)?;
                            let o_reg = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: o_reg, idx: o_idx });
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_CORS,
                                a: router_o,
                                b: o_reg,
                                c: 0,
                            });
                            continue;
                        }
                        _ => {}
                    }
                    let fid = *self
                        .mc
                        .checked
                        .func_by_name
                        .get(&entry.handler_name)
                        .ok_or_else(|| format!("Unknown router handler '{}'", entry.handler_name))?;
                    let handler_o = self.alloc_temp_o();
                    self.ops.push(Op::LoadFuncObj {
                        dst_obj: handler_o,
                        func_id: fid as u16,
                    });
                    match entry.kind {
                        HttpRouterDeclKind::Before => {
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_BEFORE,
                                a: router_o,
                                b: handler_o,
                                c: 0,
                            });
                        }
                        HttpRouterDeclKind::After => {
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_AFTER,
                                a: router_o,
                                b: handler_o,
                                c: 0,
                            });
                        }
                        HttpRouterDeclKind::GroupBefore(prefix) => {
                            let p_idx = self.mc.intern_str(&prefix)?;
                            let p_o = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: p_o, idx: p_idx });
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_GROUP_BEFORE,
                                a: router_o,
                                b: p_o,
                                c: handler_o,
                            });
                        }
                        HttpRouterDeclKind::GroupAfter(prefix) => {
                            let p_idx = self.mc.intern_str(&prefix)?;
                            let p_o = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj { dst_obj: p_o, idx: p_idx });
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux: HTTP_ROUTE_GROUP_AFTER,
                                a: router_o,
                                b: p_o,
                                c: handler_o,
                            });
                        }
                        HttpRouterDeclKind::Static(_, _) | HttpRouterDeclKind::Cors(_) => {}
                        HttpRouterDeclKind::Route(method, pat) => {
                            let pat_idx = self.mc.intern_str(&pat)?;
                            let pat_o = self.alloc_temp_o();
                            self.ops.push(Op::LoadConstObj {
                                dst_obj: pat_o,
                                idx: pat_idx,
                            });
                            let aux = http_route_aux(&method);
                            self.ops.push(Op::ExtOp {
                                kind: EXT_HTTP_OP,
                                aux,
                                a: router_o,
                                b: pat_o,
                                c: handler_o,
                            });
                            if let Some(body_name) = &entry.body_struct {
                                let sid = self.mc.checked.struct_by_name[body_name] as u32;
                                let plan_idx = self
                                    .mc
                                    .add_json_decode_plan(&crate::native::JsonType::Struct(sid))?;
                                self.ops.push(Op::ExtOp {
                                    kind: EXT_HTTP_OP,
                                    aux: HTTP_ROUTE_TYPED_BODY,
                                    a: router_o,
                                    b: 0,
                                    c: plan_idx,
                                });
                            }
                        }
                    }
                }
                if sym == "build" {
                    Ok(router_o)
                } else {
                    let addr_o = self.lower_obj(&args[0])?;
                    let dst_o = self.alloc_temp_o();
                    self.ops.push(Op::ExtOp {
                        kind: EXT_HTTP_OP,
                        aux: HTTP_SERVE,
                        a: dst_o,
                        b: router_o,
                        c: addr_o,
                    });
                    Ok(dst_o)
                }
            }
            "ok_json" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_OK_JSON,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "status_json" => {
                let b = self.lower_scalar(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_STATUS_JSON,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            "redirect" => {
                let c = self.lower_obj(&args[0])?;
                let b = if args.len() == 2 {
                    self.lower_scalar(&args[1])?
                } else {
                    let r = self.alloc_temp_s();
                    self.load_int_into(r, 302)?;
                    r
                };
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_REDIRECT,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            "no_content" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_NO_CONTENT,
                    a,
                    b: 0,
                    c: 0,
                });
                Ok(a)
            }
            "get_json" => {
                let target_ty = self.mc.resolve_type(&type_args[0])?;
                let json_ty = crate::frontend::check_native::type_to_json_type(
                    &target_ty,
                    &self.mc.checked.struct_by_name,
                )
                .ok_or_else(|| format!("Unsupported JSON type '{}' in http.get_json()", target_ty))?;
                let plan_idx = self.mc.add_json_decode_plan(&json_ty)?;
                let url_o = self.lower_obj(&args[0])?;
                let query_o = if args.len() == 2 {
                    self.lower_obj(&args[1])?
                } else {
                    let idx = self.mc.intern_empty()?;
                    let r = self.alloc_temp_o();
                    self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
                    r
                };
                let plan_s = self.alloc_temp_s();
                self.load_int_into(plan_s, plan_idx as i64)?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_HTTP_REQUEST,
                    scalar_regs: Box::new([plan_s]),
                    obj_regs: Box::new([url_o, query_o]),
                })?;
                let packed = self.alloc_temp_o();
                self.ops.push(Op::NewRecord {
                    dst_obj: packed,
                    desc_idx,
                });
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_GET_JSON,
                    a,
                    b: packed,
                    c: 0,
                });
                Ok(a)
            }
            "post_json" => {
                let b = self.lower_obj(&args[0])?;
                let c = self.lower_obj(&args[1])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_POST_JSON,
                    a,
                    b,
                    c,
                });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.http symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_http_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<u16, String> {
        use crate::vm::exec_http::*;
        let router_o = self.lower_obj(target)?;
        match (type_name, method) {
            ("Request", "header" | "param" | "query_param") => {
                let aux = match method {
                    "header" => HTTP_REQ_HEADER,
                    "param" => HTTP_REQ_PARAM,
                    _ => HTTP_REQ_QUERY_PARAM,
                };
                let name_o = self.lower_obj(&args[0])?;
                let dst_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux,
                    a: dst_o,
                    b: router_o,
                    c: name_o,
                });
                Ok(dst_o)
            }
            (
                "Router",
                "get" | "post" | "put" | "delete" | "patch" | "options" | "head" | "any",
            ) => {
                let aux = http_route_aux(method);
                let pat_o = self.lower_obj(&args[0])?;
                let handler_o = self.lower_obj(&args[1])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux,
                    a: router_o,
                    b: pat_o,
                    c: handler_o,
                });
                Ok(router_o)
            }
            ("Router", "before" | "after") => {
                let aux = if method == "before" {
                    HTTP_ROUTE_BEFORE
                } else {
                    HTTP_ROUTE_AFTER
                };
                let handler_o = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux,
                    a: router_o,
                    b: handler_o,
                    c: 0,
                });
                Ok(router_o)
            }
            ("Router", "dispatch" | "serve") => {
                let aux = if method == "dispatch" { HTTP_DISPATCH } else { HTTP_SERVE };
                let arg_o = self.lower_obj(&args[0])?;
                let dst_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux,
                    a: dst_o,
                    b: router_o,
                    c: arg_o,
                });
                Ok(dst_o)
            }
            ("Router", "stop") => {
                let dst_o = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_HTTP_OP,
                    aux: HTTP_STOP,
                    a: dst_o,
                    b: router_o,
                    c: 0,
                });
                Ok(dst_o)
            }
            _ => Err(format!("Unsupported http.{} method '.{}()'", type_name, method)),
        }
    }

    pub(crate) fn lower_native_method_obj(
        &mut self,
        tid: &TypeId,
        target: &Expr,
        method: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<u16, String> {
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.fs"
        {
            return self.lower_fs_method(&tid.name, target, method, args);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.http"
        {
            return self.lower_http_method(&tid.name, target, method, args);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.db"
        {
            return self.lower_db_method(&tid.name, target, method, type_args, args);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.cron"
        {
            return self.lower_cron_method(&tid.name, target, method);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.lock"
        {
            return self.lower_lock_method(&tid.name, target, method);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.cache"
        {
            return self.lower_cache_method(&tid.name, target, method, type_args, args);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.redis"
        {
            return self.lower_redis_method(&tid.name, target, method, type_args, args);
        }
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.time"
            && tid.name == "Timestamp"
            && matches!(method, "add" | "sub" | "diff")
        {
            let aux = match method {
                "add" => TIME_ADD,
                "sub" => TIME_SUB,
                _ => TIME_DIFF,
            };
            let b = self.lower_obj(target)?;
            let c = self.lower_obj(&args[0])?;
            let a = self.alloc_temp_o();
            self.ops.push(Op::ExtOp {
                kind: EXT_TIME_OP,
                aux,
                a,
                b,
                c,
            });
            return Ok(a);
        }
        let kind = match (&tid.module, tid.name.as_str(), method) {
            (ModuleId::Native(m), "Timestamp", "unix_seconds") if m == "cdt.time" => {
                TimeOpKind::UnixSeconds
            }
            (ModuleId::Native(m), "Timestamp", "unix_milliseconds") if m == "cdt.time" => {
                TimeOpKind::UnixMilliseconds
            }
            (ModuleId::Native(m), "Timestamp", "to_iso") if m == "cdt.time" => {
                TimeOpKind::FormatIso
            }
            (ModuleId::Native(m), "Duration", "seconds") if m == "cdt.time" => TimeOpKind::DurSeconds,
            (ModuleId::Native(m), "Duration", "milliseconds") if m == "cdt.time" => {
                TimeOpKind::DurMilliseconds
            }
            (ModuleId::Native(m), "Duration", "nanoseconds") if m == "cdt.time" => {
                TimeOpKind::DurNanoseconds
            }
            _ => return Err(format!("Unsupported native method '{}.{}()'", tid, method)),
        };
        let arg_reg = self.lower_obj(target)?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::TimeOp {
            kind: kind as u8,
            dst_obj,
            arg_reg,
        });
        Ok(dst_obj)
    }

    pub(crate) fn lower_native_member_obj(
        &mut self,
        tid: &TypeId,
        target: &Expr,
        field: &str,
    ) -> Result<u16, String> {
        let (_, field_idx) = native_member_field(tid, field)?;
        let rec_obj = self.lower_obj(target)?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::GetFieldObj {
            dst_obj,
            rec_obj,
            field_idx,
        });
        Ok(dst_obj)
    }

    pub(crate) fn lower_email_fn_val(&mut self) -> Result<u16, String> {
        let fid = if let Some(id) = self.mc.email_fn_id {
            id
        } else {
            let id = self.mc.module.functions.len() as u32;
            self.mc.module.functions.push(FunctionProto {
                name: "__native_validation_email".to_string(),
                num_scalars: 0,
                num_objs: 2,
                scalar_params: 0,
                obj_params: 1,
                returns_obj: true,
                ops: vec![
                    Op::ValidationOp {
                        kind: ValOpKind::Email as u8,
                        dst_obj: 1,
                        a_reg: 0,
                        b_reg: 0,
                    },
                    Op::ReturnObj { src: 1 },
                ]
                .into_boxed_slice(),
            });
            self.mc.email_fn_id = Some(id);
            id
        };
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::LoadFuncObj {
            dst_obj,
            func_id: fid as u16,
        });
        Ok(dst_obj)
    }

    pub(crate) fn emit_box_scalar_from(&mut self, src_s: u16, ty: &Type) -> u16 {
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::BoxScalar {
            dst_obj,
            src_s,
            kind: scalar_kind(ty),
        });
        dst_obj
    }

    pub(crate) fn lower_native_member_scalar(
        &mut self,
        tid: &TypeId,
        target: &Expr,
        field: &str,
    ) -> Result<u16, String> {
        let (_, field_idx) = native_member_field(tid, field)?;
        let rec_obj = self.lower_obj(target)?;
        let dst_s = self.alloc_temp_s();
        self.ops.push(Op::GetFieldScalar {
            dst_s,
            rec_obj,
            field_idx,
        });
        Ok(dst_s)
    }

    pub(crate) fn lower_builtin_obj_method(
        &mut self,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<Option<u16>, String> {
        if method == "to_string" {
            if target.ty.is_scalar() {
                let src_s = self.lower_scalar(target)?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ScalarToStr {
                    dst_obj,
                    src_s,
                    tag: elem_tag_for(&target.ty),
                });
                return Ok(Some(dst_obj));
            }
            if target.ty == Type::String {
                return Ok(Some(self.lower_obj(target)?));
            }
            let src_obj = self.lower_obj(target)?;
            let dst_obj = self.alloc_temp_o();
            self.ops.push(Op::ObjToStr { dst_obj, src_obj });
            return Ok(Some(dst_obj));
        }
        match (&target.ty, method) {
            (Type::String, "upper" | "lower" | "trim") => {
                let src_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let kind = match method {
                    "upper" => 0,
                    "lower" => 1,
                    _ => 2,
                };
                self.ops.push(Op::StrMethod { dst_obj, src_obj, kind });
                Ok(Some(dst_obj))
            }
            (Type::String, "split") => {
                let str_obj = self.lower_obj(target)?;
                let sep_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_STR_SPLIT,
                    aux: 0,
                    a: dst_obj,
                    b: str_obj,
                    c: sep_obj,
                });
                Ok(Some(dst_obj))
            }
            (Type::String, "replace") => {
                let str_obj = self.lower_obj(target)?;
                let from_obj = self.lower_obj(&args[0])?;
                let to_obj = self.lower_obj(&args[1])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::StrReplace {
                    dst_obj,
                    str_obj,
                    from_obj,
                    to_obj,
                });
                Ok(Some(dst_obj))
            }
            (Type::List(elem), "get") => {
                let list_obj = self.lower_obj(target)?;
                let idx_s = self.lower_scalar(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ListGet {
                    dst_obj,
                    list_obj,
                    idx_s,
                    elem_tag: elem_tag_for(elem),
                });
                Ok(Some(dst_obj))
            }
            (Type::List(elem), "first" | "last") => {
                let list_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let kind = if method == "first" { EXT_LIST_FIRST } else { EXT_LIST_LAST };
                self.ops.push(Op::ExtOp {
                    kind,
                    aux: elem_tag_for(elem),
                    a: dst_obj,
                    b: list_obj,
                    c: 0,
                });
                Ok(Some(dst_obj))
            }
            (Type::List(elem), "sort") => {
                let list_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let aux = match elem.as_ref() {
                    Type::Int => 0,
                    Type::Float => 1,
                    _ => 2,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_LIST_SORT,
                    aux,
                    a: dst_obj,
                    b: list_obj,
                    c: 0,
                });
                Ok(Some(dst_obj))
            }
            (Type::List(_), "min" | "max") => {
                let list_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let aux = if method == "min" { 1 } else { 2 };
                self.ops.push(Op::ExtOp {
                    kind: crate::vm::opcode::EXT_LIST_AGG,
                    aux,
                    a: dst_obj,
                    b: list_obj,
                    c: 0,
                });
                Ok(Some(dst_obj))
            }
            (Type::List(_), "find") => Ok(Some(self.lower_list_pred_loop(target, &args[0], 2)?)),
            (Type::List(_), "slice") => {
                let list_obj = self.lower_obj(target)?;
                let start_s = self.lower_scalar(&args[0])?;
                let end_s = self.lower_scalar(&args[1])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ListSlice {
                    dst_obj,
                    list_obj,
                    start_s,
                    end_s,
                });
                Ok(Some(dst_obj))
            }
            (Type::List(_), "reverse") => {
                let list_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ListReverse { dst_obj, list_obj });
                Ok(Some(dst_obj))
            }
            (Type::List(elem), "enumerate") => {
                let list_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ListEnumerate {
                    dst_obj,
                    list_obj,
                    elem_tag: elem_tag_for(elem),
                });
                Ok(Some(dst_obj))
            }
            (Type::List(_), "join") => {
                let list_obj = self.lower_obj(target)?;
                let sep_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ListJoin {
                    dst_obj,
                    list_obj,
                    sep_obj,
                });
                Ok(Some(dst_obj))
            }
            (Type::String, "to_int") | (Type::String, "to_float") => {
                let str_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let kind = if method == "to_int" { EXT_STR_TO_INT } else { EXT_STR_TO_FLOAT };
                self.ops.push(Op::ExtOp { kind, aux: 0, a: dst_obj, b: str_obj, c: 0 });
                Ok(Some(dst_obj))
            }
            (Type::Map(k, v), "get" | "remove") => {
                let map_obj = if method == "remove"
                    && let ExprKind::Ident(name) = &target.kind
                    && let Some((slot, _)) = self.lookup_local(name)
                {
                    slot.reg
                } else {
                    self.lower_obj(target)?
                };
                let key_reg = if k.is_scalar() {
                    self.lower_scalar(&args[0])?
                } else {
                    self.lower_obj(&args[0])?
                };
                let aux = (elem_tag_for(k) << 4) | elem_tag_for(v);
                let kind = if method == "get" { EXT_MAP_GET } else { EXT_MAP_REMOVE };
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ExtOp { kind, aux, a: dst_obj, b: map_obj, c: key_reg });
                Ok(Some(dst_obj))
            }
            (Type::Map(k, v), "keys" | "values" | "entries") => {
                let map_obj = self.lower_obj(target)?;
                let dst_obj = self.alloc_temp_o();
                let (kind, aux) = match method {
                    "keys" => (EXT_MAP_KEYS, elem_tag_for(k)),
                    "values" => (EXT_MAP_VALUES, elem_tag_for(v)),
                    _ => (EXT_MAP_ENTRIES, (elem_tag_for(k) << 4) | elem_tag_for(v)),
                };
                self.ops.push(Op::ExtOp { kind, aux, a: dst_obj, b: map_obj, c: 0 });
                Ok(Some(dst_obj))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn lower_prelude_error_ctor(
        &mut self,
        name: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match name {
            "IndexError" => {
                let (idx_e, len_e, msg_e) = if !named_args.is_empty() {
                    let find = |f: &str| {
                        named_args
                            .iter()
                            .find(|n| n.name == f)
                            .map(|n| &n.value)
                            .ok_or_else(|| format!("Missing field '{}' in IndexError", f))
                    };
                    (find("index")?, find("length")?, find("message")?)
                } else {
                    (&args[0], &args[1], &args[2])
                };
                let idx_s = self.lower_scalar(idx_e)?;
                let len_s = self.lower_scalar(len_e)?;
                let msg_o = self.lower_obj(msg_e)?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_INDEX_ERROR,
                    scalar_regs: Box::new([idx_s, len_s]),
                    obj_regs: Box::new([msg_o]),
                })?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj, desc_idx });
                Ok(dst_obj)
            }
            "KeyError" => self.lower_native_record(SCHEMA_KEY_ERROR, &["key", "message"], args, named_args),
            _ => self.lower_native_record(SCHEMA_PARSE_ERROR, &["value", "message"], args, named_args),
        }
    }

    pub(crate) fn lower_pattern_bind(
        &mut self,
        pattern: &Pattern,
        val_ty: &Type,
        src_reg: u16,
        src_is_scalar: bool,
    ) -> Result<(), String> {
        let pkey = pattern as *const Pattern as usize;
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Nil => Ok(()),
            PatternKind::Ident(name) => {
                let slot = self.pat_slots[&(pkey, name.clone())];
                match (slot.is_scalar, src_is_scalar) {
                    (true, true) => {
                        if slot.reg != src_reg {
                            self.ops.push(Op::MoveScalar { dst: slot.reg, src: src_reg });
                        }
                    }
                    (true, false) => {
                        self.ops.push(Op::UnboxScalar { dst_s: slot.reg, src_obj: src_reg });
                    }
                    (false, true) => {
                        self.ops.push(Op::BoxScalar {
                            dst_obj: slot.reg,
                            src_s: src_reg,
                            kind: scalar_kind(val_ty),
                        });
                    }
                    (false, false) => {
                        if slot.reg != src_reg {
                            self.ops.push(Op::MoveObj { dst_obj: slot.reg, src: src_reg });
                        }
                    }
                }
                self.bind_var(name.clone(), slot, val_ty.clone());
                Ok(())
            }
            PatternKind::Tuple(pats) => {
                let elem_tys: &[Type] = match val_ty {
                    Type::Tuple(ts) => ts.as_slice(),
                    _ => return Err(format!("Expected tuple type for pattern, got {}", val_ty)),
                };
                for (i, (p, ety)) in pats.iter().zip(elem_tys.iter()).enumerate() {
                    if matches!(p.kind, PatternKind::Wildcard | PatternKind::Nil) {
                        continue;
                    }
                    if let PatternKind::Ident(name) = &p.kind {
                        let ckey = p as *const Pattern as usize;
                        let slot = self.pat_slots[&(ckey, name.clone())];
                        if slot.is_scalar {
                            self.ops.push(Op::TupleGetScalar {
                                dst_s: slot.reg,
                                tuple_obj: src_reg,
                                idx: i as u16,
                            });
                        } else {
                            self.ops.push(Op::TupleGetObj {
                                dst_obj: slot.reg,
                                tuple_obj: src_reg,
                                idx: i as u16,
                            });
                        }
                        self.bind_var(name.clone(), slot, ety.clone());
                    } else {
                        let sub_o = self.alloc_temp_o();
                        self.ops.push(Op::TupleGetObj {
                            dst_obj: sub_o,
                            tuple_obj: src_reg,
                            idx: i as u16,
                        });
                        self.lower_pattern_bind(p, ety, sub_o, false)?;
                    }
                }
                Ok(())
            }
            PatternKind::Type { .. } => Err("Unexpected type pattern in value binding".to_string()),
        }
    }

    pub(crate) fn lower_db_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        use crate::vm::exec_db::*;
        match sym {
            "DbError" => self.lower_native_record(
                SCHEMA_DB_ERROR,
                &["kind", "message", "code"],
                args,
                named_args,
            ),
            "connect" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_CONNECT,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "null" => {
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_PARAM_NULL,
                    a,
                    b: 0,
                    c: 0,
                });
                Ok(a)
            }
            "int" | "float" | "bool" => {
                let b = self.lower_scalar(&args[0])?;
                let a = self.alloc_temp_o();
                let aux = match sym {
                    "int" => DB_PARAM_INT,
                    "float" => DB_PARAM_FLOAT,
                    _ => DB_PARAM_BOOL,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "text" => {
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_PARAM_TEXT,
                    a,
                    b,
                    c: 0,
                });
                Ok(a)
            }
            "ddl" => {
                let model_idx = self.resolve_db_model_idx(&type_args[0])?;
                let b = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_DDL,
                    a,
                    b,
                    c: model_idx,
                });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.db symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_db_last_insert_id(&mut self, target: &Expr) -> Result<u16, String> {
        use crate::vm::exec_db::DB_LAST_INSERT_ID;
        let b = self.lower_obj(target)?;
        let a = self.alloc_temp_s();
        self.ops.push(Op::ExtOp {
            kind: EXT_DB_OP,
            aux: DB_LAST_INSERT_ID,
            a,
            b,
            c: 0,
        });
        Ok(a)
    }

    fn pack_db_query_args(&mut self, schema_idx: Option<i64>, args: &[Expr]) -> Result<u16, String> {
        let sql_reg = self.lower_obj(&args[0])?;
        let params_reg = if args.len() >= 2 {
            self.lower_obj(&args[1])?
        } else {
            let idx = self.mc.intern_empty()?;
            let r = self.alloc_temp_o();
            self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
            r
        };
        let scalar_regs = if let Some(sid) = schema_idx {
            let s_reg = self.alloc_temp_s();
            self.load_int_into(s_reg, sid)?;
            Box::new([s_reg]) as Box<[u16]>
        } else {
            Box::new([]) as Box<[u16]>
        };
        let desc_idx = self.mc.intern_record_desc(RecordDesc {
            schema_id: SCHEMA_DB_PARAM,
            scalar_regs,
            obj_regs: Box::new([sql_reg, params_reg]),
        })?;
        let packed = self.alloc_temp_o();
        self.ops.push(Op::NewRecord {
            dst_obj: packed,
            desc_idx,
        });
        Ok(packed)
    }

    fn resolve_db_model_idx(&self, te: &TypeExpr) -> Result<u16, String> {
        let target_ty = self.mc.resolve_type(te)?;
        let Type::Custom(tid) = target_ty else {
            return Err("Expected struct type for cdt.db model operation".to_string());
        };
        let sid = self.mc.checked.struct_by_name[&tid.name] as u32;
        self.mc
            .checked
            .db_model_plans
            .iter()
            .position(|m| m.schema_id == sid)
            .map(|i| i as u16)
            .ok_or_else(|| format!("Missing DB model plan for '{}'", tid.name))
    }

    fn pack_db_model_arg(&mut self, model_idx: u16, arg_expr: &Expr) -> Result<u16, String> {
        let arg_reg = self.lower_obj(arg_expr)?;
        let m_reg = self.alloc_temp_s();
        self.load_int_into(m_reg, model_idx as i64)?;
        let desc_idx = self.mc.intern_record_desc(RecordDesc {
            schema_id: SCHEMA_DB_PARAM,
            scalar_regs: Box::new([m_reg]),
            obj_regs: Box::new([arg_reg]),
        })?;
        let packed = self.alloc_temp_o();
        self.ops.push(Op::NewRecord {
            dst_obj: packed,
            desc_idx,
        });
        Ok(packed)
    }

    pub(crate) fn lower_db_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<u16, String> {
        use crate::vm::exec_db::*;
        if type_name != "Connection" {
            return Err(format!("Unsupported db method '{}.{}()'", type_name, method));
        }
        let conn_reg = self.lower_obj(target)?;
        let dst_obj = self.alloc_temp_o();
        match method {
            "dialect" => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_DIALECT,
                    a: dst_obj,
                    b: conn_reg,
                    c: 0,
                });
                Ok(dst_obj)
            }
            "exec" => {
                let packed = self.pack_db_query_args(None, args)?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_EXEC,
                    a: dst_obj,
                    b: conn_reg,
                    c: packed,
                });
                Ok(dst_obj)
            }
            "query" | "query_one" => {
                let target_ty = self.mc.resolve_type(&type_args[0])?;
                let Type::Custom(tid) = target_ty else {
                    return Err(format!("Connection.{}<T> expects a struct type", method));
                };
                let sid = self.mc.checked.struct_by_name[&tid.name] as i64;
                let packed = self.pack_db_query_args(Some(sid), args)?;
                let aux = if method == "query_one" { DB_QUERY_ONE } else { DB_QUERY_TYPED };
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux,
                    a: dst_obj,
                    b: conn_reg,
                    c: packed,
                });
                Ok(dst_obj)
            }
            "query_rows" => {
                let packed = self.pack_db_query_args(None, args)?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_QUERY_ROWS,
                    a: dst_obj,
                    b: conn_reg,
                    c: packed,
                });
                Ok(dst_obj)
            }
            "begin" | "commit" | "rollback" | "migrate" | "close" => {
                let aux = match method {
                    "begin" => DB_BEGIN,
                    "commit" => DB_COMMIT,
                    "rollback" => DB_ROLLBACK,
                    "migrate" => DB_MIGRATE,
                    _ => DB_CLOSE,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux,
                    a: dst_obj,
                    b: conn_reg,
                    c: 0,
                });
                Ok(dst_obj)
            }
            "create_table" | "drop_table" | "all" | "count" => {
                let model_idx = self.resolve_db_model_idx(&type_args[0])?;
                let aux = match method {
                    "create_table" => DB_CREATE_TABLE,
                    "drop_table" => DB_DROP_TABLE,
                    "count" => DB_COUNT,
                    _ => DB_ALL,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux,
                    a: dst_obj,
                    b: conn_reg,
                    c: model_idx,
                });
                Ok(dst_obj)
            }
            "insert" | "update" | "find" | "delete" => {
                let model_idx = self.resolve_db_model_idx(&type_args[0])?;
                let packed = self.pack_db_model_arg(model_idx, &args[0])?;
                let aux = match method {
                    "insert" => DB_INSERT,
                    "update" => DB_UPDATE,
                    "find" => DB_FIND,
                    _ => DB_DELETE,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux,
                    a: dst_obj,
                    b: conn_reg,
                    c: packed,
                });
                Ok(dst_obj)
            }
            "find_by" => {
                let model_idx = self.resolve_db_model_idx(&type_args[0])?;
                let col_reg = self.lower_obj(&args[0])?;
                let val_reg = self.lower_obj(&args[1])?;
                let m_reg = self.alloc_temp_s();
                self.load_int_into(m_reg, model_idx as i64)?;
                let desc_idx = self.mc.intern_record_desc(RecordDesc {
                    schema_id: SCHEMA_DB_PARAM,
                    scalar_regs: Box::new([m_reg]),
                    obj_regs: Box::new([col_reg, val_reg]),
                })?;
                let packed = self.alloc_temp_o();
                self.ops.push(Op::NewRecord { dst_obj: packed, desc_idx });
                self.ops.push(Op::ExtOp {
                    kind: EXT_DB_OP,
                    aux: DB_FIND_BY,
                    a: dst_obj,
                    b: conn_reg,
                    c: packed,
                });
                Ok(dst_obj)
            }
            _ => Err(format!("Unsupported db.Connection method '{}'", method)),
        }
    }

    fn resolve_cron_schedule_idx(&self, te: &TypeExpr) -> Result<u16, String> {
        let target_ty = self.mc.resolve_type(te)?;
        let Type::Custom(tid) = target_ty else {
            return Err("Expected schedule type for cdt.cron".to_string());
        };
        self.mc
            .checked
            .cron_schedule_plans
            .iter()
            .position(|(n, _)| n == &tid.name)
            .map(|i| i as u16)
            .ok_or_else(|| format!("Unknown schedule '{}'", tid.name))
    }

    pub(crate) fn lower_cron_validate(&mut self, expr: &Expr) -> Result<u16, String> {
        use crate::vm::exec_cron::CRON_VALIDATE;
        let b = self.lower_obj(expr)?;
        let a = self.alloc_temp_s();
        self.ops.push(Op::ExtOp {
            kind: EXT_CRON_OP,
            aux: CRON_VALIDATE,
            a,
            b,
            c: 0,
        });
        Ok(a)
    }

    pub(crate) fn lower_cron_is_running(&mut self, target: &Expr) -> Result<u16, String> {
        use crate::vm::exec_cron::CRON_IS_RUNNING;
        let b = self.lower_obj(target)?;
        let a = self.alloc_temp_s();
        self.ops.push(Op::ExtOp {
            kind: EXT_CRON_OP,
            aux: CRON_IS_RUNNING,
            a,
            b,
            c: 0,
        });
        Ok(a)
    }

    pub(crate) fn lower_cron_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        use crate::vm::exec_cron::*;
        match sym {
            "CronError" => self.lower_native_record(
                SCHEMA_CRON_ERROR,
                &["kind", "message"],
                args,
                named_args,
            ),
            "run" | "start" => {
                let sched_idx = self.resolve_cron_schedule_idx(&type_args[0])?;
                let a = self.alloc_temp_o();
                let aux = if sym == "run" { CRON_RUN } else { CRON_START };
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRON_OP,
                    aux,
                    a,
                    b: sched_idx,
                    c: 0,
                });
                Ok(a)
            }
            "run_once" => {
                let sched_idx = self.resolve_cron_schedule_idx(&type_args[0])?;
                let c = self.lower_obj(&args[0])?;
                let a = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CRON_OP,
                    aux: CRON_RUN_ONCE,
                    a,
                    b: sched_idx,
                    c,
                });
                Ok(a)
            }
            other => Err(format!("Unsupported cdt.cron symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_cron_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
    ) -> Result<u16, String> {
        use crate::vm::exec_cron::CRON_STOP;
        if type_name != "Scheduler" || method != "stop" {
            return Err(format!("Unsupported cron method '{}.{}()'", type_name, method));
        }
        let b = self.lower_obj(target)?;
        let dst = self.alloc_temp_o();
        self.ops.push(Op::ExtOp {
            kind: EXT_CRON_OP,
            aux: CRON_STOP,
            a: 0,
            b,
            c: 0,
        });
        let idx = self.mc.intern_empty()?;
        self.ops.push(Op::LoadConstObj { dst_obj: dst, idx });
        Ok(dst)
    }
}

fn http_route_aux(method: &str) -> u8 {
    use crate::vm::exec_http::*;
    match method {
        "get" => HTTP_ROUTE_GET,
        "post" => HTTP_ROUTE_POST,
        "put" => HTTP_ROUTE_PUT,
        "delete" => HTTP_ROUTE_DELETE,
        "patch" => HTTP_ROUTE_PATCH,
        "options" => HTTP_ROUTE_OPTIONS,
        "head" => HTTP_ROUTE_HEAD,
        _ => HTTP_ROUTE_ANY,
    }
}
