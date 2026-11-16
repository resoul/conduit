use std::sync::Arc;

use crate::native::crypto::{
    CRYPTO_B64_DEC, CRYPTO_B64_ENC, CRYPTO_HEX_DEC, CRYPTO_HEX_ENC, CRYPTO_HMAC_SHA256,
    CRYPTO_RAND_HEX, CRYPTO_RAND_INT, CRYPTO_SHA256, CRYPTO_UUID_V4, decode_base64, decode_hex,
    encode_base64, encode_hex, hmac_sha256_bytes, random_bytes_hex, random_int, sha256_bytes,
    uuid_v4,
};
use crate::native::path::{
    PATH_BASENAME, PATH_DIRNAME, PATH_EXT, PATH_IS_ABS, PATH_JOIN, PATH_JOIN_ALL, PATH_NORMALIZE,
    PATH_STEM, PATH_WITH_EXT, path_basename, path_dirname, path_ext, path_is_abs, path_join,
    path_join_all, path_normalize, path_stem, path_with_ext,
};
use crate::native::process::{
    PROC_CWD, PROC_EXIT, PROC_PID, PROC_RUN, PROC_SET_CWD, PROC_STATUS, make_process_error,
    proc_cwd, proc_run, proc_set_cwd, proc_status,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{ListStorage, Obj};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    fn sys_str(
        &self,
        o_base: usize,
        reg: u16,
        ctx: &str,
    ) -> Result<Arc<crate::vm::string::VmString>, RuntimeError> {
        let Obj::Str(s) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected String in {}", ctx)));
        };
        Ok(Arc::clone(s))
    }

    pub(crate) fn exec_path_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            PATH_JOIN => {
                let p1 = self.sys_str(o_base, b, "path.join()")?;
                let p2 = self.sys_str(o_base, c, "path.join()")?;
                self.objs[o_base + a as usize] = path_join(&p1, &p2);
            }
            PATH_JOIN_ALL => {
                let Obj::List(ListStorage::Obj(items)) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected [String] in path.join_all()"));
                };
                self.objs[o_base + a as usize] = path_join_all(items);
            }
            PATH_DIRNAME => {
                let p = self.sys_str(o_base, b, "path.dirname()")?;
                self.objs[o_base + a as usize] = path_dirname(&p);
            }
            PATH_BASENAME => {
                let p = self.sys_str(o_base, b, "path.basename()")?;
                self.objs[o_base + a as usize] = path_basename(&p);
            }
            PATH_EXT => {
                let p = self.sys_str(o_base, b, "path.ext()")?;
                self.objs[o_base + a as usize] = path_ext(&p);
            }
            PATH_STEM => {
                let p = self.sys_str(o_base, b, "path.stem()")?;
                self.objs[o_base + a as usize] = path_stem(&p);
            }
            PATH_NORMALIZE => {
                let p = self.sys_str(o_base, b, "path.normalize()")?;
                self.objs[o_base + a as usize] = path_normalize(&p);
            }
            PATH_IS_ABS => {
                let p = self.sys_str(o_base, b, "path.is_abs()")?;
                self.scalars[s_base + a as usize] = u64::from(path_is_abs(&p));
            }
            PATH_WITH_EXT => {
                let p = self.sys_str(o_base, b, "path.with_ext()")?;
                let ext = self.sys_str(o_base, c, "path.with_ext()")?;
                self.objs[o_base + a as usize] = path_with_ext(&p, &ext);
            }
            _ => return Err(err_msg("Unknown path sub-op")),
        }
        Ok(())
    }

    pub(crate) fn exec_process_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            PROC_RUN | PROC_STATUS => {
                let cmd = self.sys_str(o_base, b, "process")?;
                let res = match &self.objs[o_base + c as usize] {
                    Obj::List(ListStorage::Obj(items)) => {
                        if aux == PROC_RUN {
                            proc_run(&cmd, items)
                        } else {
                            proc_status(&cmd, items)
                        }
                    }
                    _ => make_process_error("invalid_input", "Expected [String] arguments"),
                };
                self.objs[o_base + a as usize] = res;
            }
            PROC_CWD => {
                self.objs[o_base + a as usize] = proc_cwd();
            }
            PROC_SET_CWD => {
                let dir = self.sys_str(o_base, b, "process.set_cwd()")?;
                self.objs[o_base + a as usize] = proc_set_cwd(&dir);
            }
            PROC_PID => {
                self.scalars[s_base + a as usize] = i64::from(std::process::id()) as u64;
            }
            PROC_EXIT => {
                let code = self.scalars[s_base + a as usize] as i64 as i32;
                std::process::exit(code);
            }
            _ => return Err(err_msg("Unknown process sub-op")),
        }
        Ok(())
    }

    pub(crate) fn exec_crypto_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            CRYPTO_SHA256 => {
                let s = self.sys_str(o_base, b, "crypto.sha256()")?;
                self.objs[o_base + a as usize] = Obj::str(encode_hex(&sha256_bytes(s.as_bytes())));
            }
            CRYPTO_HMAC_SHA256 => {
                let key = self.sys_str(o_base, b, "crypto.hmac_sha256()")?;
                let data = self.sys_str(o_base, c, "crypto.hmac_sha256()")?;
                self.objs[o_base + a as usize] =
                    Obj::str(encode_hex(&hmac_sha256_bytes(key.as_bytes(), data.as_bytes())));
            }
            CRYPTO_B64_ENC => {
                let s = self.sys_str(o_base, b, "crypto.base64_encode()")?;
                self.objs[o_base + a as usize] = Obj::str(encode_base64(s.as_bytes()));
            }
            CRYPTO_B64_DEC => {
                let s = self.sys_str(o_base, b, "crypto.base64_decode()")?;
                self.objs[o_base + a as usize] = decode_base64(&s);
            }
            CRYPTO_HEX_ENC => {
                let s = self.sys_str(o_base, b, "crypto.hex_encode()")?;
                self.objs[o_base + a as usize] = Obj::str(encode_hex(s.as_bytes()));
            }
            CRYPTO_HEX_DEC => {
                let s = self.sys_str(o_base, b, "crypto.hex_decode()")?;
                self.objs[o_base + a as usize] = decode_hex(&s);
            }
            CRYPTO_UUID_V4 => {
                self.objs[o_base + a as usize] = uuid_v4();
            }
            CRYPTO_RAND_HEX => {
                let len = self.scalars[s_base + b as usize] as i64;
                self.objs[o_base + a as usize] = random_bytes_hex(len);
            }
            CRYPTO_RAND_INT => {
                let min = self.scalars[s_base + b as usize] as i64;
                let max = self.scalars[s_base + c as usize] as i64;
                self.objs[o_base + a as usize] = random_int(min, max);
            }
            _ => return Err(err_msg("Unknown crypto sub-op")),
        }
        Ok(())
    }
}
