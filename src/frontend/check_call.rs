use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::check::Checker;
use crate::frontend::types::{ModuleId, Type, TypeId};

impl<'a> Checker<'a> {
    pub(crate) fn check_closure(
        &mut self,
        params: &[Param],
        return_type: &Option<TypeExpr>,
        body: &mut Block,
        expected: Option<&Type>,
    ) -> Result<Type, String> {
        let (exp_params, exp_ret) = match expected {
            Some(Type::Function { params, return_type }) => (Some(params), Some(return_type.as_ref())),
            _ => (None, None),
        };
        let mut p_tys = Vec::with_capacity(params.len());
        for (i, p) in params.iter().enumerate() {
            if matches!(&p.ty, TypeExpr::Named(n) if n == "_") {
                let inferred = exp_params
                    .and_then(|ps| ps.get(i))
                    .cloned()
                    .ok_or_else(|| format!("Cannot infer type for parameter '{}'", p.name))?;
                p_tys.push(inferred);
            } else {
                p_tys.push(self.checked.resolve_type_mut(&p.ty)?);
            }
        }
        if return_type.is_none()
            && body.stmts.len() == 1
            && let StmtKind::Return(Some(ret_expr)) = &mut body.stmts[0].kind
        {
            self.push_scope();
            for (p, ty) in params.iter().zip(p_tys.iter()) {
                self.bind(p.name.clone(), ty.clone());
            }
            let expected_ret = exp_ret.filter(|t| **t != Type::Any);
            let res = self.check_expr(ret_expr, expected_ret);
            self.pop_scope();
            res?;
            let ret_ty = ret_expr.ty.clone();
            return Ok(Type::Function {
                params: p_tys,
                return_type: Box::new(ret_ty),
            });
        }

        let ret_ty = match return_type {
            Some(rt) => self.checked.resolve_type_mut(rt)?,
            None => Type::Void,
        };
        let saved_ret = std::mem::replace(&mut self.current_return, ret_ty.clone());
        let saved_loop = std::mem::replace(&mut self.loop_depth, 0);
        self.push_scope();
        for (p, ty) in params.iter().zip(p_tys.iter()) {
            self.bind(p.name.clone(), ty.clone());
        }
        for s in &mut body.stmts {
            if let Err(e) = self.check_stmt(s) {
                self.record_stmt_error(s, e);
            }
        }
        self.pop_scope();
        self.current_return = saved_ret;
        self.loop_depth = saved_loop;
        Ok(Type::Function {
            params: p_tys,
            return_type: Box::new(ret_ty),
        })
    }

