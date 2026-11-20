use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::native::crypto::{encode_hex, fill_random_bytes};
use crate::native::json_encode::write_escaped_bytes;
use crate::native::tls::roots::RootSource;
use crate::native::tls::{TlsConfig, TlsStream, TlsVersion, VerifyMode};
use crate::native::url::percent_decode;
use crate::vm::value::{
    Obj, SCHEMA_REDIS_CLIENT, SCHEMA_REDIS_ERROR, SCHEMA_REDIS_LOCK, StructData,
};

pub const REDIS_CONNECT: u8 = 0;
pub const REDIS_PING: u8 = 1;
pub const REDIS_CMD: u8 = 2;
pub const REDIS_CLOSE: u8 = 3;
pub const REDIS_GET: u8 = 4;
pub const REDIS_SET: u8 = 5;
pub const REDIS_SET_EX: u8 = 6;
pub const REDIS_SET_NX: u8 = 7;
pub const REDIS_DEL: u8 = 8;
pub const REDIS_EXISTS: u8 = 9;
pub const REDIS_EXPIRE: u8 = 10;
pub const REDIS_TTL: u8 = 11;
pub const REDIS_INCR: u8 = 12;
pub const REDIS_INCR_BY: u8 = 13;
pub const REDIS_DECR: u8 = 14;
pub const REDIS_KEYS: u8 = 15;
pub const REDIS_GET_JSON: u8 = 16;
pub const REDIS_SET_JSON: u8 = 17;
pub const REDIS_HSET: u8 = 18;
pub const REDIS_HGET: u8 = 19;
pub const REDIS_HGETALL: u8 = 20;
pub const REDIS_HDEL: u8 = 21;
pub const REDIS_LPUSH: u8 = 22;
pub const REDIS_RPUSH: u8 = 23;
pub const REDIS_LPOP: u8 = 24;
pub const REDIS_RPOP: u8 = 25;
pub const REDIS_LRANGE: u8 = 26;
pub const REDIS_SADD: u8 = 27;
pub const REDIS_SREM: u8 = 28;
pub const REDIS_SMEMBERS: u8 = 29;
pub const REDIS_SISMEMBER: u8 = 30;
pub const REDIS_LOCK: u8 = 31;
pub const REDIS_UNLOCK: u8 = 32;
pub const REDIS_EXTEND: u8 = 33;
pub const REDIS_WITH_LOCK: u8 = 34;

const MAX_RESP_BULK_BYTES: i64 = 64 * 1024 * 1024;
const MAX_RESP_ARRAY_LEN: i64 = 1_048_576;
const MAX_RESP_DEPTH: usize = 64;

pub const LUA_UNLOCK: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";
pub const LUA_EXTEND: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('pexpire', KEYS[1], ARGV[2]) else return 0 end";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisErr {
    pub kind: String,
    pub message: String,
}

impl RedisErr {
    #[cold]
    #[inline(never)]
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }

    pub fn into_obj(self) -> Obj {
        make_redis_error(&self.kind, self.message)
    }
}

#[inline]
pub fn make_redis_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_REDIS_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_redis_client(id: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_REDIS_CLIENT, 1, 0);
    s.inline_scalars[0] = id;
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_redis_lock(client_id: u64, key: String, token: String) -> Obj {
    let mut s = StructData::new(SCHEMA_REDIS_LOCK, 1, 2);
    s.inline_scalars[0] = client_id;
    s.inline_objs[0] = Obj::str(key);
    s.inline_objs[1] = Obj::str(token);
    Obj::Record(Arc::new(s))
}

// ── URL Parsing ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisTlsConfig {
    pub verify: VerifyMode,
    pub ca_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisConfig {
    pub is_mock: bool,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub db: u32,
    pub timeout_ms: u64,
    pub tls: Option<RedisTlsConfig>,
}

