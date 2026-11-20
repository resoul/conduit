use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, LazyLock, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::native::tls::{TlsConfig, TlsVersion, Transport, VerifyMode};

use crate::vm::string::VmString;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_HTTP_BINARY_RESPONSE, SCHEMA_HTTP_CLIENT,
    SCHEMA_HTTP_ERROR, SCHEMA_HTTP_REQUEST, SCHEMA_HTTP_RESPONSE, SCHEMA_HTTP_ROUTER, StructData,
};

pub const MAX_LINE: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 100;
pub const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub enum RouteSeg {
    Lit(Box<str>),
    Param(Arc<VmString>),
    Wildcard(Arc<VmString>),
}

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub method: String,
    pub trimmed_pattern: Box<str>,
    pub is_static: bool,
    pub has_wildcard: bool,
    pub param_count: usize,
    pub segments: Vec<RouteSeg>,
    pub handler: Obj,
    pub query_plan: Option<u16>,
    pub body_json_plan: Option<u16>,
    pub guard: Option<Obj>,
    pub auto_resp_kind: Option<u8>,
}

impl RouteEntry {
    pub fn new(method: &str, pattern: &str, handler: Obj) -> Self {
        let trimmed = pattern.trim_matches('/');
        let segments = parse_pattern(pattern);
        let mut has_wildcard = false;
        let mut param_count = 0usize;
        for seg in &segments {
            match seg {
                RouteSeg::Wildcard(_) => {
                    has_wildcard = true;
                    param_count += 1;
                }
                RouteSeg::Param(_) => param_count += 1,
                RouteSeg::Lit(_) => {}
            }
        }
        Self {
            method: method.to_string(),
            trimmed_pattern: trimmed.into(),
            is_static: param_count == 0 && !has_wildcard,
            has_wildcard,
            param_count,
            segments,
            handler,
            query_plan: None,
            body_json_plan: None,
            guard: None,
            auto_resp_kind: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct RouterState {
    pub routes: Vec<RouteEntry>,
    pub before: Vec<(Option<Box<str>>, Obj)>,
    pub after: Vec<(Option<Box<str>>, Obj)>,
    pub statics: Vec<(Box<str>, Box<str>)>,
    pub cors_origin: Option<Box<str>>,
    pub stop_flag: Arc<AtomicBool>,
    pub listen_addr: Arc<std::sync::Mutex<Option<std::net::SocketAddr>>>,
}

#[inline]
pub fn route_prefix_matches(prefix: &str, path: &str) -> bool {
    let p = prefix.trim_end_matches('/');
    if p.is_empty() {
        return true;
    }
    path == p || (path.starts_with(p) && path.as_bytes().get(p.len()) == Some(&b'/'))
}

pub fn content_type_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" | "md" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

pub fn try_serve_static(statics: &[(Box<str>, Box<str>)], method: &str, path: &str) -> Option<Obj> {
    if method != "GET" && method != "HEAD" {
        return None;
    }
    for (url_prefix, dir_path) in statics {
        if !route_prefix_matches(url_prefix, path) {
            continue;
        }
        let p = url_prefix.trim_end_matches('/');
        let rel = path.strip_prefix(p).unwrap_or("").trim_start_matches('/');
        if rel.is_empty() || rel.contains('\0') || rel.split('/').any(|seg| seg == ".." || seg == ".") {
            return None;
        }
        let base_dir = std::fs::canonicalize(dir_path.as_ref()).ok()?;
        let candidate = base_dir.join(rel);
        let canon = std::fs::canonicalize(&candidate).ok()?;
        if !canon.starts_with(&base_dir) || !canon.is_file() {
            return None;
        }
        let bytes = std::fs::read(&canon).ok()?;
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let ct = content_type_for_path(&canon);
        return Some(make_response_obj(200, body, Some(ct)));
    }
    None
}

pub fn parse_pattern(pattern: &str) -> Vec<RouteSeg> {
    let trimmed = pattern.trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split('/')
        .map(|seg| {
            if let Some(name) = seg.strip_prefix(':') {
                RouteSeg::Param(Arc::new(VmString::from(name)))
            } else if let Some(name) = seg.strip_prefix('*') {
                RouteSeg::Wildcard(Arc::new(VmString::from(name)))
            } else {
                RouteSeg::Lit(seg.into())
            }
        })
        .collect()
}

pub enum MatchedParams {
    None,
    One(Arc<VmString>, Obj),
    Many(Vec<(MapKey, MapVal)>),
}

#[inline]
fn split_path_segs<'a>(
    trimmed: &'a str,
    stack_segs: &mut [&'a str; 16],
    overflow_segs: &mut Vec<&'a str>,
) -> usize {
    if trimmed.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    for s in trimmed.split('/') {
        if n < stack_segs.len() {
            stack_segs[n] = s;
            n += 1;
        } else {
            if overflow_segs.is_empty() {
                overflow_segs.extend_from_slice(stack_segs);
            }
            overflow_segs.push(s);
            n += 1;
        }
    }
    n
}

#[inline]
fn route_matches_path_segs(r: &RouteEntry, path_segs: &[&str]) -> bool {
    if !r.has_wildcard && r.segments.len() != path_segs.len() {
        return false;
    }
    let mut pi = 0usize;
    for seg in &r.segments {
        match seg {
            RouteSeg::Wildcard(_) => return true,
            RouteSeg::Lit(lit) => {
                if pi >= path_segs.len() || path_segs[pi] != lit.as_ref() {
                    return false;
                }
                pi += 1;
            }
            RouteSeg::Param(_) => {
                if pi >= path_segs.len() || path_segs[pi].is_empty() {
                    return false;
                }
                pi += 1;
            }
        }
    }
    pi == path_segs.len()
}

#[inline]
fn extract_matched_params(r: &RouteEntry, path_segs: &[&str]) -> MatchedParams {
    if r.param_count == 0 {
        return MatchedParams::None;
    }
    if r.param_count == 1 {
        let mut pi = 0usize;
        for seg in &r.segments {
            match seg {
                RouteSeg::Wildcard(name) => {
                    let rest = wildcard_rest_obj(path_segs, pi);
                    return MatchedParams::One(Arc::clone(name), rest);
                }
                RouteSeg::Param(name) => {
                    return MatchedParams::One(Arc::clone(name), Obj::str(path_segs[pi]));
                }
                RouteSeg::Lit(_) => pi += 1,
            }
        }
        return MatchedParams::None;
    }

    let mut params = Vec::with_capacity(r.param_count);
    let mut pi = 0usize;
    for seg in &r.segments {
        match seg {
            RouteSeg::Wildcard(name) => {
                let rest = wildcard_rest_obj(path_segs, pi);
                params.push((MapKey::Str(Arc::clone(name)), MapVal::Obj(rest)));
                break;
            }
            RouteSeg::Param(name) => {
                params.push((
                    MapKey::Str(Arc::clone(name)),
                    MapVal::Obj(Obj::str(path_segs[pi])),
                ));
                pi += 1;
            }
            RouteSeg::Lit(_) => pi += 1,
        }
    }
    MatchedParams::Many(params)
}

#[inline]
fn wildcard_rest_obj(path_segs: &[&str], pi: usize) -> Obj {
    if pi + 1 == path_segs.len() {
        Obj::str(path_segs[pi])
    } else if pi >= path_segs.len() {
        Obj::str("")
    } else {
        Obj::str(path_segs[pi..].join("/"))
    }
}

#[inline]
fn route_matches_method(route_method: &str, req_method: &str) -> bool {
    route_method == "*" || route_method == req_method || route_method.eq_ignore_ascii_case(req_method)
}

pub fn lookup_route<'a>(
    routes: &'a [RouteEntry],
    method: &str,
    path: &str,
) -> Result<(&'a RouteEntry, MatchedParams), u16> {
    let trimmed = path.trim_matches('/');
    let mut stack_segs = [""; 16];
    let mut overflow_segs: Vec<&str> = Vec::new();
    let mut seg_count: Option<usize> = None;

    macro_rules! ensure_segs {
        () => {{
            let n = match seg_count {
                Some(n) => n,
                None => {
                    let n = split_path_segs(trimmed, &mut stack_segs, &mut overflow_segs);
                    seg_count = Some(n);
                    n
                }
            };
            if overflow_segs.is_empty() {
                &stack_segs[..n]
            } else {
                &overflow_segs[..]
            }
        }};
    }

    for r in routes {
        if !route_matches_method(&r.method, method) {
            continue;
        }
        if r.is_static {
            if trimmed == r.trimmed_pattern.as_ref() {
                return Ok((r, MatchedParams::None));
            }
        } else {
            let path_segs = ensure_segs!();
            if route_matches_path_segs(r, path_segs) {
                return Ok((r, extract_matched_params(r, path_segs)));
            }
        }
    }

    for r in routes {
        if route_matches_method(&r.method, method) {
            continue;
        }
        let matched = if r.is_static {
            trimmed == r.trimmed_pattern.as_ref()
        } else {
            let path_segs = ensure_segs!();
            route_matches_path_segs(r, path_segs)
        };
        if matched {
            return Err(405);
        }
    }

    Err(404)
}

#[inline]
pub fn make_http_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_ERROR, 0, 2);
    s.set_obj(0, Obj::str(kind));
    s.set_obj(1, Obj::str(message));
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_router_obj(handle: i64) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_ROUTER, 1, 0);
    s.set_scalar(0, handle as u64);
    Obj::Record(Arc::new(s))
}

