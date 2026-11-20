pub mod exec;
pub(crate) mod exec_cache;
pub(crate) mod exec_cold;
pub mod exec_cron;
pub mod exec_db;
pub(crate) mod exec_fs;
pub(crate) mod exec_http;
pub(crate) mod exec_jwt;
pub(crate) mod exec_lock;
pub(crate) mod exec_map;
pub(crate) mod exec_redis;
pub(crate) mod exec_sys;
pub(crate) mod exec_union;
pub mod opcode;
pub mod pipeline;
pub(crate) mod pipeline_leaf;
pub mod string;
pub mod value;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use value::{Obj, StructData};

use crate::ir::Module;

pub const MAX_CALL_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub message: String,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Runtime error: {}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CallFrame {
    pub func_id: u32,
    pub return_ip: u32,
    pub s_base: u32,
    pub o_base: u32,
    pub prev_closure_slot: u32,
    pub ret_reg: u16,
}

impl CallFrame {
    #[inline(always)]
    pub(crate) fn pack_closure_slot(slot: usize) -> u32 {
        if slot == usize::MAX {
            u32::MAX
        } else {
            slot as u32
        }
    }

    #[inline(always)]
    pub(crate) fn unpack_closure_slot(self) -> usize {
        if self.prev_closure_slot == u32::MAX {
            usize::MAX
        } else {
            self.prev_closure_slot as usize
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecResult {
    Scalar(u64),
    Obj(Obj),
    Void,
}

pub(crate) struct DbConnSlot {
    pub(crate) generation: u32,
    pub(crate) conn: Option<crate::native::db::SqlConn>,
    pub(crate) owner: Option<Arc<crate::native::db_pool::DbPoolState>>,
}

pub(crate) struct DbPoolSlot {
    pub(crate) generation: u32,
    pub(crate) pool: Option<Arc<crate::native::db_pool::DbPoolState>>,
}

#[derive(Default)]
pub(crate) struct VmHostState {
    pub(crate) fs_readers: Vec<Option<std::io::BufReader<std::fs::File>>>,
    pub(crate) fs_writers: Vec<Option<std::io::BufWriter<std::fs::File>>>,
    pub(crate) routers: Vec<crate::native::http::RouterState>,
    pub(crate) http_clients: Vec<crate::native::http::HttpClientState>,
    pub(crate) db_conns: Vec<DbConnSlot>,
    pub(crate) db_conn_free: Vec<usize>,
    pub(crate) db_pools: Vec<DbPoolSlot>,
    pub(crate) db_pool_free: Vec<usize>,
    pub(crate) cron_schedulers: Vec<Option<crate::native::cron::CronSchedulerState>>,
    pub(crate) cached_http_req: Option<(Arc<StructData>, Arc<StructData>)>,
}

impl VmHostState {
    #[inline]
    pub(crate) fn insert_db_conn(
        &mut self,
        conn: crate::native::db::SqlConn,
        owner: Option<Arc<crate::native::db_pool::DbPoolState>>,
    ) -> u64 {
        if let Some(idx) = self.db_conn_free.pop() {
            let slot = &mut self.db_conns[idx];
            slot.conn = Some(conn);
            slot.owner = owner;
            ((slot.generation as u64) << 32) | (idx as u64)
        } else {
            let idx = self.db_conns.len();
            self.db_conns.push(DbConnSlot {
                generation: 0,
                conn: Some(conn),
                owner,
            });
            idx as u64
        }
    }

    #[inline]
    pub(crate) fn get_db_conn(&self, handle: u64) -> Option<&crate::native::db::SqlConn> {
        let idx = (handle & 0xFFFF_FFFF) as usize;
        let generation = (handle >> 32) as u32;
        let slot = self.db_conns.get(idx)?;
        if slot.generation != generation {
            return None;
        }
        slot.conn.as_ref()
    }

    #[inline]
    pub(crate) fn take_db_conn(
        &mut self,
        handle: u64,
    ) -> Option<(
        crate::native::db::SqlConn,
        Option<Arc<crate::native::db_pool::DbPoolState>>,
    )> {
        let idx = (handle & 0xFFFF_FFFF) as usize;
        let generation = (handle >> 32) as u32;
        let slot = self.db_conns.get_mut(idx)?;
        if slot.generation != generation {
            return None;
        }
        let conn = slot.conn.take()?;
        let owner = slot.owner.take();
        slot.generation = slot.generation.wrapping_add(1);
        self.db_conn_free.push(idx);
        Some((conn, owner))
    }

    #[inline]
    pub(crate) fn insert_db_pool(
        &mut self,
        pool: Arc<crate::native::db_pool::DbPoolState>,
    ) -> u64 {
        if let Some(idx) = self.db_pool_free.pop() {
            let slot = &mut self.db_pools[idx];
            slot.pool = Some(pool);
            ((slot.generation as u64) << 32) | (idx as u64)
        } else {
            let idx = self.db_pools.len();
            self.db_pools.push(DbPoolSlot {
                generation: 0,
                pool: Some(pool),
            });
            idx as u64
        }
    }

    #[inline]
    pub(crate) fn get_db_pool(
        &self,
        handle: u64,
    ) -> Option<Arc<crate::native::db_pool::DbPoolState>> {
        let idx = (handle & 0xFFFF_FFFF) as usize;
        let generation = (handle >> 32) as u32;
        let slot = self.db_pools.get(idx)?;
        if slot.generation != generation {
            return None;
        }
        slot.pool.clone()
    }

    #[inline]
    pub(crate) fn take_db_pool(
        &mut self,
        handle: u64,
    ) -> Option<Arc<crate::native::db_pool::DbPoolState>> {
        let idx = (handle & 0xFFFF_FFFF) as usize;
        let generation = (handle >> 32) as u32;
        let slot = self.db_pools.get_mut(idx)?;
        if slot.generation != generation {
            return None;
        }
        let pool = slot.pool.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.db_pool_free.push(idx);
        Some(pool)
    }
}

pub struct Vm {
    pub scalars: Vec<u64>,
    pub objs: Vec<Obj>,
    pub(crate) frames: Vec<CallFrame>,
    pub script_args: Option<Vec<String>>,
    pub(crate) host: Box<VmHostState>,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            scalars: vec![0u64; 4096],
            objs: vec![Obj::Empty; 4096],
            frames: Vec::with_capacity(MAX_CALL_DEPTH),
            script_args: None,
            host: Box::default(),
        }
    }

    #[inline(always)]
    pub(crate) fn ensure_capacity(&mut self, s_needed: usize, o_needed: usize) {
        if s_needed > self.scalars.len() {
            self.grow_scalars(s_needed);
        }
        if o_needed > self.objs.len() {
            self.grow_objs(o_needed);
        }
    }

    #[cold]
    #[inline(never)]
    fn grow_scalars(&mut self, needed: usize) {
        let new_len = needed.next_power_of_two().max(4096);
        self.scalars.resize(new_len, 0);
    }

    #[cold]
    #[inline(never)]
    fn grow_objs(&mut self, needed: usize) {
        let new_len = needed.next_power_of_two().max(4096);
        self.objs.resize(new_len, Obj::Empty);
    }

    #[inline]
    pub fn clear_obj_range(&mut self, start: usize, count: usize) {
        if count > 0 {
            for slot in &mut self.objs[start..start + count] {
                if !matches!(slot, Obj::Empty) {
                    *slot = Obj::Empty;
                }
            }
        }
    }

    pub fn open_db(&mut self, url: &str) -> Result<usize, crate::native::db::DbErr> {
        let conn = crate::native::db::SqlConn::open(url)?;
        let handle = self.host.insert_db_conn(conn, None);
        Ok(handle as usize)
    }

    pub(crate) fn invoke_one_obj_arg_cb(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        cb: &Obj,
        arg0: Obj,
        ret_tag: u8,
    ) -> Result<Obj, RuntimeError> {
        let (func_id, has_closure) = match cb {
            Obj::Func(fid) => (*fid, false),
            Obj::Closure(cd) => (cd.func_id, true),
            _ => return Err(exec_cold::err_msg("Expected callback function")),
        };
        let closure_slot = if has_closure {
            self.ensure_capacity(s_top, o_top + 1);
            self.objs[o_top] = cb.clone();
            o_top
        } else {
            usize::MAX
        };
        let child_s = s_top;
        let child_o = if has_closure { o_top + 1 } else { o_top };
        let func = &module.functions[func_id as usize];
        let used_objs = (func.num_objs as usize).max(1);
        self.ensure_capacity(
            child_s + func.num_scalars.max(1) as usize,
            child_o + used_objs,
        );
        self.objs[child_o] = arg0;
        let res = self.exec_from(module, func_id, child_s, child_o, closure_slot);
        self.clear_obj_range(child_o, used_objs);
        if has_closure {
            self.objs[o_top] = Obj::Empty;
        }
        match res? {
            ExecResult::Void => Ok(Obj::Empty),
            ExecResult::Obj(o) => Ok(o),
            ExecResult::Scalar(bits) => Ok(match ret_tag {
                2 => Obj::boxed_float(bits),
                3 => Obj::boxed_bool(bits != 0),
                _ => Obj::boxed_int(bits as i64),
            }),
        }
    }

    pub(crate) fn invoke_zero_arg_cb(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        cb: &Obj,
        ret_tag: u8,
    ) -> Result<Obj, RuntimeError> {
        let (func_id, has_closure) = match cb {
            Obj::Func(fid) => (*fid, false),
            Obj::Closure(cd) => (cd.func_id, true),
            _ => return Err(exec_cold::err_msg("Expected callback function")),
        };
        let closure_slot = if has_closure {
            self.ensure_capacity(s_top, o_top + 1);
            self.objs[o_top] = cb.clone();
            o_top
        } else {
            usize::MAX
        };
        let child_s = s_top;
        let child_o = if has_closure { o_top + 1 } else { o_top };
        let func = &module.functions[func_id as usize];
        let used_objs = (func.num_objs as usize).max(1);
        self.ensure_capacity(
            child_s + func.num_scalars.max(1) as usize,
            child_o + used_objs,
        );
        let res = self.exec_from(module, func_id, child_s, child_o, closure_slot);
        self.clear_obj_range(child_o, used_objs);
        if has_closure {
            self.objs[o_top] = Obj::Empty;
        }
        match res? {
            ExecResult::Void => Ok(Obj::Empty),
            ExecResult::Obj(o) => Ok(o),
            ExecResult::Scalar(bits) => Ok(match ret_tag {
                2 => Obj::boxed_float(bits),
                3 => Obj::boxed_bool(bits != 0),
                _ => Obj::boxed_int(bits as i64),
            }),
        }
    }

    pub fn call_scalar(
        &mut self,
        module: &Module,
        func_id: u32,
        scalar_args: &[u64],
        obj_args: &[Obj],
    ) -> Result<u64, RuntimeError> {
        match self.call(module, func_id, scalar_args, obj_args)? {
            ExecResult::Scalar(v) => Ok(v),
            ExecResult::Void => Ok(0),
            ExecResult::Obj(_) => Err(RuntimeError {
                message: "Expected scalar return value".to_string(),
            }),
        }
    }

    pub fn call_obj(
        &mut self,
        module: &Module,
        func_id: u32,
        scalar_args: &[u64],
        obj_args: &[Obj],
    ) -> Result<Obj, RuntimeError> {
        match self.call(module, func_id, scalar_args, obj_args)? {
            ExecResult::Obj(v) => Ok(v),
            _ => Err(RuntimeError {
                message: "Expected object return value".to_string(),
            }),
        }
    }

    pub fn call(
        &mut self,
        module: &Module,
        func_id: u32,
        scalar_args: &[u64],
        obj_args: &[Obj],
    ) -> Result<ExecResult, RuntimeError> {
        module
            .validate()
            .map_err(|message| RuntimeError { message })?;
        let func = module.functions.get(func_id as usize).ok_or_else(|| RuntimeError {
            message: format!("Function {} not found", func_id),
        })?;
        if scalar_args.len() != func.scalar_params as usize
            || obj_args.len() != func.obj_params as usize
        {
            return Err(RuntimeError {
                message: format!(
                    "Arity mismatch calling '{}': expected ({}, {}), got ({}, {})",
                    func.name,
                    func.scalar_params,
                    func.obj_params,
                    scalar_args.len(),
                    obj_args.len()
                ),
            });
        }
        self.frames.clear();
        self.ensure_capacity(func.num_scalars as usize, func.num_objs as usize);
        if !scalar_args.is_empty() {
            self.scalars[..scalar_args.len()].copy_from_slice(scalar_args);
        }
        for (idx, obj) in obj_args.iter().enumerate() {
            self.objs[idx] = obj.clone();
        }
        let res = self.exec_from(module, func_id, 0, 0, usize::MAX);
        exec_map::flush_stdout();
        res
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}