pub fn parse_redis_url(raw: &str) -> Result<RedisConfig, RedisErr> {
    let (is_mock, is_tls, rest) = if let Some(r) = raw.strip_prefix("redis://") {
        (false, false, r)
    } else if let Some(r) = raw.strip_prefix("rediss://") {
        (false, true, r)
    } else if cfg!(test)
        && let Some(r) = raw.strip_prefix("redis-mock://")
    {
        (true, false, r)
    } else {
        return Err(RedisErr::new(
            "invalid_url",
            "Redis URL must start with redis:// or rediss://",
        ));
    };

    let (before_query, query) = match rest.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (rest, None),
    };

    let mut timeout_ms = 5_000u64;
    let mut tls_verify = VerifyMode::VerifyFull;
    let mut ca_file: Option<String> = None;
    if let Some(q) = query {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            let Some((k, v)) = pair.split_once('=') else {
                return Err(RedisErr::new("invalid_url", "Invalid query parameter in Redis URL"));
            };
            match k {
                "timeout_ms" => {
                    let ms = v.parse::<u64>().map_err(|_| {
                        RedisErr::new("invalid_url", format!("Invalid timeout_ms '{}'", v))
                    })?;
                    if ms == 0 {
                        return Err(RedisErr::new("invalid_url", "timeout_ms must be > 0"));
                    }
                    timeout_ms = ms;
                }
                "tls_verify" => {
                    if !is_tls {
                        return Err(RedisErr::new(
                            "invalid_url",
                            "tls_verify is only supported with rediss://",
                        ));
                    }
                    tls_verify = match v {
                        "full" => VerifyMode::VerifyFull,
                        "ca" => VerifyMode::VerifyCa,
                        "none" => VerifyMode::Disable,
                        _ => {
                            return Err(RedisErr::new(
                                "invalid_url",
                                format!("Invalid tls_verify '{}' (expected full, ca, or none)", v),
                            ));
                        }
                    };
                }
                "ca_file" => {
                    if !is_tls {
                        return Err(RedisErr::new(
                            "invalid_url",
                            "ca_file is only supported with rediss://",
                        ));
                    }
                    let decoded = percent_decode(v, false)
                        .map_err(|_| RedisErr::new("invalid_url", "Invalid ca_file encoding"))?;
                    if decoded.is_empty() {
                        return Err(RedisErr::new("invalid_url", "ca_file cannot be empty"));
                    }
                    ca_file = Some(decoded);
                }
                _ => {
                    return Err(RedisErr::new(
                        "invalid_url",
                        format!("Unsupported Redis URL query option '{}'", k),
                    ));
                }
            }
        }
    }

    let (authority, path_part) = match before_query.split_once('/') {
        Some((a, p)) => (a, p),
        None => (before_query, ""),
    };

    let db = if path_part.is_empty() {
        0u32
    } else {
        let trimmed = path_part.trim_end_matches('/');
        if trimmed.is_empty() {
            0u32
        } else {
            trimmed.parse::<u32>().map_err(|_| {
                RedisErr::new("invalid_url", format!("Invalid Redis DB index '/{}'", path_part))
            })?
        }
    };

    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, hp)) => (Some(u), hp),
        None => (None, authority),
    };

    let (username, password) = match userinfo {
        None => (None, None),
        Some(ui) => match ui.split_once(':') {
            Some((u, p)) => {
                let u_dec = if u.is_empty() {
                    None
                } else {
                    Some(
                        percent_decode(u, false)
                            .map_err(|_| RedisErr::new("invalid_url", "Invalid username encoding"))?,
                    )
                };
                let p_dec = percent_decode(p, false)
                    .map_err(|_| RedisErr::new("invalid_url", "Invalid password encoding"))?;
                (u_dec, Some(p_dec))
            }
            None => {
                let p_dec = percent_decode(ui, false)
                    .map_err(|_| RedisErr::new("invalid_url", "Invalid password encoding"))?;
                (None, Some(p_dec))
            }
        },
    };

    let (host, port) = if let Some(bracket_rest) = hostport.strip_prefix('[') {
        let Some((ipv6, after)) = bracket_rest.split_once(']') else {
            return Err(RedisErr::new("invalid_url", "Unterminated IPv6 literal in Redis URL"));
        };
        if ipv6.is_empty() {
            return Err(RedisErr::new("invalid_url", "Empty IPv6 host in Redis URL"));
        }
        let port = if after.is_empty() {
            6379u16
        } else if let Some(p_str) = after.strip_prefix(':') {
            let p = p_str
                .parse::<u16>()
                .map_err(|_| RedisErr::new("invalid_url", "Invalid Redis port"))?;
            if p == 0 {
                return Err(RedisErr::new("invalid_url", "Redis port must be > 0"));
            }
            p
        } else {
            return Err(RedisErr::new("invalid_url", "Malformed IPv6 host:port in Redis URL"));
        };
        (ipv6.to_string(), port)
    } else if let Some((h, p_str)) = hostport.rsplit_once(':') {
        let p = p_str
            .parse::<u16>()
            .map_err(|_| RedisErr::new("invalid_url", format!("Invalid Redis port '{}'", p_str)))?;
        if p == 0 {
            return Err(RedisErr::new("invalid_url", "Redis port must be > 0"));
        }
        (h.to_string(), p)
    } else {
        (hostport.to_string(), 6379u16)
    };

    if host.is_empty() {
        return Err(RedisErr::new("invalid_url", "Missing host in Redis URL"));
    }

    let tls = if is_tls {
        Some(RedisTlsConfig {
            verify: tls_verify,
            ca_file,
        })
    } else {
        None
    };

    Ok(RedisConfig {
        is_mock,
        host,
        port,
        username,
        password,
        db,
        timeout_ms,
        tls,
    })
}

// ── RESP2 Wire Codec ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespValue {
    Simple(String),
    Error(String),
    Int(i64),
    Bulk(Vec<u8>),
    Null,
    Array(Vec<RespValue>),
}

