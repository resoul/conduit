use std::path::Path;
use std::sync::Arc;

use crate::ir::Module;
use crate::native::cache::{
    CACHE_CLEAR, CACHE_COMPACT, CACHE_DEL, CACHE_DROP, CACHE_GET, CACHE_GET_JSON, CACHE_GET_OR_SET,
    CACHE_HAS, CACHE_INCR, CACHE_KEYS, CACHE_LEN, CACHE_LOAD, CACHE_OPEN, CACHE_OPEN_FILE,
    CACHE_PRUNE, CACHE_SAVE, CACHE_SET, CACHE_SET_JSON, CACHE_SET_NO_TTL, CACHE_TTL, CacheStore,
    drop_cache, get_cache_store, make_cache_error, make_cache_handle, open_cache, open_cache_file,
};
use crate::native::json_decode::decode_json_with_plan;
use crate::native::json_encode::encode_json_obj;
use crate::native::validation::is_err_or_nil_obj;
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{ListStorage, Obj, SCHEMA_CACHE_HANDLE};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    fn extract_cache_store(
        &self,
        o_base: usize,
        reg: u16,
        ctx: &str,
    ) -> Result<Option<Arc<CacheStore>>, RuntimeError> {
        let Obj::Record(rec) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected cache.Cache in {}", ctx)));
        };
        if rec.schema_id != SCHEMA_CACHE_HANDLE {
            return Err(err_msg(&format!("Expected cache.Cache in {}", ctx)));
        }
        Ok(get_cache_store(rec.get_scalar(0)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_cache_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            CACHE_OPEN => {
                let name = self.sys_str(o_base, b, "cache.open()")?;
                let max_entries = self.scalars[s_base + c as usize] as i64;
                let id = open_cache(name.as_str(), max_entries);
                self.objs[o_base + a as usize] = make_cache_handle(id);
            }
            CACHE_OPEN_FILE => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed arguments in cache.open_file()"));
                };
                let max_entries = packed.get_scalar(0) as i64;
                let Obj::Str(name) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String name in cache.open_file()"));
                };
                let Obj::Str(path) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String path in cache.open_file()"));
                };
                self.objs[o_base + a as usize] =
                    match open_cache_file(name.as_str(), path.as_str(), max_entries) {
                        Ok(id) => make_cache_handle(id),
                        Err((k, msg)) => make_cache_error(k, msg),
                    };
            }
            CACHE_DROP => {
                let name = self.sys_str(o_base, b, "cache.drop()")?;
                self.scalars[s_base + a as usize] = u64::from(drop_cache(name.as_str()));
            }
            CACHE_GET => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.get()")? else {
                    self.objs[o_base + a as usize] = Obj::Empty;
                    return Ok(());
                };
                let key = self.sys_str(o_base, c, "Cache.get()")?;
                self.objs[o_base + a as usize] = match store.get_str(key.as_str()) {
                    Some(v) => Obj::Str(v),
                    None => Obj::Empty,
                };
            }
            CACHE_SET => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.set()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Cache.set()"));
                };
                let has_ttl = packed.get_scalar(1) != 0;
                let ttl_ms = if has_ttl {
                    Some(packed.get_scalar(0) as i64)
                } else {
                    None
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Cache.set()"));
                };
                let Obj::Str(val) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String value in Cache.set()"));
                };
                self.objs[o_base + a as usize] =
                    match store.set_str(key, Arc::clone(val), ttl_ms) {
                        Ok(()) => Obj::Empty,
                        Err((k, msg)) => make_cache_error(k, msg),
                    };
            }
            CACHE_SET_NO_TTL => {
                let Some(store) = self.extract_cache_store(o_base, a, "Cache.set()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let Obj::Str(key) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String key in Cache.set()"));
                };
                let Obj::Str(val) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected String value in Cache.set()"));
                };
                self.objs[o_base + a as usize] =
                    match store.set_str(key, Arc::clone(val), None) {
                        Ok(()) => Obj::Empty,
                        Err((k, msg)) => make_cache_error(k, msg),
                    };
            }
            CACHE_GET_JSON => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.get_json()")? else {
                    self.objs[o_base + a as usize] = Obj::Empty;
                    return Ok(());
                };
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Cache.get_json()"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Cache.get_json()"));
                };
                let Some(plan) = module.json_plans.get(plan_idx) else {
                    return Err(err_msg("JSON decode plan out of bounds in Cache.get_json()"));
                };
                self.objs[o_base + a as usize] = match store.get_str(key.as_str()) {
                    None => Obj::Empty,
                    Some(raw) => match decode_json_with_plan(&raw, plan) {
                        Ok(val) => val,
                        Err(e) => make_cache_error("decode", e.to_string()),
                    },
                };
            }
            CACHE_SET_JSON => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.set_json()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Cache.set_json()"));
                };
                let has_ttl = packed.get_scalar(1) != 0;
                let ttl_ms = if has_ttl {
                    Some(packed.get_scalar(0) as i64)
                } else {
                    None
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Cache.set_json()"));
                };
                let val_obj = packed.get_obj(1);
                self.objs[o_base + a as usize] =
                    match encode_json_obj(val_obj, &module.struct_schemas) {
                        Err(e) => make_cache_error("encode", e.to_string()),
                        Ok(json) => match store.set_str(key, Arc::new(json.into()), ttl_ms) {
                            Ok(()) => Obj::Empty,
                            Err((k, msg)) => make_cache_error(k, msg),
                        },
                    };
            }
            CACHE_GET_OR_SET => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.get_or_set()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Cache.get_or_set()"));
                };
                let ttl_ms = packed.get_scalar(0) as i64;
                if ttl_ms < 0 {
                    self.objs[o_base + a as usize] =
                        make_cache_error("invalid_input", "Cache TTL must be non-negative");
                    return Ok(());
                }
                let Obj::Str(key_s) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Cache.get_or_set()"));
                };
                let key = Arc::clone(key_s);
                let cb = packed.get_obj(1).clone();

                if let Some(hit) = store.get_str(&key) {
                    self.objs[o_base + a as usize] = Obj::Str(hit);
                    return Ok(());
                }

                let key_lock = store.key_lock(&key);
                let _kguard = key_lock.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(hit) = store.get_str(&key) {
                    self.objs[o_base + a as usize] = Obj::Str(hit);
                    return Ok(());
                }

                let res = self.invoke_zero_arg_cb(module, s_top, o_top, &cb, 0)?;
                if is_err_or_nil_obj(module, &res) {
                    self.objs[o_base + a as usize] = res;
                    return Ok(());
                }
                let Obj::Str(val_s) = &res else {
                    return Err(err_msg("Cache.get_or_set() callback must return String"));
                };
                self.objs[o_base + a as usize] =
                    match store.set_str(&key, Arc::clone(val_s), Some(ttl_ms)) {
                        Ok(()) => res,
                        Err((k, msg)) => make_cache_error(k, msg),
                    };
            }
            CACHE_INCR => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.incr()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed arguments in Cache.incr()"));
                };
                let delta = packed.get_scalar(0) as i64;
                let has_ttl = packed.get_scalar(2) != 0;
                let ttl_ms = if has_ttl {
                    Some(packed.get_scalar(1) as i64)
                } else {
                    None
                };
                let Obj::Str(key) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in Cache.incr()"));
                };
                self.objs[o_base + a as usize] = match store.incr(key, delta, ttl_ms) {
                    Ok(v) => Obj::boxed_int(v),
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            CACHE_DEL => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.del()")? else {
                    self.objs[o_base + a as usize] = Obj::boxed_bool(false);
                    return Ok(());
                };
                let key = self.sys_str(o_base, c, "Cache.del()")?;
                self.objs[o_base + a as usize] = match store.del(key.as_str()) {
                    Ok(removed) => Obj::boxed_bool(removed),
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            CACHE_HAS => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.has()")? else {
                    self.scalars[s_base + a as usize] = 0;
                    return Ok(());
                };
                let key = self.sys_str(o_base, c, "Cache.has()")?;
                self.scalars[s_base + a as usize] = u64::from(store.has(key.as_str()));
            }
            CACHE_TTL => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.ttl()")? else {
                    self.objs[o_base + a as usize] = Obj::Empty;
                    return Ok(());
                };
                let key = self.sys_str(o_base, c, "Cache.ttl()")?;
                self.objs[o_base + a as usize] = match store.ttl_ms(key.as_str()) {
                    Some(ms) => Obj::boxed_int(ms),
                    None => Obj::Empty,
                };
            }
            CACHE_LEN => {
                let len = self
                    .extract_cache_store(o_base, b, "Cache.len()")?
                    .map_or(0, |s| s.len());
                self.scalars[s_base + a as usize] = len as u64;
            }
            CACHE_KEYS => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.keys()")? else {
                    self.objs[o_base + a as usize] = Obj::List(ListStorage::new_obj(0));
                    return Ok(());
                };
                let prefix: Option<String> = match &self.objs[o_base + c as usize] {
                    Obj::Empty => None,
                    Obj::Str(s) => Some(s.as_str().to_string()),
                    _ => return Err(err_msg("Expected String prefix in Cache.keys()")),
                };
                let items: Vec<Obj> = store
                    .keys(prefix.as_deref())
                    .into_iter()
                    .map(Obj::str)
                    .collect();
                self.objs[o_base + a as usize] = Obj::List(ListStorage::Obj(Arc::new(items)));
            }
            CACHE_CLEAR => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.clear()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                self.objs[o_base + a as usize] = match store.clear() {
                    Ok(()) => Obj::Empty,
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            CACHE_PRUNE => {
                let pruned = self
                    .extract_cache_store(o_base, b, "Cache.prune()")?
                    .map_or(0, |s| s.prune());
                self.scalars[s_base + a as usize] = pruned as u64;
            }
            CACHE_SAVE => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.save()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let path = self.sys_str(o_base, c, "Cache.save()")?;
                self.objs[o_base + a as usize] = match store.save_snapshot(Path::new(path.as_str()))
                {
                    Ok(n) => Obj::boxed_int(n as i64),
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            CACHE_LOAD => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.load()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                let path = self.sys_str(o_base, c, "Cache.load()")?;
                self.objs[o_base + a as usize] = match store.load_snapshot(Path::new(path.as_str()))
                {
                    Ok(n) => Obj::boxed_int(n as i64),
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            CACHE_COMPACT => {
                let Some(store) = self.extract_cache_store(o_base, b, "Cache.compact()")? else {
                    self.objs[o_base + a as usize] =
                        make_cache_error("dropped", "Cache handle has been dropped");
                    return Ok(());
                };
                self.objs[o_base + a as usize] = match store.compact() {
                    Ok(n) => Obj::boxed_int(n as i64),
                    Err((k, msg)) => make_cache_error(k, msg),
                };
            }
            _ => return Err(err_msg("Unknown EXT_CACHE_OP sub-op")),
        }
        Ok(())
    }
}
