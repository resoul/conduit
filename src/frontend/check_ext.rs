use crate::frontend::ast::{Expr, NamedArg, TypeExpr};
use crate::frontend::check::Checker;
use crate::frontend::check_native::{check_fields_ctor, is_error_type, type_to_json_type};
use crate::frontend::types::{ModuleId, Type, TypeId};

impl Checker<'_> {
    pub(crate) fn expect_arg_ty(&mut self, arg: &mut Expr, expected: &Type, ctx: &str) -> Result<(), String> {
        self.check_expr(arg, Some(expected))?;
        if !arg.ty.is_assignable_to(expected) {
            return Err(format!("{} expects {}, got {}", ctx, expected, arg.ty));
        }
        Ok(())
    }

    fn check_zero_arg_cb(&mut self, cb: &mut Expr, ctx: &str) -> Result<Type, String> {
        self.check_expr(cb, None)?;
        let Type::Function { params, return_type } = &cb.ty else {
            return Err(format!("{} expects a 0-argument function callback, got {}", ctx, cb.ty));
        };
        if !params.is_empty() {
            return Err(format!(
                "{} callback must take 0 parameters, got {}",
                ctx,
                params.len()
            ));
        }
        Ok((**return_type).clone())
    }

    // ── cdt.jwt ─────────────────────────────────────────────────────────────

    pub(crate) fn check_jwt_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let jwt_err = Type::Custom(TypeId::native("cdt.jwt", "JwtError"));
        match sym {
            "JwtError" => {
                if !type_args.is_empty() {
                    return Err("JwtError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "JwtError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    jwt_err,
                )
            }
            "sign" | "sign_alg" => {
                if !type_args.is_empty() || !named_args.is_empty() {
                    return Err(format!("jwt.{} does not accept type or named arguments", sym));
                }
                let has_alg = sym == "sign_alg";
                let min_args = if has_alg { 3 } else { 2 };
                let max_args = min_args + 1;
                if !(min_args..=max_args).contains(&args.len()) {
                    return Err(format!(
                        "jwt.{} expects {} or {} arguments",
                        sym, min_args, max_args
                    ));
                }
                let idx_claims = if has_alg {
                    self.expect_arg_ty(&mut args[0], &Type::String, "jwt.sign_alg alg")?;
                    1
                } else {
                    0
                };
                self.check_expr(&mut args[idx_claims], None)?;
                let claims_ty = &args[idx_claims].ty;
                let valid_obj_kind = matches!(
                    claims_ty,
                    Type::Custom(tid) if tid.module == ModuleId::Program
                ) || matches!(claims_ty, Type::Map(k, _) if **k == Type::String);
                if !valid_obj_kind
                    || type_to_json_type(claims_ty, self.checked).is_none()
                {
                    return Err(format!(
                        "jwt.{} claims must be a JSON-serializable struct or Map<String, ...>, got {}",
                        sym, claims_ty
                    ));
                }
                self.expect_arg_ty(
                    &mut args[idx_claims + 1],
                    &Type::String,
                    &format!("jwt.{} secret", sym),
                )?;
                if args.len() == max_args {
                    self.expect_arg_ty(
                        &mut args[idx_claims + 2],
                        &Type::Int,
                        &format!("jwt.{} ttl_secs", sym),
                    )?;
                }
                Ok(Type::union(vec![Type::String, jwt_err]))
            }
            "verify" | "verify_alg" => {
                if !named_args.is_empty() || type_args.len() != 1 {
                    return Err(format!(
                        "jwt.{}<T> requires 1 type argument and no named arguments",
                        sym
                    ));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                if type_to_json_type(&target_ty, self.checked).is_none() {
                    return Err(format!(
                        "jwt.{}<{}>: target type is not JSON-decodable",
                        sym, target_ty
                    ));
                }
                let has_alg = sym == "verify_alg";
                let min_args = if has_alg { 3 } else { 2 };
                let max_args = min_args + 1;
                if !(min_args..=max_args).contains(&args.len()) {
                    return Err(format!(
                        "jwt.{} expects {} or {} arguments",
                        sym, min_args, max_args
                    ));
                }
                let idx_tok = if has_alg {
                    self.expect_arg_ty(&mut args[0], &Type::String, "jwt.verify_alg alg")?;
                    1
                } else {
                    0
                };
                self.expect_arg_ty(
                    &mut args[idx_tok],
                    &Type::String,
                    &format!("jwt.{} token", sym),
                )?;
                self.expect_arg_ty(
                    &mut args[idx_tok + 1],
                    &Type::String,
                    &format!("jwt.{} secret", sym),
                )?;
                if args.len() == max_args {
                    self.expect_arg_ty(
                        &mut args[idx_tok + 2],
                        &Type::Int,
                        &format!("jwt.{} leeway_secs", sym),
                    )?;
                }
                Ok(Type::union(vec![target_ty, jwt_err]))
            }
            "decode_unverified" => {
                if !named_args.is_empty() || type_args.len() != 1 || args.len() != 1 {
                    return Err(
                        "jwt.decode_unverified<T>(token) expects 1 type arg and 1 String arg"
                            .to_string(),
                    );
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                if type_to_json_type(&target_ty, self.checked).is_none() {
                    return Err(format!(
                        "jwt.decode_unverified<{}>: target type is not JSON-decodable",
                        target_ty
                    ));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "jwt.decode_unverified token")?;
                Ok(Type::union(vec![target_ty, jwt_err]))
            }
            other => Err(format!("Unknown cdt.jwt symbol '{}'", other)),
        }
    }

    // ── cdt.lock ────────────────────────────────────────────────────────────

    pub(crate) fn check_lock_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let lock_err = Type::Custom(TypeId::native("cdt.lock", "LockError"));
        let guard_ty = Type::Custom(TypeId::native("cdt.lock", "Guard"));
        if sym == "LockError" {
            if !type_args.is_empty() {
                return Err("LockError does not accept type arguments".to_string());
            }
            return check_fields_ctor(
                self,
                "LockError",
                &[("kind", Type::String), ("message", Type::String)],
                args,
                named_args,
                lock_err,
            );
        }
        if !type_args.is_empty() || !named_args.is_empty() {
            return Err(format!("lock.{} does not accept type or named arguments", sym));
        }
        match sym {
            "with" | "read" | "write" | "file" | "file_try" | "file_shared" => {
                if args.len() != 2 {
                    return Err(format!("lock.{}(name, cb) expects 2 arguments", sym));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("lock.{} name", sym))?;
                let ret_ty = self.check_zero_arg_cb(&mut args[1], &format!("lock.{}", sym))?;
                Ok(Type::union(vec![ret_ty, lock_err]))
            }
            "try_with" => {
                if args.len() != 3 {
                    return Err(
                        "lock.try_with(name, timeout_ms, cb) expects 3 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "lock.try_with name")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "lock.try_with timeout_ms")?;
                let ret_ty = self.check_zero_arg_cb(&mut args[2], "lock.try_with")?;
                Ok(Type::union(vec![ret_ty, lock_err]))
            }
            "acquire" => {
                if args.len() != 1 {
                    return Err("lock.acquire(name) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "lock.acquire name")?;
                Ok(guard_ty)
            }
            "try_acquire" => {
                if args.len() != 2 {
                    return Err(
                        "lock.try_acquire(name, timeout_ms) expects 2 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "lock.try_acquire name")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "lock.try_acquire timeout_ms")?;
                Ok(Type::union(vec![guard_ty, lock_err]))
            }
            "file_acquire" | "file_try_acquire" => {
                if args.len() != 1 {
                    return Err(format!("lock.{}(path) expects 1 String argument", sym));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("lock.{} path", sym))?;
                Ok(Type::union(vec![guard_ty, lock_err]))
            }
            other => Err(format!("Unknown cdt.lock symbol '{}'", other)),
        }
    }

    pub(crate) fn check_lock_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &mut [Expr],
    ) -> Result<Type, String> {
        if type_name != "Guard" {
            return Err(format!("Type 'lock.{}' has no methods", type_name));
        }
        match method {
            "release" => {
                if !args.is_empty() {
                    return Err("Guard.release() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            "is_held" => {
                if !args.is_empty() {
                    return Err("Guard.is_held() takes 0 arguments".to_string());
                }
                Ok(Type::Bool)
            }
            _ => Err(format!("Unknown method '.{}()' on lock.Guard", method)),
        }
    }

    // ── cdt.cache ───────────────────────────────────────────────────────────

    pub(crate) fn check_cache_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let cache_err = Type::Custom(TypeId::native("cdt.cache", "CacheError"));
        let cache_ty = Type::Custom(TypeId::native("cdt.cache", "Cache"));
        if sym == "CacheError" {
            if !type_args.is_empty() {
                return Err("CacheError does not accept type arguments".to_string());
            }
            return check_fields_ctor(
                self,
                "CacheError",
                &[("kind", Type::String), ("message", Type::String)],
                args,
                named_args,
                cache_err,
            );
        }
        if !type_args.is_empty() || !named_args.is_empty() {
            return Err(format!("cache.{} does not accept type or named arguments", sym));
        }
        match sym {
            "open" => {
                if !(1..=2).contains(&args.len()) {
                    return Err("cache.open(name, max_entries?) expects 1 or 2 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "cache.open name")?;
                if args.len() == 2 {
                    self.expect_arg_ty(&mut args[1], &Type::Int, "cache.open max_entries")?;
                }
                Ok(cache_ty)
            }
            "open_file" => {
                if !(2..=3).contains(&args.len()) {
                    return Err(
                        "cache.open_file(name, path, max_entries?) expects 2 or 3 arguments"
                            .to_string(),
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "cache.open_file name")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "cache.open_file path")?;
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "cache.open_file max_entries")?;
                }
                Ok(Type::union(vec![cache_ty, cache_err]))
            }
            "drop" => {
                if args.len() != 1 {
                    return Err("cache.drop(name) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "cache.drop name")?;
                Ok(Type::Bool)
            }
            other => Err(format!("Unknown cdt.cache symbol '{}'", other)),
        }
    }

    pub(crate) fn check_cache_method(
        &mut self,
        type_name: &str,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
    ) -> Result<Type, String> {
        if type_name != "Cache" {
            return Err(format!("Type 'cache.{}' has no methods", type_name));
        }
        let cache_err = Type::Custom(TypeId::native("cdt.cache", "CacheError"));
        if !type_args.is_empty() && !matches!(method, "get_json" | "set_json") {
            return Err(format!("Cache.{}() does not accept type arguments", method));
        }
        match method {
            "get" => {
                if args.len() != 1 {
                    return Err("Cache.get(key) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.get key")?;
                Ok(Type::Optional(Box::new(Type::String)))
            }
            "set" => {
                if !(2..=3).contains(&args.len()) {
                    return Err("Cache.set(key, val, ttl_ms?) expects 2 or 3 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.set key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Cache.set val")?;
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "Cache.set ttl_ms")?;
                }
                Ok(Type::union(vec![Type::Void, cache_err]))
            }
            "get_json" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err(
                        "Cache.get_json<T>(key) expects 1 type argument and 1 String key"
                            .to_string(),
                    );
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                if type_to_json_type(&target_ty, self.checked).is_none() {
                    return Err(format!(
                        "Cache.get_json<{}>: target type is not JSON-decodable",
                        target_ty
                    ));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.get_json key")?;
                Ok(Type::union(vec![target_ty, Type::Nil, cache_err]))
            }
            "set_json" => {
                if type_args.len() > 1 || !(2..=3).contains(&args.len()) {
                    return Err(
                        "Cache.set_json(key, val, ttl_ms?) expects 2 or 3 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.set_json key")?;
                if type_args.len() == 1 {
                    let t = self.checked.resolve_type_mut(&type_args[0])?;
                    self.expect_arg_ty(&mut args[1], &t, "Cache.set_json val")?;
                } else {
                    self.check_expr(&mut args[1], None)?;
                }
                if type_to_json_type(&args[1].ty, self.checked).is_none() {
                    return Err(format!(
                        "Cache.set_json: value type {} is not JSON-serializable",
                        args[1].ty
                    ));
                }
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "Cache.set_json ttl_ms")?;
                }
                Ok(Type::union(vec![Type::Void, cache_err]))
            }
            "get_or_set" => {
                if args.len() != 3 {
                    return Err(
                        "Cache.get_or_set(key, ttl_ms, cb) expects 3 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.get_or_set key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Cache.get_or_set ttl_ms")?;
                let cb_ret = self.check_zero_arg_cb(&mut args[2], "Cache.get_or_set")?;
                let valid = match &cb_ret {
                    Type::String => true,
                    Type::Union(vars) => {
                        vars.contains(&Type::String)
                            && vars
                                .iter()
                                .all(|v| *v == Type::String || is_error_type(v, self.checked))
                    }
                    _ => false,
                };
                if !valid {
                    return Err(format!(
                        "Cache.get_or_set callback must return String or String | Error, got {}",
                        cb_ret
                    ));
                }
                Ok(Type::union(vec![cb_ret, cache_err]))
            }
            "incr" => {
                if !(1..=3).contains(&args.len()) {
                    return Err(
                        "Cache.incr(key, delta?, ttl_ms?) expects 1 to 3 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.incr key")?;
                if args.len() >= 2 {
                    self.expect_arg_ty(&mut args[1], &Type::Int, "Cache.incr delta")?;
                }
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "Cache.incr ttl_ms")?;
                }
                Ok(Type::union(vec![Type::Int, cache_err]))
            }
            "del" => {
                if args.len() != 1 {
                    return Err("Cache.del(key) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.del key")?;
                Ok(Type::union(vec![Type::Bool, cache_err]))
            }
            "has" => {
                if args.len() != 1 {
                    return Err("Cache.has(key) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.has key")?;
                Ok(Type::Bool)
            }
            "ttl" => {
                if args.len() != 1 {
                    return Err("Cache.ttl(key) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Cache.ttl key")?;
                Ok(Type::Optional(Box::new(Type::Int)))
            }
            "len" | "prune" => {
                if !args.is_empty() {
                    return Err(format!("Cache.{}() takes 0 arguments", method));
                }
                Ok(Type::Int)
            }
            "keys" => {
                if args.len() > 1 {
                    return Err("Cache.keys(prefix?) expects 0 or 1 String argument".to_string());
                }
                if args.len() == 1 {
                    self.expect_arg_ty(&mut args[0], &Type::String, "Cache.keys prefix")?;
                }
                Ok(Type::List(Box::new(Type::String)))
            }
            "clear" => {
                if !args.is_empty() {
                    return Err("Cache.clear() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::Void, cache_err]))
            }
            "save" | "load" => {
                if args.len() != 1 {
                    return Err(format!("Cache.{}(path) expects 1 String argument", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Cache.{} path", method))?;
                Ok(Type::union(vec![Type::Int, cache_err]))
            }
            "compact" => {
                if !args.is_empty() {
                    return Err("Cache.compact() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::Int, cache_err]))
            }
            _ => Err(format!("Unknown method '.{}()' on cache.Cache", method)),
        }
    }

    // ── cdt.redis ───────────────────────────────────────────────────────────

    pub(crate) fn check_redis_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let redis_err = Type::Custom(TypeId::native("cdt.redis", "RedisError"));
        let client_ty = Type::Custom(TypeId::native("cdt.redis", "Client"));
        match sym {
            "RedisError" => {
                if !type_args.is_empty() {
                    return Err("RedisError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "RedisError",
                    &[("kind", Type::String), ("message", Type::String)],
                    args,
                    named_args,
                    redis_err,
                )
            }
            "connect" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err("redis.connect(url) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "redis.connect url")?;
                Ok(Type::union(vec![client_ty, redis_err]))
            }
            other => Err(format!("Unknown cdt.redis symbol '{}'", other)),
        }
    }

    pub(crate) fn check_redis_method(
        &mut self,
        type_name: &str,
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
    ) -> Result<Type, String> {
        let redis_err = Type::Custom(TypeId::native("cdt.redis", "RedisError"));
        let lock_ty = Type::Custom(TypeId::native("cdt.redis", "Lock"));

        if type_name == "Lock" {
            if !type_args.is_empty() {
                return Err(format!("Lock.{}() does not accept type arguments", method));
            }
            return match method {
                "unlock" => {
                    if !args.is_empty() {
                        return Err("Lock.unlock() takes 0 arguments".to_string());
                    }
                    Ok(Type::union(vec![Type::Bool, redis_err]))
                }
                "extend" => {
                    if args.len() != 1 {
                        return Err("Lock.extend(ttl_ms) expects 1 Int argument".to_string());
                    }
                    self.expect_arg_ty(&mut args[0], &Type::Int, "Lock.extend ttl_ms")?;
                    Ok(Type::union(vec![Type::Bool, redis_err]))
                }
                _ => Err(format!("Unknown method '.{}()' on redis.Lock", method)),
            };
        }

        if type_name != "Client" {
            return Err(format!("Type 'redis.{}' has no methods", type_name));
        }
        if !type_args.is_empty() && !matches!(method, "get_json" | "set_json") {
            return Err(format!("Client.{}() does not accept type arguments", method));
        }

        match method {
            "ping" => {
                if !args.is_empty() {
                    return Err("Client.ping() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::String, redis_err]))
            }
            "cmd" => {
                if args.len() != 1 {
                    return Err("Client.cmd(args) expects 1 [String] argument".to_string());
                }
                self.expect_arg_ty(
                    &mut args[0],
                    &Type::List(Box::new(Type::String)),
                    "Client.cmd args",
                )?;
                Ok(Type::union(vec![Type::String, Type::Nil, redis_err]))
            }
            "close" => {
                if !args.is_empty() {
                    return Err("Client.close() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            "get" | "lpop" | "rpop" => {
                if args.len() != 1 {
                    return Err(format!("Client.{}(key) expects 1 String argument", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} key", method))?;
                Ok(Type::union(vec![Type::String, Type::Nil, redis_err]))
            }
            "set" => {
                if args.len() != 2 {
                    return Err("Client.set(key, val) expects 2 String arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.set key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Client.set val")?;
                Ok(Type::union(vec![Type::Void, redis_err]))
            }
            "set_ex" => {
                if args.len() != 3 {
                    return Err(
                        "Client.set_ex(key, val, ttl_secs) expects 3 arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.set_ex key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Client.set_ex val")?;
                self.expect_arg_ty(&mut args[2], &Type::Int, "Client.set_ex ttl_secs")?;
                Ok(Type::union(vec![Type::Void, redis_err]))
            }
            "set_nx" => {
                if !(2..=3).contains(&args.len()) {
                    return Err(
                        "Client.set_nx(key, val, ttl_secs?) expects 2 or 3 arguments".to_string(),
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.set_nx key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Client.set_nx val")?;
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "Client.set_nx ttl_secs")?;
                }
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "del" | "exists" => {
                if args.len() != 1 {
                    return Err(format!("Client.{}(key) expects 1 String argument", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} key", method))?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "expire" => {
                if args.len() != 2 {
                    return Err("Client.expire(key, ttl_secs) expects 2 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.expire key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.expire ttl_secs")?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "ttl" | "incr" | "decr" => {
                if args.len() != 1 {
                    return Err(format!("Client.{}(key) expects 1 String argument", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} key", method))?;
                Ok(Type::union(vec![Type::Int, redis_err]))
            }
            "incr_by" => {
                if args.len() != 2 {
                    return Err("Client.incr_by(key, delta) expects 2 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.incr_by key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.incr_by delta")?;
                Ok(Type::union(vec![Type::Int, redis_err]))
            }
            "keys" | "smembers" => {
                if args.len() != 1 {
                    return Err(format!("Client.{} expects 1 String argument", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} arg", method))?;
                Ok(Type::union(vec![
                    Type::List(Box::new(Type::String)),
                    redis_err,
                ]))
            }
            "get_json" => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err(
                        "Client.get_json<T>(key) expects 1 type argument and 1 String key"
                            .to_string(),
                    );
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                if type_to_json_type(&target_ty, self.checked).is_none() {
                    return Err(format!(
                        "Client.get_json<{}>: target type is not JSON-decodable",
                        target_ty
                    ));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.get_json key")?;
                Ok(Type::union(vec![target_ty, Type::Nil, redis_err]))
            }
            "set_json" => {
                if type_args.len() > 1 || !(2..=3).contains(&args.len()) {
                    return Err(
                        "Client.set_json(key, val, ttl_secs?) expects 2 or 3 arguments".to_string(),
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.set_json key")?;
                if type_args.len() == 1 {
                    let t = self.checked.resolve_type_mut(&type_args[0])?;
                    self.expect_arg_ty(&mut args[1], &t, "Client.set_json val")?;
                } else {
                    self.check_expr(&mut args[1], None)?;
                }
                if type_to_json_type(&args[1].ty, self.checked).is_none() {
                    return Err(format!(
                        "Client.set_json: value type {} is not JSON-serializable",
                        args[1].ty
                    ));
                }
                if args.len() == 3 {
                    self.expect_arg_ty(&mut args[2], &Type::Int, "Client.set_json ttl_secs")?;
                }
                Ok(Type::union(vec![Type::Void, redis_err]))
            }
            "hset" => {
                if args.len() != 3 {
                    return Err(
                        "Client.hset(key, field, val) expects 3 String arguments".to_string()
                    );
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.hset key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Client.hset field")?;
                self.expect_arg_ty(&mut args[2], &Type::String, "Client.hset val")?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "hget" => {
                if args.len() != 2 {
                    return Err("Client.hget(key, field) expects 2 String arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.hget key")?;
                self.expect_arg_ty(&mut args[1], &Type::String, "Client.hget field")?;
                Ok(Type::union(vec![Type::String, Type::Nil, redis_err]))
            }
            "hgetall" => {
                if args.len() != 1 {
                    return Err("Client.hgetall(key) expects 1 String argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.hgetall key")?;
                Ok(Type::union(vec![
                    Type::Map(Box::new(Type::String), Box::new(Type::String)),
                    redis_err,
                ]))
            }
            "hdel" | "sadd" | "srem" | "sismember" => {
                if args.len() != 2 {
                    return Err(format!("Client.{} expects 2 String arguments", method));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} key", method))?;
                self.expect_arg_ty(&mut args[1], &Type::String, &format!("Client.{} arg", method))?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "lpush" | "rpush" => {
                if args.len() != 2 {
                    return Err(format!(
                        "Client.{}(key, val) expects 2 String arguments",
                        method
                    ));
                }
                self.expect_arg_ty(&mut args[0], &Type::String, &format!("Client.{} key", method))?;
                self.expect_arg_ty(&mut args[1], &Type::String, &format!("Client.{} val", method))?;
                Ok(Type::union(vec![Type::Int, redis_err]))
            }
            "lrange" => {
                if args.len() != 3 {
                    return Err("Client.lrange(key, start, stop) expects 3 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.lrange key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.lrange start")?;
                self.expect_arg_ty(&mut args[2], &Type::Int, "Client.lrange stop")?;
                Ok(Type::union(vec![
                    Type::List(Box::new(Type::String)),
                    redis_err,
                ]))
            }
            "lock" => {
                if args.len() != 2 {
                    return Err("Client.lock(key, ttl_ms) expects 2 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.lock key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.lock ttl_ms")?;
                Ok(Type::union(vec![lock_ty, Type::Nil, redis_err]))
            }
            "unlock" => {
                if args.len() != 1 {
                    return Err("Client.unlock(lock) expects 1 redis.Lock argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &lock_ty, "Client.unlock lock")?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "extend" => {
                if args.len() != 2 {
                    return Err("Client.extend(lock, ttl_ms) expects 2 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &lock_ty, "Client.extend lock")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.extend ttl_ms")?;
                Ok(Type::union(vec![Type::Bool, redis_err]))
            }
            "with_lock" => {
                if args.len() != 3 {
                    return Err("Client.with_lock(key, ttl_ms, cb) expects 3 arguments".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::String, "Client.with_lock key")?;
                self.expect_arg_ty(&mut args[1], &Type::Int, "Client.with_lock ttl_ms")?;
                let ret_ty = self.check_zero_arg_cb(&mut args[2], "Client.with_lock")?;
                Ok(Type::union(vec![ret_ty, redis_err]))
            }
            _ => Err(format!("Unknown method '.{}()' on redis.Client", method)),
        }
    }

    // ── cdt.chan ────────────────────────────────────────────────────────────

    pub(crate) fn check_chan_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !matches!(sym, "open" | "Chan") {
            return Err(format!("Unknown cdt.chan symbol '{}'", sym));
        }
        if !named_args.is_empty() {
            return Err(format!("chan.{} does not accept named arguments", sym));
        }
        if type_args.len() != 1 || args.len() > 1 {
            return Err(format!(
                "chan.{}<T>(capacity?) expects 1 type argument and 0 or 1 capacity argument",
                sym
            ));
        }
        let elem_ty = self.checked.resolve_type_mut(&type_args[0])?;
        if elem_ty == Type::Void {
            return Err("Chan element type cannot be Void".to_string());
        }
        if args.len() == 1 {
            self.expect_arg_ty(&mut args[0], &Type::Int, "chan.open capacity")?;
        }
        Ok(Type::Generic {
            type_id: TypeId::native("cdt.chan", "Chan"),
            type_args: vec![elem_ty],
        })
    }

    pub(crate) fn check_chan_method(
        &mut self,
        target_type_args: &[Type],
        method: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("Chan.{}() does not accept type arguments", method));
        }
        let elem_ty = target_type_args.first().cloned().unwrap_or(Type::Any);
        let opt_elem_ty = if matches!(elem_ty, Type::Optional(_))
            || matches!(&elem_ty, Type::Union(vs) if vs.contains(&Type::Nil))
        {
            elem_ty.clone()
        } else {
            Type::Optional(Box::new(elem_ty.clone()))
        };
        match method {
            "send" | "try_send" => {
                if args.len() != 1 {
                    return Err(format!("Chan.{}(value) expects 1 argument", method));
                }
                self.expect_arg_ty(&mut args[0], &elem_ty, &format!("Chan.{} value", method))?;
                Ok(Type::Bool)
            }
            "recv" | "try_recv" => {
                if !args.is_empty() {
                    return Err(format!("Chan.{}() takes 0 arguments", method));
                }
                Ok(opt_elem_ty)
            }
            "recv_timeout" => {
                if args.len() != 1 {
                    return Err("Chan.recv_timeout(ms) expects 1 Int argument".to_string());
                }
                self.expect_arg_ty(&mut args[0], &Type::Int, "Chan.recv_timeout ms")?;
                Ok(opt_elem_ty)
            }
            "close" => {
                if !args.is_empty() {
                    return Err("Chan.close() takes 0 arguments".to_string());
                }
                Ok(Type::Void)
            }
            "len" | "cap" => {
                if !args.is_empty() {
                    return Err(format!("Chan.{}() takes 0 arguments", method));
                }
                Ok(Type::Int)
            }
            "is_empty" | "is_closed" => {
                if !args.is_empty() {
                    return Err(format!("Chan.{}() takes 0 arguments", method));
                }
                Ok(Type::Bool)
            }
            other => Err(format!("Chan has no method '.{}()'", other)),
        }
    }
}