pub fn encode_resp_command(args: &[&[u8]]) -> Vec<u8> {
    let approx = args.iter().map(|a| a.len() + 12).sum::<usize>() + 16;
    let mut out = Vec::with_capacity(approx);
    let _ = write!(out, "*{}\r\n", args.len());
    for arg in args {
        let _ = write!(out, "${}\r\n", arg.len());
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn read_crlf_line<R: BufRead>(r: &mut R) -> Result<String, RedisErr> {
    let mut line = String::new();
    let n = r
        .read_line(&mut line)
        .map_err(|e| RedisErr::new("io", format!("Redis read failed: {}", e)))?;
    if n == 0 {
        return Err(RedisErr::new("io", "Redis connection closed unexpectedly (EOF)"));
    }
    if !line.ends_with("\r\n") {
        return Err(RedisErr::new("protocol", "RESP2 frame line missing CRLF terminator"));
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

pub fn read_resp<R: BufRead>(r: &mut R, depth: usize) -> Result<RespValue, RedisErr> {
    if depth > MAX_RESP_DEPTH {
        return Err(RedisErr::new("protocol", "RESP2 nesting depth exceeded"));
    }
    let line = read_crlf_line(r)?;
    let (prefix, rest) = line
        .as_bytes()
        .split_first()
        .ok_or_else(|| RedisErr::new("protocol", "Empty RESP2 line"))?;
    let body = std::str::from_utf8(rest)
        .map_err(|_| RedisErr::new("protocol", "Invalid UTF-8 in RESP2 header"))?;

    match prefix {
        b'+' => Ok(RespValue::Simple(body.to_string())),
        b'-' => Ok(RespValue::Error(body.to_string())),
        b':' => {
            let n = body
                .parse::<i64>()
                .map_err(|_| RedisErr::new("protocol", format!("Invalid RESP2 integer '{}'", body)))?;
            Ok(RespValue::Int(n))
        }
        b'$' => {
            let len = body
                .parse::<i64>()
                .map_err(|_| RedisErr::new("protocol", format!("Invalid RESP2 bulk length '{}'", body)))?;
            if len == -1 {
                return Ok(RespValue::Null);
            }
            if !(0..=MAX_RESP_BULK_BYTES).contains(&len) {
                return Err(RedisErr::new("protocol", format!("Invalid RESP2 bulk length {}", len)));
            }
            let mut buf = vec![0u8; len as usize];
            r.read_exact(&mut buf)
                .map_err(|e| RedisErr::new("io", format!("Failed reading RESP2 bulk string: {}", e)))?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf)
                .map_err(|e| RedisErr::new("io", format!("Failed reading RESP2 bulk CRLF: {}", e)))?;
            if crlf != *b"\r\n" {
                return Err(RedisErr::new("protocol", "RESP2 bulk string missing trailing CRLF"));
            }
            Ok(RespValue::Bulk(buf))
        }
        b'*' => {
            let len = body
                .parse::<i64>()
                .map_err(|_| RedisErr::new("protocol", format!("Invalid RESP2 array length '{}'", body)))?;
            if len == -1 {
                return Ok(RespValue::Null);
            }
            if !(0..=MAX_RESP_ARRAY_LEN).contains(&len) {
                return Err(RedisErr::new("protocol", format!("Invalid RESP2 array length {}", len)));
            }
            let mut items = Vec::with_capacity((len as usize).min(1024));
            for _ in 0..len {
                items.push(read_resp(r, depth + 1)?);
            }
            Ok(RespValue::Array(items))
        }
        other => Err(RedisErr::new(
            "protocol",
            format!("Unknown RESP2 prefix byte 0x{:02x}", other),
        )),
    }
}

pub fn encode_resp_response(val: &RespValue, out: &mut Vec<u8>) {
    match val {
        RespValue::Simple(s) => {
            out.push(b'+');
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        RespValue::Error(s) => {
            out.push(b'-');
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        RespValue::Int(n) => {
            let _ = write!(out, ":{}\r\n", n);
        }
        RespValue::Null => out.extend_from_slice(b"$-1\r\n"),
        RespValue::Bulk(b) => {
            let _ = write!(out, "${}\r\n", b.len());
            out.extend_from_slice(b);
            out.extend_from_slice(b"\r\n");
        }
        RespValue::Array(items) => {
            let _ = write!(out, "*{}\r\n", items.len());
            for item in items {
                encode_resp_response(item, out);
            }
        }
    }
}

// ── Stream Abstraction + Test Mock Wire-Protocol Server ─────────────────────

enum RedisStream {
    Tcp(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    #[cfg(test)]
    Mock(MockRedisStream),
}

impl Read for RedisStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            RedisStream::Tcp(s) => s.read(buf),
            RedisStream::Tls(s) => s.read(buf),
            #[cfg(test)]
            RedisStream::Mock(m) => m.read(buf),
        }
    }
}

impl Write for RedisStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            RedisStream::Tcp(s) => s.write(buf),
            RedisStream::Tls(s) => s.write(buf),
            #[cfg(test)]
            RedisStream::Mock(m) => m.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            RedisStream::Tcp(s) => s.flush(),
            RedisStream::Tls(s) => s.flush(),
            #[cfg(test)]
            RedisStream::Mock(m) => m.flush(),
        }
    }
}