fn make_static_ct_headers(ct: Option<&str>) -> Obj {
    let mut headers = OrderedMap::default();
    if let Some(ct) = ct {
        headers.set(
            MapKey::Str(Arc::new(VmString::from("content-type"))),
            MapVal::Obj(Obj::str(ct)),
        );
    }
    Obj::Map(Arc::new(headers))
}

static HEADERS_EMPTY: LazyLock<Obj> = LazyLock::new(|| make_static_ct_headers(None));
static HEADERS_TEXT: LazyLock<Obj> =
    LazyLock::new(|| make_static_ct_headers(Some("text/plain; charset=utf-8")));
static HEADERS_HTML: LazyLock<Obj> =
    LazyLock::new(|| make_static_ct_headers(Some("text/html; charset=utf-8")));
static HEADERS_JSON: LazyLock<Obj> =
    LazyLock::new(|| make_static_ct_headers(Some("application/json")));

#[inline]
pub fn make_response_with_body_obj(status: i64, body: Obj, content_type: Option<&str>) -> Obj {
    let headers_obj = match content_type {
        None => HEADERS_EMPTY.clone(),
        Some("text/plain; charset=utf-8") => HEADERS_TEXT.clone(),
        Some("text/html; charset=utf-8") => HEADERS_HTML.clone(),
        Some("application/json") => HEADERS_JSON.clone(),
        Some(other) => make_static_ct_headers(Some(other)),
    };
    let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
    s.set_scalar(0, status as u64);
    s.set_obj(0, body);
    s.set_obj(1, headers_obj);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_response_obj(status: i64, body: impl Into<String>, content_type: Option<&str>) -> Obj {
    make_response_with_body_obj(status, Obj::str(body), content_type)
}

pub fn map_from_pairs(pairs: Vec<(String, String)>) -> Obj {
    if pairs.is_empty() {
        return HEADERS_EMPTY.clone();
    }
    let mut map = OrderedMap::with_capacity(pairs.len());
    for (k, v) in pairs {
        map.set(
            MapKey::Str(Arc::new(VmString::from(k))),
            MapVal::Obj(Obj::str(v)),
        );
    }
    Obj::Map(Arc::new(map))
}

pub fn make_request_obj(
    method: &str,
    path: &str,
    body: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    params: Vec<(String, String)>,
) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_REQUEST, 0, 6);
    s.set_obj(0, Obj::str(method));
    s.set_obj(1, Obj::str(path));
    s.set_obj(2, Obj::str(body));
    s.set_obj(3, map_from_pairs(headers));
    s.set_obj(4, map_from_pairs(query));
    s.set_obj(5, map_from_pairs(params));
    Obj::Record(Arc::new(s))
}

