use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::native::crypto::sha256_bytes;
use crate::native::db::{Cell, Column, DbErr, QueryResult};
use crate::native::tls::roots::RootSource;
use crate::native::tls::{TlsConfig, TlsVersion, Transport, VerifyMode};
use crate::native::url::percent_decode;

const IO_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PACKET: usize = 16 * 1024 * 1024 - 1;

// Client capability flags (MySQL Protocol 41)
const CLIENT_LONG_PASSWORD: u32 = 0x0000_0001;
const CLIENT_FOUND_ROWS: u32 = 0x0000_0002;
const CLIENT_LONG_FLAG: u32 = 0x0000_0004;
const CLIENT_CONNECT_WITH_DB: u32 = 0x0000_0008;
const CLIENT_PROTOCOL_41: u32 = 0x0000_0200;
const CLIENT_SSL: u32 = 0x0000_0800;
const CLIENT_SECURE_CONNECTION: u32 = 0x0000_8000;
const CLIENT_MULTI_RESULTS: u32 = 0x0002_0000;
const CLIENT_PLUGIN_AUTH: u32 = 0x0008_0000;
const CLIENT_DEPRECATE_EOF: u32 = 0x0100_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MysqlSslMode {
    Disabled,
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

// MySQL field types
const MYSQL_TYPE_TINY: u8 = 0x01;
const MYSQL_TYPE_SHORT: u8 = 0x02;
const MYSQL_TYPE_LONG: u8 = 0x03;
const MYSQL_TYPE_FLOAT: u8 = 0x04;
const MYSQL_TYPE_DOUBLE: u8 = 0x05;
const MYSQL_TYPE_NULL: u8 = 0x06;
const MYSQL_TYPE_LONGLONG: u8 = 0x08;
const MYSQL_TYPE_INT24: u8 = 0x09;
const MYSQL_TYPE_YEAR: u8 = 0x0d;
const MYSQL_TYPE_VAR_STRING: u8 = 0xfd;

#[derive(Debug, Clone, PartialEq)]
pub struct MysqlConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: Option<String>,
    pub connect_timeout: Duration,
    pub prepare: bool,
    pub ssl_mode: MysqlSslMode,
    pub ssl_root_cert: Option<String>,
}

fn decode_url_part(text: &str) -> Result<String, String> {
    percent_decode(text, false).map_err(|_| "Invalid percent-encoding in MySQL URL".to_string())
}

pub fn parse_mysql_url(url: &str) -> Result<MysqlConfig, String> {
    let rest = url
        .strip_prefix("mysql://")
        .ok_or_else(|| "MySQL URL must start with mysql://".to_string())?;
    let (main, query) = match rest.split_once('?') {
        Some((m, q)) => (m, q),
        None => (rest, ""),
    };
    let (authority, path) = match main.split_once('/') {
        Some((a, p)) => (a, p),
        None => (main, ""),
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        Some(info) => match info.split_once(':') {
            Some((u, p)) => (decode_url_part(u)?, decode_url_part(p)?),
            None => (decode_url_part(info)?, String::new()),
        },
        None => (String::new(), String::new()),
    };
    if user.is_empty() {
        return Err("MySQL URL requires a username: mysql://user:password@host/database".to_string());
    }
    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        let (h, tail) = after
            .split_once(']')
            .ok_or_else(|| "Unclosed '[' in MySQL URL".to_string())?;
        (h.to_string(), tail.strip_prefix(':'))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (hostport.to_string(), None),
        }
    };
    let host = if host.is_empty() {
        "localhost".to_string()
    } else {
        decode_url_part(&host)?
    };
    let port = match port {
        None | Some("") => 3306,
        Some(t) => t
            .parse::<u16>()
            .map_err(|_| format!("Invalid MySQL port '{}'", t))?,
    };
    let database = if path.is_empty() {
        None
    } else {
        Some(decode_url_part(path)?)
    };
    let mut connect_timeout = Duration::from_secs(10);
    let mut prepare = true;
    let mut ssl_mode = MysqlSslMode::Disabled;
    let mut ssl_root_cert = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, val) = pair.split_once('=').unwrap_or((pair, ""));
        let val = decode_url_part(val)?;
        match key {
            "connect_timeout" => {
                let secs: u64 = val
                    .parse()
                    .ok()
                    .filter(|n| (1..=600).contains(n))
                    .ok_or_else(|| format!("Invalid connect_timeout '{}'", val))?;
                connect_timeout = Duration::from_secs(secs);
            }
            "prepare" => match val.as_str() {
                "off" | "false" | "0" => prepare = false,
                "on" | "true" | "1" => prepare = true,
                _ => return Err(format!("Invalid prepare '{}' (expected on or off)", val)),
            },
            "ssl-mode" | "sslmode" => {
                ssl_mode = match val.as_str() {
                    "disabled" | "disable" => MysqlSslMode::Disabled,
                    "preferred" | "prefer" => MysqlSslMode::Preferred,
                    "required" | "require" => MysqlSslMode::Required,
                    "verify_ca" | "verify-ca" => MysqlSslMode::VerifyCa,
                    "verify_identity" | "verify-full" => MysqlSslMode::VerifyIdentity,
                    _ => {
                        return Err(format!(
                            "Invalid ssl-mode '{}' (expected disabled, preferred, required, verify_ca, or verify_identity)",
                            val
                        ));
                    }
                };
            }
            "sslrootcert" | "ssl-ca" => {
                if val.is_empty() {
                    return Err("sslrootcert cannot be empty".to_string());
                }
                ssl_root_cert = Some(val);
            }
            _ => {}
        }
    }
    Ok(MysqlConfig {
        host,
        port,
        user,
        password,
        database,
        connect_timeout,
        prepare,
        ssl_mode,
        ssl_root_cert,
    })
}

