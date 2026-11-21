use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::native::crypto::{
    MAX_PBKDF2_ITERATIONS, constant_time_eq, encode_base64, fill_random_bytes, hmac_sha256_bytes,
    pbkdf2_sha256, sha256_bytes,
};
use crate::native::db::{Cell, Column, DbErr, QueryResult};
use crate::native::tls::crypto::rsa::RsaHashAlg;
use crate::native::tls::crypto::sha2::{sha384_bytes, sha512_bytes};
use crate::native::tls::roots::RootSource;
use crate::native::tls::x509::{SigAlg, parse_x509};
use crate::native::tls::{TlsConfig, TlsVersion, Transport, VerifyMode};
use crate::native::url::percent_decode;

const IO_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_MESSAGE: usize = 512 * 1024 * 1024;
const PROTOCOL_3_0: i32 = 196_608;
const SSL_REQUEST_CODE: i32 = 80_877_103;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgSslMode {
    Disable,
    Allow,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PgConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub application_name: String,
    pub connect_timeout: Duration,
    pub search_path: Option<String>,
    pub prepare: bool,
    pub ssl_mode: PgSslMode,
    pub ssl_root_cert: Option<String>,
}

fn decode_url_part(text: &str) -> Result<String, String> {
    percent_decode(text, false).map_err(|_| "Invalid percent-encoding in PostgreSQL URL".to_string())
}

pub fn parse_pg_url(url: &str) -> Result<PgConfig, String> {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .ok_or_else(|| "PostgreSQL URL must start with postgres://".to_string())?;

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
            Some((u, p)) => (decode_url_part(u)?, Some(decode_url_part(p)?)),
            None => (decode_url_part(info)?, None),
        },
        None => (String::new(), None),
    };
    if user.is_empty() {
        return Err(
            "PostgreSQL URL requires a username: postgres://user:password@host/database"
                .to_string(),
        );
    }

    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        let (h, tail) = after
            .split_once(']')
            .ok_or_else(|| "Unclosed '[' in PostgreSQL URL".to_string())?;
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
        None | Some("") => 5432,
        Some(text) => text
            .parse::<u16>()
            .map_err(|_| format!("Invalid PostgreSQL port '{}'", text))?,
    };

    let database = if path.is_empty() {
        user.clone()
    } else {
        decode_url_part(path)?
    };

    let mut application_name = "conduit".to_string();
    let mut connect_timeout = Duration::from_secs(10);
    let mut search_path = None;
    let mut prepare = true;
    let mut ssl_mode = PgSslMode::Disable;
    let mut ssl_root_cert = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode_url_part(value)?;
        match key {
            "sslmode" => {
                ssl_mode = match value.as_str() {
                    "disable" => PgSslMode::Disable,
                    "allow" => PgSslMode::Allow,
                    "prefer" => PgSslMode::Prefer,
                    "require" => PgSslMode::Require,
                    "verify-ca" => PgSslMode::VerifyCa,
                    "verify-full" => PgSslMode::VerifyFull,
                    _ => {
                        return Err(format!(
                            "sslmode={value}: expected disable, allow, prefer, require, verify-ca, or verify-full"
                        ));
                    }
                };
            }
            "sslrootcert" => {
                if value.is_empty() {
                    return Err("sslrootcert cannot be empty".to_string());
                }
                ssl_root_cert = Some(value);
            }
            "application_name" => application_name = value,
            "search_path" => {
                if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return Err(format!(
                        "search_path={value}: schema name may only contain ASCII letters, digits, and '_'"
                    ));
                }
                search_path = Some(value);
            }
            "connect_timeout" => {
                let seconds: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| (1..=600).contains(n))
                    .ok_or_else(|| {
                        format!("connect_timeout={value}: expected integer seconds 1..=600")
                    })?;
                connect_timeout = Duration::from_secs(seconds);
            }
            "prepare" => match value.as_str() {
                "off" | "false" | "0" => prepare = false,
                "on" | "true" | "1" => prepare = true,
                _ => {
                    return Err(format!(
                        "prepare={value}: expected 'on' or 'off'"
                    ));
                }
            },
            other => {
                return Err(format!(
                    "Unknown PostgreSQL URL parameter '{}'; supported: sslmode, sslrootcert, application_name, connect_timeout, search_path, prepare",
                    other
                ));
            }
        }
    }

    Ok(PgConfig {
        host,
        port,
        user,
        password,
        database,
        application_name,
        connect_timeout,
        search_path,
        prepare,
        ssl_mode,
        ssl_root_cert,
    })
}

struct Out {
    bytes: Vec<u8>,
}

impl Out {
    fn message(tag: u8) -> Self {
        Self {
            bytes: vec![tag, 0, 0, 0, 0],
        }
    }
    fn startup() -> Self {
        Self {
            bytes: vec![0, 0, 0, 0],
        }
    }
    fn i16(&mut self, value: i16) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }
    fn i32(&mut self, value: i32) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }
    fn byte(&mut self, value: u8) -> &mut Self {
        self.bytes.push(value);
        self
    }
    fn raw(&mut self, data: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(data);
        self
    }
    fn cstr(&mut self, text: &str) -> &mut Self {
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
        self
    }
    fn finish(mut self, has_tag: bool) -> Vec<u8> {
        let start = usize::from(has_tag);
        let length = (self.bytes.len() - start) as i32;
        self.bytes[start..start + 4].copy_from_slice(&length.to_be_bytes());
        self.bytes
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("Truncated PostgreSQL message".to_string());
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
    fn i16(&mut self) -> Result<i16, String> {
        let arr: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| "Unexpected EOF in PG i16".to_string())?;
        Ok(i16::from_be_bytes(arr))
    }
    fn i32(&mut self) -> Result<i32, String> {
        let arr: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| "Unexpected EOF in PG i32".to_string())?;
        Ok(i32::from_be_bytes(arr))
    }
    fn cstr(&mut self) -> Result<String, String> {
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| "Missing NUL terminator in PostgreSQL string".to_string())?;
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(text)
    }
    fn rest(&mut self) -> &'a [u8] {
        let slice = &self.data[self.pos..];
        self.pos = self.data.len();
        slice
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PgError {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: String,
    pub hint: String,
}