pub struct RawHttpRequest {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub keep_alive: bool,
}

#[derive(Debug)]
pub enum ReadHttpError {
    Closed,
    Bad(String),
    TooLarge,
    Io(io::Error),
}

fn is_http_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~'
        )
}

fn read_crlf_line<R: BufRead>(reader: &mut R) -> Result<Option<String>, ReadHttpError> {
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(ReadHttpError::Io)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_LINE {
        return Err(ReadHttpError::TooLarge);
    }
    while buf.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| ReadHttpError::Bad("Invalid UTF-8 in HTTP header".to_string()))
}

pub fn read_http_request<R: BufRead>(reader: &mut R) -> Result<RawHttpRequest, ReadHttpError> {
    let Some(req_line) = read_crlf_line(reader)? else {
        return Err(ReadHttpError::Closed);
    };
    let mut parts = req_line.split_whitespace();
    let Some(method) = parts.next() else {
        return Err(ReadHttpError::Bad("Missing HTTP method".to_string()));
    };
    let Some(target) = parts.next() else {
        return Err(ReadHttpError::Bad("Missing HTTP target".to_string()));
    };
    let version = parts.next().unwrap_or("HTTP/1.1");
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let mut query = Vec::new();
    if !raw_query.is_empty() {
        for pair in raw_query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            query.push((
                crate::native::url::percent_decode(k, true).unwrap_or_else(|_| k.to_string()),
                crate::native::url::percent_decode(v, true).unwrap_or_else(|_| v.to_string()),
            ));
        }
    }

    let mut headers = Vec::new();
    let mut content_length: usize = 0;
    let mut saw_content_length = false;
    let mut conn_header: Option<String> = None;
    loop {
        let Some(line) = read_crlf_line(reader)? else {
            return Err(ReadHttpError::Closed);
        };
        if line.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(ReadHttpError::TooLarge);
        }
        let Some((name, val)) = line.split_once(':') else {
            return Err(ReadHttpError::Bad("Malformed HTTP header".to_string()));
        };
        if name.is_empty() || !name.bytes().all(is_http_tchar) {
            return Err(ReadHttpError::Bad("Invalid HTTP header name".to_string()));
        }
        let lname = name.to_ascii_lowercase();
        let val = val.trim().to_string();
        if lname == "transfer-encoding" {
            return Err(ReadHttpError::Bad(
                "Transfer-Encoding is not supported".to_string(),
            ));
        } else if lname == "content-length" {
            if saw_content_length {
                return Err(ReadHttpError::Bad("Duplicate Content-Length header".to_string()));
            }
            saw_content_length = true;
            if val.is_empty() || !val.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ReadHttpError::Bad("Invalid Content-Length".to_string()));
            }
            content_length = val
                .parse::<usize>()
                .map_err(|_| ReadHttpError::Bad("Invalid Content-Length".to_string()))?;
            if content_length > MAX_BODY {
                return Err(ReadHttpError::TooLarge);
            }
        } else if lname == "connection" {
            conn_header = Some(val.to_ascii_lowercase());
        }
        headers.push((lname, val));
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body_bytes).map_err(ReadHttpError::Io)?;
    }
    let body = String::from_utf8(body_bytes)
        .map_err(|_| ReadHttpError::Bad("Request body is not valid UTF-8".to_string()))?;

    let keep_alive = if version == "HTTP/1.0" {
        conn_header.as_deref().is_some_and(|c| c.contains("keep-alive"))
    } else {
        !conn_header.as_deref().is_some_and(|c| c.contains("close"))
    };

    Ok(RawHttpRequest {
        method: method.to_string(),
        path: raw_path.to_string(),
        query,
        headers,
        body,
        keep_alive,
    })
}

