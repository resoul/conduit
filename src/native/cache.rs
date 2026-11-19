use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::native::json_encode::write_escaped_bytes;
use crate::native::lock::{file_lock_exclusive, file_lock_shared};
use crate::vm::value::{Obj, SCHEMA_CACHE_ERROR, SCHEMA_CACHE_HANDLE, StructData};

pub const CACHE_OPEN: u8 = 0;
pub const CACHE_OPEN_FILE: u8 = 1;
pub const CACHE_DROP: u8 = 2;
pub const CACHE_GET: u8 = 3;
pub const CACHE_SET: u8 = 4;
pub const CACHE_GET_JSON: u8 = 5;
pub const CACHE_SET_JSON: u8 = 6;
pub const CACHE_GET_OR_SET: u8 = 7;
pub const CACHE_INCR: u8 = 8;
pub const CACHE_DEL: u8 = 9;
pub const CACHE_HAS: u8 = 10;
pub const CACHE_TTL: u8 = 11;
pub const CACHE_LEN: u8 = 12;
pub const CACHE_KEYS: u8 = 13;
pub const CACHE_CLEAR: u8 = 14;
pub const CACHE_PRUNE: u8 = 15;
pub const CACHE_SAVE: u8 = 16;
pub const CACHE_LOAD: u8 = 17;
pub const CACHE_COMPACT: u8 = 18;

const AUTO_COMPACT_MIN_OPS: usize = 4096;
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn make_cache_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_CACHE_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_cache_handle(id: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_CACHE_HANDLE, 1, 0);
    s.inline_scalars[0] = id;
    Obj::Record(Arc::new(s))
}

#[inline]
fn now_unix_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis().min(i64::MAX as u128) as i64,
        Err(e) => -(e.duration().as_millis().min(i64::MAX as u128) as i64),
    }
}

fn lock_path_for(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cache");
    parent.join(format!(".{}.tmp.{}.{}", name, std::process::id(), seq))
}

// ── Strict Minimal JSON Parser for WAL & Snapshot Persistence ───────────────

#[derive(Debug)]
enum MiniJson {
    Null,
    Bool,
    Int(i64),
    Str(String),
    Array(Vec<MiniJson>),
    Object(Vec<(String, MiniJson)>),
}

impl MiniJson {
    fn field(&self, key: &str) -> Option<&MiniJson> {
        let MiniJson::Object(pairs) = self else {
            return None;
        };
        pairs.iter().rfind(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            MiniJson::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            MiniJson::Int(n) => Some(*n),
            _ => None,
        }
    }
}