impl PgError {
    fn parse(body: &[u8]) -> Self {
        let mut error = Self::default();
        let mut reader = Reader::new(body);
        while let Ok(code) = reader.take(1) {
            if code[0] == 0 {
                break;
            }
            let Ok(value) = reader.cstr() else { break };
            match code[0] {
                b'S' if error.severity.is_empty() => error.severity = value,
                b'V' => error.severity = value,
                b'C' => error.code = value,
                b'M' => error.message = value,
                b'D' => error.detail = value,
                b'H' => error.hint = value,
                _ => {}
            }
        }
        error
    }

    pub fn text(&self) -> String {
        let mut text = format!("PostgreSQL: {}", self.message);
        if !self.code.is_empty() {
            text.push_str(&format!(" (SQLSTATE {})", self.code));
        }
        if !self.detail.is_empty() {
            text.push_str(&format!(". {}", self.detail));
        }
        if !self.hint.is_empty() {
            text.push_str(&format!(". Hint: {}", self.hint));
        }
        text
    }
}

fn decode_b64_raw(s: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| -> Option<i8> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as i8),
            b'a'..=b'z' => Some((b - b'a' + 26) as i8),
            b'0'..=b'9' => Some((b - b'0' + 52) as i8),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(-2),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.as_chunks::<4>().0 {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = val(chunk[2])?;
        let d = val(chunk[3])?;
        if a < 0 || b < 0 || (c == -2 && d != -2) {
            return None;
        }
        out.push(((a as u8) << 2) | ((b as u8) >> 4));
        if c >= 0 {
            out.push(((b as u8) << 4) | ((c as u8) >> 2));
            if d >= 0 {
                out.push(((c as u8) << 6) | (d as u8));
            }
        }
    }
    Some(out)
}

pub struct ScramClient {
    nonce: String,
    bare: String,
    gs2_header: String,
    cbind_input: Vec<u8>,
}

impl ScramClient {
    pub fn new(user: &str, nonce: &str) -> Self {
        Self::with_channel_binding(user, nonce, None)
    }

    pub fn with_channel_binding(
        user: &str,
        nonce: &str,
        tls_server_end_point: Option<&[u8]>,
    ) -> Self {
        let (gs2_header, cbind_input) = match tls_server_end_point {
            Some(cb_data) => {
                let hdr = "p=tls-server-end-point,,".to_string();
                let mut input = Vec::with_capacity(hdr.len() + cb_data.len());
                input.extend_from_slice(hdr.as_bytes());
                input.extend_from_slice(cb_data);
                (hdr, input)
            }
            None => {
                let hdr = "n,,".to_string();
                let input = hdr.as_bytes().to_vec();
                (hdr, input)
            }
        };
        Self {
            nonce: nonce.to_string(),
            bare: format!("n={user},r={nonce}"),
            gs2_header,
            cbind_input,
        }
    }

    pub fn first_message(&self) -> String {
        format!("{}{}", self.gs2_header, self.bare)
    }