// Compact SHA-1 (RFC 3174) strictly for MySQL `mysql_native_password` handshake.
fn sha1_bytes(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn mysql_native_password_scramble(password: &str, scramble: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let s1 = sha1_bytes(password.as_bytes());
    let s2 = sha1_bytes(&s1);
    let mut input = Vec::with_capacity(scramble.len() + 20);
    input.extend_from_slice(scramble);
    input.extend_from_slice(&s2);
    let s3 = sha1_bytes(&input);
    s1.iter().zip(s3.iter()).map(|(a, b)| a ^ b).collect()
}

fn caching_sha2_password_scramble(password: &str, scramble: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let s1 = sha256_bytes(password.as_bytes());
    let s2 = sha256_bytes(&s1);
    let mut input = Vec::with_capacity(32 + scramble.len());
    input.extend_from_slice(&s2);
    input.extend_from_slice(scramble);
    let s3 = sha256_bytes(&input);
    s1.iter().zip(s3.iter()).map(|(a, b)| a ^ b).collect()
}

struct PktReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PktReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn rem(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("Truncated MySQL packet".to_string());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let arr: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| "Unexpected EOF in MySQL u16".to_string())?;
        Ok(u16::from_le_bytes(arr))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let arr: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| "Unexpected EOF in MySQL u32".to_string())?;
        Ok(u32::from_le_bytes(arr))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let arr: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| "Unexpected EOF in MySQL u64".to_string())?;
        Ok(u64::from_le_bytes(arr))
    }
    fn lenenc_int(&mut self) -> Result<Option<u64>, String> {
        let b = self.u8()?;
        match b {
            0xfb => Ok(None),
            0xfc => Ok(Some(u64::from(self.u16()?))),
            0xfd => {
                let s = self.take(3)?;
                Ok(Some(u64::from(s[0]) | (u64::from(s[1]) << 8) | (u64::from(s[2]) << 16)))
            }
            0xfe => Ok(Some(self.u64()?)),
            other => Ok(Some(u64::from(other))),
        }
    }
    fn lenenc_bytes(&mut self) -> Result<Option<&'a [u8]>, String> {
        let Some(len) = self.lenenc_int()? else {
            return Ok(None);
        };
        Ok(Some(self.take(len as usize)?))
    }
    fn cstr(&mut self) -> Result<String, String> {
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| "Missing NUL terminator in MySQL packet".to_string())?;
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(text)
    }
}

