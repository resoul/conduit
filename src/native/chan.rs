use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::vm::value::{Obj, SCHEMA_CHAN_HANDLE, StructData};

pub const CHAN_OPEN: u8 = 0;
pub const CHAN_SEND: u8 = 1;
pub const CHAN_TRY_SEND: u8 = 2;
pub const CHAN_RECV: u8 = 3;
pub const CHAN_TRY_RECV: u8 = 4;
pub const CHAN_RECV_TIMEOUT: u8 = 5;
pub const CHAN_CLOSE: u8 = 6;
pub const CHAN_LEN: u8 = 7;
pub const CHAN_CAP: u8 = 8;
pub const CHAN_IS_EMPTY: u8 = 9;
pub const CHAN_IS_CLOSED: u8 = 10;

struct ChanInner {
    queue: VecDeque<Obj>,
    capacity: usize,
    closed: bool,
}

/// Thread-safe bounded or unbounded FIFO channel shared across `spawn` and HTTP worker threads.
pub struct ChannelState {
    inner: Mutex<ChanInner>,
    not_empty: Condvar,
    not_full: Condvar,
}

impl ChannelState {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(ChanInner {
                queue: VecDeque::with_capacity(capacity.min(256)),
                capacity,
                closed: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
        }
    }

    pub fn send(&self, val: Obj) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while !guard.closed && guard.capacity > 0 && guard.queue.len() >= guard.capacity {
            guard = self.not_full.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
        if guard.closed {
            return false;
        }
        guard.queue.push_back(val);
        self.not_empty.notify_one();
        true
    }

    pub fn try_send(&self, val: Obj) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed || (guard.capacity > 0 && guard.queue.len() >= guard.capacity) {
            return false;
        }
        guard.queue.push_back(val);
        self.not_empty.notify_one();
        true
    }

    pub fn recv(&self) -> Option<Obj> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while guard.queue.is_empty() && !guard.closed {
            guard = self.not_empty.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
        if let Some(val) = guard.queue.pop_front() {
            self.not_full.notify_one();
            Some(val)
        } else {
            None
        }
    }

    pub fn try_recv(&self) -> Option<Obj> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(val) = guard.queue.pop_front() {
            self.not_full.notify_one();
            Some(val)
        } else {
            None
        }
    }

    pub fn recv_timeout(&self, timeout_ms: i64) -> Option<Obj> {
        if timeout_ms <= 0 {
            return self.try_recv();
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while guard.queue.is_empty() && !guard.closed {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next_guard, res) = self
                .not_empty
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = next_guard;
            if res.timed_out() && guard.queue.is_empty() {
                return None;
            }
        }
        if let Some(val) = guard.queue.pop_front() {
            self.not_full.notify_one();
            Some(val)
        } else {
            None
        }
    }

    pub fn close(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).queue.len()
    }

    pub fn cap(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).capacity
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .is_empty()
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).closed
    }
}

fn chan_registry() -> &'static Mutex<Vec<Arc<ChannelState>>> {
    static CHANNELS: OnceLock<Mutex<Vec<Arc<ChannelState>>>> = OnceLock::new();
    CHANNELS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn open_channel(capacity: i64) -> u64 {
    let cap = if capacity <= 0 { 0 } else { capacity as usize };
    let mut reg = chan_registry().lock().unwrap_or_else(|e| e.into_inner());
    let id = reg.len() as u64;
    reg.push(Arc::new(ChannelState::new(cap)));
    id
}

pub fn get_channel(id: u64) -> Option<Arc<ChannelState>> {
    let reg = chan_registry().lock().unwrap_or_else(|e| e.into_inner());
    reg.get(id as usize).cloned()
}

#[inline]
pub fn make_chan_handle(id: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_CHAN_HANDLE, 1, 0);
    s.inline_scalars[0] = id;
    Obj::Record(Arc::new(s))
}