struct JsonScan<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonScan<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn parse_str(&mut self) -> Result<String, String> {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("Expected '\"' starting JSON string".to_string());
        }
        self.pos += 1;
        let start = self.pos;
        let mut i = start;
        while i < self.bytes.len() {
            let b = self.bytes[i];
            if b == b'"' {
                self.pos = i + 1;
                return Ok(self.src[start..i].to_string());
            }
            if b == b'\\' {
                break;
            }
            if b < 0x20 {
                return Err("Unescaped control byte in JSON string".to_string());
            }
            i += 1;
        }
        let mut out = String::from(&self.src[start..i]);
        self.pos = i;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = *self
                        .bytes
                        .get(self.pos)
                        .ok_or_else(|| "Unterminated JSON string escape".to_string())?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0C'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 4)
                                .ok_or_else(|| "Truncated \\u escape".to_string())?;
                            let code = u16::from_str_radix(hex, 16)
                                .map_err(|_| "Invalid \\u hex escape".to_string())?;
                            self.pos += 4;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                    return Err("Missing low surrogate in JSON string".to_string());
                                }
                                self.pos += 2;
                                let hex2 = self
                                    .src
                                    .get(self.pos..self.pos + 4)
                                    .ok_or_else(|| "Truncated surrogate \\u escape".to_string())?;
                                let low = u16::from_str_radix(hex2, 16)
                                    .map_err(|_| "Invalid surrogate \\u hex escape".to_string())?;
                                self.pos += 4;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err("Invalid UTF-16 surrogate pair".to_string());
                                }
                                let cp = 0x10000
                                    + ((((code - 0xD800) as u32) << 10) | ((low - 0xDC00) as u32));
                                out.push(
                                    char::from_u32(cp)
                                        .ok_or_else(|| "Invalid Unicode scalar".to_string())?,
                                );
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                return Err("Lone UTF-16 low surrogate".to_string());
                            } else {
                                out.push(
                                    char::from_u32(u32::from(code))
                                        .ok_or_else(|| "Invalid Unicode scalar".to_string())?,
                                );
                            }
                        }
                        _ => return Err("Unknown JSON escape sequence".to_string()),
                    }
                }
                b if b < 0x20 => return Err("Unescaped control byte in JSON string".to_string()),
                _ => {
                    let ch = self.src[self.pos..]
                        .chars()
                        .next()
                        .ok_or_else(|| "Invalid UTF-8 in JSON string".to_string())?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        Err("Unterminated JSON string".to_string())
    }

    fn parse_val(&mut self, depth: usize) -> Result<MiniJson, String> {
        if depth > 64 {
            return Err("JSON nesting depth exceeded".to_string());
        }
        self.skip_ws();
        let &b = self
            .bytes
            .get(self.pos)
            .ok_or_else(|| "Unexpected end of JSON".to_string())?;
        match b {
            b'"' => Ok(MiniJson::Str(self.parse_str()?)),
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                let mut pairs = Vec::new();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(MiniJson::Object(pairs));
                }
                loop {
                    self.skip_ws();
                    let k = self.parse_str()?;
                    self.skip_ws();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("Expected ':' in JSON object".to_string());
                    }
                    self.pos += 1;
                    let v = self.parse_val(depth + 1)?;
                    pairs.push((k, v));
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(MiniJson::Object(pairs));
                        }
                        _ => return Err("Expected ',' or '}' in JSON object".to_string()),
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                let mut items = Vec::new();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(MiniJson::Array(items));
                }
                loop {
                    items.push(self.parse_val(depth + 1)?);
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(MiniJson::Array(items));
                        }
                        _ => return Err("Expected ',' or ']' in JSON array".to_string()),
                    }
                }
            }
            b't' if self.bytes.get(self.pos..self.pos + 4) == Some(b"true") => {
                self.pos += 4;
                Ok(MiniJson::Bool)
            }
            b'f' if self.bytes.get(self.pos..self.pos + 5) == Some(b"false") => {
                self.pos += 5;
                Ok(MiniJson::Bool)
            }
            b'n' if self.bytes.get(self.pos..self.pos + 4) == Some(b"null") => {
                self.pos += 4;
                Ok(MiniJson::Null)
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                if b == b'-' {
                    self.pos += 1;
                }
                let first = *self
                    .bytes
                    .get(self.pos)
                    .ok_or_else(|| "Malformed JSON integer".to_string())?;
                if !first.is_ascii_digit() {
                    return Err("Malformed JSON integer".to_string());
                }
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                let n = self.src[start..self.pos]
                    .parse::<i64>()
                    .map_err(|_| "JSON integer out of i64 range".to_string())?;
                Ok(MiniJson::Int(n))
            }
            _ => Err("Unexpected character in JSON".to_string()),
        }
    }
}

fn parse_mini_json(src: &str) -> Result<MiniJson, String> {
    let mut scan = JsonScan::new(src);
    let val = scan.parse_val(0)?;
    scan.skip_ws();
    if scan.pos != scan.bytes.len() {
        return Err("Trailing characters after JSON value".to_string());
    }
    Ok(val)
}

// ── WAL & Snapshot Serialization Helpers ────────────────────────────────────

