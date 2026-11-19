use crate::frontend::ast::{Expr, NamedArg, TypeExpr};
use crate::frontend::check_native::type_to_json_type;
use crate::frontend::types::Type;
use crate::ir::RecordDesc;
use crate::ir::lower::FnLowerer;
use crate::native::cache::*;
use crate::native::jwt::{JWT_DECODE_UNVERIFIED, JWT_SIGN, JWT_VERIFY};
use crate::native::lock::*;
use crate::native::redis::*;
use crate::vm::opcode::{EXT_CACHE_OP, EXT_JWT_OP, EXT_LOCK_OP, EXT_REDIS_OP, Op};
use crate::vm::value::{
    SCHEMA_CACHE_ERROR, SCHEMA_CACHE_HANDLE, SCHEMA_JWT_ERROR, SCHEMA_LOCK_ERROR,
    SCHEMA_LOCK_GUARD, SCHEMA_REDIS_CLIENT, SCHEMA_REDIS_ERROR,
};

#[inline]
fn cb_ret_tag_from_expr(expr: &Expr) -> i64 {
    if let Type::Function { return_type, .. } = &expr.ty {
        match return_type.as_ref() {
            Type::Int | Type::Enum(_) => 1,
            Type::Float => 2,
            Type::Bool => 3,
            _ => 0,
        }
    } else {
        0
    }
}

impl<'a, 'b> FnLowerer<'a, 'b> {
    fn load_const_str_reg(&mut self, s: &str) -> Result<u16, String> {
        let idx = self.mc.intern_str(s)?;
        let r = self.alloc_temp_o();
        self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
        Ok(r)
    }

    fn load_empty_reg(&mut self) -> Result<u16, String> {
        let idx = self.mc.intern_empty()?;
        let r = self.alloc_temp_o();
        self.ops.push(Op::LoadConstObj { dst_obj: r, idx });
        Ok(r)
    }

    fn pack_record(&mut self, schema_id: u32, scalars: &[u16], objs: &[u16]) -> Result<u16, String> {
        let desc_idx = self.mc.intern_record_desc(RecordDesc {
            schema_id,
            scalar_regs: scalars.to_vec().into_boxed_slice(),
            obj_regs: objs.to_vec().into_boxed_slice(),
        })?;
        let dst = self.alloc_temp_o();
        self.ops.push(Op::NewRecord { dst_obj: dst, desc_idx });
        Ok(dst)
    }

    fn json_plan_for_type(&mut self, te: &TypeExpr, ctx: &str) -> Result<u16, String> {
        let target_ty = self.mc.resolve_type(te)?;
        let json_ty = type_to_json_type(&target_ty, self.mc.checked)
            .ok_or_else(|| format!("Unsupported JSON type '{}' in {}", target_ty, ctx))?;
        self.mc.add_json_decode_plan(&json_ty)
    }

    // ── cdt.jwt ─────────────────────────────────────────────────────────────

