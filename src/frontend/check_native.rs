use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, Checker};
use crate::frontend::lexer::{ESCAPED_CLOSE_BRACE, ESCAPED_OPEN_BRACE};
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::native::JsonType;

pub struct NativeModuleSpec {
    pub name: &'static str,
    pub fns: &'static [&'static str],
    pub types: &'static [&'static str],
}

pub const NATIVE_MODULES: &[NativeModuleSpec] = &[
    NativeModuleSpec {
        name: "cdt.json",
        fns: &["decode", "encode"],
        types: &["DecodeError", "EncodeError"],
    },
    NativeModuleSpec {
        name: "cdt.time",
        fns: &[
            "now", "monotonic", "elapsed", "seconds", "milliseconds", "sleep", "from_unix",
            "from_unix_millis", "parse_iso", "format_iso",
        ],
        types: &["Timestamp", "Instant", "Duration", "TimeError"],
    },
    NativeModuleSpec {
        name: "cdt.validation",
        fns: &[
            "min", "max", "email", "check", "min_len", "max_len", "not_empty", "range",
            "one_of", "url", "uuid",
        ],
        types: &["Issue", "Violation", "ValidationError"],
    },
    NativeModuleSpec {
        name: "cdt.env",
        fns: &["get", "require", "args", "load"],
        types: &["EnvError"],
    },
    NativeModuleSpec {
        name: "cdt.cli",
        fns: &["parse", "parse_args", "help", "dispatch", "dispatch_args"],
        types: &["CliError"],
    },
    NativeModuleSpec {
        name: "cdt.fs",
        fns: &[
            "read_text", "write_text", "append_text", "write_atomic", "read_lines", "each_line",
            "exists", "is_file", "is_dir", "size", "list_dir", "mkdir_all", "remove", "copy",
            "rename", "open_read", "open_write", "open_append",
        ],
        types: &["FsError", "Reader", "Writer"],
    },
    NativeModuleSpec {
        name: "cdt.path",
        fns: &[
            "join", "join_all", "dirname", "basename", "ext", "stem", "normalize", "is_abs",
            "with_ext",
        ],
        types: &[],
    },
    NativeModuleSpec {
        name: "cdt.process",
        fns: &["run", "status", "cwd", "set_cwd", "pid", "exit"],
        types: &["ProcessError", "Output"],
    },
    NativeModuleSpec {
        name: "cdt.crypto",
        fns: &[
            "sha256", "hmac_sha256", "base64_encode", "base64_decode", "hex_encode", "hex_decode",
            "uuid_v4", "random_bytes_hex", "random_int", "hash_password", "verify_password",
            "constant_time_eq",
        ],
        types: &["CryptoError"],
    },
    NativeModuleSpec {
        name: "cdt.url",
        fns: &["parse", "encode", "decode", "encode_query", "decode_query"],
        types: &["UrlError", "Url"],
    },
    NativeModuleSpec {
        name: "cdt.csv",
        fns: &["parse", "parse_with", "parse_records", "stringify", "stringify_with"],
        types: &["CsvError"],
    },
    NativeModuleSpec {
        name: "cdt.math",
        fns: &[
            "pi", "e", "tau", "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "ln",
            "log10", "floor", "ceil", "round", "trunc", "pow", "atan2", "hypot",
        ],
        types: &[],
    },
    NativeModuleSpec {
        name: "cdt.log",
        fns: &["debug", "info", "warn", "error", "set_level", "set_json", "get_level"],
        types: &[],
    },
    NativeModuleSpec {
        name: "cdt.http",
        fns: &[
            "text", "html", "json", "ok_json", "status_json", "redirect", "no_content", "response",
            "response_with", "router", "build", "serve", "get", "post", "get_json", "post_json",
            "request", "request_timeout", "get_bytes", "request_bytes",
        ],
        types: &["HttpError", "Request", "Response", "BinaryResponse", "Router"],
    },
    NativeModuleSpec {
        name: "cdt.db",
        fns: &["connect", "int", "float", "bool", "text", "null", "ddl"],
        types: &["Connection", "Param", "DbError"],
    },
    NativeModuleSpec {
        name: "cdt.cron",
        fns: &["run", "start", "run_once", "validate"],
        types: &["Scheduler", "CronError"],
    },
    NativeModuleSpec {
        name: "cdt.jwt",
        fns: &["sign", "sign_alg", "verify", "verify_alg", "decode_unverified"],
        types: &["JwtError"],
    },
    NativeModuleSpec {
        name: "cdt.lock",
        fns: &[
            "with",
            "try_with",
            "read",
            "write",
            "file",
            "file_try",
            "file_shared",
            "acquire",
            "try_acquire",
            "file_acquire",
            "file_try_acquire",
        ],
        types: &["Guard", "LockError"],
    },
    NativeModuleSpec {
        name: "cdt.cache",
        fns: &["open", "open_file", "drop"],
        types: &["Cache", "CacheError"],
    },
    NativeModuleSpec {
        name: "cdt.redis",
        fns: &["connect"],
        types: &["Client", "Lock", "RedisError"],
    },
];

pub fn find_native_module(path: &str) -> Option<&'static NativeModuleSpec> {
    NATIVE_MODULES.iter().find(|m| m.name == path)
}

