use std::sync::Arc;

use crate::ir::Module;
use crate::native::json_decode::decode_json_with_plan;
use crate::native::json_encode::encode_json_obj;
use crate::native::redis::{
    REDIS_CLOSE, REDIS_CMD, REDIS_CONNECT, REDIS_DECR, REDIS_DEL, REDIS_EXISTS, REDIS_EXPIRE,
    REDIS_EXTEND, REDIS_GET, REDIS_GET_JSON, REDIS_HDEL, REDIS_HGET, REDIS_HGETALL, REDIS_HSET,
    REDIS_INCR, REDIS_INCR_BY, REDIS_KEYS, REDIS_LOCK, REDIS_LPOP, REDIS_LPUSH, REDIS_LRANGE,
    REDIS_PING, REDIS_RPOP, REDIS_RPUSH, REDIS_SADD, REDIS_SET, REDIS_SET_EX, REDIS_SET_JSON,
    REDIS_SET_NX, REDIS_SISMEMBER, REDIS_SMEMBERS, REDIS_SREM, REDIS_TTL, REDIS_UNLOCK,
    REDIS_WITH_LOCK, RedisConn, RedisErr, RespValue, close_client, get_client_slot,
    lock_acquire_conn, lock_extend_conn, lock_unlock_conn, make_redis_client, make_redis_error,
    make_redis_lock, register_client, resp_cmd_to_opt_string, resp_to_int, resp_to_opt_string,
    resp_to_string_list,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_REDIS_CLIENT, SCHEMA_REDIS_LOCK,
};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    fn extract_redis_client_id(
        &self,
        o_base: usize,
        reg: u16,
        ctx: &str,
    ) -> Result<u64, RuntimeError> {
        let Obj::Record(rec) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected redis.Client in {}", ctx)));
        };
        if rec.schema_id != SCHEMA_REDIS_CLIENT {
            return Err(err_msg(&format!("Expected redis.Client in {}", ctx)));
        }
        Ok(rec.get_scalar(0))
    }

    fn with_redis_conn<F>(&self, id: u64, f: F) -> Obj
    where
        F: FnOnce(&mut RedisConn) -> Result<Obj, RedisErr>,
    {
        let Some(slot) = get_client_slot(id) else {
            return make_redis_error("closed", "Redis connection is closed");
        };
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let Some(conn) = guard.as_mut() else {
            return make_redis_error("closed", "Redis connection is closed");
        };
        match f(conn) {
            Ok(o) => o,
            Err(e) => e.into_obj(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_redis_op(
        &mut self,
        module: &Module,
        _s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            REDIS_CONNECT => {
                let url = self.sys_str(o_base, b, "redis.connect()")?;
                self.host
                    .permissions
                    .check_any_net()
                    .map_err(|e| err_msg(&e))?;
                if let Ok(cfg) = crate::native::redis::parse_redis_url(url.as_str()) {
                    self.host
                        .permissions
                        .check_net(&cfg.host, cfg.port)
                        .map_err(|e| err_msg(&e))?;
                }
                self.objs[o_base + a as usize] = match RedisConn::open(url.as_str()) {
                    Ok(conn) => make_redis_client(register_client(conn)),
                    Err(e) => e.into_obj(),
                };
            }
            REDIS_CLOSE => {
                let id = self.extract_redis_client_id(o_base, b, "Client.close()")?;
                close_client(id);
            }
            REDIS_PING => {
                let id = self.extract_redis_client_id(o_base, b, "Client.ping()")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"PING"])?;
                    Ok(Obj::str(
                        resp_to_opt_string(resp)?.unwrap_or_else(|| "PONG".to_string()),
                    ))
                });
            }
            REDIS_CMD => {
                let id = self.extract_redis_client_id(o_base, b, "Client.cmd()")?;
                let Obj::List(ListStorage::Obj(args_obj)) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected [String] arguments in Client.cmd()"));
                };
                if args_obj.is_empty() {
                    self.objs[o_base + a as usize] =
                        make_redis_error("invalid_input", "Client.cmd() requires at least 1 argument");
                    return Ok(());
                }
                let mut owned_args = Vec::with_capacity(args_obj.len());
                for item in args_obj.iter() {
                    let Obj::Str(s) = item else {
                        return Err(err_msg("Expected [String] arguments in Client.cmd()"));
                    };
                    owned_args.push(s.as_str().to_string());
                }
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let refs: Vec<&[u8]> = owned_args.iter().map(|s| s.as_bytes()).collect();
                    let resp = conn.send_cmd(&refs)?;
                    Ok(match resp_cmd_to_opt_string(resp)? {
                        Some(s) => Obj::str(s),
                        None => Obj::Empty,
                    })
                });
            }
            REDIS_GET => {
                let id = self.extract_redis_client_id(o_base, b, "Client.get()")?;
                let key = self.sys_str(o_base, c, "Client.get()")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"GET", key.as_bytes()])?;
                    Ok(match resp_to_opt_string(resp)? {
                        Some(s) => Obj::str(s),
                        None => Obj::Empty,
                    })
                });
            }
            REDIS_SET => {
                let id = self.extract_redis_client_id(o_base, b, "Client.set()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.set()"));
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.set()"));
                };
                let Obj::Str(val) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String val in Client.set()"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    conn.send_cmd(&[b"SET", key.as_bytes(), val.as_bytes()])?;
                    Ok(Obj::Empty)
                });
            }
            REDIS_SET_EX => {
                let id = self.extract_redis_client_id(o_base, b, "Client.set_ex()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.set_ex()"));
                };
                let ttl_secs = packed.get_scalar(0) as i64;
                if ttl_secs <= 0 {
                    self.objs[o_base + a as usize] =
                        make_redis_error("invalid_input", "Client.set_ex() ttl_secs must be > 0");
                    return Ok(());
                }
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.set_ex()"));
                };
                let Obj::Str(val) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String val in Client.set_ex()"));
                };
                let ttl_s = ttl_secs.to_string();
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    conn.send_cmd(&[
                        b"SET",
                        key.as_bytes(),
                        val.as_bytes(),
                        b"EX",
                        ttl_s.as_bytes(),
                    ])?;
                    Ok(Obj::Empty)
                });
            }
            REDIS_SET_NX => {
                let id = self.extract_redis_client_id(o_base, b, "Client.set_nx()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.set_nx()"));
                };
                let ttl_secs = packed.get_scalar(0) as i64;
                let has_ttl = packed.get_scalar(1) != 0;
                if has_ttl && ttl_secs <= 0 {
                    self.objs[o_base + a as usize] =
                        make_redis_error("invalid_input", "Client.set_nx() ttl_secs must be > 0");
                    return Ok(());
                }
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.set_nx()"));
                };
                let Obj::Str(val) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String val in Client.set_nx()"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = if has_ttl {
                        let ttl_s = ttl_secs.to_string();
                        conn.send_cmd(&[
                            b"SET",
                            key.as_bytes(),
                            val.as_bytes(),
                            b"EX",
                            ttl_s.as_bytes(),
                            b"NX",
                        ])?
                    } else {
                        conn.send_cmd(&[b"SET", key.as_bytes(), val.as_bytes(), b"NX"])?
                    };
                    let ok = matches!(resp, RespValue::Simple(ref s) if s == "OK")
                        || matches!(resp, RespValue::Int(n) if n > 0);
                    Ok(Obj::boxed_bool(ok))
                });
            }
            REDIS_DEL | REDIS_EXISTS => {
                let cmd = if aux == REDIS_DEL { b"DEL".as_slice() } else { b"EXISTS".as_slice() };
                let id = self.extract_redis_client_id(o_base, b, "Client key check/del")?;
                let key = self.sys_str(o_base, c, "Client key check/del")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[cmd, key.as_bytes()])?;
                    Ok(Obj::boxed_bool(resp_to_int(resp)? > 0))
                });
            }
            REDIS_EXPIRE => {
                let id = self.extract_redis_client_id(o_base, b, "Client.expire()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.expire()"));
                };
                let ttl_secs = packed.get_scalar(0) as i64;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.expire()"));
                };
                let ttl_s = ttl_secs.to_string();
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"EXPIRE", key.as_bytes(), ttl_s.as_bytes()])?;
                    Ok(Obj::boxed_bool(resp_to_int(resp)? > 0))
                });
            }
            REDIS_TTL | REDIS_INCR | REDIS_DECR => {
                let cmd = match aux {
                    REDIS_TTL => b"TTL".as_slice(),
                    REDIS_INCR => b"INCR".as_slice(),
                    _ => b"DECR".as_slice(),
                };
                let id = self.extract_redis_client_id(o_base, b, "Client integer op")?;
                let key = self.sys_str(o_base, c, "Client integer op")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[cmd, key.as_bytes()])?;
                    Ok(Obj::boxed_int(resp_to_int(resp)?))
                });
            }
            REDIS_INCR_BY => {
                let id = self.extract_redis_client_id(o_base, b, "Client.incr_by()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.incr_by()"));
                };
                let delta = packed.get_scalar(0) as i64;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.incr_by()"));
                };
                let delta_s = delta.to_string();
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"INCRBY", key.as_bytes(), delta_s.as_bytes()])?;
                    Ok(Obj::boxed_int(resp_to_int(resp)?))
                });
            }
            REDIS_KEYS => {
                let id = self.extract_redis_client_id(o_base, b, "Client.keys()")?;
                let pat = self.sys_str(o_base, c, "Client.keys()")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"KEYS", pat.as_bytes()])?;
                    let keys: Vec<Obj> = resp_to_string_list(resp)?.into_iter().map(Obj::str).collect();
                    Ok(Obj::List(ListStorage::Obj(Arc::new(keys))))
                });
            }
            REDIS_GET_JSON => {
                let id = self.extract_redis_client_id(o_base, b, "Client.get_json()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.get_json()"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.get_json()"));
                };
                let Some(plan) = module.json_plans.get(plan_idx) else {
                    return Err(err_msg("JSON decode plan out of bounds in Client.get_json()"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"GET", key.as_bytes()])?;
                    match resp_to_opt_string(resp)? {
                        None => Ok(Obj::Empty),
                        Some(raw) => decode_json_with_plan(&raw, plan)
                            .map_err(|e| RedisErr::new("decode", e.to_string())),
                    }
                });
            }
            REDIS_SET_JSON => {
                let id = self.extract_redis_client_id(o_base, b, "Client.set_json()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.set_json()"));
                };
                let ttl_secs = packed.get_scalar(0) as i64;
                let has_ttl = packed.get_scalar(1) != 0;
                if has_ttl && ttl_secs <= 0 {
                    self.objs[o_base + a as usize] =
                        make_redis_error("invalid_input", "Client.set_json() ttl_secs must be > 0");
                    return Ok(());
                }
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.set_json()"));
                };
                let val_obj = packed.get_obj(1);
                let json = match encode_json_obj(val_obj, &module.struct_schemas) {
                    Ok(j) => j,
                    Err(e) => {
                        self.objs[o_base + a as usize] = make_redis_error("encode", e.to_string());
                        return Ok(());
                    }
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    if has_ttl {
                        let ttl_s = ttl_secs.to_string();
                        conn.send_cmd(&[
                            b"SET",
                            key.as_bytes(),
                            json.as_bytes(),
                            b"EX",
                            ttl_s.as_bytes(),
                        ])?;
                    } else {
                        conn.send_cmd(&[b"SET", key.as_bytes(), json.as_bytes()])?;
                    }
                    Ok(Obj::Empty)
                });
            }
            REDIS_HSET => {
                let id = self.extract_redis_client_id(o_base, b, "Client.hset()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.hset()"));
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.hset()"));
                };
                let Obj::Str(field) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String field in Client.hset()"));
                };
                let Obj::Str(val) = packed.get_obj(2) else {
                    return Err(err_msg("Expected String val in Client.hset()"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[
                        b"HSET",
                        key.as_bytes(),
                        field.as_bytes(),
                        val.as_bytes(),
                    ])?;
                    Ok(Obj::boxed_bool(resp_to_int(resp)? > 0))
                });
            }
            REDIS_HGET => {
                let id = self.extract_redis_client_id(o_base, b, "Client.hget()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.hget()"));
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.hget()"));
                };
                let Obj::Str(field) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String field in Client.hget()"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"HGET", key.as_bytes(), field.as_bytes()])?;
                    Ok(match resp_to_opt_string(resp)? {
                        Some(v) => Obj::str(v),
                        None => Obj::Empty,
                    })
                });
            }
            REDIS_HGETALL => {
                let id = self.extract_redis_client_id(o_base, b, "Client.hgetall()")?;
                let key = self.sys_str(o_base, c, "Client.hgetall()")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"HGETALL", key.as_bytes()])?;
                    let flat = resp_to_string_list(resp)?;
                    if !flat.len().is_multiple_of(2) {
                        return Err(RedisErr::new("protocol", "HGETALL returned odd element count"));
                    }
                    let mut map = OrderedMap::with_capacity(flat.len() / 2);
                    for pair in flat.as_chunks::<2>().0 {
                        map.set(
                            MapKey::Str(Arc::new(pair[0].clone().into())),
                            MapVal::Obj(Obj::str(pair[1].clone())),
                        );
                    }
                    Ok(Obj::Map(Arc::new(map)))
                });
            }
            REDIS_HDEL | REDIS_SADD | REDIS_SREM | REDIS_SISMEMBER => {
                let cmd = match aux {
                    REDIS_HDEL => b"HDEL".as_slice(),
                    REDIS_SADD => b"SADD".as_slice(),
                    REDIS_SREM => b"SREM".as_slice(),
                    _ => b"SISMEMBER".as_slice(),
                };
                let id = self.extract_redis_client_id(o_base, b, "Client hash/set op")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client hash/set op"));
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client hash/set op"));
                };
                let Obj::Str(arg) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String field/member in Client hash/set op"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[cmd, key.as_bytes(), arg.as_bytes()])?;
                    Ok(Obj::boxed_bool(resp_to_int(resp)? > 0))
                });
            }
            REDIS_LPUSH | REDIS_RPUSH => {
                let cmd = if aux == REDIS_LPUSH {
                    b"LPUSH".as_slice()
                } else {
                    b"RPUSH".as_slice()
                };
                let id = self.extract_redis_client_id(o_base, b, "Client list push")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client list push"));
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client list push"));
                };
                let Obj::Str(val) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String val in Client list push"));
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[cmd, key.as_bytes(), val.as_bytes()])?;
                    Ok(Obj::boxed_int(resp_to_int(resp)?))
                });
            }
            REDIS_LPOP | REDIS_RPOP => {
                let cmd = if aux == REDIS_LPOP {
                    b"LPOP".as_slice()
                } else {
                    b"RPOP".as_slice()
                };
                let id = self.extract_redis_client_id(o_base, b, "Client list pop")?;
                let key = self.sys_str(o_base, c, "Client list pop")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[cmd, key.as_bytes()])?;
                    Ok(match resp_to_opt_string(resp)? {
                        Some(v) => Obj::str(v),
                        None => Obj::Empty,
                    })
                });
            }
            REDIS_LRANGE => {
                let id = self.extract_redis_client_id(o_base, b, "Client.lrange()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.lrange()"));
                };
                let start = packed.get_scalar(0) as i64;
                let stop = packed.get_scalar(1) as i64;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.lrange()"));
                };
                let s_str = start.to_string();
                let e_str = stop.to_string();
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[
                        b"LRANGE",
                        key.as_bytes(),
                        s_str.as_bytes(),
                        e_str.as_bytes(),
                    ])?;
                    let items: Vec<Obj> = resp_to_string_list(resp)?.into_iter().map(Obj::str).collect();
                    Ok(Obj::List(ListStorage::Obj(Arc::new(items))))
                });
            }
            REDIS_SMEMBERS => {
                let id = self.extract_redis_client_id(o_base, b, "Client.smembers()")?;
                let key = self.sys_str(o_base, c, "Client.smembers()")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    let resp = conn.send_cmd(&[b"SMEMBERS", key.as_bytes()])?;
                    let items: Vec<Obj> = resp_to_string_list(resp)?.into_iter().map(Obj::str).collect();
                    Ok(Obj::List(ListStorage::Obj(Arc::new(items))))
                });
            }
            REDIS_LOCK => {
                let id = self.extract_redis_client_id(o_base, b, "Client.lock()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.lock()"));
                };
                let ttl_ms = packed.get_scalar(0) as i64;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.lock()"));
                };
                let key_s = key.as_str().to_string();
                self.objs[o_base + a as usize] = self.with_redis_conn(id, |conn| {
                    Ok(match lock_acquire_conn(conn, &key_s, ttl_ms)? {
                        Some(tok) => make_redis_lock(id, key_s, tok),
                        None => Obj::Empty,
                    })
                });
            }
            REDIS_UNLOCK => {
                // Supports both `client.unlock(lock)` (c != 0, packed or direct Lock in c)
                // and `lock.unlock()` (b = Lock, c = 0). Wait: lower_ext uses:
                // b = Lock, c = 0 for Lock.unlock(); and for Client.unlock(lock): b = Lock, c = 0 or b = Client, c = Lock.
                let (cid, key, tok) = self.extract_lock_and_opt_client(o_base, b, c, "unlock")?;
                self.objs[o_base + a as usize] = self.with_redis_conn(cid, |conn| {
                    Ok(Obj::boxed_bool(lock_unlock_conn(conn, &key, &tok)?))
                });
            }
            REDIS_EXTEND => {
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Redis extend()"));
                };
                let ttl_ms = packed.get_scalar(0) as i64;
                let (cid, key, tok) = if matches!(
                    &self.objs[o_base + b as usize],
                    Obj::Record(r) if r.schema_id == SCHEMA_REDIS_LOCK
                ) {
                    let Obj::Record(lock_rec) = &self.objs[o_base + b as usize] else {
                        unreachable!()
                    };
                    let cid = lock_rec.get_scalar(0);
                    let Obj::Str(k) = lock_rec.get_obj(0) else {
                        return Err(err_msg("Expected String key in Lock"));
                    };
                    let Obj::Str(t) = lock_rec.get_obj(1) else {
                        return Err(err_msg("Expected String token in Lock"));
                    };
                    (cid, k.as_str().to_string(), t.as_str().to_string())
                } else {
                    let cid = self.extract_redis_client_id(o_base, b, "Client.extend()")?;
                    let Obj::Record(lock_rec) = packed.get_obj(0) else {
                        return Err(err_msg("Expected redis.Lock in Client.extend()"));
                    };
                    if lock_rec.schema_id != SCHEMA_REDIS_LOCK {
                        return Err(err_msg("Expected redis.Lock in Client.extend()"));
                    }
                    let Obj::Str(k) = lock_rec.get_obj(0) else {
                        return Err(err_msg("Expected String key in Lock"));
                    };
                    let Obj::Str(t) = lock_rec.get_obj(1) else {
                        return Err(err_msg("Expected String token in Lock"));
                    };
                    (cid, k.as_str().to_string(), t.as_str().to_string())
                };
                self.objs[o_base + a as usize] = self.with_redis_conn(cid, |conn| {
                    Ok(Obj::boxed_bool(lock_extend_conn(conn, &key, &tok, ttl_ms)?))
                });
            }
            REDIS_WITH_LOCK => {
                let id = self.extract_redis_client_id(o_base, b, "Client.with_lock()")?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Client.with_lock()"));
                };
                let ttl_ms = packed.get_scalar(0) as i64;
                let ret_tag = packed.get_scalar(1) as u8;
                let Obj::Str(key_s) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Client.with_lock()"));
                };
                let key = key_s.as_str().to_string();
                let cb = packed.get_obj(1).clone();

                let Some(slot) = get_client_slot(id) else {
                    self.objs[o_base + a as usize] =
                        make_redis_error("closed", "Redis connection is closed");
                    return Ok(());
                };
                let token = {
                    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(conn) = guard.as_mut() else {
                        self.objs[o_base + a as usize] =
                            make_redis_error("closed", "Redis connection is closed");
                        return Ok(());
                    };
                    match lock_acquire_conn(conn, &key, ttl_ms) {
                        Ok(Some(tok)) => tok,
                        Ok(None) => {
                            self.objs[o_base + a as usize] = make_redis_error(
                                "lock_busy",
                                format!("Could not acquire Redis lock '{}'", key),
                            );
                            return Ok(());
                        }
                        Err(e) => {
                            self.objs[o_base + a as usize] = e.into_obj();
                            return Ok(());
                        }
                    }
                };
                // RedisConn Mutex is released here so `cb` may invoke operations on the same Client!
                let cb_res = self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag);

                // Always attempt unlock regardless of callback result.
                let unlock_res = {
                    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
                    match guard.as_mut() {
                        Some(conn) => lock_unlock_conn(conn, &key, &token).map(|_| ()),
                        None => Ok(()),
                    }
                };

                let val = cb_res?;
                if let Err(e) = unlock_res {
                    // Only surface unlock protocol/IO error if callback itself didn't already return an error?
                    // Wait: better to preserve callback error or unlock error cleanly; if val is not error and unlock failed:
                    if !crate::native::validation::is_err_or_nil_obj(module, &val) || val == Obj::Empty
                    {
                        self.objs[o_base + a as usize] = e.into_obj();
                        return Ok(());
                    }
                }
                self.objs[o_base + a as usize] = val;
            }
            _ => return Err(err_msg("Unknown EXT_REDIS_OP sub-op")),
        }
        Ok(())
    }

    fn extract_lock_and_opt_client(
        &self,
        o_base: usize,
        b: u16,
        c: u16,
        ctx: &str,
    ) -> Result<(u64, String, String), RuntimeError> {
        let b_obj = &self.objs[o_base + b as usize];
        if let Obj::Record(lock_rec) = b_obj
            && lock_rec.schema_id == SCHEMA_REDIS_LOCK
        {
            let cid = lock_rec.get_scalar(0);
            let Obj::Str(k) = lock_rec.get_obj(0) else {
                return Err(err_msg("Expected String key in Lock"));
            };
            let Obj::Str(t) = lock_rec.get_obj(1) else {
                return Err(err_msg("Expected String token in Lock"));
            };
            return Ok((cid, k.as_str().to_string(), t.as_str().to_string()));
        }
        let cid = self.extract_redis_client_id(o_base, b, ctx)?;
        let Obj::Record(lock_rec) = &self.objs[o_base + c as usize] else {
            return Err(err_msg(&format!("Expected redis.Lock in {}", ctx)));
        };
        if lock_rec.schema_id != SCHEMA_REDIS_LOCK {
            return Err(err_msg(&format!("Expected redis.Lock in {}", ctx)));
        }
        let Obj::Str(k) = lock_rec.get_obj(0) else {
            return Err(err_msg("Expected String key in Lock"));
        };
        let Obj::Str(t) = lock_rec.get_obj(1) else {
            return Err(err_msg("Expected String token in Lock"));
        };
        Ok((cid, k.as_str().to_string(), t.as_str().to_string()))
    }
}