pub fn reason_phrase(status: i64) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

pub fn write_http_response_ext<W: Write>(
    writer: &mut W,
    status: i64,
    body: &str,
    headers: Option<&OrderedMap>,
    keep_alive: bool,
    head_only: bool,
) -> io::Result<()> {
    write!(
        writer,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        status,
        reason_phrase(status),
        body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    )?;
    if let Some(map) = headers {
        for (k, v) in map.entries.iter() {
            if let (MapKey::Str(ks), MapVal::Obj(Obj::Str(vs))) = (k, v) {
                let k_str = ks.as_str();
                if k_str.eq_ignore_ascii_case("content-length")
                    || k_str.eq_ignore_ascii_case("connection")
                {
                    continue;
                }
                write!(writer, "{}: {}\r\n", k_str, vs.as_str())?;
            }
        }
    }
    writer.write_all(b"\r\n")?;
    if !head_only {
        writer.write_all(body.as_bytes())?;
    }
    writer.flush()
}

pub fn write_error_response(stream: &mut BufWriter<TcpStream>, status: i64, body: &str) {
    let _ = write_http_response_ext(stream, status, body, None, false, false);
}

pub fn reader_writer_pair(stream: TcpStream) -> io::Result<(BufReader<TcpStream>, BufWriter<TcpStream>)> {
    let writer_stream = stream.try_clone()?;
    Ok((BufReader::new(stream), BufWriter::new(writer_stream)))
}

const MAX_CLIENT_BODY: usize = 16 * 1024 * 1024;

fn build_response_obj(status: i64, body: &[u8], headers: Vec<(String, String)>) -> Obj {
    let body_str = String::from_utf8_lossy(body);
    let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
    s.set_scalar(0, status as u64);
    s.set_obj(0, Obj::str(body_str.as_ref()));
    s.set_obj(1, map_from_pairs(headers));
    Obj::Record(Arc::new(s))
}

fn build_binary_response_obj(status: i64, body: &[u8], headers: Vec<(String, String)>) -> Obj {
    let ints: Vec<i64> = body.iter().map(|&b| b as i64).collect();
    let mut s = StructData::new(SCHEMA_HTTP_BINARY_RESPONSE, 1, 2);
    s.set_scalar(0, status as u64);
    s.set_obj(0, Obj::List(ListStorage::Int(Arc::new(ints))));
    s.set_obj(1, map_from_pairs(headers));
    Obj::Record(Arc::new(s))
}

struct ParsedUrl {
    is_https: bool,
    host: String,
    port: u16,
    host_header: String,
    path_and_query: String,
}

fn parse_http_url(raw: &str) -> Result<ParsedUrl, String> {
    let trimmed = raw.trim();
    let (is_https, default_port, rest) = if let Some(r) = trimmed.strip_prefix("http://") {
        (false, 80u16, r)
    } else if let Some(r) = trimmed.strip_prefix("https://") {
        (true, 443u16, r)
    } else {
        return Err("unsupported URL scheme (expected http:// or https://)".to_string());
    };

    let no_frag = rest.split('#').next().unwrap_or(rest);
    if no_frag.is_empty() || no_frag.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("invalid URL".to_string());
    }

    let split_idx = no_frag.find(['/', '?']).unwrap_or(no_frag.len());
    let authority = &no_frag[..split_idx];
    let tail = &no_frag[split_idx..];
    let path_and_query = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('?') {
        format!("/{}", tail)
    } else {
        tail.to_string()
    };

    if authority.is_empty() {
        return Err("missing host in URL".to_string());
    }

    let (host, port) = if let Some(after_bracket) = authority.strip_prefix('[') {
        let end = after_bracket
            .find(']')
            .ok_or_else(|| "invalid IPv6 host in URL".to_string())?;
        let h = &after_bracket[..end];
        let rem = &after_bracket[end + 1..];
        let p = if rem.is_empty() {
            default_port
        } else if let Some(port_str) = rem.strip_prefix(':') {
            port_str
                .parse::<u16>()
                .map_err(|_| "invalid port in URL".to_string())?
        } else {
            return Err("invalid authority in URL".to_string());
        };
        (format!("[{}]", h), p)
    } else if let Some((h, port_str)) = authority.rsplit_once(':') {
        if h.is_empty() {
            return Err("missing host in URL".to_string());
        }
        let p = port_str
            .parse::<u16>()
            .map_err(|_| "invalid port in URL".to_string())?;
        (h.to_string(), p)
    } else {
        (authority.to_string(), default_port)
    };

    let host_header = if port == default_port {
        host.clone()
    } else {
        format!("{}:{}", host, port)
    };
    let connect_host = host.trim_start_matches('[').trim_end_matches(']').to_string();

    Ok(ParsedUrl {
        is_https,
        host: connect_host,
        port,
        host_header,
        path_and_query,
    })
}