pub fn is_native_module(path: &str) -> bool {
    find_native_module(path).is_some()
}

pub(crate) fn is_valid_native_type(mod_name: &str, ty_name: &str) -> bool {
    find_native_module(mod_name).is_some_and(|m| m.types.contains(&ty_name))
}

pub(crate) fn register_native_imports(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    for item in &program.items {
        let ItemKind::Import { path, alias, names } = &item.kind else {
            continue;
        };
        let Some(spec_mod) = find_native_module(path) else {
            return Err(format!("Unknown or unlinked module '{}'", path));
        };
        if let Some(a) = alias {
            checked.native_aliases.insert(a.clone(), spec_mod.name);
        }
        for spec in names {
            if spec_mod.fns.contains(&spec.name.as_str()) {
                insert_unique_native(
                    &mut checked.native_direct_fns,
                    &spec.alias,
                    spec_mod.name,
                    &spec.name,
                )?;
            } else if spec_mod.types.contains(&spec.name.as_str()) {
                checked
                    .native_direct_types
                    .insert(spec.alias.clone(), (spec_mod.name, spec.name.clone()));
            } else {
                return Err(format!(
                    "Module '{}' has no exported member '{}'",
                    spec_mod.name, spec.name
                ));
            }
        }
    }
    Ok(())
}

fn insert_unique_native(
    map: &mut HashMap<String, (&'static str, String)>,
    alias: &str,
    mod_name: &'static str,
    target: &str,
) -> Result<(), String> {
    if let Some((prev_mod, prev_target)) =
        map.insert(alias.to_string(), (mod_name, target.to_string()))
        && (prev_mod != mod_name || prev_target != target)
    {
        return Err(format!("Duplicate imported identifier '{}'", alias));
    }
    Ok(())
}

pub fn resolve_type_expr(te: &TypeExpr, checked: &CheckedProgram) -> Result<Type, String> {
    match te {
        TypeExpr::Named(name) => match name.as_str() {
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Bool" => Ok(Type::Bool),
            "Nil" => Ok(Type::Nil),
            "Void" => Ok(Type::Void),
            "Any" => Ok(Type::Any),
            "IndexError" | "KeyError" | "ParseError" => Ok(Type::Custom(TypeId {
                module: ModuleId::Builtin,
                name: name.to_string(),
            })),
            _ if checked.generic_structs.contains_key(name) => Err(format!(
                "Generic type '{}' requires {} type argument(s)",
                name,
                checked.generic_structs[name].type_params.len()
            )),
            _ if checked.struct_by_name.contains_key(name) => {
                Ok(Type::Custom(TypeId::program(name)))
            }
            _ if checked.enums.contains_key(name) => Ok(Type::Enum(name.clone())),
            _ if checked.native_direct_types.contains_key(name) => {
                let (mod_name, ty_name) = &checked.native_direct_types[name];
                Ok(Type::Custom(TypeId::native(*mod_name, ty_name)))
            }
            _ if name.contains('.') => {
                let mut parts = name.splitn(2, '.');
                let alias = parts.next().unwrap_or("");
                let sub = parts.next().unwrap_or("");
                let Some(&mod_name) = checked.native_aliases.get(alias) else {
                    return Err(format!("Unknown type '{}'", name));
                };
                if !is_valid_native_type(mod_name, sub) {
                    return Err(format!("Unknown type '{}'", name));
                }
                Ok(Type::Custom(TypeId::native(mod_name, sub)))
            }
            other => Err(format!("Unknown type '{}'", other)),
        },
        TypeExpr::Generic { name, type_args } => {
            if name == "Map" && type_args.len() == 2 {
                let k = resolve_type_expr(&type_args[0], checked)?;
                validate_map_key_type(&k)?;
                let v = resolve_type_expr(&type_args[1], checked)?;
                Ok(Type::Map(Box::new(k), Box::new(v)))
            } else if checked.generic_structs.contains_key(name) {
                let mut resolved = Vec::with_capacity(type_args.len());
                for ta in type_args {
                    resolved.push(resolve_type_expr(ta, checked)?);
                }
                let mono = crate::frontend::monomorph_subst::format_mangled_name(name, &resolved);
                if checked.struct_by_name.contains_key(&mono) {
                    Ok(Type::Custom(TypeId::program(mono)))
                } else {
                    Err(format!("Unknown generic instantiation '{}'", mono))
                }
            } else {
                Err(format!("Unsupported generic type '{}'", name))
            }
        }
        TypeExpr::List(inner) => Ok(Type::List(Box::new(resolve_type_expr(inner, checked)?))),
        TypeExpr::Optional(inner) => {
            Ok(Type::Optional(Box::new(resolve_type_expr(inner, checked)?)))
        }
        TypeExpr::Union(vars) => {
            let mut out = Vec::with_capacity(vars.len());
            for v in vars {
                out.push(resolve_type_expr(v, checked)?);
            }
            Ok(Type::union(out))
        }
        TypeExpr::Tuple(elems) => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(resolve_type_expr(e, checked)?);
            }
            Ok(Type::Tuple(out))
        }
        TypeExpr::Func {
            params,
            return_type,
        } => {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(resolve_type_expr(p, checked)?);
            }
            let r = resolve_type_expr(return_type, checked)?;
            Ok(Type::Function {
                params: ps,
                return_type: Box::new(r),
            })
        }
    }
}