#[cfg(test)]
mod mock_server {
    use std::collections::{HashSet, VecDeque};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn now_ms() -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis().min(i64::MAX as u128) as i64,
            Err(_) => 0,
        }
    }

    #[derive(Default)]
    struct MockDb {
        strings: HashMap<String, (String, Option<i64>)>,
        hashes: HashMap<String, HashMap<String, String>>,
        lists: HashMap<String, VecDeque<String>>,
        sets: HashMap<String, HashSet<String>>,
    }

    impl MockDb {
        fn purge_if_expired(&mut self, key: &str) {
            let now = now_ms();
            if self
                .strings
                .get(key)
                .is_some_and(|(_, exp)| exp.is_some_and(|e| now >= e))
            {
                self.strings.remove(key);
            }
        }

        fn key_exists(&mut self, key: &str) -> bool {
            self.purge_if_expired(key);
            self.strings.contains_key(key)
                || self.hashes.contains_key(key)
                || self.lists.contains_key(key)
                || self.sets.contains_key(key)
        }

        fn del_key(&mut self, key: &str) -> bool {
            self.purge_if_expired(key);
            let mut removed = false;
            removed |= self.strings.remove(key).is_some();
            removed |= self.hashes.remove(key).is_some();
            removed |= self.lists.remove(key).is_some();
            removed |= self.sets.remove(key).is_some();
            removed
        }
    }

    pub(super) struct MockServerState {
        required_pass: Option<String>,
        dbs: HashMap<u32, MockDb>,
    }

    impl MockServerState {
        fn db_mut(&mut self, db: u32) -> &mut MockDb {
            self.dbs.entry(db).or_default()
        }

        fn exec_cmd(&mut self, cur_db: &mut u32, authed: &mut bool, args: &[String]) -> RespValue {
            if args.is_empty() {
                return RespValue::Error("ERR empty command".to_string());
            }
            let cmd = args[0].to_ascii_uppercase();
            if cmd == "AUTH" {
                let pass = if args.len() == 2 {
                    &args[1]
                } else if args.len() == 3 {
                    &args[2]
                } else {
                    return RespValue::Error("ERR wrong number of arguments for 'auth'".to_string());
                };
                if let Some(req) = &self.required_pass {
                    if req == pass {
                        *authed = true;
                        return RespValue::Simple("OK".to_string());
                    }
                    return RespValue::Error("WRONGPASS invalid username-password pair".to_string());
                }
                *authed = true;
                return RespValue::Simple("OK".to_string());
            }
            if self.required_pass.is_some() && !*authed {
                return RespValue::Error("NOAUTH Authentication required.".to_string());
            }
            match cmd.as_str() {
                "SELECT" => {
                    if args.len() != 2 {
                        return RespValue::Error("ERR wrong number of arguments".to_string());
                    }
                    match args[1].parse::<u32>() {
                        Ok(d) => {
                            *cur_db = d;
                            RespValue::Simple("OK".to_string())
                        }
                        Err(_) => RespValue::Error("ERR value is not an integer or out of range".to_string()),
                    }
                }
                "PING" => RespValue::Simple(
                    args.get(1).cloned().unwrap_or_else(|| "PONG".to_string()),
                ),
                "GET" => {
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(&args[1]);
                    match db.strings.get(&args[1]) {
                        Some((v, _)) => RespValue::Bulk(v.as_bytes().to_vec()),
                        None => RespValue::Null,
                    }
                }
                "SET" => {
                    if args.len() < 3 {
                        return RespValue::Error("ERR wrong number of arguments".to_string());
                    }
                    let key = &args[1];
                    let val = &args[2];
                    let mut nx = false;
                    let mut exp: Option<i64> = None;
                    let mut i = 3usize;
                    while i < args.len() {
                        match args[i].to_ascii_uppercase().as_str() {
                            "NX" => {
                                nx = true;
                                i += 1;
                            }
                            "EX" => {
                                if i + 1 >= args.len() {
                                    return RespValue::Error("ERR syntax error".to_string());
                                }
                                let secs = match args[i + 1].parse::<i64>() {
                                    Ok(s) if s > 0 => s,
                                    _ => return RespValue::Error("ERR invalid expire time in 'set'".to_string()),
                                };
                                exp = Some(now_ms().saturating_add(secs.saturating_mul(1000)));
                                i += 2;
                            }
                            "PX" => {
                                if i + 1 >= args.len() {
                                    return RespValue::Error("ERR syntax error".to_string());
                                }
                                let ms = match args[i + 1].parse::<i64>() {
                                    Ok(m) if m > 0 => m,
                                    _ => return RespValue::Error("ERR invalid expire time in 'set'".to_string()),
                                };
                                exp = Some(now_ms().saturating_add(ms));
                                i += 2;
                            }
                            _ => return RespValue::Error("ERR syntax error".to_string()),
                        }
                    }
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(key);
                    if nx && db.key_exists(key) {
                        return RespValue::Null;
                    }
                    db.hashes.remove(key);
                    db.lists.remove(key);
                    db.sets.remove(key);
                    db.strings.insert(key.clone(), (val.clone(), exp));
                    RespValue::Simple("OK".to_string())
                }
                "DEL" => {
                    let db = self.db_mut(*cur_db);
                    let mut count = 0i64;
                    for k in &args[1..] {
                        if db.del_key(k) {
                            count += 1;
                        }
                    }
                    RespValue::Int(count)
                }
                "EXISTS" => {
                    let db = self.db_mut(*cur_db);
                    let mut count = 0i64;
                    for k in &args[1..] {
                        if db.key_exists(k) {
                            count += 1;
                        }
                    }
                    RespValue::Int(count)
                }
                "EXPIRE" => {
                    let Ok(secs) = args[2].parse::<i64>() else {
                        return RespValue::Error("ERR value is not an integer".to_string());
                    };
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(&args[1]);
                    if let Some((_, exp)) = db.strings.get_mut(&args[1]) {
                        *exp = Some(now_ms().saturating_add(secs.saturating_mul(1000)));
                        return RespValue::Int(1);
                    }
                    RespValue::Int(0)
                }
                "TTL" => {
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(&args[1]);
                    match db.strings.get(&args[1]) {
                        None => RespValue::Int(-2),
                        Some((_, None)) => RespValue::Int(-1),
                        Some((_, Some(exp))) => {
                            let rem_secs = ((*exp - now_ms()).max(0) + 500) / 1000;
                            RespValue::Int(rem_secs)
                        }
                    }
                }
                "INCR" | "DECR" | "INCRBY" => {
                    let delta = match cmd.as_str() {
                        "INCR" => 1i64,
                        "DECR" => -1i64,
                        _ => match args[2].parse::<i64>() {
                            Ok(d) => d,
                            Err(_) => return RespValue::Error("ERR value is not an integer".to_string()),
                        },
                    };
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(&args[1]);
                    let cur = match db.strings.get(&args[1]) {
                        Some((s, _)) => match s.parse::<i64>() {
                            Ok(n) => n,
                            Err(_) => return RespValue::Error("ERR value is not an integer or out of range".to_string()),
                        },
                        None => 0,
                    };
                    let next = cur.saturating_add(delta);
                    let exp = db.strings.get(&args[1]).and_then(|(_, e)| *e);
                    db.strings.insert(args[1].clone(), (next.to_string(), exp));
                    RespValue::Int(next)
                }
                "KEYS" => {
                    let pat = &args[1];
                    let db = self.db_mut(*cur_db);
                    let now = now_ms();
                    db.strings.retain(|_, (_, exp)| !exp.is_some_and(|e| now >= e));
                    let mut keys: HashSet<String> = HashSet::new();
                    keys.extend(db.strings.keys().cloned());
                    keys.extend(db.hashes.keys().cloned());
                    keys.extend(db.lists.keys().cloned());
                    keys.extend(db.sets.keys().cloned());
                    let mut out: Vec<String> = keys
                        .into_iter()
                        .filter(|k| glob_match(pat, k))
                        .collect();
                    out.sort();
                    RespValue::Array(out.into_iter().map(|s| RespValue::Bulk(s.into_bytes())).collect())
                }
                "HSET" => {
                    let db = self.db_mut(*cur_db);
                    let h = db.hashes.entry(args[1].clone()).or_default();
                    let inserted = h.insert(args[2].clone(), args[3].clone()).is_none();
                    RespValue::Int(i64::from(inserted))
                }
                "HGET" => {
                    let db = self.db_mut(*cur_db);
                    match db.hashes.get(&args[1]).and_then(|h| h.get(&args[2])) {
                        Some(v) => RespValue::Bulk(v.as_bytes().to_vec()),
                        None => RespValue::Null,
                    }
                }
                "HGETALL" => {
                    let db = self.db_mut(*cur_db);
                    let mut items = Vec::new();
                    if let Some(h) = db.hashes.get(&args[1]) {
                        let mut sorted: Vec<(&String, &String)> = h.iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(b.0));
                        for (k, v) in sorted {
                            items.push(RespValue::Bulk(k.as_bytes().to_vec()));
                            items.push(RespValue::Bulk(v.as_bytes().to_vec()));
                        }
                    }
                    RespValue::Array(items)
                }
                "HDEL" => {
                    let db = self.db_mut(*cur_db);
                    let removed = db
                        .hashes
                        .get_mut(&args[1])
                        .is_some_and(|h| h.remove(&args[2]).is_some());
                    RespValue::Int(i64::from(removed))
                }
                "LPUSH" => {
                    let db = self.db_mut(*cur_db);
                    let l = db.lists.entry(args[1].clone()).or_default();
                    l.push_front(args[2].clone());
                    RespValue::Int(l.len() as i64)
                }
                "RPUSH" => {
                    let db = self.db_mut(*cur_db);
                    let l = db.lists.entry(args[1].clone()).or_default();
                    l.push_back(args[2].clone());
                    RespValue::Int(l.len() as i64)
                }
                "LPOP" => {
                    let db = self.db_mut(*cur_db);
                    match db.lists.get_mut(&args[1]).and_then(|l| l.pop_front()) {
                        Some(v) => RespValue::Bulk(v.into_bytes()),
                        None => RespValue::Null,
                    }
                }
                "RPOP" => {
                    let db = self.db_mut(*cur_db);
                    match db.lists.get_mut(&args[1]).and_then(|l| l.pop_back()) {
                        Some(v) => RespValue::Bulk(v.into_bytes()),
                        None => RespValue::Null,
                    }
                }
                "LRANGE" => {
                    let db = self.db_mut(*cur_db);
                    let Some(l) = db.lists.get(&args[1]) else {
                        return RespValue::Array(Vec::new());
                    };
                    let n = l.len() as i64;
                    let Ok(mut s) = args[2].parse::<i64>() else {
                        return RespValue::Error("ERR value is not an integer".to_string());
                    };
                    let Ok(mut e) = args[3].parse::<i64>() else {
                        return RespValue::Error("ERR value is not an integer".to_string());
                    };
                    if s < 0 {
                        s += n;
                    }
                    if e < 0 {
                        e += n;
                    }
                    s = s.max(0);
                    e = e.min(n - 1);
                    let mut out = Vec::new();
                    if s <= e && n > 0 {
                        for idx in (s as usize)..=(e as usize) {
                            if let Some(v) = l.get(idx) {
                                out.push(RespValue::Bulk(v.as_bytes().to_vec()));
                            }
                        }
                    }
                    RespValue::Array(out)
                }
                "SADD" => {
                    let db = self.db_mut(*cur_db);
                    let set = db.sets.entry(args[1].clone()).or_default();
                    RespValue::Int(i64::from(set.insert(args[2].clone())))
                }
                "SREM" => {
                    let db = self.db_mut(*cur_db);
                    let removed = db.sets.get_mut(&args[1]).is_some_and(|s| s.remove(&args[2]));
                    RespValue::Int(i64::from(removed))
                }
                "SMEMBERS" => {
                    let db = self.db_mut(*cur_db);
                    let mut out: Vec<String> = db
                        .sets
                        .get(&args[1])
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    out.sort();
                    RespValue::Array(out.into_iter().map(|v| RespValue::Bulk(v.into_bytes())).collect())
                }
                "SISMEMBER" => {
                    let db = self.db_mut(*cur_db);
                    let has = db.sets.get(&args[1]).is_some_and(|s| s.contains(&args[2]));
                    RespValue::Int(i64::from(has))
                }
                "EVAL" => {
                    if args.len() < 4 {
                        return RespValue::Error("ERR wrong number of arguments for 'eval'".to_string());
                    }
                    let script = &args[1];
                    let numkeys = args[2].parse::<usize>().unwrap_or(0);
                    if numkeys != 1 || args.len() < 5 {
                        return RespValue::Error("ERR invalid mock EVAL args".to_string());
                    }
                    let key = &args[3];
                    let token = &args[4];
                    let db = self.db_mut(*cur_db);
                    db.purge_if_expired(key);
                    let owner_matches = db
                        .strings
                        .get(key)
                        .is_some_and(|(cur, _)| cur == token);
                    if !owner_matches {
                        return RespValue::Int(0);
                    }
                    if script == LUA_UNLOCK {
                        db.strings.remove(key);
                        RespValue::Int(1)
                    } else if script == LUA_EXTEND {
                        let Ok(ms) = args.get(5).and_then(|s| s.parse::<i64>().ok()).ok_or(()) else {
                            return RespValue::Error("ERR invalid ttl in LUA_EXTEND".to_string());
                        };
                        if ms <= 0 {
                            return RespValue::Int(0);
                        }
                        if let Some((_, exp)) = db.strings.get_mut(key) {
                            *exp = Some(now_ms().saturating_add(ms));
                            RespValue::Int(1)
                        } else {
                            RespValue::Int(0)
                        }
                    } else {
                        RespValue::Error("ERR unsupported mock script".to_string())
                    }
                }
                other => RespValue::Error(format!("ERR unknown command '{}'", other)),
            }
        }
    }

    fn glob_match(pat: &str, text: &str) -> bool {
        let p = pat.as_bytes();
        let t = text.as_bytes();
        let (mut pi, mut ti) = (0usize, 0usize);
        let (mut star, mut match_t) = (None, 0usize);
        while ti < t.len() {
            if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
                pi += 1;
                ti += 1;
            } else if pi < p.len() && p[pi] == b'*' {
                star = Some(pi);
                match_t = ti;
                pi += 1;
            } else if let Some(sp) = star {
                pi = sp + 1;
                match_t += 1;
                ti = match_t;
            } else {
                return false;
            }
        }
        while pi < p.len() && p[pi] == b'*' {
            pi += 1;
        }
        pi == p.len()
    }

    pub(super) struct MockRedisStream {
        server: Arc<Mutex<MockServerState>>,
        cur_db: u32,
        authed: bool,
        in_buf: Vec<u8>,
        out_buf: Vec<u8>,
        out_pos: usize,
    }

    impl MockRedisStream {
        pub(super) fn connect(cfg: &RedisConfig) -> Self {
            static MOCK_SERVERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<MockServerState>>>>> =
                OnceLock::new();
            let reg = MOCK_SERVERS.get_or_init(|| Mutex::new(HashMap::new()));
            let mut guard = reg.lock().unwrap_or_else(|e| e.into_inner());
            let key = format!("{}:{}", cfg.host, cfg.port);
            let server = guard
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(Mutex::new(MockServerState {
                        required_pass: cfg.password.clone(),
                        dbs: HashMap::new(),
                    }))
                })
                .clone();
            Self {
                server,
                cur_db: 0,
                authed: false,
                in_buf: Vec::new(),
                out_buf: Vec::new(),
                out_pos: 0,
            }
        }
    }

    impl Read for MockRedisStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let avail = self.out_buf.len().saturating_sub(self.out_pos);
            if avail == 0 {
                return Ok(0);
            }
            let n = avail.min(buf.len());
            buf[..n].copy_from_slice(&self.out_buf[self.out_pos..self.out_pos + n]);
            self.out_pos += n;
            if self.out_pos == self.out_buf.len() {
                self.out_buf.clear();
                self.out_pos = 0;
            }
            Ok(n)
        }
    }

    impl Write for MockRedisStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.in_buf.extend_from_slice(buf);
            while !self.in_buf.is_empty() {
                let consumed = {
                    let mut cursor = std::io::Cursor::new(&self.in_buf);
                    let Ok(RespValue::Array(items)) = read_resp(&mut cursor, 0) else {
                        break;
                    };
                    let args: Vec<String> = items
                        .into_iter()
                        .filter_map(|v| match v {
                            RespValue::Bulk(b) => Some(String::from_utf8_lossy(&b).into_owned()),
                            RespValue::Simple(s) => Some(s),
                            _ => None,
                        })
                        .collect();
                    let resp = self
                        .server
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .exec_cmd(&mut self.cur_db, &mut self.authed, &args);
                    encode_resp_response(&resp, &mut self.out_buf);
                    cursor.position() as usize
                };
                self.in_buf.drain(..consumed);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
use mock_server::MockRedisStream;

// ── Redis Connection & High-Level Operations ────────────────────────────────

pub(crate) struct RedisConn {
    stream: BufReader<RedisStream>,
}

impl RedisConn {
    pub(crate) fn open(url: &str) -> Result<Self, RedisErr> {
        let cfg = parse_redis_url(url)?;
        let timeout = Duration::from_millis(cfg.timeout_ms);
        let raw_stream = if cfg.is_mock {
            #[cfg(test)]
            {
                RedisStream::Mock(MockRedisStream::connect(&cfg))
            }
            #[cfg(not(test))]
            {
                return Err(RedisErr::new("invalid_url", "redis-mock:// is only available in tests"));
            }
        } else {
            let addr_str = format!("{}:{}", cfg.host, cfg.port);
            let addrs: Vec<_> = addr_str
                .to_socket_addrs()
                .map_err(|e| RedisErr::new("connect", format!("DNS lookup failed: {}", e)))?
                .collect();
            let first = addrs
                .first()
                .ok_or_else(|| RedisErr::new("connect", "No addresses resolved for Redis host"))?;
            let tcp = TcpStream::connect_timeout(first, timeout)
                .map_err(|e| RedisErr::new("connect", format!("Redis connect failed: {}", e)))?;
            let _ = tcp.set_read_timeout(Some(timeout));
            let _ = tcp.set_write_timeout(Some(timeout));
            let _ = tcp.set_nodelay(true);
            if let Some(tls_cfg) = &cfg.tls {
                let mut tcfg = TlsConfig::new(&cfg.host);
                tcfg.verify = tls_cfg.verify;
                tcfg.min_version = TlsVersion::Tls12;
                tcfg.handshake_timeout = timeout;
                if let Some(ca) = &tls_cfg.ca_file {
                    tcfg.roots = RootSource::File(PathBuf::from(ca));
                }
                let tls_stream = crate::native::tls::connect(tcp, &tcfg)
                    .map_err(|e| RedisErr::new("tls_error", format!("Redis TLS handshake failed: {e}")))?;
                RedisStream::Tls(Box::new(tls_stream))
            } else {
                RedisStream::Tcp(tcp)
            }
        };

        let mut conn = Self {
            stream: BufReader::new(raw_stream),
        };

        if let Some(pass) = &cfg.password {
            let res = if let Some(user) = &cfg.username {
                conn.send_cmd(&[b"AUTH", user.as_bytes(), pass.as_bytes()])
            } else {
                conn.send_cmd(&[b"AUTH", pass.as_bytes()])
            };
            if let Err(e) = res {
                return Err(RedisErr::new("auth", e.message));
            }
        }
        if cfg.db != 0 {
            let db_s = cfg.db.to_string();
            conn.send_cmd(&[b"SELECT", db_s.as_bytes()])?;
        }
        Ok(conn)
    }

    pub(crate) fn send_cmd(&mut self, args: &[&[u8]]) -> Result<RespValue, RedisErr> {
        let wire = encode_resp_command(args);
        self.stream
            .get_mut()
            .write_all(&wire)
            .and_then(|()| self.stream.get_mut().flush())
            .map_err(|e| RedisErr::new("io", format!("Redis write failed: {}", e)))?;
        let resp = read_resp(&mut self.stream, 0)?;
        if let RespValue::Error(msg) = resp {
            return Err(RedisErr::new("redis_error", msg));
        }
        Ok(resp)
    }
}

pub(crate) fn resp_to_opt_string(resp: RespValue) -> Result<Option<String>, RedisErr> {
    match resp {
        RespValue::Null => Ok(None),
        RespValue::Bulk(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
        RespValue::Simple(s) => Ok(Some(s)),
        RespValue::Int(n) => Ok(Some(n.to_string())),
        other => Err(RedisErr::new(
            "protocol",
            format!("Expected string/null RESP2 reply, got {:?}", other),
        )),
    }
}

pub(crate) fn resp_to_int(resp: RespValue) -> Result<i64, RedisErr> {
    match resp {
        RespValue::Int(n) => Ok(n),
        other => Err(RedisErr::new(
            "protocol",
            format!("Expected integer RESP2 reply, got {:?}", other),
        )),
    }
}

pub(crate) fn resp_to_string_list(resp: RespValue) -> Result<Vec<String>, RedisErr> {
    match resp {
        RespValue::Null => Ok(Vec::new()),
        RespValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    RespValue::Bulk(b) => out.push(String::from_utf8_lossy(&b).into_owned()),
                    RespValue::Simple(s) => out.push(s),
                    RespValue::Int(n) => out.push(n.to_string()),
                    RespValue::Null => out.push(String::new()),
                    other => {
                        return Err(RedisErr::new(
                            "protocol",
                            format!("Expected string element in RESP2 array, got {:?}", other),
                        ));
                    }
                }
            }
            Ok(out)
        }
        other => Err(RedisErr::new(
            "protocol",
            format!("Expected array RESP2 reply, got {:?}", other),
        )),
    }
}