fn read_client_crlf_line<R: BufRead>(reader: &mut R, max_len: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut total = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            total += pos + 1;
            if total > max_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "header line too long"));
            }
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            if buf.ends_with(b"\r") {
                buf.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        } else {
            let len = available.len();
            total += len;
            if total > max_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "header line too long"));
            }
            buf.extend_from_slice(available);
            reader.consume(len);
        }
    }
}

fn read_chunked_body<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let size_line = read_client_crlf_line(reader, MAX_LINE)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF in chunked body"))?;
        let hex_part = size_line.split(';').next().unwrap_or("").trim();
        let chunk_size = usize::from_str_radix(hex_part, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        if chunk_size == 0 {
            // Drain optional trailing headers until blank line.
            while let Some(line) = read_client_crlf_line(reader, MAX_LINE)? {
                if line.is_empty() {
                    break;
                }
            }
            break;
        }
        if out.len().saturating_add(chunk_size) > MAX_CLIENT_BODY {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "response body too large"));
        }
        let mut chunk = vec![0u8; chunk_size];
        reader.read_exact(&mut chunk)?;
        out.extend_from_slice(&chunk);
        // Consume CRLF after chunk payload.
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
    }
    Ok(out)
}

struct RawHttpResponse {
    status: i64,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn parse_http_response_raw<R: BufRead>(
    reader: &mut R,
    method: &str,
    is_curl: bool,
) -> Result<(RawHttpResponse, bool), String> {
    let (status, headers, content_length, is_chunked, can_keep_alive) = loop {
        let status_line = read_client_crlf_line(reader, MAX_LINE)
            .map_err(|e| format!("failed to read status line: {}", e))?
            .ok_or_else(|| "empty HTTP response".to_string())?;
        let mut parts = status_line.splitn(3, ' ');
        let proto = parts.next().unwrap_or("");
        let is_http_10 = proto.eq_ignore_ascii_case("HTTP/1.0");
        let code_str = parts
            .next()
            .ok_or_else(|| format!("malformed HTTP status line: {}", status_line))?;
        let status = code_str
            .parse::<i64>()
            .map_err(|_| format!("invalid HTTP status code: {}", code_str))?;

        let mut headers = Vec::new();
        let mut content_length: Option<usize> = None;
        let mut is_chunked = false;
        let mut conn_close = false;
        let mut conn_keep_alive = false;
        let mut count = 0usize;
        loop {
            let line = read_client_crlf_line(reader, MAX_LINE)
                .map_err(|e| format!("failed to read headers: {}", e))?
                .unwrap_or_default();
            if line.is_empty() {
                break;
            }
            count += 1;
            if count > MAX_HEADERS {
                return Err("too many response headers".to_string());
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim().to_string();
                if key == "content-length" {
                    if let Ok(len) = val.parse::<usize>() {
                        content_length = Some(len);
                    }
                } else if key == "transfer-encoding" && val.to_ascii_lowercase().contains("chunked") {
                    is_chunked = true;
                } else if key == "connection" {
                    for token in val.split(',') {
                        let t = token.trim().to_ascii_lowercase();
                        if t == "close" {
                            conn_close = true;
                        } else if t == "keep-alive" {
                            conn_keep_alive = true;
                        }
                    }
                }
                headers.push((key, val));
            }
        }

        let is_proxy_connect = is_curl
            && status == 200
            && status_line.to_ascii_lowercase().contains("connection established");
        let is_curl_redirect_hop = is_curl
            && (300..400).contains(&status)
            && reader
                .fill_buf()
                .map(|b| b.starts_with(b"HTTP/"))
                .unwrap_or(false);
        if (100..200).contains(&status) || is_proxy_connect || is_curl_redirect_hop {
            continue;
        }
        let body_framed = method.eq_ignore_ascii_case("HEAD")
            || status == 204
            || status == 304
            || is_chunked
            || content_length.is_some();
        let ka = body_framed && !conn_close && (!is_http_10 || conn_keep_alive);
        break (status, headers, content_length, is_chunked, ka);
    };

    let body_bytes = if method.eq_ignore_ascii_case("HEAD") || status == 204 || status == 304 {
        Vec::new()
    } else if is_chunked && !is_curl {
        read_chunked_body(reader).map_err(|e| format!("failed to read chunked body: {}", e))?
    } else if let Some(len) = content_length.filter(|_| !is_curl) {
        if len > MAX_CLIENT_BODY {
            return Err("response body exceeds maximum size".to_string());
        }
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| format!("failed to read response body: {}", e))?;
        buf
    } else {
        let mut buf = Vec::new();
        reader
            .take((MAX_CLIENT_BODY + 1) as u64)
            .read_to_end(&mut buf)
            .map_err(|e| format!("failed to read response body: {}", e))?;
        if buf.len() > MAX_CLIENT_BODY {
            return Err("response body exceeds maximum size".to_string());
        }
        buf
    };