pub fn type_to_json_type(ty: &Type, structs: &HashMap<String, usize>) -> Option<JsonType> {
    match ty {
        Type::Int => Some(JsonType::Int),
        Type::Float => Some(JsonType::Float),
        Type::Bool => Some(JsonType::Bool),
        Type::String => Some(JsonType::String),
        Type::Optional(inner) => {
            Some(JsonType::Optional(Box::new(type_to_json_type(inner, structs)?)))
        }
        Type::List(inner) => Some(JsonType::List(Box::new(type_to_json_type(inner, structs)?))),
        Type::Map(k, v) if **k == Type::String => {
            Some(JsonType::Map(Box::new(type_to_json_type(v, structs)?)))
        }
        Type::Custom(tid) if tid.module == ModuleId::Program => {
            structs.get(&tid.name).map(|&idx| JsonType::Struct(idx as u32))
        }
        _ => None,
    }
}

pub(crate) fn is_error_type(ty: &Type, checked: &CheckedProgram) -> bool {
    match ty {
        Type::Custom(TypeId {
            module: ModuleId::Builtin,
            name,
        }) => matches!(name.as_str(), "IndexError" | "KeyError" | "ParseError"),
        Type::Custom(TypeId {
            module: ModuleId::Native(m),
            name,
        }) => match m.as_str() {
            "cdt.json" => matches!(name.as_str(), "DecodeError" | "EncodeError"),
            "cdt.time" => name == "TimeError",
            "cdt.validation" => matches!(name.as_str(), "Issue" | "ValidationError"),
            "cdt.env" => name == "EnvError",
            "cdt.cli" => name == "CliError",
            "cdt.fs" => name == "FsError",
            "cdt.process" => name == "ProcessError",
            "cdt.crypto" => name == "CryptoError",
            "cdt.url" => name == "UrlError",
            "cdt.csv" => name == "CsvError",
            "cdt.http" => name == "HttpError",
            "cdt.db" => name == "DbError",
            "cdt.cron" => name == "CronError",
            "cdt.jwt" => name == "JwtError",
            "cdt.lock" => name == "LockError",
            "cdt.cache" => name == "CacheError",
            "cdt.redis" => name == "RedisError",
            _ => false,
        },
        Type::Custom(TypeId {
            module: ModuleId::Program,
            name,
        }) => checked.error_structs.contains(name) || name.ends_with("Error"),
        _ => false,
    }
}

pub(crate) fn can_propagate_try(src_ty: &Type, fn_ret_ty: &Type, checked: &CheckedProgram) -> bool {
    let is_ret_compatible = |err_or_nil: &Type| err_or_nil.is_assignable_to(fn_ret_ty);
    match src_ty {
        Type::Optional(_) => matches!(fn_ret_ty, Type::Optional(_) | Type::Union(_)) && is_ret_compatible(&Type::Nil),
        Type::Union(vars) => {
            let errs: Vec<&Type> = vars.iter().filter(|v| is_error_type(v, checked)).collect();
            !errs.is_empty() && errs.iter().all(|e| is_ret_compatible(e))
        }
        _ => false,
    }
}

#[inline]
pub(crate) fn scalar_kind(ty: &Type) -> u8 {
    match ty {
        Type::Float => 1,
        Type::Bool => 2,
        _ => 0,
    }
}

pub(crate) fn validate_map_key_type(k: &Type) -> Result<(), String> {
    if matches!(
        k,
        Type::Int | Type::String | Type::Bool | Type::Any | Type::TypeParam(_)
    ) {
        Ok(())
    } else {
        Err(format!("Map key type must be Int, String, or Bool, got {}", k))
    }
}