    fn check_native_module_call(
        &mut self,
        mod_name: &str,
        sym: &str,
        type_args: &mut [TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        match mod_name {
            "cdt.json" => self.check_json_call(sym, type_args, args, named_args),
            "cdt.time" => self.check_time_call(sym, type_args, args, named_args),
            "cdt.validation" => self.check_validation_call(sym, type_args, args, named_args),
            "cdt.env" => self.check_env_call(sym, type_args, args, named_args),
            "cdt.cli" => self.check_cli_call(sym, type_args, args, named_args),
            "cdt.fs" => self.check_fs_call(sym, type_args, args, named_args),
            "cdt.path" => self.check_path_call(sym, type_args, args, named_args),
            "cdt.process" => self.check_process_call(sym, type_args, args, named_args),
            "cdt.crypto" => self.check_crypto_call(sym, type_args, args, named_args),
            "cdt.url" => self.check_url_call(sym, type_args, args, named_args),
            "cdt.csv" => self.check_csv_call(sym, type_args, args, named_args),
            "cdt.math" => self.check_math_call(sym, type_args, args, named_args),
            "cdt.log" => self.check_log_call(sym, type_args, args, named_args),
            "cdt.http" => self.check_http_call(sym, type_args, args, named_args),
            "cdt.db" => self.check_db_call(sym, type_args, args, named_args),
            "cdt.cron" => self.check_cron_call(sym, type_args, args, named_args),
            "cdt.jwt" => self.check_jwt_call(sym, type_args, args, named_args),
            "cdt.lock" => self.check_lock_call(sym, type_args, args, named_args),
            "cdt.cache" => self.check_cache_call(sym, type_args, args, named_args),
            "cdt.redis" => self.check_redis_call(sym, type_args, args, named_args),
            other => Err(format!("Unknown native module '{}'", other)),
        }
    }

    pub(crate) fn check_call(
        &mut self,
        callee: &mut Expr,
        type_args: &mut Vec<TypeExpr>,
        args: &mut Vec<Expr>,
        named_args: &mut Vec<NamedArg>,
    ) -> Result<Type, String> {
        if let ExprKind::Member { target, field } = &mut callee.kind {
            if let ExprKind::Ident(mod_alias) = &target.kind
                && self.lookup(mod_alias).is_none()
            {
                if mod_alias == "Map" && field == "from" {
                    return self.check_map_from(type_args, args, named_args);
                }
                if let Some(dvars) = self.checked.data_enums.get(mod_alias).cloned() {
                    if !type_args.is_empty() {
                        return Err(format!(
                            "Enum variant '{}.{}' does not accept type arguments",
                            mod_alias, field
                        ));
                    }
                    let var = dvars.iter().find(|v| &v.name == field).ok_or_else(|| {
                        format!("Enum '{}' has no variant '{}'", mod_alias, field)
                    })?;
                    let _ = self.check_struct_ctor(&var.struct_name, args, named_args)?;
                    return Ok(Type::DataEnum(mod_alias.clone()));
                }
                if let Some(&mod_name) = self.checked.native_aliases.get(mod_alias) {
                    return self.check_native_module_call(mod_name, field, type_args, args, named_args);
                }
            }
            self.check_expr(target, None)?;
            return self.check_method_call(target, field, type_args, args, named_args);
        }

        if let ExprKind::Ident(name) = &callee.kind
            && self.lookup(name).is_none()
        {
            match name.as_str() {
                "map" => return self.check_builtin_map(args, named_args),
                "filter" => return self.check_builtin_filter(args, named_args),
                "reduce" => return self.check_builtin_reduce(args, named_args),
                "print" => {
                    if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                        return Err("print() expects 1 argument".to_string());
                    }
                    self.check_expr(&mut args[0], None)?;
                    if args[0].ty == Type::Void {
                        return Err("Cannot print Void".to_string());
                    }
                    return Ok(Type::Void);
                }
                "assert" => {
                    if !type_args.is_empty()
                        || !named_args.is_empty()
                        || !(1..=2).contains(&args.len())
                    {
                        return Err(
                            "assert() expects 1 or 2 positional arguments: (cond, message?)"
                                .to_string(),
                        );
                    }
                    self.check_expr(&mut args[0], Some(&Type::Bool))?;
                    if args[0].ty != Type::Bool {
                        return Err(format!("assert() condition must be Bool, got {}", args[0].ty));
                    }
                    if args.len() == 2 {
                        self.check_expr(&mut args[1], Some(&Type::String))?;
                        if args[1].ty != Type::String {
                            return Err(format!(
                                "assert() message must be String, got {}",
                                args[1].ty
                            ));
                        }
                    }
                    return Ok(Type::Void);
                }
                "fail" => {
                    if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                        return Err("fail() expects 1 String argument".to_string());
                    }
                    self.check_expr(&mut args[0], Some(&Type::String))?;
                    if args[0].ty != Type::String {
                        return Err(format!("fail() message must be String, got {}", args[0].ty));
                    }
                    return Ok(Type::Void);
                }
                "assert_eq" | "assert_ne" => {
                    if !type_args.is_empty()
                        || !named_args.is_empty()
                        || !(2..=3).contains(&args.len())
                    {
                        return Err(format!(
                            "{}() expects 2 or 3 positional arguments: (actual, expected, message?)",
                            name
                        ));
                    }
                    self.check_expr(&mut args[0], None)?;
                    let first_ty = args[0].ty.clone();
                    self.check_expr(&mut args[1], Some(&first_ty))?;
                    if matches!(args[0].kind, ExprKind::List(ref elems) if elems.is_empty())
                        && matches!(args[1].ty, Type::List(_))
                    {
                        args[0].ty = args[1].ty.clone();
                    }
                    if args[0].ty == Type::Void || args[1].ty == Type::Void {
                        return Err(format!("Cannot compare Void in {}()", name));
                    }
                    if !args[0].ty.is_assignable_to(&args[1].ty)
                        && !args[1].ty.is_assignable_to(&args[0].ty)
                    {
                        return Err(format!(
                            "{}() operands must have compatible types, got {} and {}",
                            name, args[0].ty, args[1].ty
                        ));
                    }
                    if args.len() == 3 {
                        self.check_expr(&mut args[2], Some(&Type::String))?;
                        if args[2].ty != Type::String {
                            return Err(format!(
                                "{}() message must be String, got {}",
                                name, args[2].ty
                            ));
                        }
                    }
                    return Ok(Type::Void);
                }
                "Map" => {
                    if type_args.len() != 2 || !args.is_empty() || !named_args.is_empty() {
                        return Err(
                            "Map<K, V>() expects 2 type arguments and 0 value arguments"
                                .to_string(),
                        );
                    }
                    let k = self.checked.resolve_type_mut(&type_args[0])?;
                    crate::frontend::check_native::validate_map_key_type(&k)?;
                    let v = self.checked.resolve_type_mut(&type_args[1])?;
                    return Ok(Type::Map(Box::new(k), Box::new(v)));
                }
                "IndexError" | "KeyError" | "ParseError" => {
                    return self.check_prelude_error_ctor(name, type_args, args, named_args);
                }
                _ if self.checked.native_direct_fns.contains_key(name) => {
                    let (mod_name, sym) = self.checked.native_direct_fns[name].clone();
                    return self.check_native_module_call(mod_name, &sym, type_args, args, named_args);
                }
                _ if self.checked.native_direct_types.contains_key(name) => {
                    let (mod_name, sym) = self.checked.native_direct_types[name].clone();
                    return self.check_native_module_call(mod_name, &sym, type_args, args, named_args);
                }
                _ if self.checked.generic_structs.contains_key(name) => {
                    let base = name.clone();
                    let mono = crate::frontend::monomorph::check_generic_struct_ctor(
                        self, &base, type_args, args, named_args,
                    )?;
                    callee.kind = ExprKind::Ident(mono.clone());
                    return self.check_struct_ctor(&mono, args, named_args);
                }
                _ if self.checked.generic_funcs.contains_key(name) => {
                    let fn_name = name.clone();
                    return crate::frontend::monomorph::check_generic_func_call(
                        self, callee, &fn_name, type_args, args, named_args,
                    );
                }
                _ if self.checked.struct_by_name.contains_key(name) => {
                    if !type_args.is_empty() {
                        return Err(format!("Type '{}' is not generic", name));
                    }
                    let name = name.clone();
                    return self.check_struct_ctor(&name, args, named_args);
                }
                _ => {}
            }
        }