    Ok((
        RawHttpResponse {
            status,
            headers,
            body: body_bytes,
        },
        can_keep_alive,
    ))
}

const MAX_REDIRECTS: usize = 10;

fn resolve_redirect_url(base: &ParsedUrl, location: &str) -> String {
    let loc = location.trim();
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return loc.to_string();
    }
    let scheme = if base.is_https { "https" } else { "http" };
    if let Some(rest) = loc.strip_prefix("//") {
        return format!("{}://{}", scheme, rest);
    }
    if loc.starts_with('/') {
        return format!("{}://{}{}", scheme, base.host_header, loc);
    }
    let base_path = base.path_and_query.split('?').next().unwrap_or("/");
    let dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "/",
    };
    format!("{}://{}{}{}", scheme, base.host_header, dir, loc)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    is_https: bool,
    host: String,
    port: u16,
}

struct PooledConn {
    reader: BufReader<Transport>,
    last_used: Instant,
}

const POOL_MAX_IDLE_PER_HOST: usize = 8;
const POOL_IDLE_TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct HttpConnPool {
    idle: Mutex<HashMap<PoolKey, Vec<PooledConn>>>,
}

impl HttpConnPool {
    pub fn new() -> Self {
        Self::default()
    }

    fn take(&self, key: &PoolKey) -> Option<BufReader<Transport>> {
        let mut map = self.idle.lock().ok()?;
        let list = map.get_mut(key)?;
        let now = Instant::now();
        while let Some(conn) = list.pop() {
            if now.duration_since(conn.last_used) < POOL_IDLE_TTL {
                if list.is_empty() {
                    map.remove(key);
                }
                return Some(conn.reader);
            }
        }
        map.remove(key);
        None
    }

    fn put(&self, key: PoolKey, reader: BufReader<Transport>) {
        if !reader.buffer().is_empty() {
            return;
        }
        if let Ok(mut map) = self.idle.lock() {
            let list = map.entry(key).or_default();
            if list.len() >= POOL_MAX_IDLE_PER_HOST {
                list.remove(0);
            }
            list.push(PooledConn {
                reader,
                last_used: Instant::now(),
            });
        }
    }

    pub fn idle_count(&self) -> usize {
        let Ok(mut map) = self.idle.lock() else {
            return 0;
        };
        let now = Instant::now();
        let mut total = 0usize;
        map.retain(|_, list| {
            list.retain(|c| now.duration_since(c.last_used) < POOL_IDLE_TTL);
            total += list.len();
            !list.is_empty()
        });
        total
    }

    pub fn clear(&self) {
        if let Ok(mut map) = self.idle.lock() {
            map.clear();
        }
    }
}

#[derive(Clone)]
pub struct HttpClientState {
    pub base_url: String,
    pub default_headers: Vec<(String, String)>,
    pub timeout_ms: i64,
    pub pool: Arc<HttpConnPool>,
}

impl HttpClientState {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            default_headers: Vec::new(),
            timeout_ms: 30_000,
            pool: Arc::new(HttpConnPool::new()),
        }
    }

    pub fn set_header(&mut self, name: String, value: String) {
        if let Some((_, v)) = self
            .default_headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        {
            *v = value;
        } else {
            self.default_headers.push((name, value));
        }
    }
}

pub fn make_http_client_obj(client_id: usize) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_CLIENT, 1, 0);
    s.set_scalar(0, client_id as u64);
    Obj::Record(Arc::new(s))
}

pub fn resolve_client_url(base_url: &str, path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") || base_url.is_empty() {
        return trimmed.to_string();
    }
    let base = base_url.trim_end_matches('/');
    if trimmed.is_empty() {
        base.to_string()
    } else if trimmed.starts_with(['?', '/']) {
        format!("{}{}", base, trimmed)
    } else {
        format!("{}/{}", base, trimmed)
    }
}