fn push_lenenc_int(buf: &mut Vec<u8>, v: u64) {
    if v < 251 {
        buf.push(v as u8);
    } else if v < 65536 {
        buf.push(0xfc);
        buf.extend_from_slice(&(v as u16).to_le_bytes());
    } else if v < 16_777_216 {
        buf.push(0xfd);
        buf.push((v & 0xff) as u8);
        buf.push(((v >> 8) & 0xff) as u8);
        buf.push(((v >> 16) & 0xff) as u8);
    } else {
        buf.push(0xfe);
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

const MYSQL_STMT_CACHE_CAP: usize = 128;

#[derive(Clone, Copy)]
struct CachedMysqlStmt {
    stmt_id: u32,
    num_params: usize,
}

#[derive(Default)]
struct MysqlStmtCache {
    map: HashMap<String, CachedMysqlStmt>,
    order: VecDeque<String>,
}

impl MysqlStmtCache {
    fn get(&mut self, sql: &str) -> Option<CachedMysqlStmt> {
        let entry = *self.map.get(sql)?;
        if self.order.back().map(String::as_str) != Some(sql) {
            if let Some(pos) = self.order.iter().position(|k| k == sql) {
                self.order.remove(pos);
            }
            self.order.push_back(sql.to_string());
        }
        Some(entry)
    }

    fn insert(&mut self, sql: String, stmt: CachedMysqlStmt) -> Option<u32> {
        let mut evicted_id = None;
        if let Some(prev) = self.map.insert(sql.clone(), stmt) {
            if let Some(pos) = self.order.iter().position(|k| k == &sql) {
                self.order.remove(pos);
            }
            evicted_id = Some(prev.stmt_id);
        } else if self.order.len() >= MYSQL_STMT_CACHE_CAP
            && let Some(oldest) = self.order.pop_front()
            && let Some(old_stmt) = self.map.remove(&oldest)
        {
            evicted_id = Some(old_stmt.stmt_id);
        }
        self.order.push_back(sql);
        evicted_id
    }

    fn remove(&mut self, sql: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == sql) {
            self.order.remove(pos);
        }
        self.map.remove(sql);
    }
}

struct Conn {
    reader: BufReader<Transport>,
    seq: u8,
    deprecate_eof: bool,
    pkt_buf: Vec<u8>,
    write_buf: Vec<u8>,
    last_active: std::time::Instant,
    stmt_cache: MysqlStmtCache,
}

impl Conn {
    fn write_packet(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut offset = 0usize;
        self.write_buf.clear();
        self.write_buf.reserve(payload.len() + 4);
        loop {
            let chunk_len = (payload.len() - offset).min(MAX_PACKET);
            let hdr = [
                (chunk_len & 0xff) as u8,
                ((chunk_len >> 8) & 0xff) as u8,
                ((chunk_len >> 16) & 0xff) as u8,
                self.seq,
            ];
            self.seq = self.seq.wrapping_add(1);
            self.write_buf.extend_from_slice(&hdr);
            self.write_buf
                .extend_from_slice(&payload[offset..offset + chunk_len]);
            offset += chunk_len;
            if chunk_len < MAX_PACKET {
                break;
            }
        }
        let stream = self.reader.get_mut();
        stream
            .write_all(&self.write_buf)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("Failed to write to MySQL: {e}"))
    }

    fn read_packet_into(&mut self) -> Result<&[u8], String> {
        self.pkt_buf.clear();
        loop {
            let mut hdr = [0u8; 4];
            self.reader
                .read_exact(&mut hdr)
                .map_err(|e| format!("Failed to read MySQL packet header: {e}"))?;
            let len = usize::from(hdr[0]) | (usize::from(hdr[1]) << 8) | (usize::from(hdr[2]) << 16);
            self.seq = hdr[3].wrapping_add(1);
            let start = self.pkt_buf.len();
            self.pkt_buf.resize(start + len, 0);
            self.reader
                .read_exact(&mut self.pkt_buf[start..])
                .map_err(|e| format!("Failed to read MySQL packet body: {e}"))?;
            if len < MAX_PACKET {
                break;
            }
        }
        self.last_active = std::time::Instant::now();
        Ok(&self.pkt_buf)
    }

    fn read_packet(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.read_packet_into()?.to_vec())
    }

    fn is_alive(&mut self) -> bool {
        if self.last_active.elapsed() < Duration::from_secs(5) {
            return self.reader.buffer().is_empty();
        }
        self.reader.buffer().is_empty() && self.reader.get_mut().is_alive_probe()
    }
}

fn parse_err_packet(pkt: &[u8]) -> DbErr {
    let mut r = PktReader::new(pkt);
    let _ = r.u8(); // 0xff
    let code = r.u16().unwrap_or(0);
    let mut sqlstate = String::new();
    let msg = if r.rem() > 0 && pkt[r.pos] == b'#' {
        let _ = r.u8();
        if let Ok(s) = r.take(5) {
            sqlstate = String::from_utf8_lossy(s).into_owned();
        }
        String::from_utf8_lossy(&pkt[r.pos..]).into_owned()
    } else {
        String::from_utf8_lossy(&pkt[r.pos..]).into_owned()
    };
    DbErr::new(
        "query_error",
        format!("MySQL ({code}): {msg}"),
        if sqlstate.is_empty() {
            code.to_string()
        } else {
            sqlstate
        },
    )
}

fn parse_ok_packet(pkt: &[u8]) -> Result<(u64, u64), String> {
    let mut r = PktReader::new(pkt);
    let _ = r.u8()?; // 0x00 or 0xfe
    let affected = r.lenenc_int()?.unwrap_or(0);
    let last_id = r.lenenc_int()?.unwrap_or(0);
    Ok((affected, last_id))
}

fn is_eof_packet(pkt: &[u8]) -> bool {
    pkt.first() == Some(&0xfe) && pkt.len() < 9
}