fn append_wal_set(buf: &mut Vec<u8>, key: &str, val: &str, exp: Option<i64>) {
    buf.extend_from_slice(br#"{"op":"set","k":"#);
    write_escaped_bytes(key, buf);
    buf.extend_from_slice(br#","v":"#);
    write_escaped_bytes(val, buf);
    buf.extend_from_slice(br#","exp":"#);
    match exp {
        Some(ms) => {
            use std::io::Write as _;
            let _ = write!(buf, "{}", ms);
        }
        None => buf.extend_from_slice(b"null"),
    }
    buf.extend_from_slice(b"}\n");
}

fn append_wal_del(buf: &mut Vec<u8>, key: &str) {
    buf.extend_from_slice(br#"{"op":"del","k":"#);
    write_escaped_bytes(key, buf);
    buf.extend_from_slice(b"}\n");
}

// ── CacheStore (TTL + O(log N) LRU + Stampede Lock + Snapshot + WAL) ────────

struct CacheEntry {
    value: String,
    expires_at_ms: Option<i64>,
    lru_seq: u64,
}

impl CacheEntry {
    #[inline]
    fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms.is_some_and(|exp| now_ms >= exp)
    }
}

struct WalState {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    ops_since_compact: usize,
}

struct CacheInner {
    max_entries: usize,
    entries: HashMap<String, CacheEntry>,
    lru_order: BTreeMap<u64, String>,
    lru_clock: u64,
    wal: Option<WalState>,
}

impl CacheInner {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: HashMap::new(),
            lru_order: BTreeMap::new(),
            lru_clock: 0,
            wal: None,
        }
    }

    fn remove_key_only(&mut self, key: &str) -> bool {
        if let Some(old) = self.entries.remove(key) {
            self.lru_order.remove(&old.lru_seq);
            true
        } else {
            false
        }
    }

    fn touch_existing(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            self.lru_order.remove(&entry.lru_seq);
            self.lru_clock = self.lru_clock.wrapping_add(1);
            let seq = self.lru_clock;
            entry.lru_seq = seq;
            self.lru_order.insert(seq, key.to_string());
        }
    }

    fn insert_in_memory(&mut self, key: String, value: String, expires_at_ms: Option<i64>) {
        if let Some(old) = self.entries.remove(&key) {
            self.lru_order.remove(&old.lru_seq);
        }
        self.lru_clock = self.lru_clock.wrapping_add(1);
        let seq = self.lru_clock;
        self.lru_order.insert(seq, key.clone());
        self.entries.insert(
            key,
            CacheEntry {
                value,
                expires_at_ms,
                lru_seq: seq,
            },
        );
    }

    fn prune_expired(&mut self, now_ms: i64) -> usize {
        let expired_keys: Vec<(String, u64)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired(now_ms))
            .map(|(k, e)| (k.clone(), e.lru_seq))
            .collect();
        let count = expired_keys.len();
        for (k, seq) in expired_keys {
            self.entries.remove(&k);
            self.lru_order.remove(&seq);
        }
        count
    }

    fn enforce_cap(&mut self, now_ms: i64) -> Vec<String> {
        let mut evicted = Vec::new();
        if self.max_entries == 0 || self.entries.len() <= self.max_entries {
            return evicted;
        }
        self.prune_expired(now_ms);
        while self.entries.len() > self.max_entries {
            let Some((_, k)) = self.lru_order.pop_first() else {
                break;
            };
            self.entries.remove(&k);
            evicted.push(k);
        }
        evicted
    }

    fn write_wal_bytes(&mut self, bytes: &[u8], ops_delta: usize) -> Result<(), (&'static str, String)> {
        let should_compact = {
            let Some(wal) = self.wal.as_mut() else {
                return Ok(());
            };
            let Some(w) = wal.writer.as_mut() else {
                return Err(("io", "Cache WAL writer is closed".to_string()));
            };
            w.write_all(bytes)
                .and_then(|()| w.flush())
                .map_err(|e| ("io", format!("Failed writing cache WAL '{}': {}", wal.path.display(), e)))?;
            wal.ops_since_compact = wal.ops_since_compact.saturating_add(ops_delta);
            wal.ops_since_compact >= AUTO_COMPACT_MIN_OPS
                && wal.ops_since_compact >= self.entries.len().saturating_mul(2)
        };
        if should_compact {
            self.compact_locked()?;
        }
        Ok(())
    }

    fn compact_locked(&mut self) -> Result<usize, (&'static str, String)> {
        let wal_path = match self.wal.as_ref() {
            Some(w) => w.path.clone(),
            None => {
                return Err((
                    "invalid_input",
                    "compact() requires a file-backed cache opened with cache.open_file()"
                        .to_string(),
                ));
            }
        };
        let _flock = file_lock_exclusive(&lock_path_for(&wal_path))?;
        let now = now_unix_ms();
        self.prune_expired(now);

        let tmp = tmp_path_for(&wal_path);
        let mut buf = Vec::with_capacity(self.entries.len() * 64 + 64);
        let mut live = 0usize;
        for (k, entry) in &self.entries {
            append_wal_set(&mut buf, k, &entry.value, entry.expires_at_ms);
            live += 1;
        }
        {
            let mut f = File::create(&tmp).map_err(|e| {
                ("io", format!("Failed to create compacted WAL '{}': {}", tmp.display(), e))
            })?;
            if let Err(e) = f.write_all(&buf).and_then(|()| f.sync_all()) {
                let _ = fs::remove_file(&tmp);
                return Err(("io", format!("Failed to write compacted WAL: {}", e)));
            }
        }
        if let Some(wal) = self.wal.as_mut()
            && let Some(mut w) = wal.writer.take()
        {
            let _ = w.flush();
        }
        if let Err(e) = fs::rename(&tmp, &wal_path) {
            let _ = fs::remove_file(&tmp);
            return Err(("io", format!("Failed to swap compacted WAL '{}': {}", wal_path.display(), e)));
        }
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| ("io", format!("Failed to reopen WAL '{}': {}", wal_path.display(), e)))?;
        if let Some(wal) = self.wal.as_mut() {
            wal.writer = Some(BufWriter::new(f));
            wal.ops_since_compact = 0;
        }
        Ok(live)
    }
}