pub(crate) fn native_member_field(tid: &TypeId, field: &str) -> Result<(Type, u16), String> {
    if tid.module == ModuleId::Builtin {
        return match (tid.name.as_str(), field) {
            ("IndexError", "index") => Ok((Type::Int, 0)),
            ("IndexError", "length") => Ok((Type::Int, 1)),
            ("IndexError", "message") => Ok((Type::String, 0)),
            ("KeyError", "key") | ("ParseError", "value") => Ok((Type::String, 0)),
            ("KeyError" | "ParseError", "message") => Ok((Type::String, 1)),
            _ => Err(format!("Type '{}' has no field '{}'", tid, field)),
        };
    }
    let TypeId {
        module: ModuleId::Native(mod_name),
        name,
    } = tid
    else {
        return Err(format!("Unknown native type '{}'", tid));
    };
    match (mod_name.as_str(), name.as_str(), field) {
        ("cdt.time", "TimeError", "message") => Ok((Type::String, 0)),
        ("cdt.validation", "Issue", "code") => Ok((Type::String, 0)),
        ("cdt.validation", "Issue", "message") => Ok((Type::String, 1)),
        ("cdt.validation", "Violation", "path") => Ok((Type::String, 0)),
        ("cdt.validation", "Violation", "code") => Ok((Type::String, 1)),
        ("cdt.validation", "Violation", "message") => Ok((Type::String, 2)),
        ("cdt.validation", "ValidationError", "issues") => Ok((
            Type::List(Box::new(Type::Custom(TypeId::native("cdt.validation", "Violation")))),
            0,
        )),
        ("cdt.json", "DecodeError", "path") => Ok((Type::String, 0)),
        ("cdt.json", "DecodeError", "message") => Ok((Type::String, 1)),
        ("cdt.json", "EncodeError", "message") => Ok((Type::String, 0)),
        ("cdt.env", "EnvError", "name") => Ok((Type::String, 0)),
        ("cdt.env", "EnvError", "message") => Ok((Type::String, 1)),
        ("cdt.cli", "CliError", "kind") => Ok((Type::String, 0)),
        ("cdt.cli", "CliError", "message") => Ok((Type::String, 1)),
        ("cdt.fs", "FsError", "kind") => Ok((Type::String, 0)),
        ("cdt.fs", "FsError", "path") => Ok((Type::String, 1)),
        ("cdt.fs", "FsError", "message") => Ok((Type::String, 2)),
        ("cdt.process", "ProcessError", "kind") => Ok((Type::String, 0)),
        ("cdt.process", "ProcessError", "message") => Ok((Type::String, 1)),
        ("cdt.process", "Output", "code") => Ok((Type::Int, 0)),
        ("cdt.process", "Output", "stdout") => Ok((Type::String, 0)),
        ("cdt.process", "Output", "stderr") => Ok((Type::String, 1)),
        ("cdt.crypto", "CryptoError", "kind") => Ok((Type::String, 0)),
        ("cdt.crypto", "CryptoError", "message") => Ok((Type::String, 1)),
        ("cdt.url", "UrlError", "kind") => Ok((Type::String, 0)),
        ("cdt.url", "UrlError", "message") => Ok((Type::String, 1)),
        ("cdt.url", "Url", "port") => Ok((Type::Int, 0)),
        ("cdt.url", "Url", "scheme") => Ok((Type::String, 0)),
        ("cdt.url", "Url", "host") => Ok((Type::String, 1)),
        ("cdt.url", "Url", "path") => Ok((Type::String, 2)),
        ("cdt.url", "Url", "query") => Ok((Type::String, 3)),
        ("cdt.url", "Url", "fragment") => Ok((Type::String, 4)),
        ("cdt.csv", "CsvError", "line") => Ok((Type::Int, 0)),
        ("cdt.csv", "CsvError", "kind") => Ok((Type::String, 0)),
        ("cdt.csv", "CsvError", "message") => Ok((Type::String, 1)),
        ("cdt.http", "HttpError", "kind") => Ok((Type::String, 0)),
        ("cdt.http", "HttpError", "message") => Ok((Type::String, 1)),
        ("cdt.http", "Request", "method") => Ok((Type::String, 0)),
        ("cdt.http", "Request", "path") => Ok((Type::String, 1)),
        ("cdt.http", "Request", "body") => Ok((Type::String, 2)),
        ("cdt.http", "Request", "headers") => Ok((
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
            3,
        )),
        ("cdt.http", "Request", "query") => Ok((
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
            4,
        )),
        ("cdt.http", "Request", "params") => Ok((
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
            5,
        )),
        ("cdt.http", "Response", "status") => Ok((Type::Int, 0)),
        ("cdt.http", "Response", "body") => Ok((Type::String, 0)),
        ("cdt.http", "Response", "headers") => Ok((
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
            1,
        )),
        ("cdt.http", "BinaryResponse", "status") => Ok((Type::Int, 0)),
        ("cdt.http", "BinaryResponse", "body") => Ok((Type::List(Box::new(Type::Int)), 0)),
        ("cdt.http", "BinaryResponse", "headers") => Ok((
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
            1,
        )),
        ("cdt.db", "DbError", "kind") => Ok((Type::String, 0)),
        ("cdt.db", "DbError", "message") => Ok((Type::String, 1)),
        ("cdt.db", "DbError", "code") => Ok((Type::String, 2)),
        ("cdt.cron", "CronError", "kind") => Ok((Type::String, 0)),
        ("cdt.cron", "CronError", "message") => Ok((Type::String, 1)),
        ("cdt.jwt", "JwtError", "kind") => Ok((Type::String, 0)),
        ("cdt.jwt", "JwtError", "message") => Ok((Type::String, 1)),
        ("cdt.lock", "LockError", "kind") => Ok((Type::String, 0)),
        ("cdt.lock", "LockError", "message") => Ok((Type::String, 1)),
        ("cdt.cache", "CacheError", "kind") => Ok((Type::String, 0)),
        ("cdt.cache", "CacheError", "message") => Ok((Type::String, 1)),
        ("cdt.redis", "RedisError", "kind") => Ok((Type::String, 0)),
        ("cdt.redis", "RedisError", "message") => Ok((Type::String, 1)),
        ("cdt.redis", "Lock", "key") => Ok((Type::String, 0)),
        ("cdt.redis", "Lock", "token") => Ok((Type::String, 1)),
        _ => Err(format!("Type '{}' has no field '{}'", tid, field)),
    }
}

