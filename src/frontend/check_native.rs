use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, Checker};
use crate::frontend::lexer::{ESCAPED_CLOSE_BRACE, ESCAPED_OPEN_BRACE};
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::native::JsonType;

pub(crate) fn is_valid_native_type(mod_name: &str, ty_name: &str) -> bool {
    match mod_name {
        "cdt.json" => matches!(ty_name, "DecodeError" | "EncodeError"),
        "cdt.time" => matches!(ty_name, "Timestamp" | "Instant" | "Duration" | "TimeError"),
        "cdt.validation" => matches!(ty_name, "Issue" | "Violation" | "ValidationError"),
        "cdt.env" => ty_name == "EnvError",
        "cdt.cli" => ty_name == "CliError",
        "cdt.fs" => matches!(ty_name, "FsError" | "Reader" | "Writer"),
        "cdt.process" => matches!(ty_name, "ProcessError" | "Output"),
        "cdt.crypto" => ty_name == "CryptoError",
        "cdt.url" => matches!(ty_name, "UrlError" | "Url"),
        "cdt.csv" => ty_name == "CsvError",
        _ => false,
    }
}

pub(crate) fn register_native_imports(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    for item in &program.items {
        let ItemKind::Import { path, alias, names } = &item.kind else {
            continue;
        };
        match path.as_str() {
            "cdt.json" => {
                if let Some(a) = alias {
                    checked.json_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "decode" | "encode" => insert_unique(
                            &mut checked.json_direct_fns,
                            &spec.alias,
                            &spec.name,
                        )?,
                        "DecodeError" | "EncodeError" => {
                            checked.json_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.json' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.time" => {
                if let Some(a) = alias {
                    checked.time_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "now" | "monotonic" | "elapsed" | "seconds" | "milliseconds" => {
                            insert_unique(&mut checked.time_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "Timestamp" | "Instant" | "Duration" | "TimeError" => {
                            checked.time_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.time' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.validation" => {
                if let Some(a) = alias {
                    checked.val_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "min" | "max" | "email" | "check" => {
                            insert_unique(&mut checked.val_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "Issue" | "Violation" | "ValidationError" => {
                            checked.val_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.validation' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.env" => {
                if let Some(a) = alias {
                    checked.env_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "get" | "require" | "args" | "load" => {
                            insert_unique(&mut checked.env_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "EnvError" => {
                            checked.env_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.env' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.cli" => {
                if let Some(a) = alias {
                    checked.cli_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "parse" | "parse_args" | "help" => {
                            insert_unique(&mut checked.cli_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "CliError" => {
                            checked.cli_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.cli' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.fs" => {
                if let Some(a) = alias {
                    checked.fs_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "read_text" | "write_text" | "append_text" | "write_atomic"
                        | "read_lines" | "each_line" | "exists" | "is_file" | "is_dir" | "size"
                        | "list_dir" | "mkdir_all" | "remove" | "copy" | "rename"
                        | "open_read" | "open_write" | "open_append" => {
                            insert_unique(&mut checked.fs_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "FsError" | "Reader" | "Writer" => {
                            checked.fs_direct_types.insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!("Module 'cdt.fs' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.path" => {
                if let Some(a) = alias {
                    checked.path_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "join" | "join_all" | "dirname" | "basename" | "ext" | "stem"
                        | "normalize" | "is_abs" | "with_ext" => {
                            insert_unique(&mut checked.path_direct_fns, &spec.alias, &spec.name)?;
                        }
                        other => {
                            return Err(format!("Module 'cdt.path' has no exported member '{}'", other));
                        }
                    }
                }
            }
            "cdt.process" => {
                if let Some(a) = alias {
                    checked.process_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "run" | "status" | "cwd" | "set_cwd" | "pid" | "exit" => {
                            insert_unique(&mut checked.process_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "ProcessError" | "Output" => {
                            checked
                                .process_direct_types
                                .insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.process' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.crypto" => {
                if let Some(a) = alias {
                    checked.crypto_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "sha256" | "hmac_sha256" | "base64_encode" | "base64_decode"
                        | "hex_encode" | "hex_decode" | "uuid_v4" | "random_bytes_hex"
                        | "random_int" => {
                            insert_unique(&mut checked.crypto_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "CryptoError" => {
                            checked
                                .crypto_direct_types
                                .insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.crypto' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.url" => {
                if let Some(a) = alias {
                    checked.url_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "parse" | "encode" | "decode" | "encode_query" | "decode_query" => {
                            insert_unique(&mut checked.url_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "UrlError" | "Url" => {
                            checked
                                .url_direct_types
                                .insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.url' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.csv" => {
                if let Some(a) = alias {
                    checked.csv_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "parse" | "parse_with" | "parse_records" | "stringify" | "stringify_with" => {
                            insert_unique(&mut checked.csv_direct_fns, &spec.alias, &spec.name)?;
                        }
                        "CsvError" => {
                            checked
                                .csv_direct_types
                                .insert(spec.alias.clone(), spec.name.clone());
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.csv' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.math" => {
                if let Some(a) = alias {
                    checked.math_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "pi" | "e" | "tau" | "sqrt" | "sin" | "cos" | "tan" | "asin" | "acos"
                        | "atan" | "exp" | "ln" | "log10" | "floor" | "ceil" | "round" | "trunc"
                        | "pow" | "atan2" | "hypot" => {
                            insert_unique(&mut checked.math_direct_fns, &spec.alias, &spec.name)?;
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.math' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            "cdt.log" => {
                if let Some(a) = alias {
                    checked.log_aliases.insert(a.clone());
                }
                for spec in names {
                    match spec.name.as_str() {
                        "debug" | "info" | "warn" | "error" | "set_level" | "set_json"
                        | "get_level" => {
                            insert_unique(&mut checked.log_direct_fns, &spec.alias, &spec.name)?;
                        }
                        other => {
                            return Err(format!(
                                "Module 'cdt.log' has no exported member '{}'",
                                other
                            ));
                        }
                    }
                }
            }
            other => return Err(format!("Unknown or unlinked module '{}'", other)),
        }
    }
    Ok(())
}

fn insert_unique(
    map: &mut HashMap<String, String>,
    alias: &str,
    target: &str,
) -> Result<(), String> {
    if let Some(prev) = map.insert(alias.to_string(), target.to_string())
        && prev != target
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
            _ if checked.json_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.json",
                &checked.json_direct_types[name],
            ))),
            _ if checked.time_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.time",
                &checked.time_direct_types[name],
            ))),
            _ if checked.val_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.validation",
                &checked.val_direct_types[name],
            ))),
            _ if checked.env_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.env",
                &checked.env_direct_types[name],
            ))),
            _ if checked.cli_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.cli",
                &checked.cli_direct_types[name],
            ))),
            _ if checked.fs_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.fs",
                &checked.fs_direct_types[name],
            ))),
            _ if checked.process_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.process",
                &checked.process_direct_types[name],
            ))),
            _ if checked.crypto_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.crypto",
                &checked.crypto_direct_types[name],
            ))),
            _ if checked.url_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.url",
                &checked.url_direct_types[name],
            ))),
            _ if checked.csv_direct_types.contains_key(name) => Ok(Type::Custom(TypeId::native(
                "cdt.csv",
                &checked.csv_direct_types[name],
            ))),
            _ if name.contains('.') => {
                let mut parts = name.splitn(2, '.');
                let alias = parts.next().unwrap_or("");
                let sub = parts.next().unwrap_or("");
                let mod_name = if checked.json_aliases.contains(alias) {
                    "cdt.json"
                } else if checked.time_aliases.contains(alias) {
                    "cdt.time"
                } else if checked.val_aliases.contains(alias) {
                    "cdt.validation"
                } else if checked.env_aliases.contains(alias) {
                    "cdt.env"
                } else if checked.cli_aliases.contains(alias) {
                    "cdt.cli"
                } else if checked.fs_aliases.contains(alias) {
                    "cdt.fs"
                } else if checked.process_aliases.contains(alias) {
                    "cdt.process"
                } else if checked.crypto_aliases.contains(alias) {
                    "cdt.crypto"
                } else if checked.url_aliases.contains(alias) {
                    "cdt.url"
                } else if checked.csv_aliases.contains(alias) {
                    "cdt.csv"
                } else {
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
            "email" => {
                if !named_args.is_empty() || args.len() != 1 {
                    return Err("validation.email(val) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("validation.email expects String, got {}", args[0].ty));
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
        args: &mut [Expr],
    ) -> Result<Type, String> {
        let terr = Type::Custom(TypeId::native("cdt.time", "TimeError"));
        match (tid.module.clone(), tid.name.as_str(), method) {
            (ModuleId::Native(ref m), _, _) if m == "cdt.fs" => {
                self.check_fs_method(&tid.name, method, args)
            }
            (ModuleId::Native(ref m), "Timestamp", "unix_seconds" | "unix_milliseconds")
                if m == "cdt.time" =>
            {
                if !args.is_empty() {
                    return Err(format!("Timestamp.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Int, terr]))
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
