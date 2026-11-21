use std::sync::Arc;

use crate::native::crypto::{
    CRYPTO_B64_DEC, CRYPTO_B64_ENC, CRYPTO_CONST_TIME_EQ, CRYPTO_HASH_PASSWORD, CRYPTO_HEX_DEC,
    CRYPTO_HEX_ENC, CRYPTO_HMAC_SHA256, CRYPTO_PASSWORD_NEEDS_REHASH, CRYPTO_RAND_HEX,
    CRYPTO_RAND_INT, CRYPTO_SHA256, CRYPTO_UUID_V4, CRYPTO_VERIFY_PASSWORD, constant_time_eq,
    decode_base64, decode_hex, encode_base64, encode_hex, hash_password, hmac_sha256_bytes,
    password_needs_rehash, random_bytes_hex, random_int, sha256_bytes, uuid_v4, verify_password,
};
use crate::native::path::{
    PATH_BASENAME, PATH_DIRNAME, PATH_EXT, PATH_IS_ABS, PATH_JOIN, PATH_JOIN_ALL, PATH_NORMALIZE,
    PATH_SAFE_JOIN, PATH_STEM, PATH_WITH_EXT, path_basename, path_dirname, path_ext, path_is_abs,
    path_join, path_join_all, path_normalize, path_safe_join, path_stem, path_with_ext,
};
use crate::native::process::{
    PROC_CWD, PROC_EXIT, PROC_PID, PROC_PIPE, PROC_RUN, PROC_SET_CWD, PROC_STATUS,
    make_process_error, proc_cwd, proc_pipe, proc_run, proc_set_cwd, proc_status,
};
use crate::native::csv::{CsvError, parse_csv_objs, parse_csv_records_objs, stringify_csv};
use crate::native::log::{
    LOG_GET_LEVEL, LOG_SET_JSON, LOG_SET_LEVEL, emit_log, get_log_level, set_log_json,
    set_log_level,
};
use crate::native::math::eval_math_op;
use crate::native::url::{
    URL_DECODE, URL_DECODE_QUERY, URL_ENCODE, URL_ENCODE_QUERY, URL_PARSE, decode_query_str,
    encode_query_map, percent_decode, percent_encode, url_parse,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{ListStorage, Obj, SCHEMA_CSV_ERROR, StructData};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    pub(crate) fn sys_str(
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
            PATH_SAFE_JOIN => {
                let root = self.sys_str(o_base, b, "path.safe_join()")?;
                let rel = self.sys_str(o_base, c, "path.safe_join()")?;
                self.objs[o_base + a as usize] = path_safe_join(&root, &rel);
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
                crate::vm::exec_map::flush_stdout();
                let cmd = self.sys_str(o_base, b, "process")?;
                self.host
                    .permissions
                    .check_run(cmd.as_str())
                    .map_err(|e| err_msg(&e))?;
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
            PROC_PIPE => {
                crate::vm::exec_map::flush_stdout();
                if let Obj::List(ListStorage::Obj(stages)) = &self.objs[o_base + b as usize] {
                    for stage in stages.iter() {
                        if let Obj::List(ListStorage::Obj(argv)) = stage
                            && let Some(Obj::Str(cmd0)) = argv.first()
                        {
                            self.host
                                .permissions
                                .check_run(cmd0.as_str())
                                .map_err(|e| err_msg(&e))?;
                        }
                    }
                }
                let res = match &self.objs[o_base + b as usize] {
                    Obj::List(ListStorage::Obj(items)) => proc_pipe(items),
                    _ => make_process_error("invalid_input", "Expected [[String]] commands"),
                };
                self.objs[o_base + a as usize] = res;
            }
            PROC_CWD => {
                self.host
                    .permissions
                    .check_read_cwd()
                    .map_err(|e| err_msg(&e))?;
                self.objs[o_base + a as usize] = proc_cwd();
            }
            PROC_SET_CWD => {
                let dir = self.sys_str(o_base, b, "process.set_cwd()")?;
                self.host
                    .permissions
                    .check_read(dir.as_str())
                    .map_err(|e| err_msg(&e))?;
                self.objs[o_base + a as usize] = proc_set_cwd(&dir);
            }
            PROC_PID => {
                self.scalars[s_base + a as usize] = i64::from(std::process::id()) as u64;
            }
            PROC_EXIT => {
                crate::vm::exec_map::flush_stdout();
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
            CRYPTO_HASH_PASSWORD => {
                let pw = self.sys_str(o_base, b, "crypto.hash_password()")?;
                self.objs[o_base + a as usize] = hash_password(&pw);
            }
            CRYPTO_VERIFY_PASSWORD => {
                let pw = self.sys_str(o_base, b, "crypto.verify_password()")?;
                let stored = self.sys_str(o_base, c, "crypto.verify_password()")?;
                self.scalars[s_base + a as usize] = u64::from(verify_password(&pw, &stored));
            }
            CRYPTO_CONST_TIME_EQ => {
                let s1 = self.sys_str(o_base, b, "crypto.constant_time_eq()")?;
                let s2 = self.sys_str(o_base, c, "crypto.constant_time_eq()")?;
                self.scalars[s_base + a as usize] =
                    u64::from(constant_time_eq(s1.as_bytes(), s2.as_bytes()));
            }
            CRYPTO_PASSWORD_NEEDS_REHASH => {
                let stored = self.sys_str(o_base, b, "crypto.password_needs_rehash()")?;
                self.scalars[s_base + a as usize] = u64::from(password_needs_rehash(&stored));
            }
            _ => return Err(err_msg("Unknown crypto sub-op")),
        }
        Ok(())
    }

    pub(crate) fn exec_url_op(
        &mut self,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            URL_PARSE => {
                let s = self.sys_str(o_base, b, "url.parse()")?;
                self.objs[o_base + a as usize] = url_parse(&s);
            }
            URL_ENCODE => {
                let s = self.sys_str(o_base, b, "url.encode()")?;
                self.objs[o_base + a as usize] = Obj::str(percent_encode(&s));
            }
            URL_DECODE => {
                let s = self.sys_str(o_base, b, "url.decode()")?;
                self.objs[o_base + a as usize] = match percent_decode(&s, false) {
                    Ok(decoded) => Obj::str(decoded),
                    Err(err_obj) => err_obj,
                };
            }
            URL_ENCODE_QUERY => {
                let Obj::Map(map) = self.objs[o_base + b as usize].resolved() else {
                    return Err(err_msg("Expected Map<String, String> in url.encode_query()"));
                };
                let map_clone = Arc::clone(map);
                self.objs[o_base + a as usize] = encode_query_map(&map_clone);
            }
            URL_DECODE_QUERY => {
                let s = self.sys_str(o_base, b, "url.decode_query()")?;
                self.objs[o_base + a as usize] = decode_query_str(&s);
            }
            _ => return Err(err_msg("Unknown url sub-op")),
        }
        Ok(())
    }

    pub(crate) fn exec_csv_op(
        &mut self,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        let make_csv_err = |e: CsvError| {
            let mut s = StructData::new(SCHEMA_CSV_ERROR, 1, 2);
            s.set_scalar(0, e.line as u64);
            s.set_obj(0, Obj::str(e.kind));
            s.set_obj(1, Obj::str(e.message));
            Obj::Record(Arc::new(s))
        };
        let extract_rows = |obj: &Obj| -> Result<Vec<Vec<String>>, RuntimeError> {
            let Obj::List(ListStorage::Obj(outer)) = obj else {
                return Err(err_msg("Expected [[String]] in csv.stringify()"));
            };
            let mut out = Vec::with_capacity(outer.len());
            for row_obj in outer.iter() {
                let Obj::List(ListStorage::Obj(inner)) = row_obj else {
                    return Err(err_msg("Expected [[String]] in csv.stringify()"));
                };
                let mut row = Vec::with_capacity(inner.len());
                for cell in inner.iter() {
                    let Obj::Str(s) = cell else {
                        return Err(err_msg("Expected String cell in csv.stringify()"));
                    };
                    row.push(s.as_ref().to_string());
                }
                out.push(row);
            }
            Ok(out)
        };

        match aux {
            0 => {
                // csv.parse(text)
                let s = self.sys_str(o_base, b, "csv.parse()")?;
                self.objs[o_base + a as usize] = match parse_csv_objs(&s, ",") {
                    Ok(obj) => obj,
                    Err(e) => make_csv_err(e),
                };
            }
            1 => {
                // csv.parse_with(text, delimiter)
                let s = self.sys_str(o_base, b, "csv.parse_with()")?;
                let delim = self.sys_str(o_base, c, "csv.parse_with()")?;
                self.objs[o_base + a as usize] = match parse_csv_objs(&s, &delim) {
                    Ok(obj) => obj,
                    Err(e) => make_csv_err(e),
                };
            }
            2 => {
                // csv.parse_records(text)
                let s = self.sys_str(o_base, b, "csv.parse_records()")?;
                self.objs[o_base + a as usize] = match parse_csv_records_objs(&s) {
                    Ok(obj) => obj,
                    Err(e) => make_csv_err(e),
                };
            }
            3 => {
                // csv.stringify(rows)
                let rows = extract_rows(&self.objs[o_base + b as usize])?;
                let out = stringify_csv(&rows, ",").map_err(|e| err_msg(&e.message))?;
                self.objs[o_base + a as usize] = Obj::str(out);
            }
            4 => {
                // csv.stringify_with(rows, delimiter)
                let rows = extract_rows(&self.objs[o_base + b as usize])?;
                let delim = self.sys_str(o_base, c, "csv.stringify_with()")?;
                self.objs[o_base + a as usize] = match stringify_csv(&rows, &delim) {
                    Ok(out) => Obj::str(out),
                    Err(e) => make_csv_err(e),
                };
            }
            _ => return Err(err_msg("Unknown csv sub-op")),
        }
        Ok(())
    }

    pub(crate) fn exec_math_op(
        &mut self,
        s_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        let x = if aux >= 3 {
            f64::from_bits(self.scalars[s_base + b as usize])
        } else {
            0.0
        };
        let y = if aux >= 17 {
            f64::from_bits(self.scalars[s_base + c as usize])
        } else {
            0.0
        };
        let res = eval_math_op(aux, x, y).ok_or_else(|| err_msg("Unknown math sub-op"))?;
        self.scalars[s_base + a as usize] = res.to_bits();
        Ok(())
    }

    pub(crate) fn exec_log_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            0..=3 => {
                crate::vm::exec_map::flush_stdout();
                let msg = self.sys_str(o_base, b, "log()")?;
                emit_log(aux, &msg);
            }
            LOG_SET_LEVEL => {
                let level = self.sys_str(o_base, b, "log.set_level()")?;
                set_log_level(&level);
            }
            LOG_SET_JSON => {
                let enabled = self.scalars[s_base + b as usize] != 0;
                set_log_json(enabled);
            }
            LOG_GET_LEVEL => {
                self.objs[o_base + a as usize] = Obj::str(get_log_level());
            }
            _ => return Err(err_msg("Unknown log sub-op")),
        }
        Ok(())
    }
}