    pub(crate) fn lower_jwt_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "JwtError" => self.lower_native_record(
                SCHEMA_JWT_ERROR,
                &["kind", "message"],
                args,
                named_args,
            ),
            "sign" | "sign_alg" => {
                let (alg_o, idx) = if sym == "sign_alg" {
                    (self.lower_obj(&args[0])?, 1usize)
                } else {
                    (self.load_const_str_reg("HS256")?, 0usize)
                };
                let claims_o = self.lower_obj(&args[idx])?;
                let secret_o = self.lower_obj(&args[idx + 1])?;
                let ttl_s = self.alloc_temp_s();
                let has_ttl_s = self.alloc_temp_s();
                if let Some(ttl_expr) = args.get(idx + 2) {
                    let v = self.lower_scalar(ttl_expr)?;
                    self.ops.push(Op::MoveScalar { dst: ttl_s, src: v });
                    self.load_int_into(has_ttl_s, 1)?;
                } else {
                    self.load_int_into(ttl_s, 0)?;
                    self.load_int_into(has_ttl_s, 0)?;
                }
                let packed = self.pack_record(
                    SCHEMA_JWT_ERROR,
                    &[ttl_s, has_ttl_s],
                    &[alg_o, claims_o, secret_o],
                )?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_JWT_OP,
                    aux: JWT_SIGN,
                    a: dst,
                    b: packed,
                    c: 0,
                });
                Ok(dst)
            }
            "verify" | "verify_alg" => {
                let plan_idx = self.json_plan_for_type(&type_args[0], "jwt.verify()")?;
                let (alg_o, idx) = if sym == "verify_alg" {
                    (self.lower_obj(&args[0])?, 1usize)
                } else {
                    (self.load_const_str_reg("HS256")?, 0usize)
                };
                let token_o = self.lower_obj(&args[idx])?;
                let secret_o = self.lower_obj(&args[idx + 1])?;
                let plan_s = self.alloc_temp_s();
                self.load_int_into(plan_s, plan_idx as i64)?;
                let leeway_s = if let Some(lw) = args.get(idx + 2) {
                    self.lower_scalar(lw)?
                } else {
                    let r = self.alloc_temp_s();
                    self.load_int_into(r, 0)?;
                    r
                };
                let packed = self.pack_record(
                    SCHEMA_JWT_ERROR,
                    &[plan_s, leeway_s],
                    &[alg_o, token_o, secret_o],
                )?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_JWT_OP,
                    aux: JWT_VERIFY,
                    a: dst,
                    b: packed,
                    c: 0,
                });
                Ok(dst)
            }
            "decode_unverified" => {
                let plan_idx =
                    self.json_plan_for_type(&type_args[0], "jwt.decode_unverified()")?;
                let token_o = self.lower_obj(&args[0])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_JWT_OP,
                    aux: JWT_DECODE_UNVERIFIED,
                    a: dst,
                    b: token_o,
                    c: plan_idx,
                });
                Ok(dst)
            }
            other => Err(format!("Unsupported cdt.jwt symbol '{}'", other)),
        }
    }

    // ── cdt.lock ────────────────────────────────────────────────────────────

    pub(crate) fn lower_lock_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "LockError" => self.lower_native_record(
                SCHEMA_LOCK_ERROR,
                &["kind", "message"],
                args,
                named_args,
            ),
            "with" | "read" | "write" | "file" | "file_try" | "file_shared" => {
                let aux = match sym {
                    "with" => LOCK_WITH,
                    "read" => LOCK_READ,
                    "write" => LOCK_WRITE,
                    "file" => LOCK_FILE,
                    "file_try" => LOCK_FILE_TRY,
                    _ => LOCK_FILE_SHARED,
                };
                let key_o = self.lower_obj(&args[0])?;
                let cb_o = self.lower_obj(&args[1])?;
                let tag_s = self.alloc_temp_s();
                self.load_int_into(tag_s, cb_ret_tag_from_expr(&args[1]))?;
                let packed = self.pack_record(SCHEMA_LOCK_GUARD, &[tag_s], &[key_o, cb_o])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_LOCK_OP,
                    aux,
                    a: dst,
                    b: packed,
                    c: 0,
                });
                Ok(dst)
            }
            "try_with" => {
                let name_o = self.lower_obj(&args[0])?;
                let timeout_s = self.lower_scalar(&args[1])?;
                let cb_o = self.lower_obj(&args[2])?;
                let tag_s = self.alloc_temp_s();
                self.load_int_into(tag_s, cb_ret_tag_from_expr(&args[2]))?;
                let packed =
                    self.pack_record(SCHEMA_LOCK_GUARD, &[timeout_s, tag_s], &[name_o, cb_o])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_LOCK_OP,
                    aux: LOCK_TRY_WITH,
                    a: dst,
                    b: packed,
                    c: 0,
                });
                Ok(dst)
            }
            "acquire" | "file_acquire" | "file_try_acquire" => {
                let aux = match sym {
                    "acquire" => LOCK_ACQUIRE,
                    "file_acquire" => LOCK_FILE_ACQUIRE,
                    _ => LOCK_FILE_TRY_ACQUIRE,
                };
                let arg_o = self.lower_obj(&args[0])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_LOCK_OP,
                    aux,
                    a: dst,
                    b: arg_o,
                    c: 0,
                });
                Ok(dst)
            }
            "try_acquire" => {
                let name_o = self.lower_obj(&args[0])?;
                let timeout_s = self.lower_scalar(&args[1])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_LOCK_OP,
                    aux: LOCK_TRY_ACQUIRE,
                    a: dst,
                    b: name_o,
                    c: timeout_s,
                });
                Ok(dst)
            }
            other => Err(format!("Unsupported cdt.lock symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_lock_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
    ) -> Result<u16, String> {
        if type_name == "Guard" && method == "release" {
            let guard_o = self.lower_obj(target)?;
            self.ops.push(Op::ExtOp {
                kind: EXT_LOCK_OP,
                aux: LOCK_GUARD_RELEASE,
                a: 0,
                b: guard_o,
                c: 0,
            });
            return Ok(0);
        }
        Err(format!("Unsupported lock method '{}.{}()'", type_name, method))
    }

    pub(crate) fn lower_lock_scalar_method(
        &mut self,
        target: &Expr,
        method: &str,
    ) -> Result<u16, String> {
        if method != "is_held" {
            return Err(format!("Unsupported scalar lock method '.{}()'", method));
        }
        let guard_o = self.lower_obj(target)?;
        let dst_s = self.alloc_temp_s();
        self.ops.push(Op::ExtOp {
            kind: EXT_LOCK_OP,
            aux: LOCK_GUARD_IS_HELD,
            a: dst_s,
            b: guard_o,
            c: 0,
        });
        Ok(dst_s)
    }

    // ── cdt.cache ───────────────────────────────────────────────────────────

    pub(crate) fn lower_cache_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "CacheError" => self.lower_native_record(
                SCHEMA_CACHE_ERROR,
                &["kind", "message"],
                args,
                named_args,
            ),
            "open" => {
                let name_o = self.lower_obj(&args[0])?;
                let cap_s = if let Some(cap) = args.get(1) {
                    self.lower_scalar(cap)?
                } else {
                    let r = self.alloc_temp_s();
                    self.load_int_into(r, 0)?;
                    r
                };
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_OPEN,
                    a: dst,
                    b: name_o,
                    c: cap_s,
                });
                Ok(dst)
            }
            "open_file" => {
                let name_o = self.lower_obj(&args[0])?;
                let path_o = self.lower_obj(&args[1])?;
                let cap_s = if let Some(cap) = args.get(2) {
                    self.lower_scalar(cap)?
                } else {
                    let r = self.alloc_temp_s();
                    self.load_int_into(r, 0)?;
                    r
                };
                let packed =
                    self.pack_record(SCHEMA_CACHE_HANDLE, &[cap_s], &[name_o, path_o])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_OPEN_FILE,
                    a: dst,
                    b: packed,
                    c: 0,
                });
                Ok(dst)
            }
            other => Err(format!("Unsupported cdt.cache symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_cache_drop(&mut self, name_expr: &Expr) -> Result<u16, String> {
        let name_o = self.lower_obj(name_expr)?;
        let dst_s = self.alloc_temp_s();
        self.ops.push(Op::ExtOp {
            kind: EXT_CACHE_OP,
            aux: CACHE_DROP,
            a: dst_s,
            b: name_o,
            c: 0,
        });
        Ok(dst_s)
    }

    pub(crate) fn lower_cache_scalar_method(
        &mut self,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<u16, String> {
        let cache_o = self.lower_obj(target)?;
        let dst_s = self.alloc_temp_s();
        match method {
            "has" => {
                let key_o = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_HAS,
                    a: dst_s,
                    b: cache_o,
                    c: key_o,
                });
            }
            "len" => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_LEN,
                    a: dst_s,
                    b: cache_o,
                    c: 0,
                });
            }
            "prune" => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_PRUNE,
                    a: dst_s,
                    b: cache_o,
                    c: 0,
                });
            }
            _ => return Err(format!("Unsupported scalar Cache method '.{}()'", method)),
        }
        Ok(dst_s)
    }

    pub(crate) fn lower_cache_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<u16, String> {
        if type_name != "Cache" {
            return Err(format!("Unsupported cache method '{}.{}()'", type_name, method));
        }
        let cache_o = self.lower_obj(target)?;
        let dst = self.alloc_temp_o();
        match method {
            "get" | "del" | "ttl" | "save" | "load" => {
                let aux = match method {
                    "get" => CACHE_GET,
                    "del" => CACHE_DEL,
                    "ttl" => CACHE_TTL,
                    "save" => CACHE_SAVE,
                    _ => CACHE_LOAD,
                };
                let arg_o = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux,
                    a: dst,
                    b: cache_o,
                    c: arg_o,
                });
            }
            "keys" => {
                let prefix_o = if let Some(p) = args.first() {
                    self.lower_obj(p)?
                } else {
                    self.load_empty_reg()?
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_KEYS,
                    a: dst,
                    b: cache_o,
                    c: prefix_o,
                });
            }
            "clear" | "compact" => {
                let aux = if method == "clear" {
                    CACHE_CLEAR
                } else {
                    CACHE_COMPACT
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux,
                    a: dst,
                    b: cache_o,
                    c: 0,
                });
            }
            "set" | "set_json" => {
                let aux = if method == "set" {
                    CACHE_SET
                } else {
                    CACHE_SET_JSON
                };
                let key_o = self.lower_obj(&args[0])?;
                let val_o = self.lower_obj(&args[1])?;
                let ttl_s = self.alloc_temp_s();
                let has_ttl_s = self.alloc_temp_s();
                if let Some(ttl_expr) = args.get(2) {
                    let v = self.lower_scalar(ttl_expr)?;
                    self.ops.push(Op::MoveScalar { dst: ttl_s, src: v });
                    self.load_int_into(has_ttl_s, 1)?;
                } else {
                    self.load_int_into(ttl_s, 0)?;
                    self.load_int_into(has_ttl_s, 0)?;
                }
                let packed = self.pack_record(
                    SCHEMA_CACHE_HANDLE,
                    &[ttl_s, has_ttl_s],
                    &[key_o, val_o],
                )?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux,
                    a: dst,
                    b: cache_o,
                    c: packed,
                });
            }
            "get_json" => {
                let plan_idx = self.json_plan_for_type(&type_args[0], "Cache.get_json()")?;
                let plan_s = self.alloc_temp_s();
                self.load_int_into(plan_s, plan_idx as i64)?;
                let key_o = self.lower_obj(&args[0])?;
                let packed = self.pack_record(SCHEMA_CACHE_HANDLE, &[plan_s], &[key_o])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_GET_JSON,
                    a: dst,
                    b: cache_o,
                    c: packed,
                });
            }
            "get_or_set" => {
                let key_o = self.lower_obj(&args[0])?;
                let ttl_s = self.lower_scalar(&args[1])?;
                let cb_o = self.lower_obj(&args[2])?;
                let packed = self.pack_record(SCHEMA_CACHE_HANDLE, &[ttl_s], &[key_o, cb_o])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_GET_OR_SET,
                    a: dst,
                    b: cache_o,
                    c: packed,
                });
            }
            "incr" => {
                let key_o = self.lower_obj(&args[0])?;
                let delta_s = if let Some(d) = args.get(1) {
                    self.lower_scalar(d)?
                } else {
                    let r = self.alloc_temp_s();
                    self.load_int_into(r, 1)?;
                    r
                };
                let ttl_s = self.alloc_temp_s();
                let has_ttl_s = self.alloc_temp_s();
                if let Some(ttl_expr) = args.get(2) {
                    let v = self.lower_scalar(ttl_expr)?;
                    self.ops.push(Op::MoveScalar { dst: ttl_s, src: v });
                    self.load_int_into(has_ttl_s, 1)?;
                } else {
                    self.load_int_into(ttl_s, 0)?;
                    self.load_int_into(has_ttl_s, 0)?;
                }
                let packed = self.pack_record(
                    SCHEMA_CACHE_HANDLE,
                    &[delta_s, ttl_s, has_ttl_s],
                    &[key_o],
                )?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_CACHE_OP,
                    aux: CACHE_INCR,
                    a: dst,
                    b: cache_o,
                    c: packed,
                });
            }
            _ => return Err(format!("Unsupported Cache method '.{}()'", method)),
        }
        Ok(dst)
    }

    // ── cdt.redis ───────────────────────────────────────────────────────────

    pub(crate) fn lower_redis_call(
        &mut self,
        sym: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        match sym {
            "RedisError" => self.lower_native_record(
                SCHEMA_REDIS_ERROR,
                &["kind", "message"],
                args,
                named_args,
            ),
            "connect" => {
                let url_o = self.lower_obj(&args[0])?;
                let dst = self.alloc_temp_o();
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_CONNECT,
                    a: dst,
                    b: url_o,
                    c: 0,
                });
                Ok(dst)
            }
            other => Err(format!("Unsupported cdt.redis symbol '{}'", other)),
        }
    }

    pub(crate) fn lower_redis_method(
        &mut self,
        type_name: &str,
        target: &Expr,
        method: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<u16, String> {
        let recv_o = self.lower_obj(target)?;
        if type_name == "Lock" {
            let dst = self.alloc_temp_o();
            match method {
                "unlock" => {
                    self.ops.push(Op::ExtOp {
                        kind: EXT_REDIS_OP,
                        aux: REDIS_UNLOCK,
                        a: dst,
                        b: recv_o,
                        c: 0,
                    });
                }
                "extend" => {
                    let ttl_s = self.lower_scalar(&args[0])?;
                    let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[ttl_s], &[])?;
                    self.ops.push(Op::ExtOp {
                        kind: EXT_REDIS_OP,
                        aux: REDIS_EXTEND,
                        a: dst,
                        b: recv_o,
                        c: packed,
                    });
                }
                _ => return Err(format!("Unsupported redis.Lock method '.{}()'", method)),
            }
            return Ok(dst);
        }

        if type_name != "Client" {
            return Err(format!("Unsupported redis method '{}.{}()'", type_name, method));
        }
        if method == "close" {
            self.ops.push(Op::ExtOp {
                kind: EXT_REDIS_OP,
                aux: REDIS_CLOSE,
                a: 0,
                b: recv_o,
                c: 0,
            });
            return Ok(0);
        }

        let dst = self.alloc_temp_o();
        match method {
            "ping" => {
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_PING,
                    a: dst,
                    b: recv_o,
                    c: 0,
                });
            }
            "cmd" | "get" | "del" | "exists" | "ttl" | "incr" | "decr" | "keys" | "hgetall"
            | "lpop" | "rpop" | "smembers" | "unlock" => {
                let aux = match method {
                    "cmd" => REDIS_CMD,
                    "get" => REDIS_GET,
                    "del" => REDIS_DEL,
                    "exists" => REDIS_EXISTS,
                    "ttl" => REDIS_TTL,
                    "incr" => REDIS_INCR,
                    "decr" => REDIS_DECR,
                    "keys" => REDIS_KEYS,
                    "hgetall" => REDIS_HGETALL,
                    "lpop" => REDIS_LPOP,
                    "rpop" => REDIS_RPOP,
                    "smembers" => REDIS_SMEMBERS,
                    _ => REDIS_UNLOCK,
                };
                let arg_o = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux,
                    a: dst,
                    b: recv_o,
                    c: arg_o,
                });
            }
            "set" | "lpush" | "rpush" | "hget" | "hdel" | "sadd" | "srem" | "sismember" => {
                let aux = match method {
                    "set" => REDIS_SET,
                    "lpush" => REDIS_LPUSH,
                    "rpush" => REDIS_RPUSH,
                    "hget" => REDIS_HGET,
                    "hdel" => REDIS_HDEL,
                    "sadd" => REDIS_SADD,
                    "srem" => REDIS_SREM,
                    _ => REDIS_SISMEMBER,
                };
                let a0 = self.lower_obj(&args[0])?;
                let a1 = self.lower_obj(&args[1])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[], &[a0, a1])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "hset" => {
                let k = self.lower_obj(&args[0])?;
                let f = self.lower_obj(&args[1])?;
                let v = self.lower_obj(&args[2])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[], &[k, f, v])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_HSET,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "set_ex" => {
                let k = self.lower_obj(&args[0])?;
                let v = self.lower_obj(&args[1])?;
                let ttl_s = self.lower_scalar(&args[2])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[ttl_s], &[k, v])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_SET_EX,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "set_nx" | "set_json" => {
                let aux = if method == "set_nx" {
                    REDIS_SET_NX
                } else {
                    REDIS_SET_JSON
                };
                let k = self.lower_obj(&args[0])?;
                let v = self.lower_obj(&args[1])?;
                let ttl_s = self.alloc_temp_s();
                let has_ttl_s = self.alloc_temp_s();
                if let Some(ttl_expr) = args.get(2) {
                    let t = self.lower_scalar(ttl_expr)?;
                    self.ops.push(Op::MoveScalar { dst: ttl_s, src: t });
                    self.load_int_into(has_ttl_s, 1)?;
                } else {
                    self.load_int_into(ttl_s, 0)?;
                    self.load_int_into(has_ttl_s, 0)?;
                }
                let packed =
                    self.pack_record(SCHEMA_REDIS_CLIENT, &[ttl_s, has_ttl_s], &[k, v])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "expire" | "incr_by" | "lock" => {
                let aux = match method {
                    "expire" => REDIS_EXPIRE,
                    "incr_by" => REDIS_INCR_BY,
                    _ => REDIS_LOCK,
                };
                let k = self.lower_obj(&args[0])?;
                let n_s = self.lower_scalar(&args[1])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[n_s], &[k])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "get_json" => {
                let plan_idx = self.json_plan_for_type(&type_args[0], "Client.get_json()")?;
                let plan_s = self.alloc_temp_s();
                self.load_int_into(plan_s, plan_idx as i64)?;
                let k = self.lower_obj(&args[0])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[plan_s], &[k])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_GET_JSON,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "lrange" => {
                let k = self.lower_obj(&args[0])?;
                let s = self.lower_scalar(&args[1])?;
                let e = self.lower_scalar(&args[2])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[s, e], &[k])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_LRANGE,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "extend" => {
                let lock_o = self.lower_obj(&args[0])?;
                let ttl_s = self.lower_scalar(&args[1])?;
                let packed = self.pack_record(SCHEMA_REDIS_CLIENT, &[ttl_s], &[lock_o])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_EXTEND,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            "with_lock" => {
                let k = self.lower_obj(&args[0])?;
                let ttl_s = self.lower_scalar(&args[1])?;
                let cb_o = self.lower_obj(&args[2])?;
                let tag_s = self.alloc_temp_s();
                self.load_int_into(tag_s, cb_ret_tag_from_expr(&args[2]))?;
                let packed =
                    self.pack_record(SCHEMA_REDIS_CLIENT, &[ttl_s, tag_s], &[k, cb_o])?;
                self.ops.push(Op::ExtOp {
                    kind: EXT_REDIS_OP,
                    aux: REDIS_WITH_LOCK,
                    a: dst,
                    b: recv_o,
                    c: packed,
                });
            }
            _ => return Err(format!("Unsupported redis.Client method '.{}()'", method)),
        }
        Ok(dst)
    }
}