fn format_resp_array_json(items: &[RespValue], out: &mut Vec<u8>) {
    out.push(b'[');
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push(b',');
        }
        match item {
            RespValue::Null => out.extend_from_slice(b"null"),
            RespValue::Int(n) => {
                let _ = write!(out, "{}", n);
            }
            RespValue::Simple(s) => write_escaped_bytes(s, out),
            RespValue::Error(e) => write_escaped_bytes(e, out),
            RespValue::Bulk(b) => write_escaped_bytes(&String::from_utf8_lossy(b), out),
            RespValue::Array(sub) => format_resp_array_json(sub, out),
        }
    }
    out.push(b']');
}

pub(crate) fn resp_cmd_to_opt_string(resp: RespValue) -> Result<Option<String>, RedisErr> {
    match resp {
        RespValue::Null => Ok(None),
        RespValue::Simple(s) => Ok(Some(s)),
        RespValue::Int(n) => Ok(Some(n.to_string())),
        RespValue::Bulk(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
        RespValue::Array(items) => {
            let mut buf = Vec::with_capacity(items.len() * 16 + 8);
            format_resp_array_json(&items, &mut buf);
            Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
        }
        RespValue::Error(msg) => Err(RedisErr::new("redis_error", msg)),
    }
}

pub(crate) fn lock_acquire_conn(
    conn: &mut RedisConn,
    key: &str,
    ttl_ms: i64,
) -> Result<Option<String>, RedisErr> {
    if ttl_ms <= 0 {
        return Err(RedisErr::new(
            "invalid_input",
            "Redis lock ttl_ms must be positive",
        ));
    }
    let mut rnd = [0u8; 16];
    fill_random_bytes(&mut rnd).map_err(|e| RedisErr::new("io", e))?;
    let token = encode_hex(&rnd);
    let ttl_s = ttl_ms.to_string();
    let resp = conn.send_cmd(&[
        b"SET",
        key.as_bytes(),
        token.as_bytes(),
        b"NX",
        b"PX",
        ttl_s.as_bytes(),
    ])?;
    match resp {
        RespValue::Simple(s) if s == "OK" => Ok(Some(token)),
        RespValue::Null => Ok(None),
        other => Err(RedisErr::new(
            "protocol",
            format!("Unexpected SET NX reply: {:?}", other),
        )),
    }
}

pub(crate) fn lock_unlock_conn(
    conn: &mut RedisConn,
    key: &str,
    token: &str,
) -> Result<bool, RedisErr> {
    let resp = conn.send_cmd(&[
        b"EVAL",
        LUA_UNLOCK.as_bytes(),
        b"1",
        key.as_bytes(),
        token.as_bytes(),
    ])?;
    Ok(resp_to_int(resp)? > 0)
}

pub(crate) fn lock_extend_conn(
    conn: &mut RedisConn,
    key: &str,
    token: &str,
    ttl_ms: i64,
) -> Result<bool, RedisErr> {
    if ttl_ms <= 0 {
        return Err(RedisErr::new(
            "invalid_input",
            "Redis lock ttl_ms must be positive",
        ));
    }
    let ttl_s = ttl_ms.to_string();
    let resp = conn.send_cmd(&[
        b"EVAL",
        LUA_EXTEND.as_bytes(),
        b"1",
        key.as_bytes(),
        token.as_bytes(),
        ttl_s.as_bytes(),
    ])?;
    Ok(resp_to_int(resp)? > 0)
}

// ── Process-Wide Thread-Safe Client Registry ────────────────────────────────

pub(crate) type ClientSlot = Arc<Mutex<Option<RedisConn>>>;

fn client_table() -> &'static Mutex<HashMap<u64, ClientSlot>> {
    static CLIENTS: OnceLock<Mutex<HashMap<u64, ClientSlot>>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn register_client(conn: RedisConn) -> u64 {
    let id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let slot = Arc::new(Mutex::new(Some(conn)));
    client_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, slot);
    id
}

