use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use crate::ir::Module;
use crate::native::fs::{
    FS_APPEND_TEXT, FS_COPY, FS_EACH_LINE, FS_EXISTS, FS_IS_DIR, FS_IS_FILE, FS_LIST_DIR,
    FS_MKDIR_ALL, FS_OPEN_APPEND, FS_OPEN_READ, FS_OPEN_WRITE, FS_READ_LINES, FS_READ_TEXT,
    FS_READER_CLOSE, FS_READER_READ_CHUNK, FS_READER_READ_LINE, FS_REMOVE, FS_RENAME, FS_SIZE,
    FS_WRITE_ATOMIC, FS_WRITE_TEXT, FS_WRITER_CLOSE, FS_WRITER_FLUSH, FS_WRITER_WRITE,
    FS_WRITER_WRITE_LINE, FS_WRITER_WRITE_LINES, append_text, copy_file, io_err_obj, list_dir, make_fs_error,
    make_reader_handle, make_writer_handle, mkdir_all, open_buf_reader, open_buf_writer, read_lines,
    read_text, reader_read_chunk, reader_read_line, remove_path, rename_path, size, write_atomic,
    write_text,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{ListStorage, Obj, SCHEMA_FS_READER, SCHEMA_FS_WRITER};
use crate::vm::{RuntimeError, Vm};
use crate::vm::string::VmString;

impl Vm {
    fn extract_str(&self, o_base: usize, reg: u16, ctx: &str) -> Result<Arc<VmString>, RuntimeError> {
        let Obj::Str(s) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected String in {}", ctx)));
        };
        Ok(Arc::clone(s))
    }

    fn extract_handle(&self, o_base: usize, reg: u16, expected_schema: u32, ctx: &str) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = &self.objs[o_base + reg as usize] else {
            return Err(err_msg(&format!("Expected handle in {}", ctx)));
        };
        if rec.schema_id != expected_schema {
            return Err(err_msg(&format!("Invalid handle type in {}", ctx)));
        }
        Ok(rec.inline_scalars[0] as usize)
    }

    pub(crate) fn exec_fs_unary(
        &mut self,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            FS_READ_TEXT => {
                let path = self.extract_str(o_base, b, "fs.read_text()")?;
                self.objs[o_base + a as usize] = read_text(&path);
            }
            FS_READ_LINES => {
                let path = self.extract_str(o_base, b, "fs.read_lines()")?;
                self.objs[o_base + a as usize] = read_lines(&path);
            }
            FS_EXISTS => {
                let path = self.extract_str(o_base, b, "fs.exists()")?;
                self.scalars[s_base + a as usize] = u64::from(Path::new(path.as_str()).exists());
            }
            FS_IS_FILE => {
                let path = self.extract_str(o_base, b, "fs.is_file()")?;
                self.scalars[s_base + a as usize] = u64::from(Path::new(path.as_str()).is_file());
            }
            FS_IS_DIR => {
                let path = self.extract_str(o_base, b, "fs.is_dir()")?;
                self.scalars[s_base + a as usize] = u64::from(Path::new(path.as_str()).is_dir());
            }
            FS_SIZE => {
                let path = self.extract_str(o_base, b, "fs.size()")?;
                self.objs[o_base + a as usize] = size(&path);
            }
            FS_LIST_DIR => {
                let path = self.extract_str(o_base, b, "fs.list_dir()")?;
                self.objs[o_base + a as usize] = list_dir(&path);
            }
            FS_MKDIR_ALL => {
                let path = self.extract_str(o_base, b, "fs.mkdir_all()")?;
                self.objs[o_base + a as usize] = mkdir_all(&path);
            }
            FS_REMOVE => {
                let path = self.extract_str(o_base, b, "fs.remove()")?;
                self.objs[o_base + a as usize] = remove_path(&path);
            }
            FS_OPEN_READ => {
                let path = self.extract_str(o_base, b, "fs.open_read()")?;
                let res = match open_buf_reader(&path) {
                    Ok(reader) => {
                        let id = self.fs_readers.len();
                        self.fs_readers.push(Some(reader));
                        make_reader_handle(id)
                    }
                    Err(e) => e,
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_OPEN_WRITE | FS_OPEN_APPEND => {
                let path = self.extract_str(o_base, b, "fs.open_write()")?;
                let append = aux == FS_OPEN_APPEND;
                let res = match open_buf_writer(&path, append) {
                    Ok(writer) => {
                        let id = self.fs_writers.len();
                        self.fs_writers.push(Some(writer));
                        make_writer_handle(id)
                    }
                    Err(e) => e,
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_READER_READ_LINE => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_READER, "Reader.read_line()")?;
                let res = match self.fs_readers.get_mut(id).and_then(|s| s.as_mut()) {
                    Some(reader) => reader_read_line(reader),
                    None => make_fs_error("io", "<reader>", "Reader is already closed"),
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_READER_CLOSE => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_READER, "Reader.close()")?;
                if let Some(slot) = self.fs_readers.get_mut(id) {
                    *slot = None;
                }
            }
            FS_WRITER_FLUSH => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_WRITER, "Writer.flush()")?;
                let res = match self.fs_writers.get_mut(id).and_then(|s| s.as_mut()) {
                    Some(w) => match w.flush() {
                        Ok(()) => Obj::Empty,
                        Err(e) => io_err_obj("<writer>", &e),
                    },
                    None => make_fs_error("io", "<writer>", "Writer is already closed"),
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_WRITER_CLOSE => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_WRITER, "Writer.close()")?;
                let res = match self.fs_writers.get_mut(id).and_then(|s| s.take()) {
                    Some(mut w) => match w.flush() {
                        Ok(()) => Obj::Empty,
                        Err(e) => io_err_obj("<writer>", &e),
                    },
                    None => Obj::Empty,
                };
                self.objs[o_base + a as usize] = res;
            }
            _ => return Err(err_msg("Unknown FS unary sub-op")),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_fs_binary(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            FS_WRITE_TEXT => {
                let path = self.extract_str(o_base, b, "fs.write_text()")?;
                let data = self.extract_str(o_base, c, "fs.write_text()")?;
                self.objs[o_base + a as usize] = write_text(&path, &data);
            }
            FS_APPEND_TEXT => {
                let path = self.extract_str(o_base, b, "fs.append_text()")?;
                let data = self.extract_str(o_base, c, "fs.append_text()")?;
                self.objs[o_base + a as usize] = append_text(&path, &data);
            }
            FS_WRITE_ATOMIC => {
                let path = self.extract_str(o_base, b, "fs.write_atomic()")?;
                let data = self.extract_str(o_base, c, "fs.write_atomic()")?;
                self.objs[o_base + a as usize] = write_atomic(&path, &data);
            }
            FS_COPY => {
                let src = self.extract_str(o_base, b, "fs.copy()")?;
                let dst = self.extract_str(o_base, c, "fs.copy()")?;
                self.objs[o_base + a as usize] = copy_file(&src, &dst);
            }
            FS_RENAME => {
                let src = self.extract_str(o_base, b, "fs.rename()")?;
                let dst = self.extract_str(o_base, c, "fs.rename()")?;
                self.objs[o_base + a as usize] = rename_path(&src, &dst);
            }
            FS_WRITER_WRITE | FS_WRITER_WRITE_LINE => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_WRITER, "Writer.write()")?;
                let Obj::Str(data) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected String in Writer.write()"));
                };
                let res = match self.fs_writers.get_mut(id).and_then(|s| s.as_mut()) {
                    Some(w) => {
                        let bytes = data.as_bytes();
                        let r = if aux == FS_WRITER_WRITE_LINE {
                            if bytes.len() <= 4096 {
                                let mut tmp = [0u8; 4097];
                                tmp[..bytes.len()].copy_from_slice(bytes);
                                tmp[bytes.len()] = b'\n';
                                w.write_all(&tmp[..bytes.len() + 1])
                            } else {
                                w.write_all(bytes).and_then(|()| w.write_all(b"\n"))
                            }
                        } else {
                            w.write_all(bytes)
                        };
                        match r {
                            Ok(()) => Obj::Empty,
                            Err(e) => io_err_obj("<writer>", &e),
                        }
                    }
                    None => make_fs_error("io", "<writer>", "Writer is already closed"),
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_WRITER_WRITE_LINES => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_WRITER, "Writer.write_lines()")?;
                let Obj::List(ListStorage::Obj(lines)) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected [String] in Writer.write_lines()"));
                };
                let res = match self.fs_writers.get_mut(id).and_then(|s| s.as_mut()) {
                    Some(w) => {
                        let mut res = Obj::Empty;
                        for line in lines.iter() {
                            let Obj::Str(data) = line else {
                                return Err(err_msg("Expected String element in Writer.write_lines()"));
                            };
                            if let Err(e) = w.write_all(data.as_bytes()).and_then(|()| w.write_all(b"\n")) {
                                res = io_err_obj("<writer>", &e);
                                break;
                            }
                        }
                        res
                    }
                    None => make_fs_error("io", "<writer>", "Writer is already closed"),
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_READER_READ_CHUNK => {
                let id = self.extract_handle(o_base, b, SCHEMA_FS_READER, "Reader.read_chunk()")?;
                let n = self.scalars[s_base + c as usize] as i64;
                let res = if n < 0 {
                    make_fs_error("invalid_input", "<reader>", "read_chunk() size must be >= 0")
                } else {
                    match self.fs_readers.get_mut(id).and_then(|s| s.as_mut()) {
                        Some(r) => reader_read_chunk(r, n as usize),
                        None => make_fs_error("io", "<reader>", "Reader is already closed"),
                    }
                };
                self.objs[o_base + a as usize] = res;
            }
            FS_EACH_LINE => {
                let path = self.extract_str(o_base, b, "fs.each_line()")?;
                let cb_slot = o_base + c as usize;
                let (func_id, closure_slot) = match &self.objs[cb_slot] {
                    Obj::Func(fid) => (*fid, usize::MAX),
                    Obj::Closure(cd) => (cd.func_id, cb_slot),
                    _ => return Err(err_msg("Expected callback function in fs.each_line()")),
                };
                let mut reader = match open_buf_reader(&path) {
                    Ok(r) => r,
                    Err(e) => {
                        self.objs[o_base + a as usize] = e;
                        return Ok(());
                    }
                };
                let orig_s_len = self.scalars.len();
                let orig_o_len = self.objs.len();
                let child_s = orig_s_len;
                let child_o = orig_o_len;
                let func = &module.functions[func_id as usize];
                self.ensure_capacity(
                    child_s + func.num_scalars.max(1) as usize,
                    child_o + func.num_objs.max(1) as usize,
                );

                let mut line_arc = Arc::new(VmString::from(String::with_capacity(256)));
                let mut count: i64 = 0;
                loop {
                    let line_buf = match Arc::get_mut(&mut line_arc) {
                        Some(s) => {
                            let s = s.make_owned_mut();
                            s.clear();
                            s
                        }
                        None => {
                            line_arc = Arc::new(VmString::from(String::with_capacity(256)));
                            Arc::get_mut(&mut line_arc).unwrap().make_owned_mut()
                        }
                    };
                    match reader.read_line(line_buf) {
                        Ok(0) => break,
                        Ok(_) => {
                            if line_buf.ends_with('\n') {
                                line_buf.pop();
                                if line_buf.ends_with('\r') {
                                    line_buf.pop();
                                }
                            }
                            self.objs[child_o] = Obj::Str(Arc::clone(&line_arc));
                            self.exec_from(module, func_id, child_s, child_o, closure_slot)?;
                            self.objs[child_o] = Obj::Empty;
                            count += 1;
                        }
                        Err(e) => {
                            self.scalars.truncate(orig_s_len);
                            self.objs.truncate(orig_o_len);
                            self.objs[o_base + a as usize] = io_err_obj(&path, &e);
                            return Ok(());
                        }
                    }
                }
                self.scalars.truncate(orig_s_len);
                self.objs.truncate(orig_o_len);
                self.objs[o_base + a as usize] = Obj::boxed_int(count);
            }
            _ => return Err(err_msg("Unknown FS binary sub-op")),
        }
        Ok(())
    }
}
