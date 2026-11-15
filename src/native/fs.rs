use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::vm::value::{
    ListStorage, Obj, SCHEMA_FS_ERROR, SCHEMA_FS_READER, SCHEMA_FS_WRITER, StructData,
};
use crate::vm::string::VmString;

pub const FS_BUF_CAP: usize = 64 * 1024;

// EXT_FS_UNARY aux codes
pub const FS_READ_TEXT: u8 = 0;
pub const FS_READ_LINES: u8 = 1;
pub const FS_EXISTS: u8 = 2;
pub const FS_IS_FILE: u8 = 3;
pub const FS_IS_DIR: u8 = 4;
pub const FS_SIZE: u8 = 5;
pub const FS_LIST_DIR: u8 = 6;
pub const FS_MKDIR_ALL: u8 = 7;
pub const FS_REMOVE: u8 = 8;
pub const FS_OPEN_READ: u8 = 9;
pub const FS_OPEN_WRITE: u8 = 10;
pub const FS_OPEN_APPEND: u8 = 11;
pub const FS_READER_READ_LINE: u8 = 12;
pub const FS_READER_CLOSE: u8 = 13;
pub const FS_WRITER_FLUSH: u8 = 14;
pub const FS_WRITER_CLOSE: u8 = 15;

// EXT_FS_BINARY aux codes
pub const FS_WRITE_TEXT: u8 = 0;
pub const FS_APPEND_TEXT: u8 = 1;
pub const FS_WRITE_ATOMIC: u8 = 2;
pub const FS_COPY: u8 = 3;
pub const FS_RENAME: u8 = 4;
pub const FS_WRITER_WRITE: u8 = 5;
pub const FS_WRITER_WRITE_LINE: u8 = 6;
pub const FS_READER_READ_CHUNK: u8 = 7;
pub const FS_EACH_LINE: u8 = 8;
pub const FS_WRITER_WRITE_LINES: u8 = 9;

static ATOMIC_SEQ: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn make_fs_error(kind: &str, path: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_FS_ERROR, 0, 3);
    s.set_obj(0, Obj::str(kind));
    s.set_obj(1, Obj::str(path));
    s.set_obj(2, Obj::str(message));
    Obj::Record(Arc::new(s))
}

pub fn io_err_obj(path: &str, err: &io::Error) -> Obj {
    let kind = match err.kind() {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::AlreadyExists => "already_exists",
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => "invalid_input",
        _ => "io",
    };
    make_fs_error(kind, path, err.to_string())
}

