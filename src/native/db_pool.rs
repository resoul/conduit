use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::native::db::{DbErr, SqlConn};
use crate::vm::value::{Obj, SCHEMA_DB_POOL, StructData};

pub fn make_db_pool(handle: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_POOL, 1, 0);
    s.set_scalar(0, handle);
    Obj::Record(Arc::new(s))
}

struct IdleConn {
    conn: SqlConn,
    returned_at: Instant,
}

struct PoolInner {
    idle: Vec<IdleConn>,
    in_use: usize,
    closed: bool,
}

pub struct DbPoolState {
    url: String,
    max_conns: usize,
    idle_timeout: Option<Duration>,
    inner: Mutex<PoolInner>,
    cv: Condvar,
}

impl DbPoolState {
    pub fn open(url: &str, max_conns: i64, idle_timeout_ms: i64) -> Result<Arc<Self>, DbErr> {
        if max_conns < 1 {
            return Err(DbErr::new(
                "invalid_config",
                format!("db.pool() max_conns must be >= 1, got {}", max_conns),
                "",
            ));
        }
        if idle_timeout_ms < 0 {
            return Err(DbErr::new(
                "invalid_config",
                format!("db.pool() idle_timeout_ms must be >= 0, got {}", idle_timeout_ms),
                "",
            ));
        }
        let first = SqlConn::open(url)?;
        let idle_timeout = if idle_timeout_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(idle_timeout_ms as u64))
        };
        Ok(Arc::new(Self {
            url: url.to_string(),
            max_conns: max_conns as usize,
            idle_timeout,
            inner: Mutex::new(PoolInner {
                idle: vec![IdleConn {
                    conn: first,
                    returned_at: Instant::now(),
                }],
                in_use: 0,
                closed: false,
            }),
            cv: Condvar::new(),
        }))
    }

    pub fn acquire(&self) -> Result<SqlConn, DbErr> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if guard.closed {
                return Err(DbErr::new("closed", "Database pool is closed", ""));
            }
            if let Some(mut item) = guard.idle.pop() {
                let elapsed = item.returned_at.elapsed();
                if let Some(ttl) = self.idle_timeout
                    && elapsed > ttl
                {
                    drop(guard);
                    item.conn.close();
                    guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    self.cv.notify_one();
                    continue;
                }
                if elapsed > Duration::from_secs(30) {
                    guard.in_use += 1;
                    drop(guard);
                    if item.conn.ping().is_ok() {
                        return Ok(item.conn);
                    }
                    item.conn.close();
                    guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.in_use = guard.in_use.saturating_sub(1);
                    self.cv.notify_one();
                    continue;
                }
                guard.in_use += 1;
                return Ok(item.conn);
            }
            if guard.in_use < self.max_conns {
                guard.in_use += 1;
                drop(guard);
                match SqlConn::open(&self.url) {
                    Ok(conn) => return Ok(conn),
                    Err(e) => {
                        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        g.in_use = g.in_use.saturating_sub(1);
                        self.cv.notify_one();
                        return Err(e);
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(DbErr::new(
                    "pool_timeout",
                    "Timed out waiting for connection from db.Pool",
                    "",
                ));
            }
            let (new_guard, wait_res) = self
                .cv
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = new_guard;
            if wait_res.timed_out() && guard.idle.is_empty() && guard.in_use >= self.max_conns {
                return Err(DbErr::new(
                    "pool_timeout",
                    "Timed out waiting for connection from db.Pool",
                    "",
                ));
            }
        }
    }

    pub fn release(&self, conn: SqlConn) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.in_use = guard.in_use.saturating_sub(1);
        if guard.closed {
            drop(guard);
            conn.close();
            self.cv.notify_all();
            return;
        }
        if guard.idle.len() + guard.in_use < self.max_conns {
            guard.idle.push(IdleConn {
                conn,
                returned_at: Instant::now(),
            });
        } else {
            drop(guard);
            conn.close();
        }
        self.cv.notify_one();
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (guard.idle.len(), guard.in_use, self.max_conns)
    }

    pub fn close(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        let drained = std::mem::take(&mut guard.idle);
        drop(guard);
        for item in drained {
            item.conn.close();
        }
        self.cv.notify_all();
    }
}