pub(crate) struct CacheStore {
    state: Mutex<CacheInner>,
    key_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl CacheStore {
    fn new_in_mem(max_entries: usize) -> Self {
        Self {
            state: Mutex::new(CacheInner::new(max_entries)),
            key_locks: Mutex::new(HashMap::new()),
        }
    }

    fn open_with_wal(path: &Path, max_entries: usize) -> Result<Self, (&'static str, String)> {
        let _flock = file_lock_exclusive(&lock_path_for(path))?;
        let mut inner = CacheInner::new(max_entries);
        let now = now_unix_ms();

        if path.exists() {
            let file = File::open(path).map_err(|e| {
                ("io", format!("Failed to open cache WAL '{}': {}", path.display(), e))
            })?;
            let reader = BufReader::new(file);
            for (idx, line_res) in reader.lines().enumerate() {
                let line = line_res.map_err(|e| {
                    ("io", format!("Failed reading cache WAL '{}': {}", path.display(), e))
                })?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let val = parse_mini_json(trimmed).map_err(|e| {
                    (
                        "corrupt_wal",
                        format!("Corrupt WAL line {} in '{}': {}", idx + 1, path.display(), e),
                    )
                })?;
                let op = val
                    .field("op")
                    .and_then(MiniJson::as_str)
                    .ok_or_else(|| {
                        ("corrupt_wal", format!("WAL line {} missing string 'op'", idx + 1))
                    })?;
                match op {
                    "set" => {
                        let k = val
                            .field("k")
                            .and_then(MiniJson::as_str)
                            .ok_or_else(|| {
                                ("corrupt_wal", format!("WAL line {} missing 'k'", idx + 1))
                            })?
                            .to_string();
                        let v = val
                            .field("v")
                            .and_then(MiniJson::as_str)
                            .ok_or_else(|| {
                                ("corrupt_wal", format!("WAL line {} missing 'v'", idx + 1))
                            })?
                            .to_string();
                        let exp = match val.field("exp") {
                            None | Some(MiniJson::Null) => None,
                            Some(MiniJson::Int(ms)) => Some(*ms),
                            Some(_) => {
                                return Err((
                                    "corrupt_wal",
                                    format!("WAL line {} has invalid 'exp'", idx + 1),
                                ));
                            }
                        };
                        if exp.is_some_and(|e| now >= e) {
                            inner.remove_key_only(&k);
                        } else {
                            inner.insert_in_memory(k, v, exp);
                            let _ = inner.enforce_cap(now);
                        }
                    }
                    "del" => {
                        let k = val
                            .field("k")
                            .and_then(MiniJson::as_str)
                            .ok_or_else(|| {
                                ("corrupt_wal", format!("WAL line {} missing 'k'", idx + 1))
                            })?;
                        inner.remove_key_only(k);
                    }
                    "clear" => {
                        inner.entries.clear();
                        inner.lru_order.clear();
                    }
                    other => {
                        return Err((
                            "corrupt_wal",
                            format!("Unknown WAL op '{}' on line {}", other, idx + 1),
                        ));
                    }
                }
            }
        }

        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| {
                ("io", format!("Failed opening cache WAL '{}' for append: {}", path.display(), e))
            })?;
        inner.wal = Some(WalState {
            path: path.to_path_buf(),
            writer: Some(BufWriter::new(wal_file)),
            ops_since_compact: 0,
        });

        Ok(Self {
            state: Mutex::new(inner),
            key_locks: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn key_lock(&self, key: &str) -> Arc<Mutex<()>> {
        let mut map = self.key_locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            map.entry(key.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    pub(crate) fn get_str(&self, key: &str) -> Option<String> {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let expired = inner
            .entries
            .get(key)
            .is_some_and(|e| e.is_expired(now));
        if expired {
            inner.remove_key_only(key);
            return None;
        }
        let val = inner.entries.get(key).map(|e| e.value.clone())?;
        inner.touch_existing(key);
        Some(val)
    }

    pub(crate) fn set_str(
        &self,
        key: &str,
        val: &str,
        ttl_ms: Option<i64>,
    ) -> Result<(), (&'static str, String)> {
        let exp = match ttl_ms {
            None => None,
            Some(ms) if ms < 0 => {
                return Err(("invalid_input", "Cache TTL must be non-negative".to_string()));
            }
            Some(0) => None,
            Some(ms) => Some(now_unix_ms().saturating_add(ms)),
        };
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.insert_in_memory(key.to_string(), val.to_string(), exp);
        let evicted = inner.enforce_cap(now);
        if inner.wal.is_some() {
            let mut buf = Vec::with_capacity(key.len() + val.len() + 48);
            append_wal_set(&mut buf, key, val, exp);
            for ek in &evicted {
                append_wal_del(&mut buf, ek);
            }
            inner.write_wal_bytes(&buf, 1 + evicted.len())?;
        }
        Ok(())
    }

    pub(crate) fn incr(
        &self,
        key: &str,
        delta: i64,
        ttl_ms: Option<i64>,
    ) -> Result<i64, (&'static str, String)> {
        if let Some(ms) = ttl_ms
            && ms < 0
        {
            return Err(("invalid_input", "Cache TTL must be non-negative".to_string()));
        }
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if inner
            .entries
            .get(key)
            .is_some_and(|e| e.is_expired(now))
        {
            inner.remove_key_only(key);
        }

        let (next_val, exp) = if let Some(entry) = inner.entries.get(key) {
            let cur = entry.value.parse::<i64>().map_err(|_| {
                (
                    "invalid_value",
                    format!("Cache key '{}' value '{}' is not an integer", key, entry.value),
                )
            })?;
            let sum = cur.checked_add(delta).ok_or_else(|| {
                ("overflow", format!("Integer overflow incrementing cache key '{}'", key))
            })?;
            (sum, entry.expires_at_ms)
        } else {
            let exp = match ttl_ms {
                Some(ms) if ms > 0 => Some(now.saturating_add(ms)),
                _ => None,
            };
            (delta, exp)
        };

        let val_str = next_val.to_string();
        inner.insert_in_memory(key.to_string(), val_str.clone(), exp);
        let evicted = inner.enforce_cap(now);
        if inner.wal.is_some() {
            let mut buf = Vec::with_capacity(key.len() + val_str.len() + 48);
            append_wal_set(&mut buf, key, &val_str, exp);
            for ek in &evicted {
                append_wal_del(&mut buf, ek);
            }
            inner.write_wal_bytes(&buf, 1 + evicted.len())?;
        }
        Ok(next_val)
    }

    pub(crate) fn del(&self, key: &str) -> Result<bool, (&'static str, String)> {
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let removed = inner.remove_key_only(key);
        if removed && inner.wal.is_some() {
            let mut buf = Vec::with_capacity(key.len() + 24);
            append_wal_del(&mut buf, key);
            inner.write_wal_bytes(&buf, 1)?;
        }
        Ok(removed)
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if inner
            .entries
            .get(key)
            .is_some_and(|e| e.is_expired(now))
        {
            inner.remove_key_only(key);
            return false;
        }
        inner.entries.contains_key(key)
    }

    pub(crate) fn ttl_ms(&self, key: &str) -> Option<i64> {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = inner.entries.get(key)?;
        if entry.is_expired(now) {
            inner.remove_key_only(key);
            return None;
        }
        Some(match entry.expires_at_ms {
            None => -1,
            Some(exp) => (exp - now).max(0),
        })
    }

    pub(crate) fn len(&self) -> usize {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.prune_expired(now);
        inner.entries.len()
    }

    pub(crate) fn keys(&self, prefix: Option<&str>) -> Vec<String> {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.prune_expired(now);
        let mut out: Vec<String> = inner
            .entries
            .keys()
            .filter(|k| prefix.is_none_or(|p| k.starts_with(p)))
            .cloned()
            .collect();
        out.sort();
        out
    }

    pub(crate) fn clear(&self) -> Result<(), (&'static str, String)> {
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.clear();
        inner.lru_order.clear();
        if inner.wal.is_some() {
            inner.write_wal_bytes(b"{\"op\":\"clear\"}\n", 1)?;
        }
        Ok(())
    }

    pub(crate) fn prune(&self) -> usize {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.prune_expired(now)
    }

    pub(crate) fn save_snapshot(&self, path: &Path) -> Result<usize, (&'static str, String)> {
        let now = now_unix_ms();
        let snapshot_items: Vec<(String, String, Option<i64>)> = {
            let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
            inner.prune_expired(now);
            inner
                .entries
                .iter()
                .map(|(k, e)| (k.clone(), e.value.clone(), e.expires_at_ms))
                .collect()
        };

        let _flock = file_lock_exclusive(&lock_path_for(path))?;
        let mut out = Vec::with_capacity(snapshot_items.len() * 64 + 64);
        out.extend_from_slice(br#"{"version":1,"entries":["#);
        for (idx, (k, v, exp)) in snapshot_items.iter().enumerate() {
            if idx > 0 {
                out.push(b',');
            }
            out.extend_from_slice(br#"{"k":"#);
            write_escaped_bytes(k, &mut out);
            out.extend_from_slice(br#","v":"#);
            write_escaped_bytes(v, &mut out);
            out.extend_from_slice(br#","exp":"#);
            match exp {
                Some(ms) => {
                    let _ = write!(out, "{}", ms);
                }
                None => out.extend_from_slice(b"null"),
            }
            out.push(b'}');
        }
        out.extend_from_slice(b"]}\n");

        let tmp = tmp_path_for(path);
        {
            let mut f = File::create(&tmp).map_err(|e| {
                ("io", format!("Failed creating snapshot temp '{}': {}", tmp.display(), e))
            })?;
            if let Err(e) = f.write_all(&out).and_then(|()| f.sync_all()) {
                let _ = fs::remove_file(&tmp);
                return Err(("io", format!("Failed writing snapshot '{}': {}", tmp.display(), e)));
            }
        }
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err((
                "io",
                format!("Failed moving snapshot into '{}': {}", path.display(), e),
            ));
        }
        Ok(snapshot_items.len())
    }

    pub(crate) fn load_snapshot(&self, path: &Path) -> Result<usize, (&'static str, String)> {
        let raw = {
            let _flock = file_lock_shared(&lock_path_for(path))?;
            fs::read_to_string(path).map_err(|e| {
                ("io", format!("Failed reading cache snapshot '{}': {}", path.display(), e))
            })?
        };
        let root = parse_mini_json(&raw)
            .map_err(|e| ("corrupt_snapshot", format!("Invalid cache snapshot JSON: {}", e)))?;
        let version = root
            .field("version")
            .and_then(MiniJson::as_i64)
            .ok_or_else(|| ("corrupt_snapshot", "Snapshot missing integer 'version'".to_string()))?;
        if version != 1 {
            return Err((
                "corrupt_snapshot",
                format!("Unsupported cache snapshot version {}", version),
            ));
        }
        let Some(MiniJson::Array(items)) = root.field("entries") else {
            return Err(("corrupt_snapshot", "Snapshot missing 'entries' array".to_string()));
        };

        let now = now_unix_ms();
        let mut parsed = Vec::with_capacity(items.len());
        for item in items {
            let k = item
                .field("k")
                .and_then(MiniJson::as_str)
                .ok_or_else(|| ("corrupt_snapshot", "Snapshot entry missing 'k'".to_string()))?
                .to_string();
            let v = item
                .field("v")
                .and_then(MiniJson::as_str)
                .ok_or_else(|| ("corrupt_snapshot", "Snapshot entry missing 'v'".to_string()))?
                .to_string();
            let exp = match item.field("exp") {
                None | Some(MiniJson::Null) => None,
                Some(MiniJson::Int(ms)) => Some(*ms),
                Some(_) => {
                    return Err(("corrupt_snapshot", "Snapshot entry has invalid 'exp'".to_string()));
                }
            };
            if exp.is_none_or(|e| e > now) {
                parsed.push((k, v, exp));
            }
        }

        let loaded_count = parsed.len();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut wal_buf = Vec::new();
        let has_wal = inner.wal.is_some();
        for (k, v, exp) in parsed {
            if has_wal {
                append_wal_set(&mut wal_buf, &k, &v, exp);
            }
            inner.insert_in_memory(k, v, exp);
        }
        let evicted = inner.enforce_cap(now);
        if has_wal {
            for ek in &evicted {
                append_wal_del(&mut wal_buf, ek);
            }
            if !wal_buf.is_empty() {
                inner.write_wal_bytes(&wal_buf, loaded_count + evicted.len())?;
            }
        }
        Ok(loaded_count)
    }

    pub(crate) fn compact(&self) -> Result<usize, (&'static str, String)> {
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.compact_locked()
    }
}

// ── Process-Wide Named Cache Registry ───────────────────────────────────────

struct CacheRegistry {
    next_id: u64,
    by_name: HashMap<String, u64>,
    by_id: HashMap<u64, Arc<CacheStore>>,
}

impl CacheRegistry {
    fn new() -> Self {
        Self {
            next_id: 1,
            by_name: HashMap::new(),
            by_id: HashMap::new(),
        }
    }
}

fn cache_registry() -> &'static Mutex<CacheRegistry> {
    static REG: OnceLock<Mutex<CacheRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(CacheRegistry::new()))
}

pub(crate) fn open_cache(name: &str, max_entries: i64) -> u64 {
    let cap = max_entries.max(0) as usize;
    let mut reg = cache_registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&id) = reg.by_name.get(name)
        && let Some(store) = reg.by_id.get(&id).cloned()
    {
        if cap > 0 {
            let now = now_unix_ms();
            let mut inner = store.state.lock().unwrap_or_else(|e| e.into_inner());
            inner.max_entries = cap;
            let _ = inner.enforce_cap(now);
        }
        return id;
    }
    let id = reg.next_id;
    reg.next_id += 1;
    let store = Arc::new(CacheStore::new_in_mem(cap));
    reg.by_name.insert(name.to_string(), id);
    reg.by_id.insert(id, store);
    id
}

pub(crate) fn open_cache_file(
    name: &str,
    path_str: &str,
    max_entries: i64,
) -> Result<u64, (&'static str, String)> {
    let cap = max_entries.max(0) as usize;
    let path = Path::new(path_str);
    {
        let reg = cache_registry().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = reg.by_name.get(name)
            && let Some(store) = reg.by_id.get(&id)
        {
            let mut inner = store.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(wal) = inner.wal.as_ref()
                && wal.path == path
            {
                if cap > 0 {
                    inner.max_entries = cap;
                    let now = now_unix_ms();
                    let _ = inner.enforce_cap(now);
                }
                return Ok(id);
            }
        }
    }

    let store = Arc::new(CacheStore::open_with_wal(path, cap)?);
    let mut reg = cache_registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old_id) = reg.by_name.remove(name) {
        reg.by_id.remove(&old_id);
    }
    let id = reg.next_id;
    reg.next_id += 1;
    reg.by_name.insert(name.to_string(), id);
    reg.by_id.insert(id, store);
    Ok(id)
}

pub(crate) fn drop_cache(name: &str) -> bool {
    let mut reg = cache_registry().lock().unwrap_or_else(|e| e.into_inner());
    let Some(id) = reg.by_name.remove(name) else {
        return false;
    };
    reg.by_id.remove(&id).is_some()
}

pub(crate) fn get_cache_store(id: u64) -> Option<Arc<CacheStore>> {
    let reg = cache_registry().lock().unwrap_or_else(|e| e.into_inner());
    reg.by_id.get(&id).cloned()
}
