use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::vm::value::{Obj, SCHEMA_LOCK_ERROR, SCHEMA_LOCK_GUARD, StructData};

pub const LOCK_WITH: u8 = 0;
pub const LOCK_TRY_WITH: u8 = 1;
pub const LOCK_READ: u8 = 2;
pub const LOCK_WRITE: u8 = 3;
pub const LOCK_FILE: u8 = 4;
pub const LOCK_FILE_TRY: u8 = 5;
pub const LOCK_FILE_SHARED: u8 = 6;
pub const LOCK_ACQUIRE: u8 = 7;
pub const LOCK_TRY_ACQUIRE: u8 = 8;
pub const LOCK_FILE_ACQUIRE: u8 = 9;
pub const LOCK_FILE_TRY_ACQUIRE: u8 = 10;
pub const LOCK_GUARD_RELEASE: u8 = 11;
pub const LOCK_GUARD_IS_HELD: u8 = 12;

#[inline]
pub fn make_lock_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_LOCK_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_lock_guard(id: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_LOCK_GUARD, 1, 0);
    s.inline_scalars[0] = id;
    Obj::Record(Arc::new(s))
}

// ── In-Process Timed Named Mutex ────────────────────────────────────────────

struct TimedMutex {
    locked: Mutex<bool>,
    cv: Condvar,
}

impl TimedMutex {
    fn new() -> Self {
        Self {
            locked: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
}

pub(crate) struct NamedMutexGuard {
    mutex: Arc<TimedMutex>,
}

impl Drop for NamedMutexGuard {
    fn drop(&mut self) {
        let mut locked = self.mutex.locked.lock().unwrap_or_else(|e| e.into_inner());
        *locked = false;
        self.mutex.cv.notify_one();
    }
}

fn named_mutex(name: &str) -> Arc<TimedMutex> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<TimedMutex>>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = table.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        guard
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(TimedMutex::new())),
    )
}

pub(crate) fn lock_blocking(name: &str) -> NamedMutexGuard {
    let m = named_mutex(name);
    let mut locked = m.locked.lock().unwrap_or_else(|e| e.into_inner());
    while *locked {
        locked = m.cv.wait(locked).unwrap_or_else(|e| e.into_inner());
    }
    *locked = true;
    drop(locked);
    NamedMutexGuard { mutex: m }
}

pub(crate) fn lock_timeout(
    name: &str,
    timeout_ms: i64,
) -> Result<NamedMutexGuard, (&'static str, String)> {
    if timeout_ms < 0 {
        return Err((
            "invalid_input",
            format!("Lock timeout must be non-negative, got {}", timeout_ms),
        ));
    }
    let m = named_mutex(name);
    let mut locked = m.locked.lock().unwrap_or_else(|e| e.into_inner());
    if !*locked {
        *locked = true;
        drop(locked);
        return Ok(NamedMutexGuard { mutex: m });
    }
    if timeout_ms == 0 {
        return Err(("busy", format!("Lock '{}' is currently held", name)));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    while *locked {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err((
                "timeout",
                format!("Timed out after {}ms waiting for lock '{}'", timeout_ms, name),
            ));
        };
        let (next_locked, wait_res) = m
            .cv
            .wait_timeout(locked, remaining)
            .unwrap_or_else(|e| e.into_inner());
        locked = next_locked;
        if wait_res.timed_out() && *locked {
            return Err((
                "timeout",
                format!("Timed out after {}ms waiting for lock '{}'", timeout_ms, name),
            ));
        }
    }
    *locked = true;
    drop(locked);
    Ok(NamedMutexGuard { mutex: m })
}

// ── In-Process Writer-Fair Named RWLock ─────────────────────────────────────

#[derive(Default)]
struct RwCounts {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
}

struct NamedRwLock {
    state: Mutex<RwCounts>,
    cv: Condvar,
}

impl NamedRwLock {
    fn new() -> Self {
        Self {
            state: Mutex::new(RwCounts::default()),
            cv: Condvar::new(),
        }
    }
}

pub(crate) struct NamedReadGuard {
    rw: Arc<NamedRwLock>,
}