fn open_http_transport(parsed: &ParsedUrl, timeout: Duration) -> Result<Transport, Obj> {
    let addr_str = format!("{}:{}", parsed.host, parsed.port);
    let addrs: Vec<_> = match addr_str.to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(e) => return Err(make_http_error("dns_error", format!("failed to resolve {}: {}", addr_str, e))),
    };
    if addrs.is_empty() {
        return Err(make_http_error("dns_error", format!("no addresses resolved for {}", addr_str)));
    }

    let connect_timeout = timeout.min(Duration::from_secs(10));
    let deadline = Instant::now() + timeout;
    let mut stream = None;
    let mut last_err = None;
    for addr in addrs {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(make_http_error("timeout", "request timed out while connecting"));
        }
        match TcpStream::connect_timeout(&addr, connect_timeout.min(remaining)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let stream = match stream {
        Some(s) => s,
        None => {
            return Err(make_http_error(
                "connect_error",
                format!(
                    "failed to connect to {}: {}",
                    addr_str,
                    last_err.map(|e| e.to_string()).unwrap_or_default()
                ),
            ));
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(make_http_error("timeout", "request timed out"));
    }
    let _ = stream.set_read_timeout(Some(remaining));
    let _ = stream.set_write_timeout(Some(remaining));

    let transport = if parsed.is_https {
        let mut tls_cfg = TlsConfig::new(&parsed.host);
        tls_cfg.verify = VerifyMode::VerifyFull;
        tls_cfg.alpn = vec![b"http/1.1"];
        tls_cfg.min_version = TlsVersion::Tls12;
        tls_cfg.handshake_timeout = remaining;
        match crate::native::tls::connect(stream, &tls_cfg) {
            Ok(tls_stream) => Transport::Tls(Box::new(tls_stream)),
            Err(e) => {
                let kind = if e.is_certificate_error() {
                    "certificate_error"
                } else {
                    "tls_error"
                };
                return Err(make_http_error(kind, e.to_string()));
            }
        }
    } else {
        Transport::Plain(stream)
    };

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(make_http_error("timeout", "request timed out after TLS handshake"));
    }
    let _ = transport.set_read_timeout(Some(remaining));
    let _ = transport.set_write_timeout(Some(remaining));
    Ok(transport)
}

fn build_http_request_bytes(
    method: &str,
    parsed: &ParsedUrl,
    body: &[u8],
    headers: &[(String, String)],
    keep_alive: bool,
) -> Result<Vec<u8>, Obj> {
    let mut req = Vec::new();
    req.extend_from_slice(format!("{} {} HTTP/1.1\r\n", method, parsed.path_and_query).as_bytes());
    let has_host = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host"));
    if !has_host {
        req.extend_from_slice(format!("Host: {}\r\n", parsed.host_header).as_bytes());
    }
    let has_conn = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("connection"));
    if !has_conn {
        if keep_alive {
            req.extend_from_slice(b"Connection: keep-alive\r\n");
        } else {
            req.extend_from_slice(b"Connection: close\r\n");
        }
    }
    let has_len = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    if !has_len && (!body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH")) {
        req.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            return Err(make_http_error("invalid_header", "header name or value contains CR/LF"));
        }
        req.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body);
    Ok(req)
}

fn tcp_http_request_once(
    method: &str,
    parsed: &ParsedUrl,
    body: &[u8],
    headers: &[(String, String)],
    timeout: Duration,
    pool: Option<&HttpConnPool>,
) -> Result<RawHttpResponse, Obj> {
    let want_keep_alive = pool.is_some();
    let req = build_http_request_bytes(method, parsed, body, headers, want_keep_alive)?;
    let pool_key = PoolKey {
        is_https: parsed.is_https,
        host: parsed.host.to_ascii_lowercase(),
        port: parsed.port,
    };

    if let Some(p) = pool
        && let Some(mut reader) = p.take(&pool_key)
    {
        let _ = reader.get_ref().set_read_timeout(Some(timeout));
        let _ = reader.get_ref().set_write_timeout(Some(timeout));
        if reader
            .get_mut()
            .write_all(&req)
            .and_then(|_| reader.get_mut().flush())
            .is_ok()
            && let Ok((resp, can_ka)) = parse_http_response_raw(&mut reader, method, false)
        {
            if can_ka {
                p.put(pool_key, reader);
            }
            return Ok(resp);
        }
        // Stale pooled connection closed by peer: fall through to fresh connection.
    }

    let transport = open_http_transport(parsed, timeout)?;
    let mut reader = BufReader::new(transport);
    if let Err(e) = reader.get_mut().write_all(&req).and_then(|_| reader.get_mut().flush()) {
        return Err(make_http_error("io_error", format!("failed to send request: {}", e)));
    }
    let (resp, can_ka) = parse_http_response_raw(&mut reader, method, false)
        .map_err(|e| make_http_error("http_error", e))?;
    if let Some(p) = pool
        && can_ka
    {
        p.put(pool_key, reader);
    }
    Ok(resp)
}