#[inline]
pub fn make_reader_handle(id: usize) -> Obj {
    let mut s = StructData::new(SCHEMA_FS_READER, 1, 0);
    s.inline_scalars[0] = id as u64;
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_writer_handle(id: usize) -> Obj {
    let mut s = StructData::new(SCHEMA_FS_WRITER, 1, 0);
    s.inline_scalars[0] = id as u64;
    Obj::Record(Arc::new(s))
}

pub fn read_text(path: &str) -> Obj {
    match fs::read_to_string(path) {
        Ok(s) => Obj::str(s),
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn read_lines(path: &str) -> Obj {
    match fs::read_to_string(path) {
        Ok(s) => {
            let mut items = Vec::with_capacity((s.len() / 48).max(16));
            let text = Arc::new(s);
            items.extend(text.lines().map(|line| {
                let start = line.as_ptr() as usize - text.as_ptr() as usize;
                Obj::Str(Arc::new(VmString::shared(
                    Arc::clone(&text),
                    start..start + line.len(),
                )))
            }));
            Obj::List(ListStorage::Obj(Arc::new(items)))
        }
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn size(path: &str) -> Obj {
    match fs::metadata(path) {
        Ok(m) => Obj::boxed_int(m.len() as i64),
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn list_dir(path: &str) -> Obj {
    let entries = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => return io_err_obj(path, &e),
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => names.push(Obj::str(e.file_name().to_string_lossy().into_owned())),
            Err(e) => return io_err_obj(path, &e),
        }
    }
    Obj::List(ListStorage::Obj(Arc::new(names)))
}

pub fn mkdir_all(path: &str) -> Obj {
    match fs::create_dir_all(path) {
        Ok(()) => Obj::Empty,
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn remove_path(path: &str) -> Obj {
    let p = Path::new(path);
    let res = if p.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    };
    match res {
        Ok(()) => Obj::Empty,
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn write_text(path: &str, data: &str) -> Obj {
    match fs::write(path, data.as_bytes()) {
        Ok(()) => Obj::Empty,
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn append_text(path: &str, data: &str) -> Obj {
    let mut f = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => return io_err_obj(path, &e),
    };
    match f.write_all(data.as_bytes()) {
        Ok(()) => Obj::Empty,
        Err(e) => io_err_obj(path, &e),
    }
}

pub fn write_atomic(path: &str, data: &str) -> Obj {
    let p = Path::new(path);
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let seq = ATOMIC_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".conduit_tmp_{}_{}", std::process::id(), seq));
    if let Err(e) = fs::write(&tmp, data.as_bytes()) {
        let _ = fs::remove_file(&tmp);
        return io_err_obj(path, &e);
    }
    match fs::rename(&tmp, p) {
        Ok(()) => Obj::Empty,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            io_err_obj(path, &e)
        }
    }
}

pub fn copy_file(src: &str, dst: &str) -> Obj {
    match fs::copy(src, dst) {
        Ok(_) => Obj::Empty,
        Err(e) => io_err_obj(src, &e),
    }
}

pub fn rename_path(src: &str, dst: &str) -> Obj {
    match fs::rename(src, dst) {
        Ok(()) => Obj::Empty,
        Err(e) => io_err_obj(src, &e),
    }
}

pub fn open_buf_reader(path: &str) -> Result<BufReader<File>, Obj> {
    File::open(path)
        .map(|f| BufReader::with_capacity(FS_BUF_CAP, f))
        .map_err(|e| io_err_obj(path, &e))
}

pub fn open_buf_writer(path: &str, append: bool) -> Result<BufWriter<File>, Obj> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    opts.open(path)
        .map(|f| BufWriter::with_capacity(FS_BUF_CAP, f))
        .map_err(|e| io_err_obj(path, &e))
}

pub fn reader_read_line(reader: &mut BufReader<File>) -> Obj {
    match reader.fill_buf() {
        Ok([]) => return Obj::Empty,
        Ok(buf) => {
            if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let end = if pos > 0 && buf[pos - 1] == b'\r' {
                    pos - 1
                } else {
                    pos
                };
                let res = match std::str::from_utf8(&buf[..end]) {
                    Ok(s) => Obj::str(s),
                    Err(e) => make_fs_error("invalid_input", "<reader>", e.to_string()),
                };
                reader.consume(pos + 1);
                return res;
            }
        }
        Err(e) => return io_err_obj("<reader>", &e),
    }

    let mut line = String::with_capacity(256);
    match reader.read_line(&mut line) {
        Ok(0) => Obj::Empty,
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Obj::str(line)
        }
        Err(e) => io_err_obj("<reader>", &e),
    }
}

pub fn reader_read_chunk(reader: &mut BufReader<File>, max_bytes: usize) -> Obj {
    if max_bytes == 0 {
        return Obj::str(String::new());
    }
    match reader.fill_buf() {
        Ok([]) => Obj::Empty,
        Ok(buf) if max_bytes <= buf.len() || buf.len() < FS_BUF_CAP => {
            let take = buf.len().min(max_bytes);
            match std::str::from_utf8(&buf[..take]) {
                Ok(s) => {
                    let res = Obj::str(s);
                    reader.consume(take);
                    res
                }
                Err(e) if e.error_len().is_none() => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        let s = std::str::from_utf8(&buf[..valid]).expect("validated prefix");
                        let res = Obj::str(s);
                        reader.consume(valid);
                        res
                    } else {
                        // max_bytes < char_len (e.g. read_chunk(1) on a 2..4-byte char):
                        // advance just enough bytes from the buffer to yield one full char.
                        let end = buf.len().min(4);
                        for len in (take + 1)..=end {
                            if let Ok(s) = std::str::from_utf8(&buf[..len]) {
                                let res = Obj::str(s);
                                reader.consume(len);
                                return res;
                            }
                        }
                        make_fs_error("invalid_input", "<reader>", e.to_string())
                    }
                }
                Err(e) => make_fs_error("invalid_input", "<reader>", e.to_string()),
            }
        }
        Ok(_) => {
            let mut out = Vec::with_capacity(max_bytes);
            match reader.take(max_bytes as u64).read_to_end(&mut out) {
                Ok(0) => Obj::Empty,
                Ok(_) => {
                    if let Err(e) = std::str::from_utf8(&out)
                        && e.error_len().is_none()
                    {
                        for _ in 0..3 {
                            let mut b = [0u8; 1];
                            match reader.read(&mut b) {
                                Ok(1) => {
                                    out.push(b[0]);
                                    if std::str::from_utf8(&out).is_ok() {
                                        break;
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                    match String::from_utf8(out) {
                        Ok(s) => Obj::str(s),
                        Err(e) => make_fs_error("invalid_input", "<reader>", e.to_string()),
                    }
                }
                Err(e) => io_err_obj("<reader>", &e),
            }
        }
        Err(e) => io_err_obj("<reader>", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_lines_preserves_line_semantics_and_owns_the_snapshot() {
        let path = std::env::temp_dir().join(format!("conduit_read_lines_{}", std::process::id()));
        let path_str = path.to_str().unwrap();
        let long = "界".repeat(40_000);
        let long_input = format!("{long}\r\nend");
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("", vec![]),
            ("\n", vec![""]),
            ("\n\n", vec!["", ""]),
            ("a\n\nb\n", vec!["a", "", "b"]),
            ("a\r\nb\r\n", vec!["a", "b"]),
            ("a\rb\r", vec!["a\rb\r"]),
            ("\r\n\r\n", vec!["", ""]),
            ("Привет\n🌍\r\nfin", vec!["Привет", "🌍", "fin"]),
            ("\u{feff}a\0b", vec!["\u{feff}a\0b"]),
            (&long_input, vec![&long, "end"]),
        ];
        for (input, expected) in cases {
            fs::write(&path, input).unwrap();
            let result = read_lines(path_str);
            // Shared text is an owned snapshot, not a mapping of the file.
            fs::write(&path, "replaced").unwrap();
            fs::remove_file(&path).unwrap();
            let Obj::List(ListStorage::Obj(lines)) = result else { panic!("expected lines") };
            assert_eq!(lines.len(), expected.len());
            for (line, expected) in lines.iter().zip(expected) {
                assert_eq!(line, &Obj::str(expected));
            }
            if let Some(Obj::Str(first)) = lines.first() {
                let retained = Arc::clone(first);
                let expected = first.to_string();
                drop(lines);
                assert_eq!(retained.as_str(), expected);
            }
        }
        fs::write(&path, b"valid\ninvalid\xff\n").unwrap();
        let error = read_lines(path_str);
        assert!(matches!(error, Obj::Record(ref rec)
            if rec.schema_id == SCHEMA_FS_ERROR && rec.get_obj(0) == &Obj::str("invalid_input")));
        fs::remove_file(&path).unwrap();
    }
}