impl Drop for NamedReadGuard {
    fn drop(&mut self) {
        let mut st = self.rw.state.lock().unwrap_or_else(|e| e.into_inner());
        st.readers = st.readers.saturating_sub(1);
        if st.readers == 0 {
            self.rw.cv.notify_all();
        }
    }
}

pub(crate) struct NamedWriteGuard {
    rw: Arc<NamedRwLock>,
}

impl Drop for NamedWriteGuard {
    fn drop(&mut self) {
        let mut st = self.rw.state.lock().unwrap_or_else(|e| e.into_inner());
        st.writer = false;
        self.rw.cv.notify_all();
    }
}

fn named_rwlock(name: &str) -> Arc<NamedRwLock> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<NamedRwLock>>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = table.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        guard
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(NamedRwLock::new())),
    )
}

pub(crate) fn read_blocking(name: &str) -> NamedReadGuard {
    let rw = named_rwlock(name);
    let mut st = rw.state.lock().unwrap_or_else(|e| e.into_inner());
    while st.writer || st.waiting_writers > 0 {
        st = rw.cv.wait(st).unwrap_or_else(|e| e.into_inner());
    }
    st.readers += 1;
    drop(st);
    NamedReadGuard { rw }
}

pub(crate) fn write_blocking(name: &str) -> NamedWriteGuard {
    let rw = named_rwlock(name);
    let mut st = rw.state.lock().unwrap_or_else(|e| e.into_inner());
    st.waiting_writers += 1;
    while st.writer || st.readers > 0 {
        st = rw.cv.wait(st).unwrap_or_else(|e| e.into_inner());
    }
    st.waiting_writers -= 1;
    st.writer = true;
    drop(st);
    NamedWriteGuard { rw }
}

// ── Inter-Process OS File Locks (std::fs::File::lock*) ──────────────────────

pub(crate) struct FileLockGuard {
    file: File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn open_lock_file(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

pub(crate) fn file_lock_exclusive(path: &Path) -> Result<FileLockGuard, (&'static str, String)> {
    let f = open_lock_file(path)
        .map_err(|e| ("io", format!("Failed to open lock file '{}': {}", path.display(), e)))?;
    f.lock()
        .map_err(|e| ("io", format!("Failed to lock file '{}': {}", path.display(), e)))?;
    Ok(FileLockGuard { file: f })
}

pub(crate) fn file_try_lock_exclusive(
    path: &Path,
) -> Result<FileLockGuard, (&'static str, String)> {
    let f = open_lock_file(path)
        .map_err(|e| ("io", format!("Failed to open lock file '{}': {}", path.display(), e)))?;
    match f.try_lock() {
        Ok(()) => Ok(FileLockGuard { file: f }),
        Err(TryLockError::WouldBlock) => Err((
            "busy",
            format!("File lock '{}' is currently held", path.display()),
        )),
        Err(TryLockError::Error(e)) => Err((
            "io",
            format!("Failed to try-lock file '{}': {}", path.display(), e),
        )),
    }
}

pub(crate) fn file_lock_shared(path: &Path) -> Result<FileLockGuard, (&'static str, String)> {
    let f = open_lock_file(path)
        .map_err(|e| ("io", format!("Failed to open lock file '{}': {}", path.display(), e)))?;
    f.lock_shared()
        .map_err(|e| ("io", format!("Failed to lock file '{}' (shared): {}", path.display(), e)))?;
    Ok(FileLockGuard { file: f })
}

// ── Manual Guard Registry ───────────────────────────────────────────────────

#[allow(dead_code)]
pub(crate) enum HeldGuard {
    Mutex(NamedMutexGuard),
    File(FileLockGuard),
}

fn guard_table() -> &'static Mutex<HashMap<u64, HeldGuard>> {
    static GUARDS: OnceLock<Mutex<HashMap<u64, HeldGuard>>> = OnceLock::new();
    GUARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_GUARD_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn register_guard(g: HeldGuard) -> u64 {
    let id = NEXT_GUARD_ID.fetch_add(1, Ordering::Relaxed);
    guard_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, g);
    id
}

pub(crate) fn release_guard(id: u64) {
    let removed = guard_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    drop(removed);
}

pub(crate) fn is_guard_held(id: u64) -> bool {
    guard_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&id)
}
