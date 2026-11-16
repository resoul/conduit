use crate::frontend::ast::*;
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_native::{native_member_field, scalar_kind};
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::ir::lower::FnLowerer;
use crate::ir::{FunctionProto, RecordDesc};
use crate::native::crypto::{
    CRYPTO_B64_DEC, CRYPTO_B64_ENC, CRYPTO_HEX_DEC, CRYPTO_HEX_ENC, CRYPTO_HMAC_SHA256,
    CRYPTO_RAND_HEX, CRYPTO_RAND_INT, CRYPTO_SHA256, CRYPTO_UUID_V4,
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
use crate::native::time::TimeOpKind;
use crate::native::url::{URL_DECODE, URL_DECODE_QUERY, URL_ENCODE, URL_ENCODE_QUERY, URL_PARSE};
use crate::native::validation::ValOpKind;
use crate::vm::opcode::{
    EXT_CLI_HELP, EXT_CLI_PARSE, EXT_CLI_PARSE_ARGS, EXT_CRYPTO_OP, EXT_ENV_ARGS, EXT_ENV_GET,
    EXT_ENV_LOAD, EXT_ENV_REQUIRE, EXT_FS_BINARY, EXT_FS_UNARY, EXT_LIST_FIRST, EXT_LIST_LAST,
    EXT_LIST_SORT, EXT_MAP_ENTRIES, EXT_MAP_GET, EXT_MAP_KEYS, EXT_MAP_REMOVE, EXT_MAP_VALUES,
    EXT_PATH_OP, EXT_PROCESS_OP, EXT_STR_SPLIT, EXT_STR_TO_FLOAT, EXT_STR_TO_INT, EXT_URL_OP, Op,
};
use crate::vm::value::{
    SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT, SCHEMA_CLI_ERROR,
    SCHEMA_CRYPTO_ERROR, SCHEMA_EMPTY_TAG, SCHEMA_ENV_ERROR, SCHEMA_FS_ERROR, SCHEMA_FS_READER,
    SCHEMA_FS_WRITER, SCHEMA_INDEX_ERROR, SCHEMA_JSON_DECODE_ERROR, SCHEMA_JSON_ENCODE_ERROR,
    SCHEMA_KEY_ERROR, SCHEMA_LIST_TAG, SCHEMA_MAP_TAG, SCHEMA_PARSE_ERROR, SCHEMA_PROCESS_ERROR,
    SCHEMA_PROCESS_OUTPUT, SCHEMA_STR_TAG, SCHEMA_TIME_DURATION, SCHEMA_TIME_ERROR,
    SCHEMA_TIME_INSTANT, SCHEMA_TIME_TIMESTAMP, SCHEMA_URL, SCHEMA_URL_ERROR,
    SCHEMA_VALIDATION_ERROR, SCHEMA_VALIDATION_ISSUE, SCHEMA_VALIDATION_VIOLATION,
};

#[inline]
pub(crate) fn elem_tag_for(ty: &Type) -> u8 {
    match ty {
        Type::Int => 1,
        Type::Float => 2,
        Type::Bool => 3,
        _ => 0,
    }
}

pub(crate) fn type_tag_for_switch(ty: &Type, checked: &CheckedProgram) -> Result<u32, String> {
    match ty {
        Type::Void | Type::Nil => Ok(SCHEMA_EMPTY_TAG),
        Type::Int => Ok(SCHEMA_BOXED_INT),
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
            "milliseconds" => {
                let arg_reg = self.lower_scalar(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::TimeOp {
                    kind: TimeOpKind::Milliseconds as u8,
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
            "email" => {
                let a_reg = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::ValidationOp {
                    kind: ValOpKind::Email as u8,
                    dst_obj,
                    a_reg,
                    b_reg: 0,
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
            other => Err(format!("Unsupported cdt.cli symbol '{}'", other)),
        }
    }

    fn lower_native_record(
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

    pub(crate) fn lower_native_method_obj(
        &mut self,
        tid: &TypeId,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<u16, String> {
        if let ModuleId::Native(m) = &tid.module
            && m == "cdt.fs"
        {
            return self.lower_fs_method(&tid.name, target, method, args);
        }
        let kind = match (&tid.module, tid.name.as_str(), method) {
            (ModuleId::Native(m), "Timestamp", "unix_seconds") if m == "cdt.time" => {
                TimeOpKind::UnixSeconds
            }
            (ModuleId::Native(m), "Timestamp", "unix_milliseconds") if m == "cdt.time" => {
                TimeOpKind::UnixMilliseconds
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
}
