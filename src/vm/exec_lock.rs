use std::path::Path;

use crate::ir::Module;
use crate::native::lock::{
    HeldGuard, LOCK_ACQUIRE, LOCK_FILE, LOCK_FILE_ACQUIRE, LOCK_FILE_SHARED, LOCK_FILE_TRY,
    LOCK_FILE_TRY_ACQUIRE, LOCK_GUARD_IS_HELD, LOCK_GUARD_RELEASE, LOCK_READ, LOCK_TRY_ACQUIRE,
    LOCK_TRY_WITH, LOCK_WITH, LOCK_WRITE, file_lock_exclusive, file_lock_shared,
    file_try_lock_exclusive, is_guard_held, lock_blocking, lock_timeout, make_lock_error,
    make_lock_guard, read_blocking, register_guard, release_guard, write_blocking,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{Obj, SCHEMA_LOCK_GUARD};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    fn extract_guard_id(&self, o_base: usize, reg: u16, ctx: &str) -> Result<u64, RuntimeError> {
        let Obj::Record(rec) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected lock.Guard in {}", ctx)));
        };
        if rec.schema_id != SCHEMA_LOCK_GUARD {
            return Err(err_msg(&format!("Expected lock.Guard in {}", ctx)));
        }
        Ok(rec.get_scalar(0))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_lock_op(
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
            LOCK_WITH | LOCK_READ | LOCK_WRITE | LOCK_FILE | LOCK_FILE_TRY | LOCK_FILE_SHARED => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed lock arguments"));
                };
                let ret_tag = packed.get_scalar(0) as u8;
                let Obj::Str(key_s) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key/path in lock operation"));
                };
                let key = key_s.as_str().to_string();
                let cb = packed.get_obj(1).clone();

                let res = match aux {
                    LOCK_WITH => {
                        let _g = lock_blocking(&key);
                        self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?
                    }
                    LOCK_READ => {
                        let _g = read_blocking(&key);
                        self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?
                    }
                    LOCK_WRITE => {
                        let _g = write_blocking(&key);
                        self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?
                    }
                    LOCK_FILE => match file_lock_exclusive(Path::new(&key)) {
                        Ok(_g) => self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?,
                        Err((k, msg)) => make_lock_error(k, msg),
                    },
                    LOCK_FILE_TRY => match file_try_lock_exclusive(Path::new(&key)) {
                        Ok(_g) => self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?,
                        Err((k, msg)) => make_lock_error(k, msg),
                    },
                    _ => match file_lock_shared(Path::new(&key)) {
                        Ok(_g) => self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?,
                        Err((k, msg)) => make_lock_error(k, msg),
                    },
                };
                self.objs[o_base + a as usize] = res;
            }
            LOCK_TRY_WITH => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed arguments in lock.try_with()"));
                };
                let timeout_ms = packed.get_scalar(0) as i64;
                let ret_tag = packed.get_scalar(1) as u8;
                let Obj::Str(key_s) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String key in lock.try_with()"));
                };
                let key = key_s.as_str().to_string();
                let cb = packed.get_obj(1).clone();

                let res = match lock_timeout(&key, timeout_ms) {
                    Ok(_g) => self.invoke_zero_arg_cb(module, s_top, o_top, &cb, ret_tag)?,
                    Err((k, msg)) => make_lock_error(k, msg),
                };
                self.objs[o_base + a as usize] = res;
            }
            LOCK_ACQUIRE => {
                let name = self.sys_str(o_base, b, "lock.acquire()")?;
                let g = lock_blocking(name.as_str());
                let id = register_guard(HeldGuard::Mutex(g));
                self.objs[o_base + a as usize] = make_lock_guard(id);
            }
            LOCK_TRY_ACQUIRE => {
                let name = self.sys_str(o_base, b, "lock.try_acquire()")?;
                let timeout_ms = self.scalars[s_base + c as usize] as i64;
                self.objs[o_base + a as usize] = match lock_timeout(name.as_str(), timeout_ms) {
                    Ok(g) => make_lock_guard(register_guard(HeldGuard::Mutex(g))),
                    Err((k, msg)) => make_lock_error(k, msg),
                };
            }
            LOCK_FILE_ACQUIRE => {
                let path = self.sys_str(o_base, b, "lock.file_acquire()")?;
                self.objs[o_base + a as usize] = match file_lock_exclusive(Path::new(path.as_str()))
                {
                    Ok(g) => make_lock_guard(register_guard(HeldGuard::File(g))),
                    Err((k, msg)) => make_lock_error(k, msg),
                };
            }
            LOCK_FILE_TRY_ACQUIRE => {
                let path = self.sys_str(o_base, b, "lock.file_try_acquire()")?;
                self.objs[o_base + a as usize] =
                    match file_try_lock_exclusive(Path::new(path.as_str())) {
                        Ok(g) => make_lock_guard(register_guard(HeldGuard::File(g))),
                        Err((k, msg)) => make_lock_error(k, msg),
                    };
            }
            LOCK_GUARD_RELEASE => {
                let id = self.extract_guard_id(o_base, b, "Guard.release()")?;
                release_guard(id);
            }
            LOCK_GUARD_IS_HELD => {
                let id = self.extract_guard_id(o_base, b, "Guard.is_held()")?;
                self.scalars[s_base + a as usize] = u64::from(is_guard_held(id));
            }
            _ => return Err(err_msg("Unknown EXT_LOCK_OP sub-op")),
        }
        Ok(())
    }
}