        if !type_args.is_empty() {
            return Err("Type arguments are only allowed on generic functions and types".to_string());
        }

        self.check_expr(callee, None)?;
        let Type::Function {
            params,
            return_type,
        } = callee.ty.clone()
        else {
            return Err(format!("Value of type {} is not callable", callee.ty));
        };
        if !named_args.is_empty() {
            return Err("Named arguments are only supported on type constructors".to_string());
        }
        if args.len() < params.len()
            && let ExprKind::Ident(fn_name) = &callee.kind
            && !self.is_local(fn_name)
            && let Some(&fid) = self.checked.func_by_name.get(fn_name)
        {
            let defaults = self.checked.funcs[fid as usize].defaults.clone();
            let provided = args.len();
            for (i, def) in defaults.iter().enumerate().skip(provided) {
                if let Some(def_expr) = def {
                    args.push(def_expr.clone());
                } else {
                    return Err(format!(
                        "Expected {} arguments (missing parameter {}), got {}",
                        params.len(),
                        i + 1,
                        provided
                    ));
                }
            }
        }
        if args.len() != params.len() {
            return Err(format!(
                "Expected {} arguments, got {}",
                params.len(),
                args.len()
            ));
        }
        for (arg, param_ty) in args.iter_mut().zip(params.iter()) {
            self.check_expr(arg, Some(param_ty))?;
            if !arg.ty.is_assignable_to(param_ty) {
                return Err(format!(
                    "Argument type mismatch: expected {}, got {}",
                    param_ty, arg.ty
                ));
            }
        }
        Ok(*return_type)
    }

    fn check_json_call(
        &mut self,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !named_args.is_empty() {
            return Err(format!("json.{} does not accept named arguments", method));
        }
        match method {
            "decode" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err(
                        "json.decode<T>(raw) expects 1 type arg and 1 String arg".to_string(),
                    );
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("json.decode expects String, got {}", args[0].ty));
                }
                let target = self.checked.resolve_type_mut(&type_args[0])?;
                Ok(Type::union(vec![
                    target,
                    Type::Custom(TypeId::native("cdt.json", "DecodeError")),
                ]))
            }
            "encode" => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err("json.encode(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], None)?;
                Ok(Type::union(vec![
                    Type::String,
                    Type::Custom(TypeId::native("cdt.json", "EncodeError")),
                ]))
            }
            other => Err(format!("Unknown cdt.json function '{}'", other)),
        }
    }

    fn check_method_call(
        &mut self,
        target: &Expr,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !named_args.is_empty() {
            return Err(format!(
                "Method '{}' does not accept named arguments",
                method
            ));
        }
        if !type_args.is_empty()
            && !matches!(&target.ty, Type::Custom(tid) if matches!(tid.module, ModuleId::Native(_)))
        {
            return Err(format!("Method '.{}()' does not accept type arguments", method));
        }
        match (&target.ty, method) {
            (Type::List(_), "len") | (Type::String, "len") | (Type::Map(_, _), "len") => {
                if !args.is_empty() {
                    return Err(".len() takes 0 arguments".to_string());
                }
                Ok(Type::Int)
            }
            (Type::List(_), "is_empty") => {
                if !args.is_empty() {
                    return Err(".is_empty() takes 0 arguments".to_string());
                }
                Ok(Type::Bool)
            }
            (Type::List(elem), "first" | "last") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                if matches!(**elem, Type::Optional(_)) {
                    Ok((**elem).clone())
                } else {
                    Ok(Type::Optional(elem.clone()))
                }
            }
            (Type::List(elem), "sort") => {
                if !args.is_empty() {
                    return Err(".sort() takes 0 arguments".to_string());
                }
                if !matches!(**elem, Type::Int | Type::Float | Type::String) {
                    return Err(format!(
                        "Cannot sort list of type {}; .sort() supports [Int], [Float], and [String]",
                        target.ty
                    ));
                }
                Ok(Type::List(elem.clone()))
            }
            (Type::List(elem), "sum") => {
                if !args.is_empty() {
                    return Err(".sum() takes 0 arguments".to_string());
                }
                if !matches!(**elem, Type::Int | Type::Float) {
                    return Err(format!(
                        "Cannot call .sum() on {}; .sum() supports [Int] and [Float]",
                        target.ty
                    ));
                }
                Ok((**elem).clone())
            }
            (Type::List(elem), "min" | "max") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                if !matches!(**elem, Type::Int | Type::Float) {
                    return Err(format!(
                        "Cannot call .{}() on {}; .{}() supports [Int] and [Float]",
                        method, target.ty, method
                    ));
                }
                Ok(Type::Optional(elem.clone()))
            }
            (Type::List(elem), "find" | "any" | "all") => {
                if args.len() != 1 {
                    return Err(format!(".{}(pred) expects 1 argument", method));
                }
                let expected_pred = Type::Function {
                    params: vec![(**elem).clone()],
                    return_type: Box::new(Type::Bool),
                };
                self.check_expr(&mut args[0], Some(&expected_pred))?;
                let Type::Function { params, return_type } = args[0].ty.clone() else {
                    return Err(format!(".{} predicate must be a function, got {}", method, args[0].ty));
                };
                if params.len() != 1 || !elem.is_assignable_to(&params[0]) || *return_type != Type::Bool {
                    return Err(format!(".{} predicate must take list element and return Bool", method));
                }
                if method == "find" {
                    if matches!(**elem, Type::Optional(_)) {
                        Ok((**elem).clone())
                    } else {
                        Ok(Type::Optional(elem.clone()))
                    }
                } else {
                    Ok(Type::Bool)
                }
            }
            (Type::List(elem), "append") => {
                if args.len() != 1 {
                    return Err(".append(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(elem))?;
                if !args[0].ty.is_assignable_to(elem) {
                    return Err(format!(".append expected {}, got {}", elem, args[0].ty));
                }
                Ok(Type::Void)
            }
            (Type::List(elem), "get") => {
                if args.len() != 1 {
                    return Err(".get(index) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!(".get expects Int, got {}", args[0].ty));
                }
                if matches!(elem.as_ref(), Type::Optional(_))
                    || matches!(elem.as_ref(), Type::Union(vs) if vs.contains(&Type::Nil))
                {
                    Ok((**elem).clone())
                } else {
                    Ok(Type::Optional(elem.clone()))
                }
            }
            (Type::List(elem), "contains") => {
                if args.len() != 1 {
                    return Err(".contains(val) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(elem))?;
                if !args[0].ty.is_assignable_to(elem) {
                    return Err(format!(".contains expected {}, got {}", elem, args[0].ty));
                }
                Ok(Type::Bool)
            }
            (Type::List(elem), "slice") => {
                if args.len() != 2 {
                    return Err(".slice(start, end) expects 2 arguments".to_string());
                }
                for a in args.iter_mut() {
                    self.check_expr(a, Some(&Type::Int))?;
                    if a.ty != Type::Int {
                        return Err(format!(".slice expects Int bounds, got {}", a.ty));
                    }
                }
                Ok(Type::List(elem.clone()))
            }
            (Type::List(elem), "reverse") => {
                if !args.is_empty() {
                    return Err(".reverse() takes 0 arguments".to_string());
                }
                Ok(Type::List(elem.clone()))
            }
            (Type::List(elem), "enumerate") => {
                if !args.is_empty() {
                    return Err(".enumerate() takes 0 arguments".to_string());
                }
                Ok(Type::List(Box::new(Type::Tuple(vec![Type::Int, (**elem).clone()]))))
            }
            (Type::List(elem), "join") => {
                if **elem != Type::String {
                    return Err("join is only available on [String]".to_string());
                }
                if args.len() != 1 {
                    return Err(".join(sep) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!(".join expects String, got {}", args[0].ty));
                }
                Ok(Type::String)
            }
            (Type::String, "upper" | "lower" | "trim") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                Ok(Type::String)
            }
            (Type::String, "is_empty") => {
                if !args.is_empty() {
                    return Err(".is_empty() takes 0 arguments".to_string());
                }
                Ok(Type::Bool)
            }
            (Type::String, "contains" | "starts_with" | "ends_with") => {
                if args.len() != 1 {
                    return Err(format!(".{}() expects 1 argument", method));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!(".{} expects String, got {}", method, args[0].ty));
                }
                Ok(Type::Bool)
            }
            (Type::String, "split") => {
                if args.len() != 1 {
                    return Err(".split(sep) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!(".split expects String, got {}", args[0].ty));
                }
                Ok(Type::List(Box::new(Type::String)))
            }
            (Type::String, "replace") => {
                if args.len() != 2 {
                    return Err(".replace(from, to) expects 2 arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                self.check_expr(&mut args[1], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!(".replace 'from' expects String, got {}", args[0].ty));
                }
                if args[1].ty != Type::String {
                    return Err(format!(".replace 'to' expects String, got {}", args[1].ty));
                }
                Ok(Type::String)
            }
            (Type::String, "to_int" | "to_float") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                let ok_ty = if method == "to_int" { Type::Int } else { Type::Float };
                Ok(Type::union(vec![
                    ok_ty,
                    Type::Custom(TypeId { module: ModuleId::Builtin, name: "ParseError".into() }),
                ]))
            }
            (_, "to_string") => {
                if !args.is_empty() {
                    return Err(".to_string() takes 0 arguments".to_string());
                }
                Ok(Type::String)
            }
            (Type::Map(k, v), "set") => {
                if args.len() != 2 {
                    return Err(".set(k, v) expects 2 arguments".to_string());
                }
                self.check_expr(&mut args[0], Some(k))?;
                self.check_expr(&mut args[1], Some(v))?;
                if !args[0].ty.is_assignable_to(k) || !args[1].ty.is_assignable_to(v) {
                    return Err("Map .set key/value type mismatch".to_string());
                }
                Ok(Type::Void)
            }
            (Type::Map(k, v), "get" | "remove") => {
                if args.len() != 1 {
                    return Err(format!(".{}(k) expects 1 argument", method));
                }
                self.check_expr(&mut args[0], Some(k))?;
                if !args[0].ty.is_assignable_to(k) {
                    return Err(format!("Map .{} key type mismatch", method));
                }
                if matches!(v.as_ref(), Type::Optional(_)) {
                    Ok((**v).clone())
                } else {
                    Ok(Type::Optional(v.clone()))
                }
            }
            (Type::Map(k, _), "contains_key") => {
                if args.len() != 1 {
                    return Err(".contains_key(k) expects 1 argument".to_string());
                }
                self.check_expr(&mut args[0], Some(k))?;
                if !args[0].ty.is_assignable_to(k) {
                    return Err("Map .contains_key key type mismatch".to_string());
                }
                Ok(Type::Bool)
            }
            (Type::Map(k, v), "keys" | "values" | "entries") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                match method {
                    "keys" => Ok(Type::List(k.clone())),
                    "values" => Ok(Type::List(v.clone())),
                    _ => Ok(Type::List(Box::new(Type::Tuple(vec![(**k).clone(), (**v).clone()])))),
                }
            }
            (Type::Int, "to_float") => {
                if !args.is_empty() {
                    return Err(".to_float() takes 0 arguments".to_string());
                }
                Ok(Type::Float)
            }
            (Type::Float, "floor" | "round" | "ceil") => {
                if !args.is_empty() {
                    return Err(format!(".{}() takes 0 arguments", method));
                }
                Ok(Type::Float)
            }
            (Type::Int, "abs") | (Type::Float, "abs") => {
                if !args.is_empty() {
                    return Err(".abs() takes 0 arguments".to_string());
                }
                Ok(target.ty.clone())
            }
            (Type::Int, "min" | "max") | (Type::Float, "min" | "max") => {
                if args.len() != 1 {
                    return Err(format!(".{}(other) expects 1 argument", method));
                }
                let expected = target.ty.clone();
                self.check_expr(&mut args[0], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!(".{} expects {}, got {}", method, expected, args[0].ty));
                }
                Ok(expected)
            }
            (Type::Int, "clamp") | (Type::Float, "clamp") => {
                if args.len() != 2 {
                    return Err(".clamp(lo, hi) expects 2 arguments".to_string());
                }
                let expected = target.ty.clone();
                self.check_expr(&mut args[0], Some(&expected))?;
                self.check_expr(&mut args[1], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!(".clamp 'lo' expects {}, got {}", expected, args[0].ty));
                }
                if args[1].ty != expected {
                    return Err(format!(".clamp 'hi' expects {}, got {}", expected, args[1].ty));
                }
                Ok(expected)
            }
            (Type::Custom(tid), m) if matches!(tid.module, ModuleId::Native(_)) => {
                self.check_native_method(tid, m, type_args, args)
            }
            (Type::Generic { type_id: tid, type_args: target_targs }, m)
                if tid.module == ModuleId::Native("cdt.db".to_string()) && tid.name == "Query" =>
            {
                let target_targs = target_targs.clone();
                self.check_db_query_method(&target_targs, m, type_args, args)
            }
            (ty, m) => Err(format!("Type {} has no method '.{}()'", ty, m)),
        }
    }

    fn check_builtin_map(
        &mut self,
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !named_args.is_empty() || args.len() != 2 {
            return Err("map(list, fn) expects 2 arguments".to_string());
        }
        let (list_slice, cb_slice) = args.split_at_mut(1);
        self.check_expr(&mut list_slice[0], None)?;
        let Type::List(elem_ty) = list_slice[0].ty.clone() else {
            return Err(format!("map expects a List, got {}", list_slice[0].ty));
        };
        let expected_cb = Type::Function {
            params: vec![(*elem_ty).clone()],
            return_type: Box::new(Type::Any),
        };
        self.check_expr(&mut cb_slice[0], Some(&expected_cb))?;
        let Type::Function {
            params,
            return_type,
        } = cb_slice[0].ty.clone()
        else {
            return Err(format!(
                "map callback must be a function, got {}",
                cb_slice[0].ty
            ));
        };
        if params.len() != 1 || !elem_ty.is_assignable_to(&params[0]) {
            return Err("map callback parameter does not match list element type".to_string());
        }
        Ok(Type::List(return_type))
    }

    fn check_builtin_filter(
        &mut self,
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !named_args.is_empty() || args.len() != 2 {
            return Err("filter(list, pred) expects 2 arguments".to_string());
        }
        let (list_slice, cb_slice) = args.split_at_mut(1);
        self.check_expr(&mut list_slice[0], None)?;
        let Type::List(elem_ty) = list_slice[0].ty.clone() else {
            return Err(format!("filter expects a List, got {}", list_slice[0].ty));
        };
        let expected_pred = Type::Function {
            params: vec![(*elem_ty).clone()],
            return_type: Box::new(Type::Bool),
        };
        self.check_expr(&mut cb_slice[0], Some(&expected_pred))?;
        let Type::Function {
            params,
            return_type,
        } = cb_slice[0].ty.clone()
        else {
            return Err(format!(
                "filter predicate must be a function, got {}",
                cb_slice[0].ty
            ));
        };
        if params.len() != 1
            || !elem_ty.is_assignable_to(&params[0])
            || *return_type != Type::Bool
        {
            return Err("filter predicate must take list element and return Bool".to_string());
        }
        Ok(list_slice[0].ty.clone())
    }

    fn check_builtin_reduce(
        &mut self,
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !named_args.is_empty() || args.len() != 3 {
            return Err("reduce(list, init, fn) expects 3 arguments".to_string());
        }
        let (list_slice, rest) = args.split_at_mut(1);
        let (init_slice, cb_slice) = rest.split_at_mut(1);
        self.check_expr(&mut list_slice[0], None)?;
        let Type::List(elem_ty) = list_slice[0].ty.clone() else {
            return Err(format!("reduce expects a List, got {}", list_slice[0].ty));
        };
        self.check_expr(&mut init_slice[0], None)?;
        let init_ty = init_slice[0].ty.clone();
        let expected_cb = Type::Function {
            params: vec![init_ty.clone(), (*elem_ty).clone()],
            return_type: Box::new(init_ty.clone()),
        };
        self.check_expr(&mut cb_slice[0], Some(&expected_cb))?;
        let Type::Function {
            params,
            return_type,
        } = cb_slice[0].ty.clone()
        else {
            return Err(format!(
                "reduce callback must be a function, got {}",
                cb_slice[0].ty
            ));
        };
        if params.len() != 2
            || !init_ty.is_assignable_to(&params[0])
            || !elem_ty.is_assignable_to(&params[1])
            || !return_type.is_assignable_to(&params[0])
        {
            return Err(
                "reduce callback must have signature (Acc, Elem) -> Acc matching init and list elements"
                    .to_string(),
            );
        }
        Ok(params[0].clone())
    }

    fn check_struct_ctor(
        &mut self,
        name: &str,
        args: &mut Vec<Expr>,
        named_args: &mut Vec<NamedArg>,
    ) -> Result<Type, String> {
        let sid = self.checked.struct_by_name[name];
        let fields = self.checked.structs[sid].fields.clone();
        if !named_args.is_empty() {
            if !args.is_empty() {
                return Err(format!("Cannot mix positional and named args in '{}'", name));
            }
            let expected_rec = Type::Custom(TypeId::program(name));
            let has_spread = named_args[0].name == "..";
            let explicit_named = if has_spread {
                self.check_expr(&mut named_args[0].value, Some(&expected_rec))?;
                if !named_args[0].value.ty.is_assignable_to(&expected_rec) {
                    return Err(format!(
                        "'..' base in '{}' expected {}, got {}",
                        name, expected_rec, named_args[0].value.ty
                    ));
                }
                &mut named_args[1..]
            } else {
                &mut named_args[..]
            };
            let mut seen = HashSet::new();
            for na in explicit_named.iter_mut() {
                if na.name == ".." {
                    return Err("Record base '..expr' can only appear once at the start".to_string());
                }
                if !seen.insert(na.name.clone()) {
                    return Err(format!("Duplicate field '{}' in '{}'", na.name, name));
                }
                let f = fields
                    .iter()
                    .find(|f| f.name == na.name)
                    .ok_or_else(|| format!("Type '{}' has no field '{}'", name, na.name))?;
                self.check_expr(&mut na.value, Some(&f.ty))?;
                if !na.value.ty.is_assignable_to(&f.ty) {
                    return Err(format!(
                        "Field '{}' expected {}, got {}",
                        f.name, f.ty, na.value.ty
                    ));
                }
            }
            if !has_spread {
                for f in &fields {
                    if !seen.contains(&f.name) {
                        if let Some(def_expr) = &f.default {
                            named_args.push(NamedArg {
                                name: f.name.clone(),
                                value: def_expr.clone(),
                            });
                        } else {
                            return Err(format!(
                                "'{}' missing required field '{}'",
                                name, f.name
                            ));
                        }
                    }
                }
            }
        } else {
            if args.len() > fields.len() {
                return Err(format!(
                    "'{}' expects {} arguments, got {}",
                    name,
                    fields.len(),
                    args.len()
                ));
            }
            for (arg, f) in args.iter_mut().zip(fields.iter()) {
                self.check_expr(arg, Some(&f.ty))?;
                if !arg.ty.is_assignable_to(&f.ty) {
                    return Err(format!(
                        "Field '{}' expected {}, got {}",
                        f.name, f.ty, arg.ty
                    ));
                }
            }
            let provided = args.len();
            for f in &fields[provided..] {
                if let Some(def_expr) = &f.default {
                    args.push(def_expr.clone());
                } else {
                    return Err(format!(
                        "'{}' expects {} arguments, got {}",
                        name,
                        fields.len(),
                        provided
                    ));
                }
            }
        }
        Ok(Type::Custom(TypeId::program(name)))
    }

    pub(crate) fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &mut Expr,
        right: &mut Expr,
    ) -> Result<Type, String> {
        self.check_expr(left, None)?;
        self.check_expr(right, Some(&left.ty))?;
        match op {
            BinaryOp::Add => {
                if left.ty == right.ty
                    && (left.ty.is_int() || left.ty.is_float() || left.ty == Type::String)
                {
                    Ok(left.ty.clone())
                } else {
                    Err(format!("Invalid '+' operands: {} and {}", left.ty, right.ty))
                }
            }
            BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                if left.ty == right.ty
                    && (left.ty.is_int() || (left.ty.is_float() && op != BinaryOp::Mod))
                {
                    Ok(left.ty.clone())
                } else {
                    Err(format!(
                        "Invalid arithmetic {:?} operands: {} and {}",
                        op, left.ty, right.ty
                    ))
                }
            }
            BinaryOp::Eq | BinaryOp::NotEq => {
                if left.ty.is_assignable_to(&right.ty) || right.ty.is_assignable_to(&left.ty) {
                    Ok(Type::Bool)
                } else {
                    Err(format!("Cannot compare {} and {}", left.ty, right.ty))
                }
            }
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                if left.ty == right.ty && (left.ty.is_int() || left.ty.is_float()) {
                    Ok(Type::Bool)
                } else {
                    Err(format!("Cannot order-compare {} and {}", left.ty, right.ty))
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                if left.ty.is_bool() && right.ty.is_bool() {
                    Ok(Type::Bool)
                } else {
                    Err(format!("Logical op requires Bool, got {} and {}", left.ty, right.ty))
                }
            }
            BinaryOp::Range => {
                if left.ty.is_int() && right.ty.is_int() {
                    Ok(Type::List(Box::new(Type::Int)))
                } else {
                    Err(format!("Range '..' requires Int, got {} and {}", left.ty, right.ty))
                }
            }
            BinaryOp::Coalesce => match &left.ty {
                Type::Optional(inner) => {
                    if !right.ty.is_assignable_to(inner) {
                        return Err(format!(
                            "Coalesce fallback {} does not match {}",
                            right.ty, inner
                        ));
                    }
                    Ok((**inner).clone())
                }
                other => Ok(other.clone()),
            },
        }
    }
}