pub(crate) fn validator_email_fn_ty() -> Type {
    Type::Function {
        params: vec![Type::String],
        return_type: Box::new(Type::union(vec![
            Type::Void,
            Type::Custom(TypeId::native("cdt.validation", "Issue")),
        ])),
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn check_time_call(
        &mut self,
        func: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("time.{} does not accept type arguments", func));
        }
        let ts = Type::Custom(TypeId::native("cdt.time", "Timestamp"));
        let inst = Type::Custom(TypeId::native("cdt.time", "Instant"));
        let dur = Type::Custom(TypeId::native("cdt.time", "Duration"));
        let terr = Type::Custom(TypeId::native("cdt.time", "TimeError"));
        if matches!(func, "Timestamp" | "Instant" | "Duration") {
            return Err(format!("Cannot construct opaque type 'time.{}' directly", func));
        }
        if func == "TimeError" {
            return check_fields_ctor(
                self,
                "TimeError",
                &[("message", Type::String)],
                args,
                named_args,
                terr,
            );
        }
        if !named_args.is_empty() {
            return Err(format!("time.{} does not accept named arguments", func));
        }
        match func {
            "now" => {
                if !args.is_empty() {
                    return Err("time.now() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![ts, terr]))
            }
            "monotonic" => {
                if !args.is_empty() {
                    return Err("time.monotonic() takes 0 arguments".to_string());
                }
                Ok(inst)
            }
            "elapsed" => {
                if args.len() != 1 {
                    return Err("time.elapsed(start) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&inst))?;
                if args[0].ty != inst {
                    return Err(format!("time.elapsed expects time.Instant, got {}", args[0].ty));
                }
                Ok(Type::union(vec![dur, terr]))
            }
            "seconds" | "milliseconds" => {
                if args.len() != 1 {
                    return Err(format!("time.{}(value) expects 1 Int argument", func));
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if !args[0].ty.is_int() {
                    return Err(format!("time.{} expects Int, got {}", func, args[0].ty));
                }
                Ok(Type::union(vec![dur, terr]))
            }
            "sleep" => {
                if args.len() != 1 {
                    return Err("time.sleep(ms) expects 1 Int argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if !args[0].ty.is_int() {
                    return Err(format!("time.sleep expects Int milliseconds, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, terr]))
            }
            "from_unix" | "from_unix_millis" => {
                if args.len() != 1 {
                    return Err(format!("time.{}(value) expects 1 Int argument", func));
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if !args[0].ty.is_int() {
                    return Err(format!("time.{} expects Int, got {}", func, args[0].ty));
                }
                Ok(ts)
            }
            "parse_iso" => {
                if args.len() != 1 {
                    return Err("time.parse_iso(s) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("time.parse_iso expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![ts, terr]))
            }
            "format_iso" => {
                if args.len() != 1 {
                    return Err("time.format_iso(ts) expects 1 time.Timestamp argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&ts))?;
                if args[0].ty != ts {
                    return Err(format!("time.format_iso expects time.Timestamp, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::String, terr]))
            }
            other => Err(format!("Unknown cdt.time symbol '{}'", other)),
        }
    }

    pub(crate) fn check_validation_call(
        &mut self,
        func: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("validation.{} does not accept type arguments", func));
        }
        let issue = Type::Custom(TypeId::native("cdt.validation", "Issue"));
        let violation = Type::Custom(TypeId::native("cdt.validation", "Violation"));
        let val_err = Type::Custom(TypeId::native("cdt.validation", "ValidationError"));
        match func {
            "Issue" => check_fields_ctor(
                self,
                "Issue",
                &[("code", Type::String), ("message", Type::String)],
                args,
                named_args,
                issue,
            ),
            "Violation" => check_fields_ctor(
                self,
                "Violation",
                &[
                    ("path", Type::String),
                    ("code", Type::String),
                    ("message", Type::String),
                ],
                args,
                named_args,
                violation,
            ),
            "email" | "url" | "uuid" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("validation.{}(val) expects 1 String argument", func));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("validation.{} expects String, got {}", func, args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "not_empty" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err("validation.not_empty(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], None)?;
                if !matches!(args[0].ty, Type::String | Type::List(_) | Type::Map(_, _)) {
                    return Err(format!(
                        "Unsupported type {} for validation.not_empty (expected String, List, or Map)",
                        args[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "min_len" | "max_len" => {
                if !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("validation.{}(val, len) expects 2 arguments", func));
                }
                let (v_slice, b_slice) = args.split_at_mut(1);
                self.check_expr(&mut v_slice[0], None)?;
                if !matches!(v_slice[0].ty, Type::String | Type::List(_) | Type::Map(_, _)) {
                    return Err(format!(
                        "Unsupported type {} for validation.{} (expected String, List, or Map)",
                        v_slice[0].ty, func
                    ));
                }
                self.check_expr(&mut b_slice[0], Some(&Type::Int))?;
                if !b_slice[0].ty.is_int() {
                    return Err(format!(
                        "validation.{} expects Int length bound, got {}",
                        func, b_slice[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "range" => {
                if !named_args.is_empty() || args.len() != 3 {
                    return Err("validation.range(val, min, max) expects 3 arguments".to_string());
                }
                let (head, rest) = args.split_at_mut(1);
                let (lo_s, hi_s) = rest.split_at_mut(1);
                self.check_expr(&mut head[0], None)?;
                if !head[0].ty.is_int() && !head[0].ty.is_float() {
                    return Err(format!(
                        "validation.range expects Int or Float value, got {}",
                        head[0].ty
                    ));
                }
                let bound_ty = head[0].ty.clone();
                self.check_expr(&mut lo_s[0], Some(&bound_ty))?;
                if lo_s[0].ty != bound_ty {
                    return Err(format!(
                        "validation.range expected {} min bound, got {}",
                        bound_ty, lo_s[0].ty
                    ));
                }
                self.check_expr(&mut hi_s[0], Some(&bound_ty))?;
                if hi_s[0].ty != bound_ty {
                    return Err(format!(
                        "validation.range expected {} max bound, got {}",
                        bound_ty, hi_s[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "one_of" => {
                if !named_args.is_empty() || args.len() != 2 {
                    return Err("validation.one_of(val, allowed) expects 2 arguments".to_string());
                }
                let (v_slice, a_slice) = args.split_at_mut(1);
                self.check_expr(&mut v_slice[0], Some(&Type::String))?;
                if v_slice[0].ty != Type::String {
                    return Err(format!(
                        "validation.one_of expects String value, got {}",
                        v_slice[0].ty
                    ));
                }
                let list_str_ty = Type::List(Box::new(Type::String));
                self.check_expr(&mut a_slice[0], Some(&list_str_ty))?;
                if a_slice[0].ty != list_str_ty {
                    return Err(format!(
                        "validation.one_of expects [String] allowed list, got {}",
                        a_slice[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "min" | "max" => {
                if !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("validation.{}(val, bound) expects 2 arguments", func));
                }
                let (v_slice, b_slice) = args.split_at_mut(1);
                self.check_expr(&mut v_slice[0], None)?;
                let exp_bound = if v_slice[0].ty.is_float() {
                    Type::Float
                } else {
                    Type::Int
                };
                self.check_expr(&mut b_slice[0], Some(&exp_bound))?;
                let ok_val = matches!(
                    v_slice[0].ty,
                    Type::Int | Type::Float | Type::String | Type::List(_) | Type::Map(_, _) | Type::Union(_) | Type::Any
                );
                if !ok_val {
                    return Err(format!("Unsupported type {} for validation.{}", v_slice[0].ty, func));
                }
                if !b_slice[0].ty.is_assignable_to(&exp_bound) && !matches!(v_slice[0].ty, Type::Union(_) | Type::Any) {
                    return Err(format!(
                        "validation.{} expected {} bound, got {}",
                        func, exp_bound, b_slice[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, issue]))
            }
            "check" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err("validation.check(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], None)?;
                Ok(Type::union(vec![args[0].ty.clone(), val_err]))
            }
            other => Err(format!("Unknown cdt.validation symbol '{}'", other)),
        }
    }

    pub(crate) fn check_and_desugar_string_literal(&mut self, expr: &mut Expr) -> Result<Type, String> {
        let span = expr.span;
        let raw = match &expr.kind {
            ExprKind::String(s) => s.clone(),
            _ => unreachable!(),
        };
        let mut parts: Vec<Expr> = Vec::new();
        let mut lit = String::new();
        let mut has_hole = false;
        let mut chars = raw.chars();

        let flush_lit = |buf: &mut String, out: &mut Vec<Expr>| {
            if !buf.is_empty() {
                let mut e = Expr::new(ExprKind::String(std::mem::take(buf)), span);
                e.ty = Type::String;
                out.push(e);
            }
        };

        while let Some(ch) = chars.next() {
            if ch == ESCAPED_OPEN_BRACE {
                lit.push('{');
            } else if ch == ESCAPED_CLOSE_BRACE {
                lit.push('}');
            } else if ch == '{' {
                let mut seg = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    seg.push(inner);
                }
                let seg_parts: Vec<&str> = seg.split('.').collect();
                if closed && !seg.is_empty() && seg_parts.iter().all(|p| is_interp_ident(p)) {
                    has_hole = true;
                    flush_lit(&mut lit, &mut parts);
                    if self.lookup(seg_parts[0]).is_none()
                        && !self.checked.consts.contains_key(seg_parts[0])
                    {
                        return Err(format!(
                            "Unknown variable '{}' in string interpolation",
                            seg_parts[0]
                        ));
                    }
                    let mut hole = Expr::new(ExprKind::Ident(seg_parts[0].to_string()), span);
                    for f in &seg_parts[1..] {
                        hole = Expr::new(
                            ExprKind::Member {
                                target: Box::new(hole),
                                field: (*f).to_string(),
                            },
                            span,
                        );
                    }
                    self.check_expr(&mut hole, None)?;
                    if hole.ty != Type::String {
                        let callee = Expr::new(
                            ExprKind::Member {
                                target: Box::new(hole),
                                field: "to_string".to_string(),
                            },
                            span,
                        );
                        hole = Expr::new(
                            ExprKind::Call {
                                callee: Box::new(callee),
                                type_args: Vec::new(),
                                args: Vec::new(),
                                named_args: Vec::new(),
                            },
                            span,
                        );
                        hole.ty = Type::String;
                    }
                    parts.push(hole);
                } else {
                    lit.push('{');
                    for c in seg.chars() {
                        if c == ESCAPED_OPEN_BRACE {
                            lit.push('{');
                        } else if c == ESCAPED_CLOSE_BRACE {
                            lit.push('}');
                        } else {
                            lit.push(c);
                        }
                    }
                    if closed {
                        lit.push('}');
                    }
                }
            } else {
                lit.push(ch);
            }
        }

        if !has_hole {
            expr.kind = ExprKind::String(lit);
            return Ok(Type::String);
        }
        flush_lit(&mut lit, &mut parts);
        let mut iter = parts.into_iter();
        let mut acc = iter.next().unwrap();
        for rhs in iter {
            acc = Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(acc),
                    right: Box::new(rhs),
                },
                span,
            );
            acc.ty = Type::String;
        }
        *expr = acc;
        Ok(Type::String)
    }

    pub(crate) fn check_native_method(
        &mut self,
        tid: &TypeId,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
    ) -> Result<Type, String> {
        let terr = Type::Custom(TypeId::native("cdt.time", "TimeError"));
        match (tid.module.clone(), tid.name.as_str(), method) {
            (ModuleId::Native(ref m), _, _) if m == "cdt.fs" => {
                if !type_args.is_empty() {
                    return Err(format!("Method '.{}()' does not accept type arguments", method));
                }
                self.check_fs_method(&tid.name, method, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.http" => {
                if !type_args.is_empty() {
                    return Err(format!("Method '.{}()' does not accept type arguments", method));
                }
                self.check_http_method(&tid.name, method, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.db" => {
                self.check_db_method(&tid.name, method, type_args, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.cron" => {
                if !type_args.is_empty() {
                    return Err(format!("Method '.{}()' does not accept type arguments", method));
                }
                self.check_cron_method(&tid.name, method, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.lock" => {
                if !type_args.is_empty() {
                    return Err(format!("Method '.{}()' does not accept type arguments", method));
                }
                self.check_lock_method(&tid.name, method, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.cache" => {
                self.check_cache_method(&tid.name, method, type_args, args)
            }
            (ModuleId::Native(ref m), _, _) if m == "cdt.redis" => {
                self.check_redis_method(&tid.name, method, type_args, args)
            }
            (ModuleId::Native(ref m), "Timestamp", "unix_seconds" | "unix_milliseconds")
                if m == "cdt.time" =>
            {
                if !args.is_empty() {
                    return Err(format!("Timestamp.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Int, terr]))
            }
            (ModuleId::Native(ref m), "Timestamp", "to_iso") if m == "cdt.time" => {
                if !args.is_empty() {
                    return Err("Timestamp.to_iso() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::String, terr]))
            }
            (
                ModuleId::Native(ref m),
                "Timestamp",
                "year" | "month" | "day" | "hour" | "minute" | "second" | "weekday",
            ) if m == "cdt.time" => {
                if !args.is_empty() {
                    return Err(format!("Timestamp.{}() takes 0 arguments", method));
                }
                Ok(Type::Int)
            }
            (ModuleId::Native(ref m), "Timestamp", "add" | "sub") if m == "cdt.time" => {
                let dur = Type::Custom(TypeId::native("cdt.time", "Duration"));
                let ts = Type::Custom(TypeId::native("cdt.time", "Timestamp"));
                if args.len() != 1 {
                    return Err(format!("Timestamp.{}(dur) expects 1 Duration argument", method));
                }
                self.check_expr(&mut args[0], Some(&dur))?;
                if args[0].ty != dur {
                    return Err(format!("Timestamp.{} expects time.Duration, got {}", method, args[0].ty));
                }
                Ok(Type::union(vec![ts, terr]))
            }
            (ModuleId::Native(ref m), "Timestamp", "diff") if m == "cdt.time" => {
                let dur = Type::Custom(TypeId::native("cdt.time", "Duration"));
                let ts = Type::Custom(TypeId::native("cdt.time", "Timestamp"));
                if args.len() != 1 {
                    return Err("Timestamp.diff(other) expects 1 Timestamp argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&ts))?;
                if args[0].ty != ts {
                    return Err(format!("Timestamp.diff expects time.Timestamp, got {}", args[0].ty));
                }
                Ok(Type::union(vec![dur, terr]))
            }
            (ModuleId::Native(ref m), "Duration", "seconds" | "milliseconds" | "nanoseconds")
                if m == "cdt.time" =>
            {
                if !args.is_empty() {
                    return Err(format!("Duration.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Int, terr]))
            }
            _ => Err(format!("Type '{}' has no method '.{}()'", tid, method)),
        }
    }

    pub(crate) fn check_env_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let env_err = Type::Custom(TypeId::native("cdt.env", "EnvError"));
        match sym {
            "EnvError" => check_fields_ctor(
                self,
                "EnvError",
                &[("name", Type::String), ("message", Type::String)],
                args,
                named_args,
                env_err,
            ),
            "get" | "require" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("env.{}(name) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("env.{} expects String, got {}", sym, args[0].ty));
                }
                if sym == "get" {
                    Ok(Type::Optional(Box::new(Type::String)))
                } else {
                    Ok(Type::union(vec![Type::String, env_err]))
                }
            }
            "args" => {
                if !type_args.is_empty() || !named_args.is_empty() || !args.is_empty() {
                    return Err("env.args() takes 0 arguments".to_string());
                }
                Ok(Type::List(Box::new(Type::String)))
            }
            "load" => {
                if type_args.len() != 1 || !named_args.is_empty() || !args.is_empty() {
                    return Err("env.load<T>() expects 1 type argument and 0 value arguments".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("env.load<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!("env.load<T>() requires a program struct, got {}", target_ty));
                }
                let sid = self.checked.struct_by_name[&tid.name];
                let st = &self.checked.structs[sid];
                let mut fields = Vec::with_capacity(st.fields.len());
                for f in &st.fields {
                    let env_name = f
                        .env_name
                        .clone()
                        .unwrap_or_else(|| f.name.to_ascii_uppercase());
                    let (is_optional, inner_ty) = match &f.ty {
                        Type::Optional(inner) => (true, (**inner).clone()),
                        other => (false, other.clone()),
                    };
                    let kind = match inner_ty {
                        Type::String => crate::native::env::EnvFieldKind::String,
                        Type::Int => crate::native::env::EnvFieldKind::Int,
                        Type::Float => crate::native::env::EnvFieldKind::Float,
                        Type::Bool => crate::native::env::EnvFieldKind::Bool,
                        _ => {
                            return Err(format!(
                                "env.load<{}>: field '{}' has unsupported type {} (expected String, Int, Float, Bool, or optional)",
                                tid.name, f.name, f.ty
                            ));
                        }
                    };
                    let default = match &f.default {
                        Some(expr) => Some(expr_to_env_default(expr).ok_or_else(|| {
                            format!(
                                "env.load<{}>: default value for field '{}' must be a literal constant",
                                tid.name, f.name
                            )
                        })?),
                        None => None,
                    };
                    fields.push(crate::native::env::EnvFieldPlan {
                        env_name,
                        kind,
                        is_optional,
                        is_scalar: f.is_scalar,
                        slot_idx: f.slot_idx,
                        default,
                    });
                }
                if !self.checked.env_plans.iter().any(|p| p.schema_id == st.schema_id) {
                    self.checked.env_plans.push(crate::native::env::EnvLoadPlan {
                        schema_id: st.schema_id,
                        scalar_len: st.scalar_len,
                        obj_len: st.obj_len,
                        fields: fields.into_boxed_slice(),
                    });
                }
                Ok(Type::union(vec![target_ty, env_err]))
            }
            other => Err(format!("Unknown cdt.env symbol '{}'", other)),
        }
    }
}

fn expr_to_env_default(expr: &Expr) -> Option<crate::native::env::EnvDefault> {
    use crate::native::env::EnvDefault;
    match &expr.kind {
        ExprKind::Int(v) => Some(EnvDefault::Int(*v)),
        ExprKind::Float(v) => Some(EnvDefault::Float(v.to_bits())),
        ExprKind::Bool(b) => Some(EnvDefault::Bool(*b)),
        ExprKind::String(s) => Some(EnvDefault::Str(s.clone())),
        ExprKind::Nil => Some(EnvDefault::Nil),
        ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => match &inner.kind {
            ExprKind::Int(v) => Some(EnvDefault::Int(-*v)),
            ExprKind::Float(v) => Some(EnvDefault::Float((-*v).to_bits())),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn check_fields_ctor(
    tc: &mut Checker<'_>,
    name: &str,
    fields: &[(&str, Type)],
    args: &mut [Expr],
    named_args: &mut [NamedArg],
    ret_ty: Type,
) -> Result<Type, String> {
    if !named_args.is_empty() {
        if !args.is_empty() || named_args.len() != fields.len() {
            return Err(format!("'{}' expects {} named arguments", name, fields.len()));
        }
        let mut seen = HashSet::new();
        for na in named_args.iter_mut() {
            if !seen.insert(na.name.clone()) {
                return Err(format!("Duplicate field '{}' in '{}'", na.name, name));
            }
            let (_, fty) = fields
                .iter()
                .find(|(fnm, _)| *fnm == na.name)
                .ok_or_else(|| format!("Type '{}' has no field '{}'", name, na.name))?;
            tc.check_expr(&mut na.value, Some(fty))?;
            if !na.value.ty.is_assignable_to(fty) {
                return Err(format!("Field '{}' expected {}, got {}", na.name, fty, na.value.ty));
            }
        }
    } else {
        if args.len() != fields.len() {
            return Err(format!("'{}' expects {} arguments", name, fields.len()));
        }
        for (arg, (fname, fty)) in args.iter_mut().zip(fields.iter()) {
            tc.check_expr(arg, Some(fty))?;
            if !arg.ty.is_assignable_to(fty) {
                return Err(format!("Field '{}' expected {}, got {}", fname, fty, arg.ty));
            }
        }
    }
    Ok(ret_ty)
}

fn is_interp_ident(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}