struct MysqlCol {
    name: String,
    col_type: u8,
    flags: u16,
}

fn parse_col_def(pkt: &[u8]) -> Result<MysqlCol, String> {
    let mut r = PktReader::new(pkt);
    let _catalog = r.lenenc_bytes()?;
    let _schema = r.lenenc_bytes()?;
    let _table = r.lenenc_bytes()?;
    let _org_table = r.lenenc_bytes()?;
    let name_bytes = r.lenenc_bytes()?.unwrap_or(&[]);
    let name = String::from_utf8_lossy(name_bytes).into_owned();
    let _org_name = r.lenenc_bytes()?;
    let _fixed_len = r.u8()?; // 0x0c
    let _charset = r.u16()?;
    let _col_len = r.u32()?;
    let col_type = r.u8()?;
    let flags = r.u16()?;
    Ok(MysqlCol {
        name,
        col_type,
        flags,
    })
}

fn decode_text_cell(col: &MysqlCol, bytes: Option<&[u8]>) -> Cell {
    let Some(bytes) = bytes else {
        return Cell::Null;
    };
    match col.col_type {
        MYSQL_TYPE_TINY
        | MYSQL_TYPE_SHORT
        | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_INT24
        | MYSQL_TYPE_YEAR => std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .map(Cell::Int)
            .unwrap_or_else(|| Cell::Text(String::from_utf8_lossy(bytes).into_owned())),
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map(Cell::Float)
            .unwrap_or_else(|| Cell::Text(String::from_utf8_lossy(bytes).into_owned())),
        _ => Cell::Text(
            String::from_utf8(bytes.to_vec())
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
        ),
    }
}

fn decode_binary_cell(col: &MysqlCol, r: &mut PktReader<'_>) -> Result<Cell, String> {
    let is_unsigned = (col.flags & 0x0020) != 0;
    match col.col_type {
        MYSQL_TYPE_TINY => {
            let b = r.u8()?;
            Ok(Cell::Int(if is_unsigned {
                i64::from(b)
            } else {
                i64::from(b as i8)
            }))
        }
        MYSQL_TYPE_SHORT | MYSQL_TYPE_YEAR => {
            let v = r.u16()?;
            Ok(Cell::Int(if is_unsigned {
                i64::from(v)
            } else {
                i64::from(v as i16)
            }))
        }
        MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 => {
            let v = r.u32()?;
            Ok(Cell::Int(if is_unsigned {
                i64::from(v)
            } else {
                i64::from(v as i32)
            }))
        }
        MYSQL_TYPE_LONGLONG => {
            let v = r.u64()?;
            Ok(Cell::Int(v as i64))
        }
        MYSQL_TYPE_FLOAT => {
            let bits = r.u32()?;
            Ok(Cell::Float(f64::from(f32::from_bits(bits))))
        }
        MYSQL_TYPE_DOUBLE => {
            let bits = r.u64()?;
            Ok(Cell::Float(f64::from_bits(bits)))
        }
        _ => {
            let bytes = r.lenenc_bytes()?.unwrap_or(&[]);
            Ok(Cell::Text(String::from_utf8_lossy(bytes).into_owned()))
        }
    }
}

struct State {
    conn: Option<Conn>,
    last_insert_id: u64,
}

pub struct MysqlConn {
    config: MysqlConfig,
    state: Mutex<State>,
}

impl MysqlConn {
    pub fn connect(config: MysqlConfig) -> Result<Self, String> {
        let conn = open_connection(&config)?;
        Ok(Self {
            config,
            state: Mutex::new(State {
                conn: Some(conn),
                last_insert_id: 0,
            }),
        })
    }

    pub fn last_insert_id(&self) -> u64 {
        self.state.lock().map(|s| s.last_insert_id).unwrap_or(0)
    }

    pub fn execute(&self, sql: &str, params: &[Cell]) -> Result<u64, DbErr> {
        self.exchange(sql, params).map(|(aff, _)| aff)
    }

    pub fn query(&self, sql: &str, params: &[Cell]) -> Result<QueryResult, DbErr> {
        self.exchange(sql, params).map(|(_, res)| res)
    }

    pub fn close(&self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(mut conn) = state.conn.take()
        {
            conn.seq = 0;
            let _ = conn.write_packet(&[0x01]); // COM_QUIT
        }
    }

