use crate::frontend::ast::{Expr, ExprKind, NamedArg, TypeExpr};
use crate::frontend::check::{CheckedProgram, Checker, HttpRouterDeclEntry, HttpRouterDeclKind};
use crate::frontend::check_native::check_fields_ctor;
use crate::frontend::types::{ModuleId, Type, TypeId};

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
            "sha256" | "base64_encode" | "base64_decode" | "hex_encode" | "hex_decode"
            | "hash_password" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("crypto.{}(data) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("crypto.{} expects String, got {}", sym, args[0].ty));
                }
                if matches!(sym, "base64_decode" | "hex_decode" | "hash_password") {
                    Ok(Type::union(vec![Type::String, crypto_err]))
                } else {
                    Ok(Type::String)
                }
            }
            "verify_password" | "constant_time_eq" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("crypto.{}(a, b) expects 2 String arguments", sym));
                }
                for (i, arg) in args.iter_mut().enumerate() {
                    self.check_expr(arg, Some(&Type::String))?;
                    if arg.ty != Type::String {
                        return Err(format!(
                            "crypto.{} argument {} expects String, got {}",
                            sym,
                            i + 1,
                            arg.ty
                        ));
                    }
                }
                Ok(Type::Bool)
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

    pub(crate) fn check_csv_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let csv_err = Type::Custom(TypeId::native("cdt.csv", "CsvError"));
        let rows_ty = Type::List(Box::new(Type::List(Box::new(Type::String))));
        let records_ty = Type::List(Box::new(Type::Map(
            Box::new(Type::String),
            Box::new(Type::String),
        )));
        match sym {
            "CsvError" => {
                if !type_args.is_empty() {
                    return Err("CsvError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "CsvError",
                    &[
                        ("kind", Type::String),
                        ("line", Type::Int),
                        ("message", Type::String),
                    ],
                    args,
                    named_args,
                    csv_err,
                )
            }
            "parse" | "parse_records" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("csv.{}(text) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("csv.{} expects String, got {}", sym, args[0].ty));
                }
                if sym == "parse" {
                    Ok(Type::union(vec![rows_ty, csv_err]))
                } else {
                    Ok(Type::union(vec![records_ty, csv_err]))
                }
            }
            "parse_with" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("csv.parse_with(text, delimiter) expects 2 String arguments".to_string());
                }
                for (i, arg) in args.iter_mut().enumerate() {
                    self.check_expr(arg, Some(&Type::String))?;
                    if arg.ty != Type::String {
                        return Err(format!(
                            "csv.parse_with argument {} expects String, got {}",
                            i + 1,
                            arg.ty
                        ));
                    }
                }
                Ok(Type::union(vec![rows_ty, csv_err]))
            }
            "stringify" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("csv.stringify(rows) expects 1 [[String]] argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&rows_ty))?;
                if args[0].ty != rows_ty {
                    return Err(format!("csv.stringify expects [[String]], got {}", args[0].ty));
                }
                Ok(Type::String)
            }
            "stringify_with" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err(
                        "csv.stringify_with(rows, delimiter) expects [[String]] and String"
                            .to_string(),
                    );
                }
                self.check_expr(&mut args[0], Some(&rows_ty))?;
                if args[0].ty != rows_ty {
                    return Err(format!(
                        "csv.stringify_with expects [[String]], got {}",
                        args[0].ty
                    ));
                }
                self.check_expr(&mut args[1], Some(&Type::String))?;
                if args[1].ty != Type::String {
                    return Err(format!(
                        "csv.stringify_with delimiter expects String, got {}",
                        args[1].ty
                    ));
                }
                Ok(Type::union(vec![Type::String, csv_err]))
            }
            other => Err(format!("Unknown cdt.csv symbol '{}'", other)),
        }
    }

    pub(crate) fn check_math_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() || !named_args.is_empty() {
            return Err(format!("math.{} does not accept type or named arguments", sym));
        }
        match sym {
            "pi" | "e" | "tau" => {
                if !args.is_empty() {
                    return Err(format!("math.{}() expects 0 arguments", sym));
                }
                Ok(Type::Float)
            }
            "sqrt" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "exp" | "ln"
            | "log10" | "floor" | "ceil" | "round" | "trunc" => {
                if args.len() != 1 {
                    return Err(format!("math.{}(x) expects 1 Float argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::Float))?;
                if args[0].ty != Type::Float {
                    return Err(format!("math.{} expects Float, got {}", sym, args[0].ty));
                }
                Ok(Type::Float)
            }
            "pow" | "atan2" | "hypot" => {
                if args.len() != 2 {
                    return Err(format!("math.{}(a, b) expects 2 Float arguments", sym));
                }
                for (i, arg) in args.iter_mut().enumerate() {
                    self.check_expr(arg, Some(&Type::Float))?;
                    if arg.ty != Type::Float {
                        return Err(format!(
                            "math.{} argument {} expects Float, got {}",
                            sym,
                            i + 1,
                            arg.ty
                        ));
                    }
                }
                Ok(Type::Float)
            }
            other => Err(format!("Unknown cdt.math symbol '{}'", other)),
        }
    }

    pub(crate) fn check_log_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() || !named_args.is_empty() {
            return Err(format!("log.{} does not accept type or named arguments", sym));
        }
        match sym {
            "debug" | "info" | "warn" | "error" | "set_level" => {
                if args.len() != 1 {
                    return Err(format!("log.{}(msg) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("log.{} expects String, got {}", sym, args[0].ty));
                }
                Ok(Type::Void)
            }
            "set_json" => {
                if args.len() != 1 {
                    return Err("log.set_json(enabled) expects 1 Bool argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Bool))?;
                if args[0].ty != Type::Bool {
                    return Err(format!("log.set_json expects Bool, got {}", args[0].ty));
                }
                Ok(Type::Void)
            }
            "get_level" => {
                if !args.is_empty() {
                    return Err("log.get_level() expects 0 arguments".to_string());
                }
                Ok(Type::String)
            }
            other => Err(format!("Unknown cdt.log symbol '{}'", other)),
        }
    }

    pub(crate) fn check_http_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let http_err = Type::Custom(TypeId::native("cdt.http", "HttpError"));
        let req_ty = Type::Custom(TypeId::native("cdt.http", "Request"));
        let resp_ty = Type::Custom(TypeId::native("cdt.http", "Response"));
        let bin_resp_ty = Type::Custom(TypeId::native("cdt.http", "BinaryResponse"));
        let router_ty = Type::Custom(TypeId::native("cdt.http", "Router"));
        let str_map = Type::Map(Box::new(Type::String), Box::new(Type::String));
        let int_list = Type::List(Box::new(Type::Int));

        match sym {
            "HttpError" => {
                if !type_args.is_empty() {
                    return Err("HttpError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "HttpError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    http_err,
                )
            }
            "Request" => {
                if !type_args.is_empty() {
                    return Err("Request does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "Request",
                    &[
                        ("method", Type::String),
                        ("path", Type::String),
                        ("body", Type::String),
                        ("headers", str_map.clone()),
                        ("query", str_map.clone()),
                        ("params", str_map),
                    ],
                    args,
                    named_args,
                    req_ty,
                )
            }
            "Response" => {
                if !type_args.is_empty() {
                    return Err("Response does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "Response",
                    &[
                        ("status", Type::Int),
                        ("body", Type::String),
                        ("headers", str_map),
                    ],
                    args,
                    named_args,
                    resp_ty,
                )
            }
            "BinaryResponse" => {
                if !type_args.is_empty() {
                    return Err("BinaryResponse does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "BinaryResponse",
                    &[
                        ("status", Type::Int),
                        ("body", int_list),
                        ("headers", str_map),
                    ],
                    args,
                    named_args,
                    bin_resp_ty,
                )
            }
            "text" | "html" | "json" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("http.{}(body) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.{} expects String, got {}", sym, args[0].ty));
                }
                Ok(resp_ty)
            }
            "response" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("http.response(status, body) expects Int and String".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!("http.response status expects Int, got {}", args[0].ty));
                }
                self.check_expr(&mut args[1], Some(&Type::String))?;
                if args[1].ty != Type::String {
                    return Err(format!("http.response body expects String, got {}", args[1].ty));
                }
                Ok(resp_ty)
            }
            "response_with" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 3 {
                    return Err(
                        "http.response_with(status, body, headers) expects (Int, String, Map<String, String>)"
                            .to_string(),
                    );
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!("http.response_with status expects Int, got {}", args[0].ty));
                }
                self.check_expr(&mut args[1], Some(&Type::String))?;
                if args[1].ty != Type::String {
                    return Err(format!("http.response_with body expects String, got {}", args[1].ty));
                }
                self.check_expr(&mut args[2], Some(&str_map))?;
                if args[2].ty != str_map {
                    return Err(format!(
                        "http.response_with headers expects Map<String, String>, got {}",
                        args[2].ty
                    ));
                }
                Ok(resp_ty)
            }
            "router" => {
                if !type_args.is_empty() || !named_args.is_empty() || !args.is_empty() {
                    return Err("http.router() takes 0 arguments".to_string());
                }
                Ok(router_ty)
            }
            "build" | "serve" => {
                let expect_args = usize::from(sym == "serve");
                if type_args.len() != 1 || !named_args.is_empty() || args.len() != expect_args {
                    return Err(format!(
                        "http.{}<T>({}) expects 1 type argument and {} value argument(s)",
                        sym,
                        if expect_args == 1 { "addr" } else { "" },
                        expect_args
                    ));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("http.{}<T>() requires a router type, got {}", sym, target_ty));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!("http.{}<T>() requires a program router type, got {}", sym, target_ty));
                }
                if expect_args == 1 {
                    self.check_expr(&mut args[0], Some(&Type::String))?;
                    if args[0].ty != Type::String {
                        return Err(format!("http.serve<T> addr expects String, got {}", args[0].ty));
                    }
                }
                ensure_http_router_plan(self.checked, &tid.name, &req_ty, &resp_ty)?;
                if sym == "build" {
                    Ok(router_ty)
                } else {
                    Ok(Type::union(vec![Type::Void, http_err]))
                }
            }
            "get" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("http.get(url) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.get url expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![resp_ty, http_err]))
            }
            "post" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("http.post(url, body) expects 2 String arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.post url expects String, got {}", args[0].ty));
                }
                self.check_expr(&mut args[1], Some(&Type::String))?;
                if args[1].ty != Type::String {
                    return Err(format!("http.post body expects String, got {}", args[1].ty));
                }
                Ok(Type::union(vec![resp_ty, http_err]))
            }
            "request" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 4 {
                    return Err(
                        "http.request(method, url, body, headers) expects (String, String, String, Map<String, String>)"
                            .to_string(),
                    );
                }
                for (i, label) in [(0, "method"), (1, "url"), (2, "body")] {
                    self.check_expr(&mut args[i], Some(&Type::String))?;
                    if args[i].ty != Type::String {
                        return Err(format!(
                            "http.request {} expects String, got {}",
                            label, args[i].ty
                        ));
                    }
                }
                self.check_expr(&mut args[3], Some(&str_map))?;
                if args[3].ty != str_map {
                    return Err(format!(
                        "http.request headers expects Map<String, String>, got {}",
                        args[3].ty
                    ));
                }
                Ok(Type::union(vec![resp_ty, http_err]))
            }
            "request_timeout" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 5 {
                    return Err(
                        "http.request_timeout(method, url, body, headers, timeout_ms) expects (String, String, String, Map<String, String>, Int)"
                            .to_string(),
                    );
                }
                for (i, label) in [(0, "method"), (1, "url"), (2, "body")] {
                    self.check_expr(&mut args[i], Some(&Type::String))?;
                    if args[i].ty != Type::String {
                        return Err(format!(
                            "http.request_timeout {} expects String, got {}",
                            label, args[i].ty
                        ));
                    }
                }
                self.check_expr(&mut args[3], Some(&str_map))?;
                if args[3].ty != str_map {
                    return Err(format!(
                        "http.request_timeout headers expects Map<String, String>, got {}",
                        args[3].ty
                    ));
                }
                self.check_expr(&mut args[4], Some(&Type::Int))?;
                if args[4].ty != Type::Int {
                    return Err(format!(
                        "http.request_timeout timeout_ms expects Int, got {}",
                        args[4].ty
                    ));
                }
                Ok(Type::union(vec![resp_ty, http_err]))
            }
            "get_bytes" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("http.get_bytes(url) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.get_bytes url expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![bin_resp_ty, http_err]))
            }
            "request_bytes" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 4 {
                    return Err(
                        "http.request_bytes(method, url, body, headers) expects (String, String, [Int], Map<String, String>)"
                            .to_string(),
                    );
                }
                for (i, label) in [(0, "method"), (1, "url")] {
                    self.check_expr(&mut args[i], Some(&Type::String))?;
                    if args[i].ty != Type::String {
                        return Err(format!(
                            "http.request_bytes {} expects String, got {}",
                            label, args[i].ty
                        ));
                    }
                }
                self.check_expr(&mut args[2], Some(&int_list))?;
                if args[2].ty != int_list {
                    return Err(format!(
                        "http.request_bytes body expects [Int], got {}",
                        args[2].ty
                    ));
                }
                self.check_expr(&mut args[3], Some(&str_map))?;
                if args[3].ty != str_map {
                    return Err(format!(
                        "http.request_bytes headers expects Map<String, String>, got {}",
                        args[3].ty
                    ));
                }
                Ok(Type::union(vec![bin_resp_ty, http_err]))
            }
            "ok_json" => {
                if !type_args.is_empty() || args.len() != 1 || !named_args.is_empty() {
                    return Err("http.ok_json(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], None)?;
                Ok(resp_ty)
            }
            "status_json" => {
                if !type_args.is_empty() || args.len() != 2 || !named_args.is_empty() {
                    return Err("http.status_json(status, val) expects 2 arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!("http.status_json status expects Int, got {}", args[0].ty));
                }
                self.check_expr(&mut args[1], None)?;
                Ok(resp_ty)
            }
            "redirect" => {
                if !type_args.is_empty() || !(1..=2).contains(&args.len()) || !named_args.is_empty() {
                    return Err("http.redirect(url, status?) expects 1 or 2 arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.redirect url expects String, got {}", args[0].ty));
                }
                if args.len() == 2 {
                    self.check_expr(&mut args[1], Some(&Type::Int))?;
                    if args[1].ty != Type::Int {
                        return Err(format!("http.redirect status expects Int, got {}", args[1].ty));
                    }
                }
                Ok(resp_ty)
            }
            "no_content" => {
                if !type_args.is_empty() || !args.is_empty() || !named_args.is_empty() {
                    return Err("http.no_content() takes 0 arguments".to_string());
                }
                Ok(resp_ty)
            }
            "get_json" => {
                if type_args.len() != 1 || !(1..=2).contains(&args.len()) || !named_args.is_empty() {
                    return Err("http.get_json<T>(url, query?) expects 1 type argument and 1 or 2 arguments".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.get_json url expects String, got {}", args[0].ty));
                }
                if args.len() == 2 {
                    let q_ty = Type::Map(Box::new(Type::String), Box::new(Type::String));
                    self.check_expr(&mut args[1], Some(&q_ty))?;
                    if args[1].ty != q_ty {
                        return Err(format!("http.get_json query expects Map<String, String>, got {}", args[1].ty));
                    }
                }
                Ok(Type::union(vec![target_ty, http_err]))
            }
            "post_json" => {
                if !type_args.is_empty() || args.len() != 2 || !named_args.is_empty() {
                    return Err("http.post_json(url, body) expects 2 arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("http.post_json url expects String, got {}", args[0].ty));
                }
                self.check_expr(&mut args[1], None)?;
                Ok(Type::union(vec![resp_ty, http_err]))
            }
            other => Err(format!("Unknown cdt.http symbol '{}'", other)),
        }
    }

    pub(crate) fn check_http_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &mut [Expr],
    ) -> Result<Type, String> {
        let http_err = Type::Custom(TypeId::native("cdt.http", "HttpError"));
        let req_ty = Type::Custom(TypeId::native("cdt.http", "Request"));
        let resp_ty = Type::Custom(TypeId::native("cdt.http", "Response"));
        match (type_name, method) {
            (
                "Router",
                "get" | "post" | "put" | "delete" | "patch" | "options" | "head" | "any",
            ) => {
                if args.len() != 2 {
                    return Err(format!("Router.{}(pattern, handler) expects 2 arguments", method));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("Router.{} pattern expects String, got {}", method, args[0].ty));
                }
                let expected_cb = Type::Function {
                    params: vec![req_ty.clone()],
                    return_type: Box::new(resp_ty.clone()),
                };
                self.check_expr(&mut args[1], Some(&expected_cb))?;
                let Type::Function { params, return_type } = &args[1].ty else {
                    return Err(format!("Router.{} handler must be (Request) -> Response, got {}", method, args[1].ty));
                };
                if params.len() != 1 || params[0] != req_ty || **return_type != resp_ty {
                    return Err(format!(
                        "Router.{} handler must be (Request) -> Response, got {}",
                        method, args[1].ty
                    ));
                }
                Ok(Type::Void)
            }
            ("Request", "header" | "param" | "query_param") => {
                if args.len() != 1 {
                    return Err(format!("Request.{}(name: String) expects 1 argument", method));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("Request.{}() expects String, got {}", method, args[0].ty));
                }
                Ok(Type::Optional(Box::new(Type::String)))
            }
            ("Router", "before") => {
                if args.len() != 1 {
                    return Err("Router.before(handler) expects 1 function argument".to_string());
                }
                let opt_resp = Type::Optional(Box::new(resp_ty));
                let expected_cb = Type::Function {
                    params: vec![req_ty.clone()],
                    return_type: Box::new(opt_resp.clone()),
                };
                self.check_expr(&mut args[0], Some(&expected_cb))?;
                let Type::Function { params, return_type } = &args[0].ty else {
                    return Err(format!(
                        "Router.before handler must be (Request) -> Response?, got {}",
                        args[0].ty
                    ));
                };
                if params.len() != 1 || params[0] != req_ty || **return_type != opt_resp {
                    return Err(format!(
                        "Router.before handler must be (Request) -> Response?, got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::Void)
            }
            ("Router", "after") => {
                if args.len() != 1 {
                    return Err("Router.after(handler) expects 1 function argument".to_string());
                }
                let expected_cb = Type::Function {
                    params: vec![req_ty.clone(), resp_ty.clone()],
                    return_type: Box::new(resp_ty.clone()),
                };
                self.check_expr(&mut args[0], Some(&expected_cb))?;
                let Type::Function { params, return_type } = &args[0].ty else {
                    return Err(format!(
                        "Router.after handler must be (Request, Response) -> Response, got {}",
                        args[0].ty
                    ));
                };
                if params.len() != 2
                    || params[0] != req_ty
                    || params[1] != resp_ty
                    || **return_type != resp_ty
                {
                    return Err(format!(
                        "Router.after handler must be (Request, Response) -> Response, got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::Void)
            }
            ("Router", "dispatch") => {
                if args.len() != 1 {
                    return Err("Router.dispatch(req) expects 1 Request argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&req_ty))?;
                if args[0].ty != req_ty {
                    return Err(format!("Router.dispatch expects Request, got {}", args[0].ty));
                }
                Ok(resp_ty)
            }
            ("Router", "serve") => {
                if args.len() != 1 {
                    return Err("Router.serve(addr) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("Router.serve expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, http_err]))
            }
            ("Router", "stop") => {
                if !args.is_empty() {
                    return Err("Router.stop() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            _ => Err(format!("Type 'http.{}' has no method '.{}()'", type_name, method)),
        }
    }
}

pub(crate) fn ensure_http_router_plan(
    checked: &mut CheckedProgram,
    router_name: &str,
    req_ty: &Type,
    resp_ty: &Type,
) -> Result<(), String> {
    if checked.http_router_plans.iter().any(|(n, _)| n == router_name) {
        return Ok(());
    }
    let sid = *checked
        .struct_by_name
        .get(router_name)
        .ok_or_else(|| format!("Unknown router '{}' in cdt.http", router_name))?;
    let st = &checked.structs[sid];
    let mut entries = Vec::new();
    let opt_resp = Type::Optional(Box::new(resp_ty.clone()));
    for f in &st.fields {
        let Some(ann) = &f.router_ann else {
            return Err(format!(
                "Type '{}' is not a 'router' declaration (missing route definitions)",
                router_name
            ));
        };
        match ann.target.as_str() {
            "router.before" => {
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.first() else {
                    return Err(format!("Invalid before handler in router '{}'", router_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in router '{}'", handler_name, router_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if sig.params.len() != 1 || sig.params[0] != *req_ty || sig.return_type != opt_resp {
                    return Err(format!(
                        "router '{}' before handler '{}' must have signature (http.Request) -> http.Response?",
                        router_name, handler_name
                    ));
                }
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::Before,
                    handler_name: handler_name.clone(),
                    body_struct: None,
                });
            }
            "router.after" => {
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.first() else {
                    return Err(format!("Invalid after handler in router '{}'", router_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in router '{}'", handler_name, router_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if sig.params.len() != 2
                    || sig.params[0] != *req_ty
                    || sig.params[1] != *resp_ty
                    || sig.return_type != *resp_ty
                {
                    return Err(format!(
                        "router '{}' after handler '{}' must have signature (http.Request, http.Response) -> http.Response",
                        router_name, handler_name
                    ));
                }
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::After,
                    handler_name: handler_name.clone(),
                    body_struct: None,
                });
            }
            "router.group_before" => {
                let Some(Expr { kind: ExprKind::String(prefix), .. }) = ann.args.first() else {
                    return Err(format!("Invalid group before prefix in router '{}'", router_name));
                };
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid group before handler in router '{}'", router_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in router '{}'", handler_name, router_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if sig.params.len() != 1 || sig.params[0] != *req_ty || sig.return_type != opt_resp {
                    return Err(format!(
                        "router '{}' group '{}' before handler '{}' must have signature (http.Request) -> http.Response?",
                        router_name, prefix, handler_name
                    ));
                }
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::GroupBefore(prefix.clone()),
                    handler_name: handler_name.clone(),
                    body_struct: None,
                });
            }
            "router.group_after" => {
                let Some(Expr { kind: ExprKind::String(prefix), .. }) = ann.args.first() else {
                    return Err(format!("Invalid group after prefix in router '{}'", router_name));
                };
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid group after handler in router '{}'", router_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in router '{}'", handler_name, router_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if sig.params.len() != 2
                    || sig.params[0] != *req_ty
                    || sig.params[1] != *resp_ty
                    || sig.return_type != *resp_ty
                {
                    return Err(format!(
                        "router '{}' group '{}' after handler '{}' must have signature (http.Request, http.Response) -> http.Response",
                        router_name, prefix, handler_name
                    ));
                }
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::GroupAfter(prefix.clone()),
                    handler_name: handler_name.clone(),
                    body_struct: None,
                });
            }
            "router.static" => {
                let Some(Expr { kind: ExprKind::String(url_prefix), .. }) = ann.args.first() else {
                    return Err(format!("Invalid static URL prefix in router '{}'", router_name));
                };
                let Some(Expr { kind: ExprKind::String(dir_path), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid static directory in router '{}'", router_name));
                };
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::Static(url_prefix.clone(), dir_path.clone()),
                    handler_name: String::new(),
                    body_struct: None,
                });
            }
            "router.cors" => {
                let Some(Expr { kind: ExprKind::String(origin), .. }) = ann.args.first() else {
                    return Err(format!("Invalid cors origin in router '{}'", router_name));
                };
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::Cors(origin.clone()),
                    handler_name: String::new(),
                    body_struct: None,
                });
            }
            tag => {
                let Some(method) = tag.strip_prefix("router.") else {
                    continue;
                };
                let Some(Expr { kind: ExprKind::String(pat), .. }) = ann.args.first() else {
                    return Err(format!("Invalid route pattern in router '{}'", router_name));
                };
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid route handler in router '{}'", router_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in router '{}'", handler_name, router_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                let body_struct = if sig.params.len() == 1
                    && sig.params[0] == *req_ty
                    && sig.return_type == *resp_ty
                {
                    None
                } else if sig.params.len() == 2
                    && sig.params[0] == *req_ty
                    && sig.return_type == *resp_ty
                    && let Type::Custom(tid) = &sig.params[1]
                    && tid.module == ModuleId::Program
                    && checked.struct_by_name.contains_key(&tid.name)
                {
                    Some(tid.name.clone())
                } else if sig.params.len() == 2
                    && sig.params[0] == *req_ty
                    && sig.return_type == *resp_ty
                    && let Type::DataEnum(ename) = &sig.params[1]
                    && checked.data_enums.contains_key(ename)
                {
                    Some(ename.clone())
                } else {
                    return Err(format!(
                        "router '{}' route '{} {}' handler '{}' must have signature (http.Request) -> http.Response or (http.Request, BodyStruct) -> http.Response",
                        router_name,
                        method.to_ascii_uppercase(),
                        pat,
                        handler_name
                    ));
                };
                entries.push(HttpRouterDeclEntry {
                    kind: HttpRouterDeclKind::Route(method.to_string(), pat.clone()),
                    handler_name: handler_name.clone(),
                    body_struct,
                });
            }
        }
    }
    checked.http_router_plans.push((router_name.to_string(), entries));
    Ok(())
}

impl Checker<'_> {
    pub(crate) fn check_db_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let db_err = Type::Custom(TypeId::native("cdt.db", "DbError"));
        let db_conn = Type::Custom(TypeId::native("cdt.db", "Connection"));
        let db_param = Type::Custom(TypeId::native("cdt.db", "Param"));
        match sym {
            "DbError" => {
                if !type_args.is_empty() {
                    return Err("DbError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "DbError",
                    &[
                        ("kind", Type::String),
                        ("message", Type::String),
                        ("code", Type::String),
                    ],
                    args,
                    named_args,
                    db_err,
                )
            }
            "connect" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("db.connect(url) expects 1 String argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("db.connect expects String, got {}", args[0].ty));
                }
                Ok(Type::union(vec![db_conn, db_err]))
            }
            "null" => {
                if !type_args.is_empty() || !named_args.is_empty() || !args.is_empty() {
                    return Err("db.null() takes 0 arguments".to_string());
                }
                Ok(db_param)
            }
            "int" | "float" | "bool" | "text" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("db.{}(val) expects 1 argument", sym));
                }
                let expected = match sym {
                    "int" => Type::Int,
                    "float" => Type::Float,
                    "bool" => Type::Bool,
                    _ => Type::String,
                };
                self.check_expr(&mut args[0], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!("db.{} expects {}, got {}", sym, expected, args[0].ty));
                }
                Ok(db_param)
            }
            "ddl" => {
                if type_args.len() != 1 {
                    return Err("db.ddl<T>(dialect: String) expects 1 type argument".to_string());
                }
                if args.len() != 1 || !named_args.is_empty() {
                    return Err("db.ddl<T>(dialect: String) expects 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("db.ddl<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("db.ddl<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("db.ddl<T>() dialect expects String, got {}", args[0].ty));
                }
                Ok(Type::String)
            }
            other => Err(format!("Unknown cdt.db symbol '{}'", other)),
        }
    }

    fn check_sql_and_opt_params(
        &mut self,
        method: &str,
        args: &mut [Expr],
    ) -> Result<(), String> {
        if !(1..=2).contains(&args.len()) {
            return Err(format!(
                "Connection.{}(sql, params?) expects 1 or 2 arguments",
                method
            ));
        }
        self.check_expr(&mut args[0], Some(&Type::String))?;
        if args[0].ty != Type::String {
            return Err(format!(
                "Connection.{} sql expects String, got {}",
                method, args[0].ty
            ));
        }
        if args.len() == 2 {
            let expected = Type::List(Box::new(Type::Custom(TypeId::native("cdt.db", "Param"))));
            self.check_expr(&mut args[1], Some(&expected))?;
            if args[1].ty != expected {
                return Err(format!(
                    "Connection.{} params expects [db.Param], got {}",
                    method, args[1].ty
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn check_db_method(
        &mut self,
        type_name: &str,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
    ) -> Result<Type, String> {
        if type_name != "Connection" {
            return Err(format!("Type 'db.{}' has no method '.{}()'", type_name, method));
        }
        let db_err = Type::Custom(TypeId::native("cdt.db", "DbError"));
        match method {
            "exec" => {
                if !type_args.is_empty() {
                    return Err("Connection.exec() does not accept type arguments".to_string());
                }
                self.check_sql_and_opt_params("exec", args)?;
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "query" | "query_one" => {
                if type_args.len() != 1 {
                    return Err(format!("Connection.{}<T>(sql, params?) expects 1 type argument", method));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!(
                        "Connection.{}<T>() requires a struct type, got {}",
                        method, target_ty
                    ));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!(
                        "Connection.{}<T>() requires a program struct type, got {}",
                        method, target_ty
                    ));
                }
                self.check_sql_and_opt_params(method, args)?;
                let ok_ty = if method == "query_one" {
                    Type::Optional(Box::new(target_ty))
                } else {
                    Type::List(Box::new(target_ty))
                };
                Ok(Type::union(vec![ok_ty, db_err]))
            }
            "query_rows" => {
                if !type_args.is_empty() {
                    return Err("Connection.query_rows() does not accept type arguments".to_string());
                }
                self.check_sql_and_opt_params("query_rows", args)?;
                let row_map = Type::Map(Box::new(Type::String), Box::new(Type::String));
                Ok(Type::union(vec![Type::List(Box::new(row_map)), db_err]))
            }
            "begin" | "commit" | "rollback" => {
                if !type_args.is_empty() || !args.is_empty() {
                    return Err(format!("Connection.{}() takes 0 arguments", method));
                }
                Ok(Type::union(vec![Type::Void, db_err]))
            }
            "migrate" => {
                if !type_args.is_empty() || !args.is_empty() {
                    return Err("Connection.migrate() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "dialect" => {
                if !type_args.is_empty() || !args.is_empty() {
                    return Err("Connection.dialect() takes 0 arguments".to_string());
                }
                Ok(Type::String)
            }
            "last_insert_id" => {
                if !type_args.is_empty() || !args.is_empty() {
                    return Err("Connection.last_insert_id() takes 0 arguments".to_string());
                }
                Ok(Type::Int)
            }
            "close" => {
                if !type_args.is_empty() || !args.is_empty() {
                    return Err("Connection.close() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            "create_table" | "drop_table" => {
                if type_args.len() != 1 || !args.is_empty() {
                    return Err(format!("Connection.{}<T>() expects 1 type argument and 0 value arguments", method));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.{}<T>() requires a struct type, got {}", method, target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.{}<T>() requires a program struct type, got {}", method, target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                Ok(Type::union(vec![Type::Void, db_err]))
            }
            "insert" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err("Connection.insert<T>(row: T) expects 1 type argument and 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.insert<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.insert<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], Some(&target_ty))?;
                if !args[0].ty.is_assignable_to(&target_ty) {
                    return Err(format!("Connection.insert<{}> expects {}, got {}", tid.name, target_ty, args[0].ty));
                }
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "all" => {
                if type_args.len() != 1 || !args.is_empty() {
                    return Err("Connection.all<T>() expects 1 type argument and 0 arguments".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.all<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.all<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                Ok(Type::union(vec![Type::List(Box::new(target_ty)), db_err]))
            }
            "find" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err("Connection.find<T>(id) expects 1 type argument and 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.find<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.find<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], None)?;
                Ok(Type::union(vec![Type::Optional(Box::new(target_ty)), db_err]))
            }
            "delete" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err("Connection.delete<T>(id) expects 1 type argument and 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.delete<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.delete<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], None)?;
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "update" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err("Connection.update<T>(row: T) expects 1 type argument and 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.update<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.update<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], Some(&target_ty))?;
                if !args[0].ty.is_assignable_to(&target_ty) {
                    return Err(format!("Connection.update<{}> expects {}, got {}", tid.name, target_ty, args[0].ty));
                }
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "count" => {
                if type_args.len() != 1 || !args.is_empty() {
                    return Err("Connection.count<T>() expects 1 type argument and 0 arguments".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.count<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.count<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                Ok(Type::union(vec![Type::Int, db_err]))
            }
            "find_by" => {
                if type_args.len() != 1 || args.len() != 2 {
                    return Err("Connection.find_by<T>(column, value) expects 1 type argument and 2 arguments".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("Connection.find_by<T>() requires a struct type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program || !self.checked.struct_by_name.contains_key(&tid.name) {
                    return Err(format!("Connection.find_by<T>() requires a program struct type, got {}", target_ty));
                }
                ensure_db_model_plan(self.checked, &tid.name)?;
                let (c_slice, v_slice) = args.split_at_mut(1);
                self.check_expr(&mut c_slice[0], Some(&Type::String))?;
                if c_slice[0].ty != Type::String {
                    return Err(format!(
                        "Connection.find_by<T> column must be String, got {}",
                        c_slice[0].ty
                    ));
                }
                self.check_expr(&mut v_slice[0], None)?;
                Ok(Type::union(vec![Type::List(Box::new(target_ty)), db_err]))
            }
            other => Err(format!("db.Connection has no method '.{}()'", other)),
        }
    }

    pub(crate) fn check_cron_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let cron_err = Type::Custom(TypeId::native("cdt.cron", "CronError"));
        let sched_ty = Type::Custom(TypeId::native("cdt.cron", "Scheduler"));
        match sym {
            "CronError" => {
                if !type_args.is_empty() {
                    return Err("CronError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "CronError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    cron_err,
                )
            }
            "validate" => {
                if !type_args.is_empty() || args.len() != 1 || !named_args.is_empty() {
                    return Err("cron.validate(expr: String) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("cron.validate() expects String, got {}", args[0].ty));
                }
                Ok(Type::Bool)
            }
            "run" | "start" => {
                if type_args.len() != 1 || !args.is_empty() || !named_args.is_empty() {
                    return Err(format!(
                        "cron.{}<T>() expects 1 schedule type argument and 0 value arguments",
                        sym
                    ));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("cron.{}<T>() requires a schedule type, got {}", sym, target_ty));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!("cron.{}<T>() requires a program schedule type, got {}", sym, target_ty));
                }
                ensure_cron_schedule_plan(self.checked, &tid.name)?;
                if sym == "run" {
                    Ok(Type::union(vec![Type::Void, cron_err]))
                } else {
                    Ok(Type::union(vec![sched_ty, cron_err]))
                }
            }
            "run_once" => {
                if type_args.len() != 1 || args.len() != 1 || !named_args.is_empty() {
                    return Err("cron.run_once<T>(job: String) expects 1 type argument and 1 argument".to_string());
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("cron.run_once<T>() requires a schedule type, got {}", target_ty));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!("cron.run_once<T>() requires a program schedule type, got {}", target_ty));
                }
                ensure_cron_schedule_plan(self.checked, &tid.name)?;
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("cron.run_once<T>() expects String job name, got {}", args[0].ty));
                }
                Ok(Type::union(vec![Type::Void, cron_err]))
            }
            other => Err(format!("Unknown cdt.cron symbol '{}'", other)),
        }
    }

    pub(crate) fn check_cron_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &mut [Expr],
    ) -> Result<Type, String> {
        if type_name != "Scheduler" {
            return Err(format!("Type 'cron.{}' has no method '.{}()'", type_name, method));
        }
        match method {
            "stop" => {
                if !args.is_empty() {
                    return Err("Scheduler.stop() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            "is_running" => {
                if !args.is_empty() {
                    return Err("Scheduler.is_running() takes 0 arguments".to_string());
                }
                Ok(Type::Bool)
            }
            other => Err(format!("cron.Scheduler has no method '.{}()'", other)),
        }
    }
}

fn validate_sql_ident(ident: &str, ctx: &str) -> Result<(), String> {
    let mut chars = ident.chars();
    let Some(first) = chars.next() else {
        return Err(format!("Empty SQL identifier in {}", ctx));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!(
            "Invalid SQL identifier '{}' in {} (allowed: [a-zA-Z_][a-zA-Z0-9_]*)",
            ident, ctx
        ));
    }
    Ok(())
}

pub(crate) fn ensure_db_model_plan(checked: &mut CheckedProgram, struct_name: &str) -> Result<(), String> {
    let sid = *checked
        .struct_by_name
        .get(struct_name)
        .ok_or_else(|| format!("Unknown struct '{}' in cdt.db", struct_name))?;
    if checked.db_model_plans.iter().any(|m| m.schema_id == sid as u32) {
        return Ok(());
    }
    let st = &checked.structs[sid];
    let table_name = st
        .db_table
        .clone()
        .unwrap_or_else(|| struct_name.to_ascii_lowercase());
    validate_sql_ident(&table_name, &format!("@db.table on '{}'", struct_name))?;
    let mut columns = Vec::with_capacity(st.fields.len());
    for f in &st.fields {
        let base_ty = match &f.ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        if !matches!(base_ty, Type::Int | Type::Float | Type::Bool | Type::String) {
            return Err(format!(
                "DB model '{}' field '{}' has unsupported column type '{}' (supported: Int, Float, Bool, String, and Optionals)",
                struct_name, f.name, f.ty
            ));
        }
        let mut col_name = f.name.clone();
        let mut is_pk = false;
        let mut is_auto = false;
        let mut is_unique = false;
        let mut is_index = false;
        let mut default_sql = None;
        let mut fk = None;
        for ann in &f.db_anns {
            match ann.target.as_str() {
                "db.pk" => is_pk = true,
                "db.auto" => {
                    if *base_ty != Type::Int {
                        return Err(format!(
                            "@db.auto on '{}.{}' requires Int field",
                            struct_name, f.name
                        ));
                    }
                    is_auto = true;
                }
                "db.unique" => is_unique = true,
                "db.index" => is_index = true,
                "db.col" => {
                    let [Expr { kind: ExprKind::String(s), .. }] = ann.args.as_slice() else {
                        return Err(format!("@db.col(...) on '{}.{}' expects 1 String", struct_name, f.name));
                    };
                    validate_sql_ident(s, &format!("@db.col on '{}.{}'", struct_name, f.name))?;
                    col_name = s.clone();
                }
                "db.default" => {
                    let [Expr { kind: ExprKind::String(s), .. }] = ann.args.as_slice() else {
                        return Err(format!("@db.default(...) on '{}.{}' expects 1 String", struct_name, f.name));
                    };
                    if s.contains(';') || s.contains("--") || s.contains("/*") {
                        return Err(format!(
                            "Unsafe SQL default '{}' in @db.default on '{}.{}'",
                            s, struct_name, f.name
                        ));
                    }
                    default_sql = Some(s.clone());
                }
                "db.fk" => {
                    let [Expr { kind: ExprKind::String(s), .. }] = ann.args.as_slice() else {
                        return Err(format!("@db.fk(...) on '{}.{}' expects 1 String ('table.col')", struct_name, f.name));
                    };
                    let Some((t, c)) = s.split_once('.') else {
                        return Err(format!("@db.fk(\"{}\") on '{}.{}' must be 'table.column'", s, struct_name, f.name));
                    };
                    validate_sql_ident(t, &format!("@db.fk table on '{}.{}'", struct_name, f.name))?;
                    validate_sql_ident(c, &format!("@db.fk column on '{}.{}'", struct_name, f.name))?;
                    fk = Some((t.to_string(), c.to_string()));
                }
                other => {
                    return Err(format!(
                        "Unknown DB field annotation '@{}' on '{}.{}'",
                        other, struct_name, f.name
                    ));
                }
            }
        }
        columns.push(crate::ir::DbColumnMeta {
            field_name: f.name.clone(),
            col_name,
            is_pk,
            is_auto,
            is_unique,
            is_index,
            default_sql,
            fk,
        });
    }
    checked.db_model_plans.push(crate::ir::DbModelMeta {
        schema_id: sid as u32,
        table_name,
        columns,
    });
    Ok(())
}

pub(crate) fn ensure_cron_schedule_plan(
    checked: &mut CheckedProgram,
    schedule_name: &str,
) -> Result<(), String> {
    if checked.cron_schedule_plans.iter().any(|(n, _)| n == schedule_name) {
        return Ok(());
    }
    let sid = *checked
        .struct_by_name
        .get(schedule_name)
        .ok_or_else(|| format!("Unknown schedule '{}' in cdt.cron", schedule_name))?;
    let st = &checked.structs[sid];
    let mut entries = Vec::new();
    for f in &st.fields {
        let Some(ann) = &f.router_ann else {
            return Err(format!(
                "Type '{}' is not a 'schedule' declaration (missing job definitions)",
                schedule_name
            ));
        };
        match ann.target.as_str() {
            "schedule.every" => {
                let Some(Expr { kind: ExprKind::String(expr), .. }) = ann.args.first() else {
                    return Err(format!("Invalid cron expression in schedule '{}'", schedule_name));
                };
                crate::native::cron::parse_cron_expr(expr).map_err(|e| {
                    format!("Invalid cron expression '{}' in schedule '{}': {}", expr, schedule_name, e)
                })?;
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid job handler in schedule '{}'", schedule_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in schedule '{}'", handler_name, schedule_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if !sig.params.is_empty() || !is_cron_handler_return(&sig.return_type, checked) {
                    return Err(format!(
                        "schedule '{}' job '{}' must have signature () -> Void or () -> Void | Error",
                        schedule_name, handler_name
                    ));
                }
                entries.push(crate::frontend::check::CronScheduleEntry {
                    name: handler_name.clone(),
                    cron_expr: Some(expr.clone()),
                    interval_ms: None,
                    handler_name: handler_name.clone(),
                });
            }
            "schedule.interval" => {
                let Some(Expr { kind: ExprKind::Int(ms), .. }) = ann.args.first() else {
                    return Err(format!("Invalid interval in schedule '{}'", schedule_name));
                };
                let Some(Expr { kind: ExprKind::Ident(handler_name), .. }) = ann.args.get(1) else {
                    return Err(format!("Invalid job handler in schedule '{}'", schedule_name));
                };
                let fid = *checked.func_by_name.get(handler_name).ok_or_else(|| {
                    format!("Unknown handler '{}' in schedule '{}'", handler_name, schedule_name)
                })?;
                let sig = &checked.funcs[fid as usize];
                if !sig.params.is_empty() || !is_cron_handler_return(&sig.return_type, checked) {
                    return Err(format!(
                        "schedule '{}' job '{}' must have signature () -> Void or () -> Void | Error",
                        schedule_name, handler_name
                    ));
                }
                entries.push(crate::frontend::check::CronScheduleEntry {
                    name: handler_name.clone(),
                    cron_expr: None,
                    interval_ms: Some(*ms as u64),
                    handler_name: handler_name.clone(),
                });
            }
            _ => {
                return Err(format!(
                    "Type '{}' is not a 'schedule' declaration",
                    schedule_name
                ));
            }
        }
    }
    checked.cron_schedule_plans.push((schedule_name.to_string(), entries));
    Ok(())
}

fn is_cron_handler_return(ty: &Type, checked: &CheckedProgram) -> bool {
    match ty {
        Type::Void => true,
        Type::Union(vars) => vars
            .iter()
            .all(|v| *v == Type::Void || crate::frontend::check_native::is_error_type(v, checked)),
        _ => false,
    }
}