pub(crate) fn get_client_slot(id: u64) -> Option<ClientSlot> {
    client_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
}

pub(crate) fn close_client(id: u64) {
    let slot = client_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    if let Some(s) = slot {
        *s.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_redis_url_variants_and_rejections() {
        let c1 = parse_redis_url("redis://localhost").unwrap();
        assert_eq!(c1.host, "localhost");
        assert_eq!(c1.port, 6379);
        assert_eq!(c1.db, 0);
        assert_eq!(c1.username, None);
        assert_eq!(c1.password, None);
        assert_eq!(c1.timeout_ms, 5000);

        let c2 = parse_redis_url("redis://alice:p%40ss@10.0.0.5:6380/3?timeout_ms=1200").unwrap();
        assert_eq!(c2.host, "10.0.0.5");
        assert_eq!(c2.port, 6380);
        assert_eq!(c2.db, 3);
        assert_eq!(c2.username.as_deref(), Some("alice"));
        assert_eq!(c2.password.as_deref(), Some("p@ss"));
        assert_eq!(c2.timeout_ms, 1200);

        let c3 = parse_redis_url("redis://:secret@[::1]:6379/1").unwrap();
        assert_eq!(c3.host, "::1");
        assert_eq!(c3.username, None);
        assert_eq!(c3.password.as_deref(), Some("secret"));
        assert_eq!(c3.db, 1);

        assert!(parse_redis_url("http://localhost:6379").is_err());
        assert!(parse_redis_url("redis://").is_err());
        assert!(parse_redis_url("redis://localhost:0").is_err());
        assert!(parse_redis_url("redis://localhost/bad").is_err());

        let c_tls = parse_redis_url(
            "rediss://default:tok@redis.example.com:6380/0?tls_verify=ca&ca_file=/tmp/ca.pem",
        )
        .unwrap();
        assert!(c_tls.tls.is_some());
        let t = c_tls.tls.unwrap();
        assert_eq!(t.verify, VerifyMode::VerifyCa);
        assert_eq!(t.ca_file.as_deref(), Some("/tmp/ca.pem"));
        assert!(parse_redis_url("redis://localhost?tls_verify=none").is_err());
    }

    #[test]
    fn resp2_roundtrip_all_frame_types() {
        let wire = encode_resp_command(&[b"SET", b"mykey", b"hello\r\nworld"]);
        assert_eq!(
            wire,
            b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$12\r\nhello\r\nworld\r\n"
        );

        let payload = b"+OK\r\n-ERR nope\r\n:42\r\n$-1\r\n$3\r\nfoo\r\n*2\r\n$1\r\na\r\n:9\r\n";
        let mut r = BufReader::new(Cursor::new(&payload[..]));
        assert_eq!(read_resp(&mut r, 0).unwrap(), RespValue::Simple("OK".into()));
        assert_eq!(
            read_resp(&mut r, 0).unwrap(),
            RespValue::Error("ERR nope".into())
        );
        assert_eq!(read_resp(&mut r, 0).unwrap(), RespValue::Int(42));
        assert_eq!(read_resp(&mut r, 0).unwrap(), RespValue::Null);
        assert_eq!(read_resp(&mut r, 0).unwrap(), RespValue::Bulk(b"foo".to_vec()));
        assert_eq!(
            read_resp(&mut r, 0).unwrap(),
            RespValue::Array(vec![RespValue::Bulk(b"a".to_vec()), RespValue::Int(9)])
        );
    }
}