    fn exchange(&self, sql: &str, params: &[Cell]) -> Result<(u64, QueryResult), DbErr> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| DbErr::new("internal", "MySQL state lock poisoned", ""))?;
        if state.conn.as_mut().is_some_and(|c| !c.is_alive()) {
            state.conn = None;
        }
        if state.conn.is_none() {
            let conn = open_connection(&self.config)
                .map_err(|m| DbErr::new("connect_error", m, ""))?;
            state.conn = Some(conn);
        }
        let conn = state
            .conn
            .as_mut()
            .ok_or_else(|| DbErr::new("closed", "MySQL connection is closed", ""))?;
        let outcome = if params.is_empty() {
            run_text_query(conn, sql)
        } else {
            run_prepared_query(conn, self.config.prepare, sql, params)
        };
        match outcome {
            Ok((affected, last_id, res)) => {
                if last_id != 0 {
                    state.last_insert_id = last_id;
                }
                Ok((affected, res))
            }
            Err(MysqlFail::Server(e)) => Err(e),
            Err(MysqlFail::Broken(msg)) => {
                state.conn = None;
                Err(DbErr::new("io_error", msg, ""))
            }
        }
    }
}

impl Drop for MysqlConn {
    fn drop(&mut self) {
        self.close();
    }
}

enum MysqlFail {
    Server(DbErr),
    Broken(String),
}

impl From<String> for MysqlFail {
    fn from(s: String) -> Self {
        Self::Broken(s)
    }
}

fn run_text_query(conn: &mut Conn, sql: &str) -> Result<(u64, u64, QueryResult), MysqlFail> {
    conn.seq = 0;
    let mut pkt = Vec::with_capacity(1 + sql.len());
    pkt.push(0x03); // COM_QUERY
    pkt.extend_from_slice(sql.as_bytes());
    conn.write_packet(&pkt)?;
    read_resultset(conn, false)
}

fn run_prepared_query(
    conn: &mut Conn,
    prepare: bool,
    sql: &str,
    params: &[Cell],
) -> Result<(u64, u64, QueryResult), MysqlFail> {
    let (stmt_id, num_params, from_cache) = if prepare
        && let Some(cached) = conn.stmt_cache.get(sql)
    {
        (cached.stmt_id, cached.num_params, true)
    } else {
        conn.seq = 0;
        let mut prep = Vec::with_capacity(1 + sql.len());
        prep.push(0x16); // COM_STMT_PREPARE
        prep.extend_from_slice(sql.as_bytes());
        conn.write_packet(&prep)?;

        let ok = conn.read_packet()?;
        if ok.first() == Some(&0xff) {
            return Err(MysqlFail::Server(parse_err_packet(&ok)));
        }
        if ok.first() != Some(&0x00) {
            return Err(MysqlFail::Broken("Unexpected MySQL prepare response".to_string()));
        }
        let mut r = PktReader::new(&ok[1..]);
        let stmt_id = r.u32()?;
        let num_cols = r.u16()? as usize;
        let num_params = r.u16()? as usize;

        if num_params > 0 {
            for _ in 0..num_params {
                let _ = conn.read_packet_into()?;
            }
            if !conn.deprecate_eof {
                let _ = conn.read_packet_into()?;
            }
        }
        if num_cols > 0 {
            for _ in 0..num_cols {
                let _ = conn.read_packet_into()?;
            }
            if !conn.deprecate_eof {
                let _ = conn.read_packet_into()?;
            }
        }

        if prepare
            && let Some(evicted_id) = conn.stmt_cache.insert(
                sql.to_string(),
                CachedMysqlStmt {
                    stmt_id,
                    num_params,
                },
            )
        {
            close_stmt(conn, evicted_id);
        }
        (stmt_id, num_params, false)
    };

    if num_params != params.len() {
        if !prepare {
            close_stmt(conn, stmt_id);
        }
        return Err(MysqlFail::Server(DbErr::new(
            "query_error",
            format!(
                "MySQL query expects {} '?' parameter(s), got {}",
                num_params,
                params.len()
            ),
            "",
        )));
    }

    conn.seq = 0;
    let mut exec = Vec::with_capacity(32 + params.len() * 16);
    exec.push(0x17); // COM_STMT_EXECUTE
    exec.extend_from_slice(&stmt_id.to_le_bytes());
    exec.push(0x00); // flags: CURSOR_TYPE_NO_CURSOR
    exec.extend_from_slice(&1u32.to_le_bytes()); // iteration_count

    if !params.is_empty() {
        let null_bitmap_len = params.len().div_ceil(8);
        let null_start = exec.len();
        exec.resize(null_start + null_bitmap_len, 0);
        for (i, p) in params.iter().enumerate() {
            if matches!(p, Cell::Null) {
                exec[null_start + i / 8] |= 1 << (i % 8);
            }
        }
        exec.push(0x01); // new_params_bound_flag
        for p in params {
            match p {
                Cell::Null => {
                    exec.push(MYSQL_TYPE_NULL);
                    exec.push(0);
                }
                Cell::Int(_) => {
                    exec.push(MYSQL_TYPE_LONGLONG);
                    exec.push(0);
                }
                Cell::Float(_) => {
                    exec.push(MYSQL_TYPE_DOUBLE);
                    exec.push(0);
                }
                Cell::Bool(_) => {
                    exec.push(MYSQL_TYPE_TINY);
                    exec.push(0);
                }
                Cell::Text(_) => {
                    exec.push(MYSQL_TYPE_VAR_STRING);
                    exec.push(0);
                }
            }
        }
        for p in params {
            match p {
                Cell::Null => {}
                Cell::Int(n) => exec.extend_from_slice(&n.to_le_bytes()),
                Cell::Float(f) => exec.extend_from_slice(&f.to_bits().to_le_bytes()),
                Cell::Bool(b) => exec.push(u8::from(*b)),
                Cell::Text(s) => {
                    push_lenenc_int(&mut exec, s.len() as u64);
                    exec.extend_from_slice(s.as_bytes());
                }
            }
        }
    }

    conn.write_packet(&exec)?;
    let res = read_resultset(conn, true);
    if !prepare {
        close_stmt(conn, stmt_id);
    } else if from_cache && matches!(&res, Err(MysqlFail::Server(e)) if e.message.contains("(1243)")) {
        conn.stmt_cache.remove(sql);
    }
    res
}

