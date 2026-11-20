use std::collections::{BTreeSet, HashMap};
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

// ── CacheStore (TTL + O(1) Slab-LRU + Stampede Lock + Snapshot + WAL) ────────

const LRU_NONE: u32 = u32::MAX;

struct CacheSlot {
    key: Arc<str>,
    value: String,
    expires_at_ms: Option<i64>,
    prev: u32,
    next: u32,
    alive: bool,
}

impl CacheSlot {
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
    slots: Vec<CacheSlot>,
    free_slots: Vec<u32>,
    index: HashMap<Arc<str>, u32>,
    head: u32,
    tail: u32,
    live_count: usize,
    ttl_index: BTreeSet<(i64, u32)>,
    wal: Option<WalState>,
}

impl CacheInner {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            slots: Vec::new(),
            free_slots: Vec::new(),
            index: HashMap::new(),
            head: LRU_NONE,
            tail: LRU_NONE,
            live_count: 0,
            ttl_index: BTreeSet::new(),
            wal: None,
        }
    }

    #[inline]
    fn unlink_slot(&mut self, idx: u32) {
        let prev = self.slots[idx as usize].prev;
        let next = self.slots[idx as usize].next;
        if prev != LRU_NONE {
            self.slots[prev as usize].next = next;
        } else {
            self.head = next;
        }
        if next != LRU_NONE {
            self.slots[next as usize].prev = prev;
        } else {
            self.tail = prev;
        }
        self.slots[idx as usize].prev = LRU_NONE;
        self.slots[idx as usize].next = LRU_NONE;
    }

    #[inline]
    fn push_front(&mut self, idx: u32) {
        let old_head = self.head;
        self.slots[idx as usize].prev = LRU_NONE;
        self.slots[idx as usize].next = old_head;
        if old_head != LRU_NONE {
            self.slots[old_head as usize].prev = idx;
        } else {
            self.tail = idx;
        }
        self.head = idx;
    }

    #[inline]
    fn move_to_front(&mut self, idx: u32) {
        if self.head == idx {
            return;
        }
        self.unlink_slot(idx);
        self.push_front(idx);
    }

    fn remove_slot(&mut self, idx: u32) -> Arc<str> {
        self.unlink_slot(idx);
        let slot = &mut self.slots[idx as usize];
        slot.alive = false;
        let key = Arc::clone(&slot.key);
        slot.value.clear();
        if let Some(exp) = slot.expires_at_ms.take() {
            self.ttl_index.remove(&(exp, idx));
        }
        self.index.remove(key.as_ref());
        self.live_count -= 1;
        self.free_slots.push(idx);
        key
    }

    fn clear_in_memory(&mut self) {
        self.slots.clear();
        self.free_slots.clear();
        self.index.clear();
        self.head = LRU_NONE;
        self.tail = LRU_NONE;
        self.live_count = 0;
        self.ttl_index.clear();
    }

    fn remove_key_only(&mut self, key: &str) -> bool {
        if let Some(&idx) = self.index.get(key) {
            self.remove_slot(idx);
            true
        } else {
            false
        }
    }

    fn insert_in_memory(&mut self, key: &str, value: &str, expires_at_ms: Option<i64>) {
        if let Some(&idx) = self.index.get(key) {
            let old_exp = self.slots[idx as usize].expires_at_ms;
            if old_exp != expires_at_ms {
                if let Some(old) = old_exp {
                    self.ttl_index.remove(&(old, idx));
                }
                if let Some(new_exp) = expires_at_ms {
                    self.ttl_index.insert((new_exp, idx));
                }
                self.slots[idx as usize].expires_at_ms = expires_at_ms;
            }
            self.slots[idx as usize].value.clear();
            self.slots[idx as usize].value.push_str(value);
            self.move_to_front(idx);
            return;
        }
        let arc_key: Arc<str> = Arc::from(key);
        let idx = if let Some(free_idx) = self.free_slots.pop() {
            let slot = &mut self.slots[free_idx as usize];
            slot.key = Arc::clone(&arc_key);
            slot.value.clear();
            slot.value.push_str(value);
            slot.expires_at_ms = expires_at_ms;
            slot.prev = LRU_NONE;
            slot.next = LRU_NONE;
            slot.alive = true;
            free_idx
        } else {
            let new_idx = self.slots.len() as u32;
            self.slots.push(CacheSlot {
                key: Arc::clone(&arc_key),
                value: value.to_string(),
                expires_at_ms,
                prev: LRU_NONE,
                next: LRU_NONE,
                alive: true,
            });
            new_idx
        };
        if let Some(exp) = expires_at_ms {
            self.ttl_index.insert((exp, idx));
        }
        self.index.insert(arc_key, idx);
        self.live_count += 1;
        self.push_front(idx);
    }

    fn prune_expired(&mut self, now_ms: i64) -> usize {
        let mut count = 0usize;
        while let Some(&(exp, idx)) = self.ttl_index.first() {
            if now_ms < exp {
                break;
            }
            self.ttl_index.pop_first();
            if self.slots[idx as usize].alive && self.slots[idx as usize].expires_at_ms == Some(exp)
            {
                self.slots[idx as usize].expires_at_ms = None;
                self.remove_slot(idx);
                count += 1;
            }
        }
        count
    }

    fn enforce_cap(&mut self, now_ms: i64) -> Vec<String> {
        let mut evicted = Vec::new();
        if self.max_entries == 0 || self.live_count <= self.max_entries {
            return evicted;
        }
        if !self.ttl_index.is_empty() {
            self.prune_expired(now_ms);
        }
        let record_evicted = self.wal.is_some();
        while self.live_count > self.max_entries {
            let lru_idx = self.tail;
            if lru_idx == LRU_NONE {
                break;
            }
            let key = self.remove_slot(lru_idx);
            if record_evicted {
                evicted.push(key.as_ref().to_string());
            }
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
                && wal.ops_since_compact >= self.live_count.saturating_mul(2)
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
        let mut buf = Vec::with_capacity(self.live_count * 64 + 64);
        let mut live = 0usize;
        for slot in &self.slots {
            if slot.alive {
                append_wal_set(&mut buf, &slot.key, &slot.value, slot.expires_at_ms);
                live += 1;
            }
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
                            inner.insert_in_memory(&k, &v, exp);
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
                        inner.clear_in_memory();
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
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *guard;
        let idx = *inner.index.get(key)?;
        if inner.slots[idx as usize].expires_at_ms.is_some()
            && inner.slots[idx as usize].is_expired(now_unix_ms())
        {
            inner.remove_slot(idx);
            return None;
        }
        let val = inner.slots[idx as usize].value.clone();
        inner.move_to_front(idx);
        Some(val)
    }

    pub(crate) fn set_str(
        &self,
        key: &str,
        val: &str,
        ttl_ms: Option<i64>,
    ) -> Result<(), (&'static str, String)> {
        let mut now_cached: Option<i64> = None;
        let exp = match ttl_ms {
            None => None,
            Some(ms) if ms < 0 => {
                return Err(("invalid_input", "Cache TTL must be non-negative".to_string()));
            }
            Some(0) => None,
            Some(ms) => {
                let n = now_unix_ms();
                now_cached = Some(n);
                Some(n.saturating_add(ms))
            }
        };
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.insert_in_memory(key, val, exp);
        let now = if !inner.ttl_index.is_empty() {
            now_cached.unwrap_or_else(now_unix_ms)
        } else {
            0
        };
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
        if let Some(&idx) = inner.index.get(key)
            && inner.slots[idx as usize].is_expired(now)
        {
            inner.remove_slot(idx);
        }

        let (next_val, exp) = if let Some(&idx) = inner.index.get(key) {
            let entry = &inner.slots[idx as usize];
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
        inner.insert_in_memory(key, &val_str, exp);
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
        let Some(&idx) = inner.index.get(key) else {
            return false;
        };
        if inner.slots[idx as usize].is_expired(now) {
            inner.remove_slot(idx);
            return false;
        }
        true
    }

    pub(crate) fn ttl_ms(&self, key: &str) -> Option<i64> {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let idx = *inner.index.get(key)?;
        if inner.slots[idx as usize].is_expired(now) {
            inner.remove_slot(idx);
            return None;
        }
        Some(match inner.slots[idx as usize].expires_at_ms {
            None => -1,
            Some(exp) => (exp - now).max(0),
        })
    }

    pub(crate) fn len(&self) -> usize {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.prune_expired(now);
        inner.live_count
    }

    pub(crate) fn keys(&self, prefix: Option<&str>) -> Vec<String> {
        let now = now_unix_ms();
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.prune_expired(now);
        let mut out: Vec<String> = inner
            .slots
            .iter()
            .filter(|s| s.alive && prefix.is_none_or(|p| s.key.starts_with(p)))
            .map(|s| s.key.as_ref().to_string())
            .collect();
        out.sort();
        out
    }

    pub(crate) fn clear(&self) -> Result<(), (&'static str, String)> {
        let mut inner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        inner.clear_in_memory();
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
                .slots
                .iter()
                .filter(|s| s.alive)
                .map(|s| (s.key.as_ref().to_string(), s.value.clone(), s.expires_at_ms))
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
            inner.insert_in_memory(&k, &v, exp);
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