    pub fn final_message(&self, password: &str, server_first: &str) -> Result<(String, Vec<u8>), String> {
        let (mut combined, mut salt, mut iterations) = (None, None, None);
        for part in server_first.split(',') {
            match part.split_once('=') {
                Some(("r", v)) => combined = Some(v),
                Some(("s", v)) => salt = Some(v),
                Some(("i", v)) => iterations = Some(v),
                Some(("m", _)) => return Err("Unsupported SCRAM extension".to_string()),
                _ => {}
            }
        }
        let (Some(combined), Some(salt), Some(iterations)) = (combined, salt, iterations) else {
            return Err("Incomplete SCRAM server-first message".to_string());
        };
        if !combined.starts_with(&self.nonce) || combined.len() <= self.nonce.len() {
            return Err("Server returned invalid SCRAM nonce".to_string());
        }
        let salt = decode_b64_raw(salt).ok_or_else(|| "Invalid SCRAM salt base64".to_string())?;
        let iterations: u32 = iterations
            .parse()
            .ok()
            .filter(|n| (1..=MAX_PBKDF2_ITERATIONS).contains(n))
            .ok_or_else(|| format!("Invalid SCRAM iteration count '{}'", iterations))?;

        let mut salted = [0u8; 32];
        pbkdf2_sha256(password.as_bytes(), &salt, iterations, &mut salted);
        let client_key = hmac_sha256_bytes(&salted, b"Client Key");
        let stored_key = sha256_bytes(&client_key);

        let without_proof = format!("c={},r={combined}", encode_base64(&self.cbind_input));
        let auth_message = format!("{},{server_first},{without_proof}", self.bare);
        let client_signature = hmac_sha256_bytes(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(client_signature)
            .map(|(a, b)| a ^ b)
            .collect();

        let server_key = hmac_sha256_bytes(&salted, b"Server Key");
        let server_signature = hmac_sha256_bytes(&server_key, auth_message.as_bytes()).to_vec();
        Ok((
            format!("{without_proof},p={}", encode_base64(&proof)),
            server_signature,
        ))
    }

    pub fn verify_server_final(expected: &[u8], server_final: &str) -> Result<(), String> {
        if let Some(reason) = server_final.strip_prefix("e=") {
            return Err(format!("Authentication rejected by server: {reason}"));
        }
        let Some(sig) = server_final.strip_prefix("v=").and_then(decode_b64_raw) else {
            return Err("Malformed SCRAM server-final message".to_string());
        };
        if constant_time_eq(&sig, expected) {
            Ok(())
        } else {
            Err("SCRAM server signature mismatch".to_string())
        }
    }
}

pub fn translate_placeholders(sql: &str) -> (String, usize) {
    let bytes = sql.as_bytes();
    if !bytes
        .iter()
        .any(|&b| matches!(b, b'?' | b'\'' | b'"' | b'-' | b'/' | b'$'))
    {
        return (sql.to_string(), 0);
    }
    let mut out = String::with_capacity(sql.len() + 8);
    let mut count = 0usize;
    let mut i = 0usize;
    let mut copy_start = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                let escape = i > 0
                    && matches!(bytes[i - 1], b'E' | b'e')
                    && (i < 2 || !(bytes[i - 2].is_ascii_alphanumeric() || bytes[i - 2] == b'_'));
                i += 1;
                while i < bytes.len() {
                    if escape && bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                let mut j = i + 1;
                while j < bytes.len() {
                    let b = bytes[j];
                    if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let is_dollar_quote = bytes.get(j) == Some(&b'$')
                    && !bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit());
                if is_dollar_quote {
                    let delimiter = &bytes[i..=j];
                    i = j + 1;
                    while i < bytes.len() {
                        if bytes[i..].starts_with(delimiter) {
                            i += delimiter.len();
                            break;
                        }
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            b'?' => {
                out.push_str(&sql[copy_start..i]);
                if bytes.get(i + 1) == Some(&b'?') {
                    out.push('?');
                    i += 2;
                } else {
                    count += 1;
                    out.push('$');
                    out.push_str(&count.to_string());
                    i += 1;
                }
                copy_start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    if copy_start < sql.len() {
        out.push_str(&sql[copy_start..]);
    }
    (out, count)
}

mod oid {
    pub const BOOL: u32 = 16;
    pub const INT8: u32 = 20;
    pub const INT2: u32 = 21;
    pub const INT4: u32 = 23;
    pub const OID: u32 = 26;
    pub const FLOAT4: u32 = 700;
    pub const FLOAT8: u32 = 701;
    pub const UUID: u32 = 2950;
}

fn decode_cell(type_oid: u32, bytes: Option<&[u8]>) -> Cell {
    let Some(bytes) = bytes else {
        return Cell::Null;
    };
    match type_oid {
        oid::BOOL => Cell::Bool(bytes == b"t"),
        oid::INT2 | oid::INT4 | oid::INT8 | oid::OID => std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .map(Cell::Int)
            .unwrap_or_else(|| Cell::Text(String::from_utf8_lossy(bytes).into_owned())),
        oid::FLOAT4 | oid::FLOAT8 => std::str::from_utf8(bytes)
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

fn supports_binary_result(type_oid: u32) -> bool {
    matches!(type_oid, oid::BOOL | oid::INT2 | oid::INT4 | oid::INT8 | oid::OID | oid::FLOAT8 | oid::UUID)
}

fn decode_binary_cell(type_oid: u32, bytes: &[u8]) -> Result<Cell, Failure> {
    let wrong_length = |_| Failure::Broken(format!("Invalid PostgreSQL binary value for OID {type_oid}"));
    Ok(match type_oid {
        oid::BOOL => match bytes {
            [0] => Cell::Bool(false),
            [1] => Cell::Bool(true),
            _ => return Err(Failure::Broken("Invalid PostgreSQL binary Bool".to_owned())),
        },
        oid::INT2 => Cell::Int(i64::from(i16::from_be_bytes(bytes.try_into().map_err(wrong_length)?))),
        oid::INT4 => Cell::Int(i64::from(i32::from_be_bytes(bytes.try_into().map_err(wrong_length)?))),
        oid::INT8 => Cell::Int(i64::from_be_bytes(bytes.try_into().map_err(wrong_length)?)),
        oid::OID => Cell::Int(i64::from(u32::from_be_bytes(bytes.try_into().map_err(wrong_length)?))),
        oid::FLOAT8 => Cell::Float(f64::from_be_bytes(bytes.try_into().map_err(wrong_length)?)),
        oid::UUID if bytes.len() == 16 => {
            let mut uuid = String::with_capacity(36);
            const HEX: &[u8; 16] = b"0123456789abcdef";
            for (i, &byte) in bytes.iter().enumerate() {
                if matches!(i, 4 | 6 | 8 | 10) {
                    uuid.push('-');
                }
                uuid.push(char::from(HEX[usize::from(byte >> 4)]));
                uuid.push(char::from(HEX[usize::from(byte & 0xf)]));
            }
            Cell::Text(uuid)
        }
        _ => return Err(Failure::Broken(format!("Unsupported PostgreSQL binary value for OID {type_oid}"))),
    })
}

fn encode_param(cell: &Cell) -> Result<Option<Vec<u8>>, String> {
    Ok(match cell {
        Cell::Null => None,
        Cell::Int(n) => Some(n.to_string().into_bytes()),
        Cell::Bool(b) => Some(if *b { b"true".to_vec() } else { b"false".to_vec() }),
        Cell::Float(x) if x.is_nan() => Some(b"NaN".to_vec()),
        Cell::Float(x) if x.is_infinite() => Some(if *x > 0.0 {
            b"Infinity".to_vec()
        } else {
            b"-Infinity".to_vec()
        }),
        Cell::Float(x) => Some(x.to_string().into_bytes()),
        Cell::Text(text) => {
            if text.contains('\0') {
                return Err("PostgreSQL does not allow NUL bytes in string parameters".to_string());
            }
            Some(text.clone().into_bytes())
        }
    })
}

fn affected_rows(tag: &str) -> u64 {
    let mut words = tag.split_whitespace();
    match words.next() {
        Some("INSERT" | "UPDATE" | "DELETE" | "MERGE") => tag
            .split_whitespace()
            .last()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

const PG_STMT_CACHE_CAP: usize = 128;

#[derive(Clone)]
struct CachedPgStmt {
    name: String,
    placeholders: usize,
    columns: Vec<(String, u32)>,
}

#[derive(Default)]
struct PgStmtCache {
    map: HashMap<std::sync::Arc<str>, CachedPgStmt, crate::vm::value::FxBuildHasher>,
    order: VecDeque<std::sync::Arc<str>>,
    next_id: u64,
}

impl PgStmtCache {
    fn alloc_name(&mut self) -> String {
        self.next_id += 1;
        format!("__cdt_s{}", self.next_id)
    }

    fn get(&mut self, sql: &str) -> Option<CachedPgStmt> {
        let (key_arc, entry) = self.map.get_key_value(sql)?;
        let key_arc = std::sync::Arc::clone(key_arc);
        let entry = entry.clone();
        if self.order.back().map(|k| &**k) != Some(sql) {
            if let Some(pos) = self.order.iter().position(|k| &**k == sql) {
                self.order.remove(pos);
            }
            self.order.push_back(key_arc);
        }
        Some(entry)
    }

    fn insert(&mut self, sql: String, stmt: CachedPgStmt) -> Option<String> {
        let key_arc: std::sync::Arc<str> = std::sync::Arc::from(sql);
        let mut evicted_name = None;
        if let Some(prev) = self.map.insert(std::sync::Arc::clone(&key_arc), stmt) {
            if let Some(pos) = self.order.iter().position(|k| *k == key_arc) {
                self.order.remove(pos);
            }
            evicted_name = Some(prev.name);
        } else if self.order.len() >= PG_STMT_CACHE_CAP
            && let Some(oldest) = self.order.pop_front()
            && let Some(old_stmt) = self.map.remove(&oldest)
        {
            evicted_name = Some(old_stmt.name);
        }
        self.order.push_back(key_arc);
        evicted_name
    }

    fn remove(&mut self, sql: &str) -> Option<String> {
        if let Some(pos) = self.order.iter().position(|k| &**k == sql) {
            self.order.remove(pos);
        }
        self.map.remove(sql).map(|s| s.name)
    }
}

struct Conn {
    reader: BufReader<Transport>,
    msg_buf: Vec<u8>,
    last_active: std::time::Instant,
    stmt_cache: PgStmtCache,
}

impl Conn {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let stream = self.reader.get_mut();
        stream
            .write_all(bytes)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("Failed to write to PostgreSQL: {e}"))
    }

    fn read_message_into(&mut self) -> Result<u8, String> {
        let mut header = [0u8; 5];
        self.reader.read_exact(&mut header).map_err(io_error)?;
        let length = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        if length < 4 || length as usize - 4 > MAX_MESSAGE {
            return Err(format!("Invalid PostgreSQL message length ({length})"));
        }
        let body_len = length as usize - 4;
        self.msg_buf.resize(body_len, 0);
        self.reader.read_exact(&mut self.msg_buf).map_err(io_error)?;
        self.last_active = std::time::Instant::now();
        Ok(header[0])
    }

    fn read_message(&mut self) -> Result<(u8, Vec<u8>), String> {
        let tag = self.read_message_into()?;
        Ok((tag, self.msg_buf.clone()))
    }

    fn is_alive(&mut self) -> bool {
        if self.last_active.elapsed() < Duration::from_secs(5) {
            return self.reader.buffer().is_empty();
        }
        self.reader.buffer().is_empty() && self.reader.get_mut().is_alive_probe()
    }
}

fn io_error(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => "PostgreSQL closed the connection".to_string(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            "PostgreSQL connection timed out".to_string()
        }
        _ => format!("PostgreSQL I/O error: {e}"),
    }
}

struct Exchange {
    result: QueryResult,
    affected: u64,
}

struct State {
    conn: Option<Conn>,
    search_path: Option<String>,
}

pub struct PgConn {
    config: PgConfig,
    state: Mutex<State>,
}

impl PgConn {
    pub fn connect(config: PgConfig) -> Result<Self, String> {
        let mut conn = open_connection(&config)?;
        if let Some(schema) = &config.search_path {
            run_simple(&mut conn, &format!("SET search_path TO \"{schema}\""))?;
        }
        let search_path = config.search_path.clone();
        Ok(Self {
            config,
            state: Mutex::new(State {
                conn: Some(conn),
                search_path,
            }),
        })
    }

    pub fn execute(&self, sql: &str, params: &[Cell]) -> Result<u64, DbErr> {
        self.exchange(sql, params, false).map(|e| e.affected)
    }

    pub fn query(&self, sql: &str, params: &[Cell]) -> Result<QueryResult, DbErr> {
        self.exchange(sql, params, true).map(|e| e.result)
    }

    pub fn close(&self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(mut conn) = state.conn.take()
        {
            let _ = conn.send(&Out::message(b'X').finish(true));
        }
    }

    fn exchange(&self, sql: &str, params: &[Cell], want_rows: bool) -> Result<Exchange, DbErr> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| DbErr::new("internal", "PostgreSQL state lock poisoned", ""))?;

        if state.conn.as_mut().is_some_and(|c| !c.is_alive()) {
            state.conn = None;
        }
        if state.conn.is_none() {
            let mut conn = open_connection(&self.config)
                .map_err(|m| DbErr::new("connect_error", m, ""))?;
            if let Some(schema) = state.search_path.clone() {
                run_simple(&mut conn, &format!("SET search_path TO \"{schema}\""))
                    .map_err(|m| DbErr::new("query_error", m, ""))?;
            }
            state.conn = Some(conn);
        }

        let conn = state
            .conn
            .as_mut()
            .ok_or_else(|| DbErr::new("closed", "PostgreSQL connection is closed", ""))?;
        let outcome = run_statement(conn, self.config.prepare, sql, params, want_rows);
        if let Err(Failure::Broken(_)) = &outcome {
            state.conn = None;
        }
        outcome.map_err(|f| match f {
            Failure::Server(err) => DbErr::new("query_error", err.text(), err.code),
            Failure::Msg(msg) => DbErr::new("query_error", msg, ""),
            Failure::Broken(msg) => DbErr::new("io_error", msg, ""),
        })
    }
}

impl Drop for PgConn {
    fn drop(&mut self) {
        self.close();
    }
}

enum Failure {
    Server(PgError),
    Msg(String),
    Broken(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Broken(message)
    }
}

fn run_statement(
    conn: &mut Conn,
    prepare: bool,
    sql: &str,
    params: &[Cell],
    want_rows: bool,
) -> Result<Exchange, Failure> {
    if params.is_empty() && !want_rows {
        let (translated, placeholders) = translate_placeholders(sql);
        if placeholders > 0 {
            return Err(Failure::Msg(format!(
                "Query has {} '?' placeholder(s) but 0 parameters were given",
                placeholders
            )));
        }
        let mut message = Out::message(b'Q');
        message.cstr(&translated);
        conn.send(&message.finish(true))?;
        return read_response(conn, None, false).map(|(ex, _)| ex);
    }

    if params.len() > 65_535 {
        return Err(Failure::Msg(
            "PostgreSQL supports at most 65535 parameters".to_string(),
        ));
    }

    if prepare && let Some(cached) = conn.stmt_cache.get(sql) {
        if cached.placeholders != params.len() {
            return Err(Failure::Msg(format!(
                "Query has {} '?' placeholder(s) but {} parameter(s) were given",
                cached.placeholders,
                params.len()
            )));
        }
        let encoded: Vec<Option<Vec<u8>>> = params
            .iter()
            .map(encode_param)
            .collect::<Result<_, _>>()
            .map_err(Failure::Msg)?;

        let mut request = Vec::with_capacity(128);
        let mut bind = Out::message(b'B');
        bind.cstr("").cstr(&cached.name).i16(0).i16(encoded.len() as i16);
        for value in &encoded {
            match value {
                None => {
                    bind.i32(-1);
                }
                Some(bytes) => {
                    bind.i32(bytes.len() as i32).raw(bytes);
                }
            }
        }
        let binary = cached.columns.iter().any(|(_, oid)| supports_binary_result(*oid));
        if binary {
            bind.i16(cached.columns.len() as i16);
            for &(_, oid) in &cached.columns {
                bind.i16(i16::from(supports_binary_result(oid)));
            }
        } else {
            bind.i16(0);
        }
        request.extend(bind.finish(true));

        let mut execute = Out::message(b'E');
        execute.cstr("").i32(0);
        request.extend(execute.finish(true));
        request.extend(Out::message(b'S').finish(true));

        conn.send(&request)?;
        return match read_response(conn, Some((&cached.columns, binary)), want_rows) {
            Ok((ex, _)) => Ok(ex),
            Err(Failure::Server(err)) if err.code == "26000" || err.code == "0A000" => {
                conn.stmt_cache.remove(sql);
                Err(Failure::Server(err))
            }
            Err(e) => Err(e),
        };
    }

    let (translated, placeholders) = translate_placeholders(sql);
    if placeholders != params.len() {
        return Err(Failure::Msg(format!(
            "Query has {} '?' placeholder(s) but {} parameter(s) were given",
            placeholders,
            params.len()
        )));
    }
    let encoded: Vec<Option<Vec<u8>>> = params
        .iter()
        .map(encode_param)
        .collect::<Result<_, _>>()
        .map_err(Failure::Msg)?;

    let stmt_name = if prepare {
        conn.stmt_cache.alloc_name()
    } else {
        String::new()
    };

    let mut request: Vec<u8> = Vec::with_capacity(128 + translated.len());
    let mut parse = Out::message(b'P');
    parse.cstr(&stmt_name).cstr(&translated).i16(0);
    request.extend(parse.finish(true));

    let mut bind = Out::message(b'B');
    bind.cstr("").cstr(&stmt_name).i16(0).i16(encoded.len() as i16);
    for value in &encoded {
        match value {
            None => {
                bind.i32(-1);
            }
            Some(bytes) => {
                bind.i32(bytes.len() as i32).raw(bytes);
            }
        }
    }
    bind.i16(0);
    request.extend(bind.finish(true));

    let mut describe = Out::message(b'D');
    if prepare {
        describe.byte(b'S').cstr(&stmt_name);
    } else {
        describe.byte(b'P').cstr("");
    }
    request.extend(describe.finish(true));

    let mut execute = Out::message(b'E');
    execute.cstr("").i32(0);
    request.extend(execute.finish(true));
    request.extend(Out::message(b'S').finish(true));

    conn.send(&request)?;
    let (ex, cols) = read_response(conn, None, want_rows)?;
    if prepare
        && let Some(evicted) = conn.stmt_cache.insert(
            sql.to_string(),
            CachedPgStmt {
                name: stmt_name,
                placeholders,
                columns: cols,
            },
        )
    {
        let mut close = Out::message(b'C');
        close.byte(b'S').cstr(&evicted);
        let mut flush = close.finish(true);
        flush.extend(Out::message(b'S').finish(true));
        if conn.send(&flush).is_ok() {
            let _ = read_response(conn, None, false);
        }
    }
    Ok(ex)
}

fn read_response(
    conn: &mut Conn,
    preset_cols: Option<(&[(String, u32)], bool)>,
    want_rows: bool,
) -> Result<(Exchange, Vec<(String, u32)>), Failure> {
    let (preset, mut binary) = preset_cols.unwrap_or_default();
    let mut columns = preset.to_vec();
    let mut result = QueryResult::default();
    if !columns.is_empty() {
        result.columns = columns
            .iter()
            .map(|(name, type_oid)| Column {
                name: name.clone(),
                is_bool: *type_oid == oid::BOOL,
            })
            .collect();
    }
    let mut affected = 0u64;
    let mut server_error: Option<PgError> = None;

    loop {
        let tag = conn.read_message_into()?;
        let body = &conn.msg_buf;
        match tag {
            b'T' => {
                let mut reader = Reader::new(body);
                let count = reader.i16()? as usize;
                // Only the cached Bind path requests binary formats and omits Describe.
                binary = false;
                columns.clear();
                for _ in 0..count {
                    let name = reader.cstr()?;
                    reader.take(4 + 2)?;
                    let type_oid = reader.i32()? as u32;
                    reader.take(2 + 4 + 2)?;
                    columns.push((name, type_oid));
                }
                result.columns = columns
                    .iter()
                    .map(|(name, type_oid)| Column {
                        name: name.clone(),
                        is_bool: *type_oid == oid::BOOL,
                    })
                    .collect();
            }
            b'D' if want_rows => {
                let mut reader = Reader::new(body);
                let count = reader.i16()? as usize;
                if count != columns.len() {
                    return Err(Failure::Broken(format!(
                        "Column count mismatch: got {} instead of {}",
                        count,
                        columns.len()
                    )));
                }
                let mut row = Vec::with_capacity(count);
                for (_, type_oid) in &columns {
                    let length = reader.i32()?;
                    row.push(if length < 0 {
                        decode_cell(*type_oid, None)
                    } else {
                        let bytes = reader.take(length as usize)?;
                        if binary && supports_binary_result(*type_oid) {
                            decode_binary_cell(*type_oid, bytes)?
                        } else {
                            decode_cell(*type_oid, Some(bytes))
                        }
                    });
                }
                result.rows.push(row);
            }
            b'C' => {
                let tag_text = Reader::new(body).cstr()?;
                affected += affected_rows(&tag_text);
            }
            b'E' => server_error = Some(PgError::parse(body)),
            b'Z' => break,
            _ => {}
        }
    }

    match server_error {
        Some(error) => Err(Failure::Server(error)),
        None => Ok((Exchange { result, affected }, columns)),
    }
}

fn run_simple(conn: &mut Conn, sql: &str) -> Result<(), String> {
    let mut message = Out::message(b'Q');
    message.cstr(sql);
    conn.send(&message.finish(true))?;
    read_response(conn, None, false).map(|_| ()).map_err(|f| match f {
        Failure::Server(e) => e.text(),
        Failure::Msg(m) | Failure::Broken(m) => m,
    })
}

fn tls_server_end_point_hash(der: &[u8]) -> Vec<u8> {
    if let Ok(cert) = parse_x509(der) {
        match cert.sig_alg {
            SigAlg::RsaPkcs1(RsaHashAlg::Sha384)
            | SigAlg::RsaPss { hash: RsaHashAlg::Sha384, .. }
            | SigAlg::EcdsaSha384 => return sha384_bytes(der).to_vec(),
            SigAlg::RsaPkcs1(RsaHashAlg::Sha512)
            | SigAlg::RsaPss { hash: RsaHashAlg::Sha512, .. }
            | SigAlg::EcdsaSha512 => return sha512_bytes(der).to_vec(),
            _ => {}
        }
    }
    sha256_bytes(der).to_vec()
}

fn warn_if_insecure_pg_tls(config: &PgConfig) {
    match config.ssl_mode {
        PgSslMode::Disable => crate::native::tls::warn_db_insecure_tls_once(
            "cdt.db.pg",
            &config.host,
            "sslmode=disable sends credentials and queries in plaintext",
            "'?sslmode=verify-full' (or '?sslmode=verify-ca&sslrootcert=/path/to/ca.pem' if server cert lacks hostname in SAN, or keep explicit '?sslmode=disable' only on a trusted network)",
        ),
        PgSslMode::Allow | PgSslMode::Prefer => crate::native::tls::warn_db_insecure_tls_once(
            "cdt.db.pg",
            &config.host,
            "sslmode=allow/prefer does not verify server certificates and allows plaintext downgrade",
            "'?sslmode=verify-full' (or '?sslmode=verify-ca&sslrootcert=/path/to/ca.pem' for SAN-less internal certs)",
        ),
        PgSslMode::Require if config.ssl_root_cert.is_none() => {
            crate::native::tls::warn_db_insecure_tls_once(
                "cdt.db.pg",
                &config.host,
                "sslmode=require without sslrootcert encrypts without authenticating the server",
                "'?sslmode=verify-full' (or '?sslmode=verify-ca&sslrootcert=/path/to/ca.pem')",
            )
        }
        PgSslMode::Require | PgSslMode::VerifyCa | PgSslMode::VerifyFull => {}
    }
}

fn open_connection(config: &PgConfig) -> Result<Conn, String> {
    warn_if_insecure_pg_tls(config);
    let target = format!("{}:{}", config.host, config.port);
    let addresses = target
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve PostgreSQL host '{target}': {e}"))?;
    let mut last_error = None;
    let mut stream = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, config.connect_timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| match last_error {
        Some(e) => format!("Failed to connect to PostgreSQL '{target}': {e}"),
        None => format!("No addresses resolved for '{target}'"),
    })?;
    let _ = stream.set_nodelay(true);
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let transport = if config.ssl_mode != PgSslMode::Disable {
        let mut ssl_req = [0u8; 8];
        ssl_req[..4].copy_from_slice(&8i32.to_be_bytes());
        ssl_req[4..8].copy_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
        stream
            .write_all(&ssl_req)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("Failed to send PostgreSQL SSLRequest: {e}"))?;

        let mut reply = [0u8; 1];
        stream
            .read_exact(&mut reply)
            .map_err(|e| format!("Failed to read PostgreSQL SSLRequest response: {e}"))?;

        match reply[0] {
            b'S' => {
                let verify = match config.ssl_mode {
                    PgSslMode::Allow | PgSslMode::Prefer => VerifyMode::Disable,
                    PgSslMode::Require => {
                        if config.ssl_root_cert.is_some() {
                            VerifyMode::VerifyCa
                        } else {
                            VerifyMode::Disable
                        }
                    }
                    PgSslMode::VerifyCa => VerifyMode::VerifyCa,
                    PgSslMode::VerifyFull => VerifyMode::VerifyFull,
                    PgSslMode::Disable => return Err("internal error: invalid TLS mode".to_string()),
                };
                let mut tls_cfg = TlsConfig::new(&config.host);
                tls_cfg.verify = verify;
                tls_cfg.min_version = TlsVersion::Tls12;
                tls_cfg.handshake_timeout = config.connect_timeout;
                if let Some(ca_path) = &config.ssl_root_cert {
                    tls_cfg.roots = RootSource::File(PathBuf::from(ca_path));
                }
                let tls_stream = crate::native::tls::connect(stream, &tls_cfg)
                    .map_err(|e| format!("PostgreSQL TLS handshake failed: {e}"))?;
                Transport::Tls(Box::new(tls_stream))
            }
            b'N' => {
                if matches!(
                    config.ssl_mode,
                    PgSslMode::Require | PgSslMode::VerifyCa | PgSslMode::VerifyFull
                ) {
                    return Err("PostgreSQL server does not support TLS".to_string());
                }
                Transport::Plain(stream)
            }
            other => {
                return Err(format!(
                    "Unexpected PostgreSQL SSLRequest reply byte 0x{other:02x}"
                ));
            }
        }
    } else {
        Transport::Plain(stream)
    };

    let mut conn = Conn {
        reader: BufReader::with_capacity(64 * 1024, transport),
        msg_buf: Vec::with_capacity(4096),
        last_active: std::time::Instant::now(),
        stmt_cache: PgStmtCache::default(),
    };

    let mut startup = Out::startup();
    startup.i32(PROTOCOL_3_0);
    for (name, value) in [
        ("user", config.user.as_str()),
        ("database", config.database.as_str()),
        ("client_encoding", "UTF8"),
        ("application_name", config.application_name.as_str()),
    ] {
        startup.cstr(name).cstr(value);
    }
    startup.byte(0);
    conn.send(&startup.finish(false))?;

    let mut scram: Option<(ScramClient, Option<Vec<u8>>)> = None;
    loop {
        let (tag, body) = conn.read_message()?;
        match tag {
            b'R' => {
                let mut reader = Reader::new(&body);
                match reader.i32()? {
                    0 => {}
                    10 => {
                        let mut methods = Vec::new();
                        loop {
                            let method = reader.cstr()?;
                            if method.is_empty() {
                                break;
                            }
                            methods.push(method);
                        }
                        let use_plus = methods.iter().any(|m| m == "SCRAM-SHA-256-PLUS")
                            && conn.reader.get_ref().leaf_cert_der().is_some();
                        let selected_method = if use_plus {
                            "SCRAM-SHA-256-PLUS"
                        } else if methods.iter().any(|m| m == "SCRAM-SHA-256") {
                            "SCRAM-SHA-256"
                        } else {
                            return Err(format!(
                                "Unsupported PostgreSQL auth methods {:?} (only SCRAM-SHA-256 supported)",
                                methods
                            ));
                        };
                        if config.password.is_none() {
                            return Err("PostgreSQL requested password, none provided in URL".to_string());
                        }
                        let cb_data = if use_plus {
                            conn.reader
                                .get_ref()
                                .leaf_cert_der()
                                .map(tls_server_end_point_hash)
                        } else {
                            None
                        };
                        let mut rnd = [0u8; 18];
                        fill_random_bytes(&mut rnd)?;
                        let nonce = encode_base64(&rnd);
                        let client =
                            ScramClient::with_channel_binding("", &nonce, cb_data.as_deref());
                        let first = client.first_message();
                        let mut message = Out::message(b'p');
                        message
                            .cstr(selected_method)
                            .i32(first.len() as i32)
                            .raw(first.as_bytes());
                        conn.send(&message.finish(true))?;
                        scram = Some((client, None));
                    }
                    11 => {
                        let (client, expected) = scram
                            .as_mut()
                            .ok_or_else(|| "Unexpected SCRAM continuation".to_string())?;
                        let server_first = String::from_utf8_lossy(reader.rest()).into_owned();
                        let password = config.password.as_deref().unwrap_or_default();
                        let (final_msg, sig) = client.final_message(password, &server_first)?;
                        *expected = Some(sig);
                        let mut message = Out::message(b'p');
                        message.raw(final_msg.as_bytes());
                        conn.send(&message.finish(true))?;
                    }
                    12 => {
                        let (_, expected) = scram
                            .as_ref()
                            .ok_or_else(|| "Unexpected SCRAM final".to_string())?;
                        let expected = expected
                            .as_ref()
                            .ok_or_else(|| "Missing expected SCRAM signature".to_string())?;
                        ScramClient::verify_server_final(
                            expected,
                            &String::from_utf8_lossy(reader.rest()),
                        )?;
                    }
                    3 => {
                        return Err(
                            "PostgreSQL requested cleartext password (configure SCRAM-SHA-256)"
                                .to_string(),
                        );
                    }
                    5 => {
                        return Err(
                            "PostgreSQL requested legacy md5 auth (configure SCRAM-SHA-256)"
                                .to_string(),
                        );
                    }
                    other => {
                        return Err(format!("Unsupported PostgreSQL auth type code {other}"));
                    }
                }
            }
            b'E' => return Err(PgError::parse(&body).text()),
            b'Z' => return Ok(conn),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_result_formats_preserve_database_cell_types() {
        for (oid, bytes, expected) in [
            (oid::INT2, (-23i16).to_be_bytes().to_vec(), Cell::Int(-23)),
            (oid::INT4, (-42i32).to_be_bytes().to_vec(), Cell::Int(-42)),
            (oid::INT8, i64::MIN.to_be_bytes().to_vec(), Cell::Int(i64::MIN)),
            (oid::OID, u32::MAX.to_be_bytes().to_vec(), Cell::Int(i64::from(u32::MAX))),
            (oid::BOOL, vec![1], Cell::Bool(true)),
            (oid::FLOAT8, (-1.25f64).to_be_bytes().to_vec(), Cell::Float(-1.25)),
            (oid::UUID, vec![0xff; 16], Cell::Text("ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned())),
        ] {
            assert!(supports_binary_result(oid));
            assert_eq!(decode_binary_cell(oid, &bytes).ok(), Some(expected));
            assert!(decode_binary_cell(oid, &bytes[..bytes.len() - 1]).is_err());
        }
    }

    #[test]
    fn pg_url_parsing() {
        let cfg = parse_pg_url("postgres://ann:p%40ss@127.0.0.1:5433/shop?sslmode=disable").unwrap();
        assert_eq!(cfg.user, "ann");
        assert_eq!(cfg.password.as_deref(), Some("p@ss"));
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 5433);
        assert_eq!(cfg.database, "shop");
        assert_eq!(cfg.ssl_mode, PgSslMode::Disable);

        let cfg2 = parse_pg_url(
            "postgres://ann:pw@db.internal/app?sslmode=verify-full&sslrootcert=/tmp/ca.pem",
        )
        .unwrap();
        assert_eq!(cfg2.ssl_mode, PgSslMode::VerifyFull);
        assert_eq!(cfg2.ssl_root_cert.as_deref(), Some("/tmp/ca.pem"));
        assert!(parse_pg_url("postgres://ann@localhost/app?sslmode=bogus").is_err());
    }

    #[test]
    fn pg_placeholder_translation() {
        assert_eq!(
            translate_placeholders("SELECT * FROM t WHERE a = ? AND b = ?"),
            ("SELECT * FROM t WHERE a = $1 AND b = $2".to_string(), 2)
        );
        assert_eq!(
            translate_placeholders("SELECT '?', data ?? 'k', ?"),
            ("SELECT '?', data ? 'k', $1".to_string(), 1)
        );
    }

    #[test]
    fn scram_rfc7677_vector() {
        let client = ScramClient::new("user", "rOprNGfwEbeRWgbNEkqO");
        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let (final_msg, sig) = client.final_message("pencil", server_first).unwrap();
        assert_eq!(
            final_msg,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert_eq!(encode_base64(&sig), "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
    }
}