fn close_stmt(conn: &mut Conn, stmt_id: u32) {
    conn.seq = 0;
    let mut pkt = [0u8; 5];
    pkt[0] = 0x19; // COM_STMT_CLOSE
    pkt[1..5].copy_from_slice(&stmt_id.to_le_bytes());
    let _ = conn.write_packet(&pkt);
}

fn read_resultset(conn: &mut Conn, binary: bool) -> Result<(u64, u64, QueryResult), MysqlFail> {
    let first = conn.read_packet_into()?;
    if first.first() == Some(&0xff) {
        return Err(MysqlFail::Server(parse_err_packet(first)));
    }
    if first.first() == Some(&0x00) {
        let (aff, last_id) = parse_ok_packet(first)?;
        return Ok((aff, last_id, QueryResult::default()));
    }
    let mut r = PktReader::new(first);
    let col_count = r.lenenc_int()?.unwrap_or(0) as usize;
    let mut cols = Vec::with_capacity(col_count);
    for _ in 0..col_count {
        let col_pkt = conn.read_packet_into()?;
        cols.push(parse_col_def(col_pkt)?);
    }
    if !conn.deprecate_eof {
        let eof = conn.read_packet_into()?;
        if !is_eof_packet(eof) {
            return Err(MysqlFail::Broken("Expected EOF after MySQL columns".to_string()));
        }
    }

    let columns = cols
        .iter()
        .map(|c| Column {
            name: c.name.clone(),
            is_bool: c.col_type == MYSQL_TYPE_TINY,
        })
        .collect();
    let deprecate_eof = conn.deprecate_eof;
    let mut rows = Vec::new();
    loop {
        let row_pkt = conn.read_packet_into()?;
        if row_pkt.first() == Some(&0xff) {
            return Err(MysqlFail::Server(parse_err_packet(row_pkt)));
        }
        if is_eof_packet(row_pkt)
            || (deprecate_eof && row_pkt.first() == Some(&0xfe) && row_pkt.len() >= 7)
        {
            break;
        }
        if binary {
            if row_pkt.first() != Some(&0x00) {
                return Err(MysqlFail::Broken("Invalid binary row header".to_string()));
            }
            let bitmap_len = (col_count + 9) / 8;
            let mut rr = PktReader::new(&row_pkt[1..]);
            let bitmap = rr.take(bitmap_len)?;
            let mut row = Vec::with_capacity(col_count);
            for (i, col) in cols.iter().enumerate() {
                let bit_idx = i + 2;
                let is_null = (bitmap[bit_idx / 8] & (1 << (bit_idx % 8))) != 0;
                if is_null {
                    row.push(Cell::Null);
                } else {
                    row.push(decode_binary_cell(col, &mut rr)?);
                }
            }
            rows.push(row);
        } else {
            let mut rr = PktReader::new(row_pkt);
            let mut row = Vec::with_capacity(col_count);
            for col in &cols {
                let val = rr.lenenc_bytes()?;
                row.push(decode_text_cell(col, val));
            }
            rows.push(row);
        }
    }
    Ok((0, 0, QueryResult { columns, rows }))
}

