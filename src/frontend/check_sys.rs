use crate::frontend::ast::{Expr, NamedArg, TypeExpr};
use crate::frontend::check::Checker;
use crate::frontend::check_native::check_fields_ctor;
use crate::frontend::types::{Type, TypeId};

impl Checker<'_> {
    pub(crate) fn check_path_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() || !named_args.is_empty() {
            return Err(format!("path.{} does not accept type or named arguments", sym));
        }
        match sym {
            "join" | "with_ext" => {
                if args.len() != 2 {
                    return Err(format!("path.{} expects 2 String arguments", sym));
                }
                let (a0, a1) = args.split_at_mut(1);
                self.check_expr(&mut a0[0], Some(&Type::String))?;
                if a0[0].ty != Type::String {
                    return Err(format!("path.{} expects String, got {}", sym, a0[0].ty));
                }
                self.check_expr(&mut a1[0], Some(&Type::String))?;
                if a1[0].ty != Type::String {
                    return Err(format!("path.{} expects String, got {}", sym, a1[0].ty));
                }
                Ok(Type::String)
            }
            "join_all" => {
                if args.len() != 1 {
                    return Err("path.join_all(parts) expects 1 [String] argument".to_string());
                }
                let expected = Type::List(Box::new(Type::String));
                self.check_expr(&mut args[0], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!("path.join_all expects [String], got {}", args[0].ty));
                }
                Ok(Type::String)
            }
            "dirname" | "basename" | "ext" | "stem" | "normalize" | "is_abs" => {
                if args.len() != 1 {
                    return Err(format!("path.{}(p) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("path.{} expects String, got {}", sym, args[0].ty));
                }
                if sym == "is_abs" {
                    Ok(Type::Bool)
                } else {
                    Ok(Type::String)
                }
            }
            other => Err(format!("Unknown cdt.path symbol '{}'", other)),
        }
    }

    pub(crate) fn check_process_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let proc_err = Type::Custom(TypeId::native("cdt.process", "ProcessError"));
        let proc_out = Type::Custom(TypeId::native("cdt.process", "Output"));
        match sym {
            "ProcessError" => {
                if !type_args.is_empty() {
                    return Err("ProcessError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "ProcessError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    proc_err,
                )
            }
            "Output" => {
                if !type_args.is_empty() {
                    return Err("Output does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "Output",
                    &[
                        ("code", Type::Int),
                        ("stdout", Type::String),
                        ("stderr", Type::String),
                    ],
                    args,
                    named_args,
                    proc_out,
                )
            }
            "run" | "status" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("process.{}(cmd, args) expects 2 arguments", sym));
                }
                let (a0, a1) = args.split_at_mut(1);
                self.check_expr(&mut a0[0], Some(&Type::String))?;
                if a0[0].ty != Type::String {
                    return Err(format!("process.{} expects String cmd, got {}", sym, a0[0].ty));
                }
                let expected_args = Type::List(Box::new(Type::String));
                self.check_expr(&mut a1[0], Some(&expected_args))?;
                if a1[0].ty != expected_args {
                    return Err(format!("process.{} expects [String] args, got {}", sym, a1[0].ty));
                }
                if sym == "run" {
                    Ok(Type::union(vec![proc_out, proc_err]))
                } else {
                    Ok(Type::union(vec![Type::Int, proc_err]))
                }
            }
            "cwd" | "pid" => {
                if !type_args.is_empty() || !named_args.is_empty() || !args.is_empty() {
                    return Err(format!("process.{}() takes 0 arguments", sym));
                }
                if sym == "cwd" {
                    Ok(Type::union(vec![Type::String, proc_err]))
                } else {
                    Ok(Type::Int)
                }
            }
            "set_cwd" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("process.set_cwd(dir) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("process.set_cwd expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, proc_err]))
            }
            "exit" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("process.exit(code) expects 1 Int argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!("process.exit expects Int, got {}", args[0].ty));
                }
                Ok(Type::Void)
            }
            other => Err(format!("Unknown cdt.process symbol '{}'", other)),
        }
    }

    pub(crate) fn check_crypto_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let crypto_err = Type::Custom(TypeId::native("cdt.crypto", "CryptoError"));
        match sym {
            "CryptoError" => {
                if !type_args.is_empty() {
                    return Err("CryptoError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "CryptoError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    crypto_err,
                )
            }
            "sha256" | "base64_encode" | "base64_decode" | "hex_encode" | "hex_decode" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("crypto.{}(data) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("crypto.{} expects String, got {}", sym, args[0].ty));
                }
                if matches!(sym, "base64_decode" | "hex_decode") {
                    Ok(Type::union(vec![Type::String, crypto_err]))
                } else {
                    Ok(Type::String)
                }
            }
            "hmac_sha256" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("crypto.hmac_sha256(key, data) expects 2 String arguments".to_string());
                }
                let (a0, a1) = args.split_at_mut(1);
                self.check_expr(&mut a0[0], Some(&Type::String))?;
                if a0[0].ty != Type::String {
                    return Err(format!("crypto.hmac_sha256 expects String key, got {}", a0[0].ty));
                }
                self.check_expr(&mut a1[0], Some(&Type::String))?;
                if a1[0].ty != Type::String {
                    return Err(format!("crypto.hmac_sha256 expects String data, got {}", a1[0].ty));
                }
                Ok(Type::String)
            }
            "uuid_v4" => {
                if !type_args.is_empty() || !named_args.is_empty() || !args.is_empty() {
                    return Err("crypto.uuid_v4() takes 0 arguments".to_string());
                }
                Ok(Type::String)
            }
            "random_bytes_hex" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("crypto.random_bytes_hex(len) expects 1 Int argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!(
                        "crypto.random_bytes_hex expects Int, got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::String, crypto_err]))
            }
            "random_int" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("crypto.random_int(min, max) expects 2 Int arguments".to_string());
                }
                let (a0, a1) = args.split_at_mut(1);
                self.check_expr(&mut a0[0], Some(&Type::Int))?;
                if a0[0].ty != Type::Int {
                    return Err(format!("crypto.random_int expects Int min, got {}", a0[0].ty));
                }
                self.check_expr(&mut a1[0], Some(&Type::Int))?;
                if a1[0].ty != Type::Int {
                    return Err(format!("crypto.random_int expects Int max, got {}", a1[0].ty));
                }
                Ok(Type::union(vec![Type::Int, crypto_err]))
            }
            other => Err(format!("Unknown cdt.crypto symbol '{}'", other)),
        }
    }

    pub(crate) fn check_url_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let url_err = Type::Custom(TypeId::native("cdt.url", "UrlError"));
        let url_ty = Type::Custom(TypeId::native("cdt.url", "Url"));
        match sym {
            "UrlError" => {
                if !type_args.is_empty() {
                    return Err("UrlError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "UrlError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    url_err,
                )
            }
            "Url" => {
                if !type_args.is_empty() {
                    return Err("Url does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "Url",
                    &[
                        ("scheme", Type::String),
                        ("host", Type::String),
                        ("port", Type::Int),
                        ("path", Type::String),
                        ("query", Type::String),
                        ("fragment", Type::String),
                    ],
                    args,
                    named_args,
                    url_ty,
                )
            }
            "parse" | "encode" | "decode" | "decode_query" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("url.{}(s) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("url.{} expects String, got {}", sym, args[0].ty));
                }
                match sym {
                    "parse" => Ok(Type::union(vec![url_ty, url_err])),
                    "encode" => Ok(Type::String),
                    "decode" => Ok(Type::union(vec![Type::String, url_err])),
                    "decode_query" => Ok(Type::union(vec![
                        Type::Map(Box::new(Type::String), Box::new(Type::String)),
                        url_err,
                    ])),
                    _ => unreachable!(),
                }
            }
            "encode_query" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(
                        "url.encode_query(params) expects 1 Map<String, String> argument"
                            .to_string(),
                    );
                }
                let expected = Type::Map(Box::new(Type::String), Box::new(Type::String));
                self.check_expr(&mut args[0], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!(
                        "url.encode_query expects Map<String, String>, got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::String)
            }
            other => Err(format!("Unknown cdt.url symbol '{}'", other)),
        }
    }
}