fn http_client_request_raw(
    method: &str,
    url: &str,
    body: &[u8],
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: Option<&HttpConnPool>,
) -> Result<RawHttpResponse, Obj> {
    if timeout_ms <= 0 {
        return Err(make_http_error("invalid_timeout", "timeout_ms must be > 0"));
    }
    let timeout = Duration::from_millis(timeout_ms as u64);
    let mut cur_method = method.trim().to_ascii_uppercase();
    if cur_method.is_empty()
        || cur_method
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(make_http_error(
            "invalid_method",
            format!("invalid HTTP method: {:?}", method),
        ));
    }
    let mut cur_url = url.trim().to_string();
    let mut cur_body = body.to_vec();

    for hop in 0..=MAX_REDIRECTS {
        let parsed = match parse_http_url(&cur_url) {
            Ok(p) => p,
            Err(e) => return Err(make_http_error("invalid_url", e)),
        };
        let resp = tcp_http_request_once(&cur_method, &parsed, &cur_body, headers, timeout, pool)?;
        if matches!(resp.status, 301..=303 | 307 | 308)
            && let Some((_, loc)) = resp.headers.iter().find(|(k, _)| k == "location")
        {
            if hop == MAX_REDIRECTS {
                return Err(make_http_error(
                    "too_many_redirects",
                    format!("exceeded {} redirects", MAX_REDIRECTS),
                ));
            }
            let next_url = resolve_redirect_url(&parsed, loc);
            if parsed.is_https && next_url.starts_with("http://") {
                return Err(make_http_error(
                    "tls_error",
                    "refusing insecure redirect from https:// to http://",
                ));
            }
            cur_url = next_url;
            if matches!(resp.status, 301..=303) && cur_method != "HEAD" {
                cur_method = "GET".to_string();
                cur_body.clear();
            }
            continue;
        }
        return Ok(resp);
    }

    Err(make_http_error(
        "too_many_redirects",
        format!("exceeded {} redirects", MAX_REDIRECTS),
    ))
}

pub fn http_client_request(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
) -> Obj {
    http_client_request_with_timeout(method, url, body, headers, 30_000)
}

pub fn http_client_request_with_timeout(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
) -> Obj {
    match http_client_request_raw(method, url, body.as_bytes(), headers, timeout_ms, None) {
        Ok(resp) => build_response_obj(resp.status, &resp.body, resp.headers),
        Err(err_obj) => err_obj,
    }
}

pub fn http_client_pooled_request(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: &HttpConnPool,
) -> Obj {
    match http_client_request_raw(method, url, body.as_bytes(), headers, timeout_ms, Some(pool)) {
        Ok(resp) => build_response_obj(resp.status, &resp.body, resp.headers),
        Err(err_obj) => err_obj,
    }
}

pub fn http_client_bytes_request(
    method: &str,
    url: &str,
    body: &[u8],
    headers: &[(String, String)],
    timeout_ms: i64,
) -> Obj {
    match http_client_request_raw(method, url, body, headers, timeout_ms, None) {
        Ok(resp) => build_binary_response_obj(resp.status, &resp.body, resp.headers),
        Err(err_obj) => err_obj,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn resolve_client_url_joins_base_and_paths() {
        assert_eq!(
            resolve_client_url("https://api.example.com/v1/", "/users"),
            "https://api.example.com/v1/users"
        );
        assert_eq!(
            resolve_client_url("https://api.example.com/v1", "users"),
            "https://api.example.com/v1/users"
        );
        assert_eq!(
            resolve_client_url("https://api.example.com/v1", "?page=2"),
            "https://api.example.com/v1?page=2"
        );
        assert_eq!(
            resolve_client_url("https://api.example.com/v1", "http://other.local/x"),
            "http://other.local/x"
        );
    }

    #[test]
    fn parse_http_response_detects_keep_alive_framing() {
        let raw_ka = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let mut r1 = BufReader::new(Cursor::new(&raw_ka[..]));
        let (resp1, ka1) = parse_http_response_raw(&mut r1, "GET", false).unwrap();
        assert_eq!(resp1.status, 200);
        assert_eq!(resp1.body, b"ok");
        assert!(ka1);

        let raw_close = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok";
        let mut r2 = BufReader::new(Cursor::new(&raw_close[..]));
        let (_, ka2) = parse_http_response_raw(&mut r2, "GET", false).unwrap();
        assert!(!ka2);

        let raw_10 = b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let mut r3 = BufReader::new(Cursor::new(&raw_10[..]));
        let (_, ka3) = parse_http_response_raw(&mut r3, "GET", false).unwrap();
        assert!(!ka3);

        let raw_unframed = b"HTTP/1.1 200 OK\r\n\r\nhello";
        let mut r4 = BufReader::new(Cursor::new(&raw_unframed[..]));
        let (resp4, ka4) = parse_http_response_raw(&mut r4, "GET", false).unwrap();
        assert_eq!(resp4.body, b"hello");
        assert!(!ka4);
    }
}