fn read_initial_handshake_raw(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut hdr = [0u8; 4];
    stream
        .read_exact(&mut hdr)
        .map_err(|e| format!("Failed to read MySQL handshake header: {e}"))?;
    let len = usize::from(hdr[0]) | (usize::from(hdr[1]) << 8) | (usize::from(hdr[2]) << 16);
    if len == 0 || len > MAX_PACKET {
        return Err("Invalid MySQL initial handshake packet length".to_string());
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .map_err(|e| format!("Failed to read MySQL handshake body: {e}"))?;
    Ok((hdr[3].wrapping_add(1), body))
}

fn open_connection(config: &MysqlConfig) -> Result<Conn, String> {
    let target = format!("{}:{}", config.host, config.port);
    let addresses = target
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve MySQL host '{target}': {e}"))?;
    let mut last_error = None;
    let mut stream = None;
    for addr in addresses {
        match TcpStream::connect_timeout(&addr, config.connect_timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| match last_error {
        Some(e) => format!("Failed to connect to MySQL '{target}': {e}"),
        None => format!("No addresses resolved for '{target}'"),
    })?;
    let _ = stream.set_nodelay(true);
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let (mut seq, hs) = read_initial_handshake_raw(&mut stream)?;
    if hs.first() == Some(&0xff) {
        return Err(parse_err_packet(&hs).message);
    }
    let mut r = PktReader::new(&hs);
    let proto = r.u8()?;
    if proto != 10 {
        return Err(format!("Unsupported MySQL handshake protocol version {}", proto));
    }
    let _ver = r.cstr()?;
    let _conn_id = r.u32()?;
    let auth1 = r.take(8)?.to_vec();
    let _filler = r.u8()?;
    let cap_lo = r.u16()?;
    let _charset = r.u8()?;
    let _status = r.u16()?;
    let cap_hi = r.u16()?;
    let server_caps = u32::from(cap_lo) | (u32::from(cap_hi) << 16);
    let auth_len = r.u8()?;
    let _reserved = r.take(10)?;

    let mut scramble = auth1;
    if (server_caps & CLIENT_SECURE_CONNECTION) != 0 {
        let rest_len = usize::from(auth_len.max(21)).saturating_sub(8).min(r.rem());
        let auth2 = r.take(rest_len)?;
        let trim_end = auth2.iter().position(|&b| b == 0).unwrap_or(auth2.len());
        scramble.extend_from_slice(&auth2[..trim_end]);
    }
    let plugin_name = if (server_caps & CLIENT_PLUGIN_AUTH) != 0 && r.rem() > 0 {
        r.cstr().unwrap_or_else(|_| "mysql_native_password".to_string())
    } else {
        "mysql_native_password".to_string()
    };

    let deprecate_eof = (server_caps & CLIENT_DEPRECATE_EOF) != 0;

    let mut client_caps = CLIENT_LONG_PASSWORD
        | CLIENT_FOUND_ROWS
        | CLIENT_LONG_FLAG
        | CLIENT_PROTOCOL_41
        | CLIENT_SECURE_CONNECTION
        | CLIENT_MULTI_RESULTS
        | CLIENT_PLUGIN_AUTH;
    if deprecate_eof {
        client_caps |= CLIENT_DEPRECATE_EOF;
    }
    if config.database.is_some() {
        client_caps |= CLIENT_CONNECT_WITH_DB;
    }

    let transport = if config.ssl_mode != MysqlSslMode::Disabled {
        if (server_caps & CLIENT_SSL) == 0 {
            if config.ssl_mode != MysqlSslMode::Preferred {
                return Err("MySQL server does not support TLS".to_string());
            }
            Transport::Plain(stream)
        } else {
            client_caps |= CLIENT_SSL;
            let mut ssl_req = Vec::with_capacity(36);
            ssl_req.extend_from_slice(&32u32.to_le_bytes()[..3]);
            ssl_req.push(seq);
            seq = seq.wrapping_add(1);
            ssl_req.extend_from_slice(&client_caps.to_le_bytes());
            ssl_req.extend_from_slice(&(16 * 1024 * 1024u32).to_le_bytes());
            ssl_req.push(45);
            ssl_req.extend_from_slice(&[0u8; 23]);
            stream
                .write_all(&ssl_req)
                .and_then(|_| stream.flush())
                .map_err(|e| format!("Failed to write MySQL SSLRequest: {e}"))?;

            let verify = match config.ssl_mode {
                MysqlSslMode::Preferred | MysqlSslMode::Required => VerifyMode::Disable,
                MysqlSslMode::VerifyCa => VerifyMode::VerifyCa,
                MysqlSslMode::VerifyIdentity => VerifyMode::VerifyFull,
                MysqlSslMode::Disabled => return Err("internal error: invalid TLS mode".to_string()),
            };
            let mut tls_cfg = TlsConfig::new(&config.host);
            tls_cfg.verify = verify;
            tls_cfg.min_version = TlsVersion::Tls12;
            tls_cfg.handshake_timeout = config.connect_timeout;
            if let Some(ca_path) = &config.ssl_root_cert {
                tls_cfg.roots = RootSource::File(PathBuf::from(ca_path));
            }
            let tls_stream = crate::native::tls::connect(stream, &tls_cfg)
                .map_err(|e| format!("MySQL TLS handshake failed: {e}"))?;
            Transport::Tls(Box::new(tls_stream))
        }
    } else {
        Transport::Plain(stream)
    };

    let mut conn = Conn {
        reader: BufReader::with_capacity(64 * 1024, transport),
        seq,
        deprecate_eof,
        pkt_buf: Vec::with_capacity(4096),
        write_buf: Vec::with_capacity(1024),
        last_active: std::time::Instant::now(),
        stmt_cache: MysqlStmtCache::default(),
    };

    let auth_resp = if plugin_name == "caching_sha2_password" {
        caching_sha2_password_scramble(&config.password, &scramble)
    } else {
        mysql_native_password_scramble(&config.password, &scramble)
    };

    let mut resp = Vec::with_capacity(128);
    resp.extend_from_slice(&client_caps.to_le_bytes());
    resp.extend_from_slice(&(16 * 1024 * 1024u32).to_le_bytes());
    resp.push(45); // utf8mb4_general_ci / utf8mb4_0900_ai_ci
    resp.extend_from_slice(&[0u8; 23]);
    resp.extend_from_slice(config.user.as_bytes());
    resp.push(0);
    resp.push(auth_resp.len() as u8);
    resp.extend_from_slice(&auth_resp);
    if let Some(db) = &config.database {
        resp.extend_from_slice(db.as_bytes());
        resp.push(0);
    }
    resp.extend_from_slice(plugin_name.as_bytes());
    resp.push(0);
    conn.write_packet(&resp)?;

    loop {
        let pkt = conn.read_packet()?;
        match pkt.first() {
            Some(0x00) => return Ok(conn),
            Some(0xff) => return Err(parse_err_packet(&pkt).message),
            Some(0x01) => {
                // AuthMoreData (caching_sha2_password)
                match pkt.get(1) {
                    Some(0x03) => {} // fast auth succeeded; wait for OK_Packet
                    Some(0x04) => {
                        if matches!(conn.reader.get_ref(), Transport::Tls(_)) {
                            let mut pw_nul = Vec::with_capacity(config.password.len() + 1);
                            pw_nul.extend_from_slice(config.password.as_bytes());
                            pw_nul.push(0);
                            conn.write_packet(&pw_nul)?;
                        } else {
                            return Err(
                                "MySQL caching_sha2_password requires TLS full auth (enable ssl-mode=required or use mysql_native_password / cached user)".to_string(),
                            );
                        }
                    }
                    _ => return Err("Unexpected MySQL AuthMoreData packet".to_string()),
                }
            }
            Some(0xfe) => {
                // AuthSwitchRequest
                let mut sr = PktReader::new(&pkt[1..]);
                let new_plugin = sr.cstr()?;
                let new_scramble_raw = sr.take(sr.rem())?;
                let trim_end = new_scramble_raw
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(new_scramble_raw.len());
                let new_scramble = &new_scramble_raw[..trim_end];
                let switch_resp = if new_plugin == "caching_sha2_password" {
                    caching_sha2_password_scramble(&config.password, new_scramble)
                } else {
                    mysql_native_password_scramble(&config.password, new_scramble)
                };
                conn.write_packet(&switch_resp)?;
            }
            _ => return Err("Unexpected MySQL authentication response".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_url_parsing() {
        let cfg = parse_mysql_url("mysql://root:p%40ss@127.0.0.1:3307/app?connect_timeout=3").unwrap();
        assert_eq!(cfg.user, "root");
        assert_eq!(cfg.password, "p@ss");
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.ssl_mode, MysqlSslMode::Disabled);

        let cfg2 = parse_mysql_url(
            "mysql://root:pw@db.example.com/app?ssl-mode=verify_identity&sslrootcert=/tmp/ca.pem",
        )
        .unwrap();
        assert_eq!(cfg2.ssl_mode, MysqlSslMode::VerifyIdentity);
        assert_eq!(cfg2.ssl_root_cert.as_deref(), Some("/tmp/ca.pem"));
        assert!(parse_mysql_url("mysql://root@localhost/app?ssl-mode=invalid").is_err());
        assert_eq!(cfg.port, 3307);
        assert_eq!(cfg.database.as_deref(), Some("app"));
        assert_eq!(cfg.connect_timeout, Duration::from_secs(3));
    }
}
