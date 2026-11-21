use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, LazyLock, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::native::tls::{TlsConfig, TlsVersion, Transport, VerifyMode};

use crate::vm::string::VmString;
use crate::vm::value::{
    FxBuildHasher, MapKey, MapVal, Obj, OrderedMap, SCHEMA_HTTP_BINARY_RESPONSE,
    SCHEMA_HTTP_CLIENT, SCHEMA_HTTP_ERROR, SCHEMA_HTTP_REQUEST, SCHEMA_HTTP_RESPONSE,
    SCHEMA_HTTP_ROUTER, StructData,
};

pub const MAX_LINE: usize = 8 * 1024;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_HEADERS: usize = 100;
pub const MAX_BODY: usize = 1024 * 1024;
pub(crate) const BODY_INITIAL_CAP: usize = 64 * 1024;

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
pub struct RouterServerOptions {
    pub workers: Option<usize>,
    pub idle_timeout_ms: Option<u64>,
    pub header_timeout_ms: Option<u64>,
    pub body_timeout_ms: Option<u64>,
    pub max_conns_per_ip: Option<usize>,
    pub trusted_proxies: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct RouterState {
    pub routes: Vec<RouteEntry>,
    pub(crate) route_index: Arc<RouteIndex>,
    pub before: Vec<(Option<Box<str>>, Obj)>,
    pub after: Vec<(Option<Box<str>>, Obj)>,
    pub on_start: Vec<Obj>,
    pub on_stop: Vec<Obj>,
    pub statics: Vec<(Box<str>, Box<str>)>,
    pub cors_origin: Option<Box<str>>,
    pub server_opts: RouterServerOptions,
    pub stop_flag: Arc<AtomicBool>,
    pub listen_addr: Arc<std::sync::Mutex<Option<std::net::SocketAddr>>>,
}

impl RouterState {
    pub fn push_route(&mut self, entry: RouteEntry) {
        let route_idx = self.routes.len();
        Arc::make_mut(&mut self.route_index).insert(route_idx, &entry);
        self.routes.push(entry);
    }

    pub fn lookup_route(&self, method: &str, path: &str) -> Result<(&RouteEntry, MatchedParams), u16> {
        if self.route_index.indexed_len == self.routes.len() {
            self.route_index.lookup(&self.routes, method, path)
        } else {
            lookup_route(&self.routes, method, path)
        }
    }
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
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

pub enum StaticFileBody {
    HeadOnly { len: u64 },
    File { file: std::fs::File, len: u64 },
}

pub struct ResolvedStaticFile {
    pub content_type: &'static str,
    pub body: StaticFileBody,
}

impl ResolvedStaticFile {
    pub fn len(&self) -> u64 {
        match &self.body {
            StaticFileBody::HeadOnly { len } | StaticFileBody::File { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn resolve_static_file(
    statics: &[(Box<str>, Box<str>)],
    method: &str,
    path: &str,
) -> Option<ResolvedStaticFile> {
    resolve_static_file_checked(
        statics,
        method,
        path,
        &crate::vm::Permissions::allow_all(),
    )
}

/// Resolves a static asset path if it falls cleanly under a mounted static root and passes `permissions`.
pub fn resolve_static_file_checked(
    statics: &[(Box<str>, Box<str>)],
    method: &str,
    path: &str,
    permissions: &crate::vm::Permissions,
) -> Option<ResolvedStaticFile> {
    if method != "GET" && method != "HEAD" {
        return None;
    }
    for (url_prefix, dir_path) in statics {
        if !route_prefix_matches(url_prefix, path) {
            continue;
        }
        let p = url_prefix.trim_end_matches('/');
        let rel = path.strip_prefix(p).unwrap_or("").trim_start_matches('/');
        if rel.is_empty()
            || rel.contains('\0')
            || rel.split(['/', '\\']).any(|seg| seg == ".." || seg == ".")
        {
            return None;
        }
        let Ok(base_dir) = std::fs::canonicalize(dir_path.as_ref()) else {
            continue;
        };
        let candidate = base_dir.join(rel);
        let Ok(canon) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        if !canon.starts_with(&base_dir) || !permissions.is_read_allowed(&canon) {
            return None;
        }
        let content_type = content_type_for_path(&canon);
        if method == "HEAD" {
            let Ok(meta) = std::fs::metadata(&canon) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            return Some(ResolvedStaticFile {
                content_type,
                body: StaticFileBody::HeadOnly { len: meta.len() },
            });
        }
        let Ok(file) = std::fs::File::open(&canon) else {
            continue;
        };
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        return Some(ResolvedStaticFile {
            content_type,
            body: StaticFileBody::File {
                file,
                len: meta.len(),
            },
        });
    }
    None
}

pub fn try_serve_static(statics: &[(Box<str>, Box<str>)], method: &str, path: &str) -> Option<Obj> {
    let resolved = resolve_static_file(statics, method, path)?;
    match resolved.body {
        StaticFileBody::HeadOnly { .. } => {
            Some(make_response_obj(200, "", Some(resolved.content_type)))
        }
        StaticFileBody::File { mut file, len } => {
            let mut bytes = Vec::with_capacity((len as usize).min(MAX_BODY));
            file.read_to_end(&mut bytes).ok()?;
            let body = String::from_utf8_lossy(&bytes).into_owned();
            Some(make_response_obj(200, body, Some(resolved.content_type)))
        }
    }
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

const METHOD_IDX_ANY: usize = 7;
const METHOD_MASK_ANY: u16 = 1 << 7;
const METHOD_MASK_OTHER: u16 = 1 << 8;
const METHOD_MASK_ALL: u16 = 0x01FF;

#[inline]
fn classify_method(method: &str) -> Option<usize> {
    match method {
        "GET" => Some(0),
        "POST" => Some(1),
        "PUT" => Some(2),
        "DELETE" => Some(3),
        "PATCH" => Some(4),
        "HEAD" => Some(5),
        "OPTIONS" => Some(6),
        "*" => Some(METHOD_IDX_ANY),
        _ => {
            if method.eq_ignore_ascii_case("GET") {
                Some(0)
            } else if method.eq_ignore_ascii_case("POST") {
                Some(1)
            } else if method.eq_ignore_ascii_case("PUT") {
                Some(2)
            } else if method.eq_ignore_ascii_case("DELETE") {
                Some(3)
            } else if method.eq_ignore_ascii_case("PATCH") {
                Some(4)
            } else if method.eq_ignore_ascii_case("HEAD") {
                Some(5)
            } else if method.eq_ignore_ascii_case("OPTIONS") {
                Some(6)
            } else {
                None
            }
        }
    }
}

#[inline]
fn method_mask_for_route(route_method: &str) -> u16 {
    match classify_method(route_method) {
        Some(METHOD_IDX_ANY) => METHOD_MASK_ALL,
        Some(idx) => 1u16 << idx,
        None => METHOD_MASK_OTHER,
    }
}

#[inline]
fn method_mask_for_request(req_method: &str) -> u16 {
    match classify_method(req_method) {
        Some(METHOD_IDX_ANY) => METHOD_MASK_ANY,
        Some(idx) => (1u16 << idx) | METHOD_MASK_ANY,
        None => METHOD_MASK_OTHER | METHOD_MASK_ANY,
    }
}

#[derive(Debug, Clone)]
struct StaticPathBucket {
    first_by_method: [u32; 8],
    other_methods: Vec<(Box<str>, u32)>,
}

impl Default for StaticPathBucket {
    fn default() -> Self {
        Self {
            first_by_method: [u32::MAX; 8],
            other_methods: Vec::new(),
        }
    }
}

impl StaticPathBucket {
    fn insert(&mut self, route_idx: usize, method: &str) {
        let idx = route_idx as u32;
        if let Some(m_idx) = classify_method(method) {
            if self.first_by_method[m_idx] == u32::MAX {
                self.first_by_method[m_idx] = idx;
            }
        } else if !self
            .other_methods
            .iter()
            .any(|(m, _)| m.eq_ignore_ascii_case(method))
        {
            self.other_methods.push((method.into(), idx));
        }
    }

    #[inline]
    fn best_for_method(&self, req_method: &str) -> Option<usize> {
        let any_idx = self.first_by_method[METHOD_IDX_ANY];
        let best = match classify_method(req_method) {
            Some(METHOD_IDX_ANY) => any_idx,
            Some(idx) => self.first_by_method[idx].min(any_idx),
            None => {
                let mut best = any_idx;
                for (m, idx) in &self.other_methods {
                    if m.eq_ignore_ascii_case(req_method) {
                        best = best.min(*idx);
                        break;
                    }
                }
                best
            }
        };
        (best != u32::MAX).then_some(best as usize)
    }
}

#[derive(Debug, Clone)]
struct DynamicTrieNode {
    min_route_idx: usize,
    method_mask: u16,
    literals: HashMap<Box<str>, DynamicTrieNode, FxBuildHasher>,
    param: Option<Box<DynamicTrieNode>>,
    terminal_routes: Vec<usize>,
    wildcard_routes: Vec<usize>,
}

impl Default for DynamicTrieNode {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicTrieNode {
    fn new() -> Self {
        Self {
            min_route_idx: usize::MAX,
            method_mask: 0,
            literals: HashMap::with_hasher(FxBuildHasher),
            param: None,
            terminal_routes: Vec::new(),
            wildcard_routes: Vec::new(),
        }
    }

    fn insert(&mut self, route_idx: usize, entry: &RouteEntry) {
        let mask = method_mask_for_route(&entry.method);
        let mut node = self;
        for seg in &entry.segments {
            node.min_route_idx = node.min_route_idx.min(route_idx);
            node.method_mask |= mask;
            match seg {
                RouteSeg::Lit(lit) => {
                    node = node
                        .literals
                        .entry(lit.clone())
                        .or_default();
                }
                RouteSeg::Param(_) => {
                    node = node
                        .param
                        .get_or_insert_with(|| Box::new(DynamicTrieNode::new()));
                }
                RouteSeg::Wildcard(_) => {
                    node.wildcard_routes.push(route_idx);
                    return;
                }
            }
        }
        node.min_route_idx = node.min_route_idx.min(route_idx);
        node.method_mask |= mask;
        node.terminal_routes.push(route_idx);
    }

    fn search_best(
        &self,
        routes: &[RouteEntry],
        method: &str,
        req_mask: u16,
        path_segs: &[&str],
        depth: usize,
        best_idx: &mut usize,
    ) {
        if self.min_route_idx >= *best_idx || (self.method_mask & req_mask) == 0 {
            return;
        }
        for &idx in &self.wildcard_routes {
            if idx >= *best_idx {
                break;
            }
            if route_matches_method(&routes[idx].method, method) {
                *best_idx = idx;
                break;
            }
        }
        if depth == path_segs.len() {
            for &idx in &self.terminal_routes {
                if idx >= *best_idx {
                    break;
                }
                if route_matches_method(&routes[idx].method, method) {
                    *best_idx = idx;
                    break;
                }
            }
            return;
        }
        let seg = path_segs[depth];
        let lit_child = self.literals.get(seg);
        let param_child = if seg.is_empty() {
            None
        } else {
            self.param.as_deref()
        };
        match (lit_child, param_child) {
            (Some(lc), Some(pc)) => {
                if lc.min_route_idx <= pc.min_route_idx {
                    lc.search_best(routes, method, req_mask, path_segs, depth + 1, best_idx);
                    pc.search_best(routes, method, req_mask, path_segs, depth + 1, best_idx);
                } else {
                    pc.search_best(routes, method, req_mask, path_segs, depth + 1, best_idx);
                    lc.search_best(routes, method, req_mask, path_segs, depth + 1, best_idx);
                }
            }
            (Some(child), None) | (None, Some(child)) => {
                child.search_best(routes, method, req_mask, path_segs, depth + 1, best_idx);
            }
            (None, None) => {}
        }
    }

    fn matches_any_method(&self, path_segs: &[&str], depth: usize) -> bool {
        if !self.wildcard_routes.is_empty() {
            return true;
        }
        if depth == path_segs.len() {
            return !self.terminal_routes.is_empty();
        }
        let seg = path_segs[depth];
        if let Some(lc) = self.literals.get(seg)
            && lc.matches_any_method(path_segs, depth + 1)
        {
            return true;
        }
        if !seg.is_empty()
            && let Some(pc) = self.param.as_deref()
            && pc.matches_any_method(path_segs, depth + 1)
        {
            return true;
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct RouteIndex {
    pub(crate) indexed_len: usize,
    static_by_path: HashMap<Box<str>, StaticPathBucket, FxBuildHasher>,
    dynamic_root: DynamicTrieNode,
}

impl Default for RouteIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteIndex {
    pub fn new() -> Self {
        Self {
            indexed_len: 0,
            static_by_path: HashMap::with_hasher(FxBuildHasher),
            dynamic_root: DynamicTrieNode::new(),
        }
    }

    pub fn from_routes(routes: &[RouteEntry]) -> Self {
        let mut index = Self::new();
        for (idx, entry) in routes.iter().enumerate() {
            index.insert(idx, entry);
        }
        index
    }

    pub fn insert(&mut self, route_idx: usize, entry: &RouteEntry) {
        debug_assert_eq!(route_idx, self.indexed_len);
        self.indexed_len = self.indexed_len.max(route_idx + 1);
        if entry.is_static {
            self.static_by_path
                .entry(entry.trimmed_pattern.clone())
                .or_default()
                .insert(route_idx, &entry.method);
        } else {
            self.dynamic_root.insert(route_idx, entry);
        }
    }

    pub fn lookup<'a>(
        &self,
        routes: &'a [RouteEntry],
        method: &str,
        path: &str,
    ) -> Result<(&'a RouteEntry, MatchedParams), u16> {
        let trimmed = path.trim_matches('/');
        let static_bucket = self.static_by_path.get(trimmed);
        let mut best_idx = static_bucket
            .and_then(|b| b.best_for_method(method))
            .unwrap_or(usize::MAX);

        let mut segs = PathSegCache::new(trimmed);
        let req_mask = method_mask_for_request(method);
        if self.dynamic_root.min_route_idx < best_idx
            && (self.dynamic_root.method_mask & req_mask) != 0
        {
            self.dynamic_root
                .search_best(routes, method, req_mask, segs.get(), 0, &mut best_idx);
        }

        if best_idx != usize::MAX {
            let entry = &routes[best_idx];
            let params = if entry.is_static {
                MatchedParams::None
            } else {
                extract_matched_params(entry, segs.get())
            };
            return Ok((entry, params));
        }

        if static_bucket.is_some() {
            return Err(405);
        }
        if self.dynamic_root.min_route_idx != usize::MAX
            && self.dynamic_root.matches_any_method(segs.get(), 0)
        {
            return Err(405);
        }
        Err(404)
    }
}

struct PathSegCache<'a> {
    trimmed: &'a str,
    stack: [&'a str; 16],
    overflow: Vec<&'a str>,
    count: Option<usize>,
}

impl<'a> PathSegCache<'a> {
    #[inline]
    fn new(trimmed: &'a str) -> Self {
        Self {
            trimmed,
            stack: [""; 16],
            overflow: Vec::new(),
            count: None,
        }
    }

    #[inline]
    fn get(&mut self) -> &[&'a str] {
        let n = match self.count {
            Some(n) => n,
            None => {
                let n = split_path_segs(self.trimmed, &mut self.stack, &mut self.overflow);
                self.count = Some(n);
                n
            }
        };
        if self.overflow.is_empty() {
            &self.stack[..n]
        } else {
            &self.overflow[..]
        }
    }
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

macro_rules! define_common_headers {
    ($($key:literal => $slot:ident),+ $(,)?) => {
        $(static $slot: LazyLock<Arc<VmString>> = LazyLock::new(|| Arc::new(VmString::from($key)));)+

        pub(crate) fn header_key(name: &str) -> Arc<VmString> {
            match name {
                $($key => Arc::clone(&$slot),)+
                _ => Arc::new(VmString::from(name)),
            }
        }

        pub(crate) fn header_key_bytes(name: &[u8]) -> Arc<VmString> {
            $(
                if name.eq_ignore_ascii_case($key.as_bytes()) {
                    return Arc::clone(&$slot);
                }
            )+
            Arc::new(VmString::from(String::from_utf8_lossy(name).to_ascii_lowercase()))
        }
    };
}

define_common_headers!(
    "accept" => HDR_ACCEPT,
    "accept-encoding" => HDR_ACCEPT_ENCODING,
    "accept-language" => HDR_ACCEPT_LANGUAGE,
    "authorization" => HDR_AUTHORIZATION,
    "cache-control" => HDR_CACHE_CONTROL,
    "connection" => HDR_CONNECTION,
    "content-length" => HDR_CONTENT_LENGTH,
    "content-type" => HDR_CONTENT_TYPE,
    "cookie" => HDR_COOKIE,
    "host" => HDR_HOST,
    "if-modified-since" => HDR_IF_MODIFIED_SINCE,
    "if-none-match" => HDR_IF_NONE_MATCH,
    "location" => HDR_LOCATION,
    "origin" => HDR_ORIGIN,
    "referer" => HDR_REFERER,
    "upgrade-insecure-requests" => HDR_UPGRADE_INSECURE_REQUESTS,
    "user-agent" => HDR_USER_AGENT,
    "x-forwarded-for" => HDR_X_FORWARDED_FOR,
    "x-forwarded-host" => HDR_X_FORWARDED_HOST,
    "x-forwarded-proto" => HDR_X_FORWARDED_PROTO,
    "x-real-ip" => HDR_X_REAL_IP,
    "x-request-id" => HDR_X_REQUEST_ID,
    "access-control-allow-headers" => HDR_CORS_HEADERS,
    "access-control-allow-methods" => HDR_CORS_METHODS,
    "access-control-allow-origin" => HDR_CORS_ORIGIN,
);

pub(crate) fn intern_http_method(method: &str) -> Arc<VmString> {
    macro_rules! common_methods {
        ($($m:literal => $slot:ident),+ $(,)?) => {{
            $(static $slot: LazyLock<Arc<VmString>> = LazyLock::new(|| Arc::new(VmString::from($m)));)+
            match method {
                $($m => Arc::clone(&$slot),)+
                _ => Arc::new(VmString::from(method)),
            }
        }};
    }
    common_methods!(
        "GET" => METHOD_GET,
        "POST" => METHOD_POST,
        "PUT" => METHOD_PUT,
        "DELETE" => METHOD_DELETE,
        "PATCH" => METHOD_PATCH,
        "HEAD" => METHOD_HEAD,
        "OPTIONS" => METHOD_OPTIONS,
    )
}

fn make_static_ct_headers(ct: Option<&str>) -> Obj {
    let mut headers = OrderedMap::default();
    if let Some(ct) = ct {
        headers.set(
            MapKey::Str(header_key("content-type")),
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

pub(crate) fn request_from_fields(mut fields: [Obj; 6]) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_REQUEST, 0, 8);
    match &fields[2] {
        Obj::Str(text) => {
            let text = Arc::clone(text);
            s.set_obj(6, Obj::deferred_field(move || Obj::Bytes(Arc::new(text.as_bytes().to_vec()))));
        }
        Obj::Bytes(bytes) => {
            s.set_obj(6, Obj::Bytes(Arc::clone(bytes)));
            let bytes = Arc::clone(bytes);
            fields[2] = Obj::deferred_field(move || {
                Obj::str(String::from_utf8_lossy(&bytes).into_owned())
            });
        }
        _ => {}
    }
    let headers = if matches!(&fields[3], Obj::Map(map) if map.is_empty()) {
        HEADERS_EMPTY.clone()
    } else {
        let headers = fields[3].clone();
        Obj::deferred_field(move || {
            let Obj::Map(headers) = headers.resolved() else { return HEADERS_EMPTY.clone() };
            let mut raw = OrderedMap::with_capacity(headers.len());
            for (key, value) in headers.entries() {
                if let MapVal::Obj(Obj::Str(text)) = value {
                    raw.set(key.clone(), MapVal::Obj(Obj::Bytes(Arc::new(text.as_bytes().to_vec()))));
                }
            }
            Obj::Map(Arc::new(raw))
        })
    };
    s.set_obj(7, headers);
    for (idx, value) in fields.into_iter().enumerate() {
        s.set_obj(idx, value);
    }
    Obj::Record(Arc::new(s))
}

pub fn make_request_obj(
    method: &str,
    path: &str,
    body: String,
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    params: Vec<(String, String)>,
) -> Obj {
    fn deferred_pairs(pairs: Vec<(String, String)>) -> Obj {
        if pairs.is_empty() { HEADERS_EMPTY.clone() }
        else { Obj::deferred_field(move || map_from_pairs(pairs)) }
    }
    request_from_fields([
        Obj::str(method),
        Obj::str(path),
        Obj::str(body),
        deferred_pairs(headers),
        deferred_pairs(query),
        deferred_pairs(params),
    ])
}

pub struct RawHttpRequest {
    pub method: Arc<VmString>,
    pub path: String,
    pub query: String,
    pub headers: Vec<(Arc<VmString>, Vec<u8>)>,
    pub body: Vec<u8>,
    pub keep_alive: bool,
}

impl RawHttpRequest {
    pub(crate) fn into_request_obj(self) -> Obj {
        let mut req = StructData::new(SCHEMA_HTTP_REQUEST, 0, 8);
        req.set_obj(0, Obj::Str(self.method));
        req.set_obj(1, Obj::str(self.path));
        let body = Arc::new(self.body);
        let text = Arc::clone(&body);
        req.set_obj(2, Obj::deferred_field(move || {
            Obj::str(String::from_utf8_lossy(&text).into_owned())
        }));
        req.set_obj(6, Obj::Bytes(body));
        if self.headers.is_empty() {
            req.set_obj(3, HEADERS_EMPTY.clone());
            req.set_obj(7, HEADERS_EMPTY.clone());
        } else {
            let headers = Arc::new(self.headers);
            let text_headers = Arc::clone(&headers);
            req.set_obj(3, Obj::deferred_field(move || build_request_headers(&text_headers, false)));
            req.set_obj(7, Obj::deferred_field(move || build_request_headers(&headers, true)));
        }
        let query = self.query;
        req.set_obj(4, if query.is_empty() { HEADERS_EMPTY.clone() } else {
            Obj::deferred_field(move || query_map(&query))
        });
        req.set_obj(5, HEADERS_EMPTY.clone());
        Obj::Record(Arc::new(req))
    }
}

fn build_request_headers(headers: &[(Arc<VmString>, Vec<u8>)], binary: bool) -> Obj {
    let mut map = OrderedMap::with_capacity(headers.len());
    for (key, value) in headers {
        let value = if binary {
            Obj::Bytes(Arc::new(value.clone()))
        } else {
            Obj::str(String::from_utf8_lossy(value).into_owned())
        };
        map.set(MapKey::Str(Arc::clone(key)), MapVal::Obj(value));
    }
    Obj::Map(Arc::new(map))
}

fn query_map(raw: &str) -> Obj {
    let pairs = raw.split('&').filter(|pair| !pair.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (
            crate::native::url::percent_decode(k, true).unwrap_or_else(|_| k.to_string()),
            crate::native::url::percent_decode(v, true).unwrap_or_else(|_| v.to_string()),
        )
    }).collect();
    map_from_pairs(pairs)
}

#[derive(Debug)]
pub enum ReadHttpError {
    Closed,
    Bad(String),
    TooLarge,
    Timeout,
    Io(io::Error),
}

pub trait DeadlineSource {
    fn set_idle(&mut self, idle_timeout: Duration, header_timeout: Duration);
    fn arm_deadline(&mut self, timeout: Duration);
    fn has_deadline(&self) -> bool;
}

pub struct DeadlineState {
    deadline: Option<Instant>,
    arm_on_first_byte: Option<Duration>,
    idle_timeout: Duration,
}

impl DeadlineState {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            deadline: None,
            arm_on_first_byte: None,
            idle_timeout,
        }
    }

    pub fn set_idle(&mut self, idle_timeout: Duration, header_timeout: Duration) {
        self.deadline = None;
        self.idle_timeout = idle_timeout;
        self.arm_on_first_byte = Some(header_timeout);
    }

    pub fn arm_deadline(&mut self, timeout: Duration) {
        self.arm_on_first_byte = None;
        self.deadline = Some(Instant::now() + timeout);
    }

    #[inline]
    pub fn has_deadline(&self) -> bool {
        self.deadline.is_some()
    }

    #[inline]
    pub fn next_read_timeout(&self) -> io::Result<Duration> {
        if let Some(dl) = self.deadline {
            let rem = dl.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HTTP request deadline exceeded",
                ));
            }
            Ok(rem.max(Duration::from_millis(1)))
        } else {
            Ok(self.idle_timeout.max(Duration::from_millis(1)))
        }
    }

    #[inline]
    pub fn on_read_ok(&mut self, bytes_read: usize) {
        if bytes_read > 0
            && let Some(phase_dur) = self.arm_on_first_byte.take()
            && self.deadline.is_none()
        {
            self.deadline = Some(Instant::now() + phase_dur);
        }
    }
}

#[cfg(windows)]
fn wait_readable_windows(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket as _;

    #[repr(C)]
    struct WsaPollFd {
        fd: usize,
        events: i16,
        revents: i16,
    }

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAPoll(fdarray: *mut WsaPollFd, nfds: u32, timeout: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }

    const POLLRDNORM: i16 = 0x0100;
    let timeout_ms = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
    let mut pollfd = WsaPollFd {
        fd: stream.as_raw_socket() as usize,
        events: POLLRDNORM,
        revents: 0,
    };
    // SAFETY: `pollfd` points to one live `SOCKET`; polling avoids canceling a blocking `recv` via `SO_RCVTIMEO`.
    let rc = unsafe { WSAPoll(&mut pollfd, 1, timeout_ms) };
    if rc == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "HTTP request deadline exceeded",
        ));
    }
    if rc < 0 {
        // SAFETY: Called immediately after `WSAPoll` returned `SOCKET_ERROR` (-1) on the current thread.
        let code = unsafe { WSAGetLastError() };
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(())
}

pub struct TcpDeadlineStream {
    inner: TcpStream,
    state: DeadlineState,
    #[cfg(not(windows))]
    last_timeout_ms: Option<u64>,
}

impl TcpDeadlineStream {
    pub fn new(inner: TcpStream, initial_idle: Duration) -> Self {
        #[cfg(windows)]
        let _ = inner.set_read_timeout(None);
        Self {
            inner,
            state: DeadlineState::new(initial_idle),
            #[cfg(not(windows))]
            last_timeout_ms: None,
        }
    }

    #[cfg(not(windows))]
    fn apply_timeout(&mut self, dur: Duration) {
        let ms = dur.as_millis().clamp(1, u64::MAX as u128) as u64;
        if self.last_timeout_ms != Some(ms) {
            let _ = self.inner.set_read_timeout(Some(Duration::from_millis(ms)));
            self.last_timeout_ms = Some(ms);
        }
    }
}

impl DeadlineSource for TcpDeadlineStream {
    fn set_idle(&mut self, idle_timeout: Duration, header_timeout: Duration) {
        self.state.set_idle(idle_timeout, header_timeout);
        #[cfg(not(windows))]
        self.apply_timeout(idle_timeout);
    }

    fn arm_deadline(&mut self, timeout: Duration) {
        self.state.arm_deadline(timeout);
        #[cfg(not(windows))]
        self.apply_timeout(timeout);
    }

    fn has_deadline(&self) -> bool {
        self.state.has_deadline()
    }
}

impl Read for TcpDeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let timeout = self.state.next_read_timeout()?;
        #[cfg(windows)]
        wait_readable_windows(&self.inner, timeout)?;
        #[cfg(not(windows))]
        self.apply_timeout(timeout);
        let n = self.inner.read(buf)?;
        self.state.on_read_ok(n);
        Ok(n)
    }
}

fn is_http_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~'
        )
}

#[inline]
pub fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(is_http_tchar)
}

#[inline]
pub fn is_valid_header_value(val: &str) -> bool {
    !val.bytes().any(|b| (b < b' ' && b != b'\t') || b == 127)
}

#[inline]
pub(crate) fn find_crlf_crlf(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    for i in 0..=buf.len() - 4 {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
    }
    None
}

#[inline]
pub(crate) fn has_bare_lf(buf: &[u8], base_prev_byte: Option<u8>) -> bool {
    for (i, &b) in buf.iter().enumerate() {
        if b == b'\n' {
            let prev = if i == 0 { base_prev_byte } else { Some(buf[i - 1]) };
            if prev != Some(b'\r') {
                return true;
            }
        }
    }
    false
}

fn read_http_head<R: BufRead, M>(
    reader: &mut R,
    map_io_err: &M,
) -> Result<Vec<u8>, ReadHttpError>
where
    M: Fn(&R, io::Error) -> ReadHttpError,
{
    let mut head = Vec::with_capacity(2048);
    loop {
        let prev_len = head.len();
        let prev_byte = head.last().copied();
        let (avail_len, take) = match reader.fill_buf() {
            Ok(avail) => {
                if avail.is_empty() {
                    return Err(ReadHttpError::Closed);
                }
                let cap_left = MAX_HEADER_BYTES - prev_len;
                if cap_left == 0 {
                    return Err(ReadHttpError::TooLarge);
                }
                let take = avail.len().min(cap_left);
                head.extend_from_slice(&avail[..take]);
                (avail.len(), take)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(map_io_err(reader, e)),
        };

        let search_from = prev_len.saturating_sub(3);
        if let Some(rel_end) = find_crlf_crlf(&head[search_from..]) {
            let end_in_head = search_from + rel_end;
            if has_bare_lf(&head[prev_len..end_in_head], prev_byte) {
                return Err(ReadHttpError::Bad("Bare LF in HTTP request".to_string()));
            }
            reader.consume(end_in_head - prev_len);
            head.truncate(end_in_head);
            return Ok(head);
        }
        if has_bare_lf(&head[prev_len..], prev_byte) {
            return Err(ReadHttpError::Bad("Bare LF in HTTP request".to_string()));
        }
        reader.consume(take);
        if head.len() >= MAX_HEADER_BYTES || (take < avail_len && head.len() >= MAX_HEADER_BYTES) {
            return Err(ReadHttpError::TooLarge);
        }
    }
}

#[inline]
fn next_crlf_line<'a>(rem: &mut &'a [u8]) -> Option<&'a [u8]> {
    for i in 0..rem.len().saturating_sub(1) {
        if rem[i] == b'\r' && rem[i + 1] == b'\n' {
            let line = &rem[..i];
            *rem = &rem[i + 2..];
            return Some(line);
        }
    }
    let line = *rem;
    *rem = &[];
    Some(line)
}

#[inline]
fn map_deadline_io_err<R: DeadlineSource>(source: &R, err: io::Error) -> ReadHttpError {
    if matches!(err.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
        if source.has_deadline() {
            ReadHttpError::Timeout
        } else {
            ReadHttpError::Closed
        }
    } else {
        ReadHttpError::Io(err)
    }
}

pub struct ParsedHttpHead {
    pub method: Arc<VmString>,
    pub path: String,
    pub query: String,
    pub headers: Vec<(Arc<VmString>, Vec<u8>)>,
    pub content_length: usize,
    pub keep_alive: bool,
}

pub fn parse_http_head(mut rem: &[u8]) -> Result<ParsedHttpHead, ReadHttpError> {
    let req_line_bytes = next_crlf_line(&mut rem).unwrap_or(&[]);
    if req_line_bytes.len() > MAX_LINE {
        return Err(ReadHttpError::TooLarge);
    }
    if req_line_bytes.is_empty() {
        return Err(ReadHttpError::Bad("Missing HTTP request line".to_string()));
    }
    if req_line_bytes.contains(&b'\r') {
        return Err(ReadHttpError::Bad("Bare CR in HTTP request line".to_string()));
    }
    let req_line = std::str::from_utf8(req_line_bytes)
        .map_err(|_| ReadHttpError::Bad("Request line is not valid UTF-8".to_owned()))?;
    let mut parts = req_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadHttpError::Bad("Malformed HTTP request line".to_string()));
    };
    if method.is_empty() || !method.bytes().all(is_http_tchar) {
        return Err(ReadHttpError::Bad("Invalid HTTP method".to_string()));
    }
    if target.is_empty() || target.bytes().any(|b| b <= b' ' || b == 127) {
        return Err(ReadHttpError::Bad("Invalid HTTP target".to_string()));
    }
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(ReadHttpError::Bad(format!("Unsupported HTTP version '{}'", version)));
    }
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let mut headers = Vec::new();
    let mut content_length: usize = 0;
    let mut saw_content_length = false;
    let mut conn_close = false;
    let mut conn_keep_alive = false;
    while !rem.is_empty() {
        let Some(line) = next_crlf_line(&mut rem) else {
            break;
        };
        if line.len() > MAX_LINE {
            return Err(ReadHttpError::TooLarge);
        }
        if line.is_empty() {
            return Err(ReadHttpError::Bad("Unexpected empty header line".to_string()));
        }
        if line.contains(&b'\r') {
            return Err(ReadHttpError::Bad("Bare CR in HTTP header".to_string()));
        }
        if headers.len() >= MAX_HEADERS {
            return Err(ReadHttpError::TooLarge);
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            return Err(ReadHttpError::Bad("Malformed HTTP header".to_string()));
        };
        let name = &line[..colon];
        if name.is_empty() || !name.iter().copied().all(is_http_tchar) {
            return Err(ReadHttpError::Bad("Invalid HTTP header name".to_string()));
        }
        let mut val = &line[colon + 1..];
        while val.first().is_some_and(|&b| matches!(b, b' ' | b'\t')) {
            val = &val[1..];
        }
        while val.last().is_some_and(|&b| matches!(b, b' ' | b'\t')) {
            val = &val[..val.len() - 1];
        }
        if val.iter().any(|&b| (b < b' ' && b != b'\t') || b == 127) {
            return Err(ReadHttpError::Bad("Invalid HTTP header value".to_string()));
        }
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Err(ReadHttpError::Bad(
                "Transfer-Encoding is not supported".to_string(),
            ));
        } else if name.eq_ignore_ascii_case(b"content-length") {
            if saw_content_length {
                return Err(ReadHttpError::Bad("Duplicate Content-Length header".to_string()));
            }
            saw_content_length = true;
            if val.is_empty() || !val.iter().all(u8::is_ascii_digit) {
                return Err(ReadHttpError::Bad("Invalid Content-Length".to_string()));
            }
            content_length = std::str::from_utf8(val)
                .ok()
                .and_then(|val| val.parse::<usize>().ok())
                .ok_or(())
                .map_err(|_| ReadHttpError::Bad("Invalid Content-Length".to_string()))?;
            if content_length > MAX_BODY {
                return Err(ReadHttpError::TooLarge);
            }
        } else if name.eq_ignore_ascii_case(b"connection") {
            for part in val.split(|&b| b == b',') {
                let mut tok = part;
                while tok.first().is_some_and(|&b| matches!(b, b' ' | b'\t')) {
                    tok = &tok[1..];
                }
                while tok.last().is_some_and(|&b| matches!(b, b' ' | b'\t')) {
                    tok = &tok[..tok.len() - 1];
                }
                if tok.eq_ignore_ascii_case(b"close") {
                    conn_close = true;
                } else if tok.eq_ignore_ascii_case(b"keep-alive") {
                    conn_keep_alive = true;
                }
            }
        }
        headers.push((header_key_bytes(name), val.to_vec()));
    }

    let keep_alive = if version == "HTTP/1.0" {
        conn_keep_alive && !conn_close
    } else {
        !conn_close
    };

    Ok(ParsedHttpHead {
        method: intern_http_method(method),
        path: raw_path.to_string(),
        query: raw_query.to_owned(),
        headers,
        content_length,
        keep_alive,
    })
}

fn read_http_request_inner<R: BufRead, F, M>(
    reader: &mut R,
    on_headers_done: F,
    map_io_err: M,
) -> Result<RawHttpRequest, ReadHttpError>
where
    F: FnOnce(&mut R),
    M: Fn(&R, io::Error) -> ReadHttpError,
{
    let head_buf = read_http_head(reader, &map_io_err)?;
    let parsed = parse_http_head(&head_buf[..head_buf.len() - 4])?;

    on_headers_done(reader);
    let mut body_bytes = Vec::with_capacity(parsed.content_length.min(BODY_INITIAL_CAP));
    if parsed.content_length > 0 {
        let mut remaining = parsed.content_length;
        let mut chunk = [0u8; 16 * 1024];
        while remaining > 0 {
            let want = remaining.min(chunk.len());
            match reader.read(&mut chunk[..want]) {
                Ok(0) => {
                    return Err(map_io_err(
                        reader,
                        io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF in HTTP body"),
                    ));
                }
                Ok(n) => {
                    body_bytes.extend_from_slice(&chunk[..n]);
                    remaining -= n;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(map_io_err(reader, e)),
            }
        }
    }

    Ok(RawHttpRequest {
        method: parsed.method,
        path: parsed.path,
        query: parsed.query,
        headers: parsed.headers,
        body: body_bytes,
        keep_alive: parsed.keep_alive,
    })
}

pub fn read_http_request<R: BufRead>(reader: &mut R) -> Result<RawHttpRequest, ReadHttpError> {
    read_http_request_inner(reader, |_| {}, |_, e| ReadHttpError::Io(e))
}

pub fn read_http_request_deadline<R: Read + DeadlineSource>(
    reader: &mut BufReader<R>,
    idle_timeout: Duration,
    header_timeout: Duration,
    body_timeout: Duration,
) -> Result<RawHttpRequest, ReadHttpError> {
    if reader.buffer().is_empty() {
        reader.get_mut().set_idle(idle_timeout, header_timeout);
    } else {
        reader.get_mut().arm_deadline(header_timeout);
    }
    read_http_request_inner(
        reader,
        |r| r.get_mut().arm_deadline(body_timeout),
        |r, e| map_deadline_io_err(r.get_ref(), e),
    )
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
        408 => "Request Timeout",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

pub fn write_http_response_head<W: Write>(
    writer: &mut W,
    status: i64,
    content_length: u64,
    headers: Option<&OrderedMap>,
    keep_alive: bool,
) -> io::Result<()> {
    if !(100..=999).contains(&status) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HTTP status code",
        ));
    }
    if let Some(map) = headers {
        for (k, v) in map.entries.iter() {
            if let (MapKey::Str(ks), MapVal::Obj(Obj::Str(vs))) = (k, v) {
                let k_str = ks.as_str();
                if k_str.eq_ignore_ascii_case("content-length")
                    || k_str.eq_ignore_ascii_case("connection")
                    || k_str.eq_ignore_ascii_case("transfer-encoding")
                {
                    continue;
                }
                if !is_valid_header_name(k_str) || !is_valid_header_value(vs.as_str()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid HTTP response header",
                    ));
                }
            }
        }
    }
    write!(
        writer,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        status,
        reason_phrase(status),
        content_length,
        if keep_alive { "keep-alive" } else { "close" }
    )?;
    if let Some(map) = headers {
        for (k, v) in map.entries.iter() {
            if let (MapKey::Str(ks), MapVal::Obj(Obj::Str(vs))) = (k, v) {
                let k_str = ks.as_str();
                if k_str.eq_ignore_ascii_case("content-length")
                    || k_str.eq_ignore_ascii_case("connection")
                    || k_str.eq_ignore_ascii_case("transfer-encoding")
                {
                    continue;
                }
                write!(writer, "{}: {}\r\n", k_str, vs.as_str())?;
            }
        }
    }
    writer.write_all(b"\r\n")
}

pub fn write_http_response_ext<W: Write>(
    writer: &mut W,
    status: i64,
    body: &str,
    headers: Option<&OrderedMap>,
    keep_alive: bool,
    head_only: bool,
) -> io::Result<()> {
    write_http_response_head(writer, status, body.len() as u64, headers, keep_alive)?;
    if !head_only {
        writer.write_all(body.as_bytes())?;
    }
    writer.flush()
}

pub fn write_http_file_response<W: Write>(
    writer: &mut W,
    status: i64,
    file: Option<&mut std::fs::File>,
    len: u64,
    headers: Option<&OrderedMap>,
    keep_alive: bool,
    head_only: bool,
) -> io::Result<()> {
    write_http_response_head(writer, status, len, headers, keep_alive)?;
    if !head_only && len > 0 {
        let Some(file) = file else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "missing static file handle",
            ));
        };
        let mut remaining = len;
        let mut buf = vec![0u8; (len as usize).min(64 * 1024)];
        while remaining > 0 {
            let want = (remaining as usize).min(buf.len());
            let read_res = loop {
                match file.read(&mut buf[..want]) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    other => break other,
                }
            };
            let n = read_res?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "static file truncated while streaming",
                ));
            }
            writer.write_all(&buf[..n])?;
            remaining -= n as u64;
        }
    }
    writer.flush()
}

pub fn write_error_response(stream: &mut BufWriter<TcpStream>, status: i64, body: &str) {
    let _ = write_http_response_ext(stream, status, body, None, false, false);
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
pub(crate) fn format_error_response_bytes(status: i64, body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + body.len());
    let _ = write_http_response_ext(&mut out, status, body, None, false, false);
    out
}

pub fn reader_writer_pair(stream: TcpStream) -> io::Result<(BufReader<TcpStream>, BufWriter<TcpStream>)> {
    let writer_stream = stream.try_clone()?;
    Ok((BufReader::new(stream), BufWriter::new(writer_stream)))
}

pub fn deadline_reader_writer_pair(
    stream: TcpStream,
    initial_idle: Duration,
) -> io::Result<(BufReader<TcpDeadlineStream>, BufWriter<TcpStream>)> {
    let writer_stream = stream.try_clone()?;
    Ok((
        BufReader::new(TcpDeadlineStream::new(stream, initial_idle)),
        BufWriter::new(writer_stream),
    ))
}

const MAX_CLIENT_BODY: usize = 16 * 1024 * 1024;

fn response_headers(headers: Vec<(String, String)>) -> Obj {
    if headers.is_empty() {
        return HEADERS_EMPTY.clone();
    }
    let mut map = OrderedMap::with_capacity(headers.len());
    for (key, val) in headers {
        map.set(MapKey::Str(header_key(&key)), MapVal::Obj(Obj::str(val)));
    }
    Obj::Map(Arc::new(map))
}

fn build_response_obj(status: i64, body: &[u8], headers: Vec<(String, String)>) -> Obj {
    let body_str = String::from_utf8_lossy(body);
    let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
    s.set_scalar(0, status as u64);
    s.set_obj(0, Obj::str(body_str.as_ref()));
    s.set_obj(1, response_headers(headers));
    Obj::Record(Arc::new(s))
}

fn build_binary_response_obj(status: i64, body: Vec<u8>, headers: Vec<(String, String)>) -> Obj {
    let mut s = StructData::new(SCHEMA_HTTP_BINARY_RESPONSE, 1, 2);
    s.set_scalar(0, status as u64);
    s.set_obj(0, Obj::Bytes(Arc::new(body)));
    s.set_obj(1, response_headers(headers));
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

fn redirect_headers<'a>(
    mut headers: Cow<'a, [(String, String)]>,
    previous: &ParsedUrl,
    next: &ParsedUrl,
    discard_body: bool,
) -> Cow<'a, [(String, String)]> {
    if previous.is_https != next.is_https
        || previous.port != next.port
        || !previous.host.eq_ignore_ascii_case(&next.host)
    {
        // Arbitrary caller-supplied headers may carry credentials.
        return Cow::Borrowed(&[]);
    }
    if discard_body && !headers.is_empty() {
        headers.to_mut().retain(|(name, _)| {
            ![
                "Content-Length",
                "Transfer-Encoding",
                "Content-Type",
                "Content-Encoding",
                "Expect",
            ]
            .iter()
            .any(|header| name.eq_ignore_ascii_case(header))
        });
    }
    headers
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    is_https: bool,
    host: String,
    port: u16,
    deny_private_addrs: bool,
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
    pub deny_private_addrs: bool,
    pub pool: Arc<HttpConnPool>,
}

impl HttpClientState {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            default_headers: Vec::new(),
            timeout_ms: 30_000,
            deny_private_addrs: false,
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

pub fn is_private_or_special_ip(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || (a == 100 && (64..=127).contains(&b)) // RFC 6598 CGNAT 100.64.0.0/10
                || (a == 198 && (b == 18 || b == 19)) // RFC 2544 benchmarking 198.18.0.0/15
                || a >= 240 // Reserved 240.0.0.0/4
        }
        std::net::IpAddr::V6(v6) => {
            let segs = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local() // fc00::/7
                || v6.is_unicast_link_local() // fe80::/10
                || (segs[0] & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
                || (segs[0] == 0x2001 && segs[1] == 0x0db8) // documentation 2001:db8::/32
        }
    }
}

fn check_target_host_allowed(parsed: &ParsedUrl, deny_private_addrs: bool) -> Result<(), Obj> {
    if !deny_private_addrs {
        return Ok(());
    }
    let clean = parsed.host.trim_end_matches('.');
    if clean.eq_ignore_ascii_case("localhost") {
        return Err(make_http_error(
            "blocked_addr",
            format!("refusing connection to restricted host '{}'", parsed.host),
        ));
    }
    if let Ok(ip) = clean.parse::<std::net::IpAddr>()
        && is_private_or_special_ip(ip)
    {
        return Err(make_http_error(
            "blocked_addr",
            format!("refusing connection to private or restricted address {}", ip),
        ));
    }
    Ok(())
}

fn open_http_transport(
    parsed: &ParsedUrl,
    timeout: Duration,
    deny_private_addrs: bool,
) -> Result<Transport, Obj> {
    check_target_host_allowed(parsed, deny_private_addrs)?;
    let addr_str = format!("{}:{}", parsed.host, parsed.port);
    let addrs: Vec<_> = match addr_str.to_socket_addrs() {
        Ok(it) => it.collect(),
        Err(e) => return Err(make_http_error("dns_error", format!("failed to resolve {}: {}", addr_str, e))),
    };
    if addrs.is_empty() {
        return Err(make_http_error("dns_error", format!("no addresses resolved for {}", addr_str)));
    }
    if deny_private_addrs {
        for addr in &addrs {
            if is_private_or_special_ip(addr.ip()) {
                return Err(make_http_error(
                    "blocked_addr",
                    format!(
                        "refusing connection to private or restricted address {} (resolved from {})",
                        addr.ip(),
                        parsed.host
                    ),
                ));
            }
        }
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
        if !is_valid_header_name(k) || !is_valid_header_value(v) {
            return Err(make_http_error(
                "invalid_header",
                "invalid HTTP header name or value",
            ));
        }
        req.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body);
    Ok(req)
}

fn send_http_request(
    reader: &mut BufReader<Transport>,
    method: &str,
    request: &[u8],
) -> Result<(RawHttpResponse, bool), Obj> {
    let transport = reader.get_mut();
    transport
        .write_all(request)
        .and_then(|_| transport.flush())
        .map_err(|err| make_http_error("io_error", format!("failed to send request: {err}")))?;
    parse_http_response_raw(reader, method, false).map_err(|err| make_http_error("http_error", err))
}

fn can_retry_pooled_request(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE" | "PUT" | "DELETE")
}

enum HttpClientFailure {
    HttpErr(Obj),
    PermissionDenied(String),
}

impl From<Obj> for HttpClientFailure {
    fn from(obj: Obj) -> Self {
        Self::HttpErr(obj)
    }
}

#[allow(clippy::too_many_arguments)]
fn tcp_http_request_once(
    method: &str,
    parsed: &ParsedUrl,
    body: &[u8],
    headers: &[(String, String)],
    timeout: Duration,
    pool: Option<&HttpConnPool>,
    deny_private_addrs: bool,
    permissions: Option<&crate::vm::Permissions>,
) -> Result<RawHttpResponse, HttpClientFailure> {
    if let Some(perms) = permissions {
        perms
            .check_net(&parsed.host, parsed.port)
            .map_err(HttpClientFailure::PermissionDenied)?;
    }
    check_target_host_allowed(parsed, deny_private_addrs)?;
    let want_keep_alive = pool.is_some();
    let req = build_http_request_bytes(method, parsed, body, headers, want_keep_alive)?;
    let pool_key = PoolKey {
        is_https: parsed.is_https,
        host: parsed.host.to_ascii_lowercase(),
        port: parsed.port,
        deny_private_addrs,
    };

    if let Some(p) = pool
        && let Some(mut reader) = p.take(&pool_key)
    {
        let _ = reader.get_ref().set_read_timeout(Some(timeout));
        let _ = reader.get_ref().set_write_timeout(Some(timeout));
        match send_http_request(&mut reader, method, &req) {
            Ok((resp, can_ka)) => {
                if can_ka {
                    p.put(pool_key, reader);
                }
                return Ok(resp);
            }
            // The peer may already have handled the request before the error.
            Err(err) if !can_retry_pooled_request(method) => return Err(err.into()),
            Err(_) => {}
        }
    }

    let transport = open_http_transport(parsed, timeout, deny_private_addrs)?;
    let mut reader = BufReader::new(transport);
    let (resp, can_ka) = send_http_request(&mut reader, method, &req)?;
    if let Some(p) = pool
        && can_ka
    {
        p.put(pool_key, reader);
    }
    Ok(resp)
}

#[allow(clippy::too_many_arguments)]
fn http_client_request_raw(
    method: &str,
    url: &str,
    body: &[u8],
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: Option<&HttpConnPool>,
    deny_private_addrs: bool,
    permissions: Option<&crate::vm::Permissions>,
) -> Result<RawHttpResponse, HttpClientFailure> {
    if timeout_ms <= 0 {
        return Err(make_http_error("invalid_timeout", "timeout_ms must be > 0").into());
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
        )
        .into());
    }
    let mut parsed =
        parse_http_url(url).map_err(|err| HttpClientFailure::HttpErr(make_http_error("invalid_url", err)))?;
    let mut cur_body = body;
    let mut cur_headers = Cow::Borrowed(headers);

    for hop in 0..=MAX_REDIRECTS {
        let resp = tcp_http_request_once(
            &cur_method,
            &parsed,
            cur_body,
            &cur_headers,
            timeout,
            pool,
            deny_private_addrs,
            permissions,
        )?;
        if matches!(resp.status, 301..=303 | 307 | 308)
            && let Some((_, loc)) = resp.headers.iter().find(|(k, _)| k == "location")
        {
            if hop == MAX_REDIRECTS {
                return Err(make_http_error(
                    "too_many_redirects",
                    format!("exceeded {} redirects", MAX_REDIRECTS),
                )
                .into());
            }
            let next_url = resolve_redirect_url(&parsed, loc);
            let next = parse_http_url(&next_url)
                .map_err(|err| HttpClientFailure::HttpErr(make_http_error("invalid_url", err)))?;
            if parsed.is_https && !next.is_https {
                return Err(make_http_error(
                    "tls_error",
                    "refusing insecure redirect from https:// to http://",
                )
                .into());
            }
            let discard_body = matches!(resp.status, 301..=303) && cur_method != "HEAD";
            cur_headers = redirect_headers(cur_headers, &parsed, &next, discard_body);
            if discard_body {
                cur_method = "GET".to_string();
                cur_body = &[];
            }
            parsed = next;
            continue;
        }
        return Ok(resp);
    }

    Err(make_http_error(
        "too_many_redirects",
        format!("exceeded {} redirects", MAX_REDIRECTS),
    )
    .into())
}

fn finish_http_client_result<F: FnOnce(RawHttpResponse) -> Obj>(
    res: Result<RawHttpResponse, HttpClientFailure>,
    on_ok: F,
) -> Result<Obj, String> {
    match res {
        Ok(resp) => Ok(on_ok(resp)),
        Err(HttpClientFailure::HttpErr(err_obj)) => Ok(err_obj),
        Err(HttpClientFailure::PermissionDenied(msg)) => Err(msg),
    }
}

pub fn http_client_request(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
) -> Obj {
    http_client_request_with_timeout(method, url, body, headers, 30_000)
}

/// Performs an HTTP/HTTPS request while enforcing `permissions` on the target URL and redirects.
pub fn http_client_request_checked(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    permissions: &crate::vm::Permissions,
) -> Result<Obj, String> {
    http_client_request_with_timeout_checked(method, url, body, headers, 30_000, permissions)
}

pub fn http_client_request_with_timeout(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
) -> Obj {
    finish_http_client_result(
        http_client_request_raw(method, url, body.as_bytes(), headers, timeout_ms, None, false, None),
        |resp| build_response_obj(resp.status, &resp.body, resp.headers),
    )
    .expect("unchecked HTTP client never fails permission check")
}

/// Performs a timed HTTP/HTTPS request while enforcing `permissions` on the target URL and redirects.
pub fn http_client_request_with_timeout_checked(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
    permissions: &crate::vm::Permissions,
) -> Result<Obj, String> {
    finish_http_client_result(
        http_client_request_raw(
            method,
            url,
            body.as_bytes(),
            headers,
            timeout_ms,
            None,
            false,
            Some(permissions),
        ),
        |resp| build_response_obj(resp.status, &resp.body, resp.headers),
    )
}

pub fn http_client_pooled_request_with_options(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: &HttpConnPool,
    deny_private_addrs: bool,
) -> Obj {
    finish_http_client_result(
        http_client_request_raw(
            method,
            url,
            body.as_bytes(),
            headers,
            timeout_ms,
            Some(pool),
            deny_private_addrs,
            None,
        ),
        |resp| build_response_obj(resp.status, &resp.body, resp.headers),
    )
    .expect("unchecked HTTP client never fails permission check")
}

/// Performs a pooled HTTP/HTTPS request while enforcing `permissions` on the target URL and redirects.
#[allow(clippy::too_many_arguments)]
pub fn http_client_pooled_request_with_options_checked(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: &HttpConnPool,
    deny_private_addrs: bool,
    permissions: &crate::vm::Permissions,
) -> Result<Obj, String> {
    finish_http_client_result(
        http_client_request_raw(
            method,
            url,
            body.as_bytes(),
            headers,
            timeout_ms,
            Some(pool),
            deny_private_addrs,
            Some(permissions),
        ),
        |resp| build_response_obj(resp.status, &resp.body, resp.headers),
    )
}

pub fn http_client_pooled_request(
    method: &str,
    url: &str,
    body: &str,
    headers: &[(String, String)],
    timeout_ms: i64,
    pool: &HttpConnPool,
) -> Obj {
    http_client_pooled_request_with_options(method, url, body, headers, timeout_ms, pool, false)
}

pub fn http_client_bytes_request(
    method: &str,
    url: &str,
    body: &[u8],
    headers: &[(String, String)],
    timeout_ms: i64,
) -> Obj {
    finish_http_client_result(
        http_client_request_raw(method, url, body, headers, timeout_ms, None, false, None),
        |resp| build_binary_response_obj(resp.status, resp.body, resp.headers),
    )
    .expect("unchecked HTTP client never fails permission check")
}

/// Performs a binary HTTP/HTTPS request while enforcing `permissions` on the target URL and redirects.
pub fn http_client_bytes_request_checked(
    method: &str,
    url: &str,
    body: &[u8],
    headers: &[(String, String)],
    timeout_ms: i64,
    permissions: &crate::vm::Permissions,
) -> Result<Obj, String> {
    finish_http_client_result(
        http_client_request_raw(
            method,
            url,
            body,
            headers,
            timeout_ms,
            None,
            false,
            Some(permissions),
        ),
        |resp| build_binary_response_obj(resp.status, resp.body, resp.headers),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn binary_request_retains_body_and_header_bytes() {
        let wire = b"POST /upload?k=a+b HTTP/1.1\r\nContent-Length: 3\r\nX-Data: \xff\t \r\n\r\n\0\xff\x80";
        let request = read_http_request(&mut wire.as_slice()).unwrap();
        let Obj::Record(request) = request.into_request_obj() else { panic!("expected Request") };
        assert!(matches!(request.raw_obj(4), Obj::DeferredField(_)));
        assert!(matches!(request.get_obj(6), Obj::Bytes(bytes) if bytes.as_slice() == [0, 255, 128]));
        assert!(matches!(request.get_obj(2), Obj::Str(text) if text.as_str() == "\0��"));
        let Obj::Map(headers) = request.get_obj(7) else { panic!("expected headers") };
        assert!(matches!(headers.get_str("x-data"), Some(MapVal::Obj(Obj::Bytes(bytes))) if bytes.as_slice() == [255]));
        let Obj::Map(query) = request.get_obj(4) else { panic!("expected query") };
        assert!(matches!(query.get_str("k"), Some(MapVal::Obj(Obj::Str(value))) if value.as_str() == "a b"));
    }

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
    fn redirect_headers_are_scoped_to_the_original_origin() {
        let original = parse_http_url("https://api.example.test/start").unwrap();
        let headers = [("X-Service-Key".into(), "test-only".into())];
        let same_origin = parse_http_url("https://API.example.test:443/next").unwrap();
        assert_eq!(
            redirect_headers(Cow::Borrowed(&headers), &original, &same_origin, false).as_ref(),
            &headers
        );

        for url in [
            "https://other.example.test/next",
            "https://api.example.test:8443/next",
            "http://api.example.test:443/next",
        ] {
            let next = parse_http_url(url).unwrap();
            let forwarded = redirect_headers(Cow::Borrowed(&headers), &original, &next, false);
            assert!(forwarded.is_empty());
            assert!(redirect_headers(forwarded, &next, &original, false).is_empty());
        }
    }

    #[test]
    fn redirect_drops_body_headers_when_it_discards_the_body() {
        let origin = parse_http_url("https://api.example.test/").unwrap();
        let headers = [
            ("aUtHoRiZaTiOn".into(), "test-only".into()),
            ("content-length".into(), "4".into()),
            ("Content-Type".into(), "text/plain".into()),
            ("Expect".into(), "100-continue".into()),
        ];
        assert_eq!(
            redirect_headers(Cow::Borrowed(&headers), &origin, &origin, true).as_ref(),
            &headers[..1]
        );
    }

    #[test]
    fn pooled_retries_are_always_limited_to_known_idempotent_methods() {
        for method in ["GET", "HEAD", "OPTIONS", "TRACE", "PUT", "DELETE"] {
            assert!(can_retry_pooled_request(method), "{method}");
        }
        for method in ["POST", "PATCH", "CONNECT", "CUSTOM"] {
            assert!(!can_retry_pooled_request(method), "{method}");
        }
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

    struct DripReader {
        chunks: Vec<(u64, &'static [u8])>,
        idx: usize,
        state: DeadlineState,
    }

    impl DripReader {
        fn new(chunks: Vec<(u64, &'static [u8])>, initial_idle: Duration) -> Self {
            Self {
                chunks,
                idx: 0,
                state: DeadlineState::new(initial_idle),
            }
        }
    }

    impl DeadlineSource for DripReader {
        fn set_idle(&mut self, idle_timeout: Duration, header_timeout: Duration) {
            self.state.set_idle(idle_timeout, header_timeout);
        }

        fn arm_deadline(&mut self, timeout: Duration) {
            self.state.arm_deadline(timeout);
        }

        fn has_deadline(&self) -> bool {
            self.state.has_deadline()
        }
    }

    impl Read for DripReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let rem = self.state.next_read_timeout()?;
            if self.idx >= self.chunks.len() {
                return Ok(0);
            }
            let (delay_ms, chunk) = self.chunks[self.idx];
            if delay_ms > 0 {
                let delay = Duration::from_millis(delay_ms);
                if delay > rem {
                    std::thread::sleep(rem);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "simulated socket read timeout",
                    ));
                }
                std::thread::sleep(delay);
            }
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            if n == chunk.len() {
                self.idx += 1;
            } else {
                self.chunks[self.idx].1 = &chunk[n..];
            }
            self.state.on_read_ok(n);
            Ok(n)
        }
    }

    struct IdleTimeoutReader {
        state: DeadlineState,
    }

    impl DeadlineSource for IdleTimeoutReader {
        fn set_idle(&mut self, idle_timeout: Duration, header_timeout: Duration) {
            self.state.set_idle(idle_timeout, header_timeout);
        }

        fn arm_deadline(&mut self, timeout: Duration) {
            self.state.arm_deadline(timeout);
        }

        fn has_deadline(&self) -> bool {
            self.state.has_deadline()
        }
    }

    impl Read for IdleTimeoutReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout"))
        }
    }

    #[test]
    fn response_writer_rejects_crlf_nul_and_bad_header_names() {
        for bad_val in ["ok\r\nSet-Cookie: admin=1", "ok\nX-Test: 1", "ok\0nul", "bad\x07ctrl"] {
            let mut hdrs = OrderedMap::new();
            hdrs.set(MapKey::Str(header_key("Location")), MapVal::Obj(Obj::str(bad_val)));
            let mut wire = Vec::new();
            let err = write_http_response_ext(&mut wire, 302, "", Some(&hdrs), true, false)
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(wire.is_empty(), "must not write partial bytes on rejected value");
        }

        for bad_name in ["", "Bad Name", "X:Inject", "X\r\n"] {
            let mut hdrs = OrderedMap::new();
            hdrs.set(MapKey::Str(header_key(bad_name)), MapVal::Obj(Obj::str("val")));
            let mut wire = Vec::new();
            let err = write_http_response_ext(&mut wire, 200, "ok", Some(&hdrs), true, false)
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(wire.is_empty(), "must not write partial bytes on rejected name");
        }

        let mut ok_hdrs = OrderedMap::new();
        ok_hdrs.set(MapKey::Str(header_key("Transfer-Encoding")), MapVal::Obj(Obj::str("chunked")));
        ok_hdrs.set(MapKey::Str(header_key("X-Custom")), MapVal::Obj(Obj::str("tab\tok")));
        let mut wire = Vec::new();
        write_http_response_ext(&mut wire, 200, "ok", Some(&ok_hdrs), true, false).unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(!text.contains("Transfer-Encoding"));
        assert!(text.contains("X-Custom: tab\tok\r\n"));
    }

    #[test]
    fn client_request_builder_rejects_invalid_headers() {
        let parsed = parse_http_url("http://example.com/test").unwrap();
        let bad_val = [("X-Test".to_string(), "ok\0bad".to_string())];
        assert!(build_http_request_bytes("GET", &parsed, &[], &bad_val, false).is_err());

        let bad_name = [("Bad Header".to_string(), "ok".to_string())];
        assert!(build_http_request_bytes("GET", &parsed, &[], &bad_name, false).is_err());
    }

    #[test]
    fn read_http_request_deadline_enforces_header_body_and_idle_timeouts() {
        let mut idle = BufReader::new(IdleTimeoutReader {
            state: DeadlineState::new(Duration::from_millis(50)),
        });
        assert!(matches!(
            read_http_request_deadline(
                &mut idle,
                Duration::from_millis(50),
                Duration::from_millis(100),
                Duration::from_millis(100),
            ),
            Err(ReadHttpError::Closed)
        ));

        let slow_headers = DripReader::new(
            vec![
                (0, b"GET / HTTP/1.1\r\n"),
                (20, b"Host: a\r\n"),
                (20, b"X-Slow: 1\r\n"),
                (20, b"\r\n"),
            ],
            Duration::from_millis(500),
        );
        let mut slow_hdr_reader = BufReader::new(slow_headers);
        assert!(matches!(
            read_http_request_deadline(
                &mut slow_hdr_reader,
                Duration::from_millis(500),
                Duration::from_millis(30),
                Duration::from_millis(500),
            ),
            Err(ReadHttpError::Timeout)
        ));

        let slow_body = DripReader::new(
            vec![
                (0, b"POST / HTTP/1.1\r\nContent-Length: 6\r\n\r\n"),
                (20, b"ab"),
                (20, b"cd"),
                (20, b"ef"),
            ],
            Duration::from_millis(500),
        );
        let mut slow_body_reader = BufReader::new(slow_body);
        assert!(matches!(
            read_http_request_deadline(
                &mut slow_body_reader,
                Duration::from_millis(500),
                Duration::from_millis(500),
                Duration::from_millis(25),
            ),
            Err(ReadHttpError::Timeout)
        ));
    }

    #[test]
    fn deny_private_addrs_blocks_loopback_private_cgnat_metadata_and_ipv6() {
        use std::net::IpAddr;

        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.10",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "0.0.0.0",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456:789a::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "2001:db8::1",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(is_private_or_special_ip(ip), "{blocked} must be blocked");
        }

        for public in ["93.184.216.34", "1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            let ip: IpAddr = public.parse().unwrap();
            assert!(!is_private_or_special_ip(ip), "{public} must be allowed");
        }

        let pool = HttpConnPool::new();
        for url in [
            "http://127.0.0.1:8080/secret",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost:8080/",
            "http://[::ffff:10.0.0.1]/admin",
        ] {
            let err_obj = http_client_pooled_request_with_options(
                "GET",
                url,
                "",
                &[],
                1000,
                &pool,
                true,
            );
            let Obj::Record(rec) = err_obj else { panic!("expected HttpError for {url}") };
            assert_eq!(rec.schema_id, SCHEMA_HTTP_ERROR);
            assert_eq!(rec.get_obj(0).format_obj(), "blocked_addr", "url={url}");
        }
    }

    #[test]
    fn strict_http_parser_rejects_malformed_requests_and_enforces_total_header_limit() {
        // 1. Bare LF rejected
        assert!(matches!(
            read_http_request(&mut b"GET / HTTP/1.1\nHost: example.com\n\n".as_slice()),
            Err(ReadHttpError::Bad(_))
        ));
        assert!(matches!(
            read_http_request(&mut b"GET / HTTP/1.1\r\nHost: example.com\n\r\n".as_slice()),
            Err(ReadHttpError::Bad(_))
        ));

        // 2. Bare CR inside header rejected
        assert!(matches!(
            read_http_request(&mut b"GET / HTTP/1.1\r\nHost: a\rb\r\n\r\n".as_slice()),
            Err(ReadHttpError::Bad(_))
        ));

        // 3. Request line strict validation (missing version, extra whitespace, tab, wrong version, bad method)
        for bad_line in [
            b"GET /\r\nHost: a\r\n\r\n".as_slice(),
            b"GET  / HTTP/1.1\r\nHost: a\r\n\r\n".as_slice(),
            b"GET\t/\tHTTP/1.1\r\nHost: a\r\n\r\n".as_slice(),
            b"GET / HTTP/1.1 extra\r\nHost: a\r\n\r\n".as_slice(),
            b"GET / HTTP/2.0\r\nHost: a\r\n\r\n".as_slice(),
            b"BAD@METHOD / HTTP/1.1\r\nHost: a\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(read_http_request(&mut &*bad_line), Err(ReadHttpError::Bad(_))),
                "expected 400 for {:?}",
                String::from_utf8_lossy(bad_line)
            );
        }

        // 4. Total header block > MAX_HEADER_BYTES rejected as TooLarge
        let mut huge = Vec::from(&b"GET / HTTP/1.1\r\n"[..]);
        for i in 0..20 {
            huge.extend_from_slice(format!("X-Pad-{i:02}: {}\r\n", "a".repeat(3500)).as_bytes());
        }
        huge.extend_from_slice(b"\r\n");
        assert!(huge.len() > MAX_HEADER_BYTES);
        assert!(matches!(
            read_http_request(&mut huge.as_slice()),
            Err(ReadHttpError::TooLarge)
        ));

        // 5. Pipelined requests + HTTP/1.0 no headers + >64 KiB body work cleanly on one BufReader
        let mut pipelined = Vec::new();
        pipelined.extend_from_slice(b"GET /one HTTP/1.0\r\n\r\n");
        let big_body = vec![b'z'; 70_000];
        pipelined.extend_from_slice(
            format!("POST /two HTTP/1.1\r\nHost: example.com\r\nContent-Length: {}\r\n\r\n", big_body.len())
                .as_bytes(),
        );
        pipelined.extend_from_slice(&big_body);
        pipelined.extend_from_slice(b"GET /three HTTP/1.1\r\nHost: example.com\r\n\r\n");

        let mut br = BufReader::new(Cursor::new(pipelined));
        let r1 = read_http_request(&mut br).unwrap();
        assert_eq!(r1.path, "/one");
        assert!(!r1.keep_alive);
        let r2 = read_http_request(&mut br).unwrap();
        assert_eq!(r2.path, "/two");
        assert_eq!(r2.body, big_body);
        assert!(r2.keep_alive);
        let r3 = read_http_request(&mut br).unwrap();
        assert_eq!(r3.path, "/three");
    }

    #[test]
    fn http_request_interns_common_headers_and_methods() {
        let raw_a = b"GET /a HTTP/1.1\r\nHost: one.local\r\nX-Forwarded-For: 10.0.0.1\r\nContent-Type: application/json\r\nX-Custom-Header: val\r\n\r\n";
        let raw_b = b"GET /b HTTP/1.1\r\nhost: two.local\r\nx-forwarded-for: 10.0.0.2\r\nCONTENT-TYPE: text/plain\r\nx-custom-header: val2\r\n\r\n";
        let ra = read_http_request(&mut raw_a.as_slice()).unwrap();
        let rb = read_http_request(&mut raw_b.as_slice()).unwrap();
        assert!(Arc::ptr_eq(&ra.method, &rb.method));
        for i in 0..3 {
            assert!(Arc::ptr_eq(&ra.headers[i].0, &rb.headers[i].0), "header {i} not interned");
        }
        assert_eq!(ra.headers[3].0.as_str(), "x-custom-header");
        assert!(!Arc::ptr_eq(&ra.headers[3].0, &rb.headers[3].0));
    }

    #[test]
    fn static_file_resolution_and_wire_streaming_preserves_binary_and_skips_head_read() {
        let tmp = std::env::temp_dir().join(format!("conduit_static_bin_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();

        let mut png_bytes: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF, 0xFE, 0x80, 0x9F,
        ];
        png_bytes.extend((0..1024).map(|i| ((i * 37 + 13) & 0xFF) as u8));
        assert!(std::str::from_utf8(&png_bytes).is_err());
        std::fs::write(tmp.join("sub/icon.png"), &png_bytes).unwrap();
        std::fs::write(tmp.join("sub/app.woff"), b"\x00\x01\xff\x80woff").unwrap();

        let statics: Vec<(Box<str>, Box<str>)> =
            vec![("/assets".into(), tmp.to_string_lossy().as_ref().into())];

        // Traversal, NUL, empty segment, and non-GET/HEAD methods are rejected.
        assert!(resolve_static_file(&statics, "POST", "/assets/sub/icon.png").is_none());
        assert!(resolve_static_file(&statics, "GET", "/assets/../sub/icon.png").is_none());
        assert!(resolve_static_file(&statics, "GET", "/assets/./sub/icon.png").is_none());
        assert!(resolve_static_file(&statics, "GET", "/assets/sub/icon.png\0").is_none());

        // HEAD reads metadata only, returns HeadOnly with exact Content-Length.
        let head_res = resolve_static_file(&statics, "HEAD", "/assets/sub/icon.png").unwrap();
        assert_eq!(head_res.content_type, "image/png");
        assert_eq!(head_res.len(), png_bytes.len() as u64);
        assert!(matches!(head_res.body, StaticFileBody::HeadOnly { len } if len == png_bytes.len() as u64));

        let mut hdrs = OrderedMap::new();
        hdrs.set(
            MapKey::Str(header_key("content-type")),
            MapVal::Obj(Obj::str(head_res.content_type)),
        );
        let mut head_wire = Vec::new();
        write_http_file_response(
            &mut head_wire,
            200,
            None,
            head_res.len(),
            Some(&hdrs),
            true,
            true,
        )
        .unwrap();
        let head_txt = String::from_utf8(head_wire).unwrap();
        let expected_hdr_end = head_txt.find("\r\n\r\n").unwrap() + 4;
        assert_eq!(head_txt.len(), expected_hdr_end);
        assert!(head_txt.contains(&format!("Content-Length: {}\r\n", png_bytes.len())));
        assert!(head_txt.contains("content-type: image/png\r\n"));

        // GET opens file and streams exact binary bytes without lossy UTF-8 conversion.
        let get_res = resolve_static_file(&statics, "GET", "/assets/sub/icon.png").unwrap();
        assert_eq!(get_res.content_type, "image/png");
        let StaticFileBody::File { mut file, len } = get_res.body else {
            panic!("expected StaticFileBody::File");
        };
        assert_eq!(len, png_bytes.len() as u64);
        let mut get_wire = Vec::new();
        write_http_file_response(
            &mut get_wire,
            200,
            Some(&mut file),
            len,
            Some(&hdrs),
            true,
            false,
        )
        .unwrap();
        let split = find_crlf_crlf(&get_wire).unwrap();
        let wire_head = std::str::from_utf8(&get_wire[..split]).unwrap();
        assert!(wire_head.contains(&format!("Content-Length: {}\r\n", png_bytes.len())));
        assert_eq!(&get_wire[split..], png_bytes.as_slice());

        // Truncated file (requested len > actual bytes) yields UnexpectedEof.
        let mut trunc_file = std::fs::File::open(tmp.join("sub/app.woff")).unwrap();
        let mut trunc_wire = Vec::new();
        let err = write_http_file_response(
            &mut trunc_wire,
            200,
            Some(&mut trunc_file),
            1024,
            Some(&hdrs),
            true,
            false,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let font_res = resolve_static_file(&statics, "HEAD", "/assets/sub/app.woff").unwrap();
        assert_eq!(font_res.content_type, "font/woff");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn route_index_matches_linear_lookup_across_precedence_and_edge_cases() {
        fn params_vec(p: MatchedParams) -> Vec<(String, String)> {
            match p {
                MatchedParams::None => Vec::new(),
                MatchedParams::One(k, v) => vec![(k.as_str().to_string(), v.format_obj())],
                MatchedParams::Many(entries) => entries
                    .into_iter()
                    .map(|(k, v)| match (k, v) {
                        (MapKey::Str(ks), MapVal::Obj(vo)) => {
                            (ks.as_str().to_string(), vo.format_obj())
                        }
                        _ => unreachable!(),
                    })
                    .collect(),
            }
        }

        let mut router = RouterState::default();
        let defs = [
            // Dynamic registered before static: must win over later static on /users/me.
            ("GET", "/users/:id"),
            ("GET", "/users/me"),
            // Static registered before dynamic: must win over later dynamic on /health.
            ("GET", "/health"),
            ("GET", "/:slug"),
            // Wildcard registered before specific static & param.
            ("GET", "/files/*rest"),
            ("GET", "/files/static"),
            ("GET", "/files/:name/meta"),
            // ANY (`*`) interleaved with specific methods.
            ("*", "/any-first"),
            ("POST", "/any-first"),
            ("POST", "/post-first"),
            ("*", "/post-first"),
            // Duplicate static routes (earlier wins).
            ("GET", "/dup"),
            ("GET", "/dup"),
            // Multi-param and deep (>16 segment) route.
            ("PATCH", "/orgs/:org/repos/:repo/issues/:num"),
            (
                "GET",
                "/d/1/2/3/4/5/6/7/8/9/10/11/12/13/14/15/16/17/:tail",
            ),
            // Custom method + root + static empty segment pattern.
            ("PROPFIND", "/dav/:item"),
            ("PROPFIND", "/dav-static"),
            ("GET", "/"),
        ];
        for (i, &(m, p)) in defs.iter().enumerate() {
            router.push_route(RouteEntry::new(m, p, Obj::Func(i as u32)));
        }
        // Append extra routes up to 1,000 total to exercise large static + dynamic tables.
        for i in 0..982 {
            if i % 2 == 0 {
                router.push_route(RouteEntry::new(
                    "GET",
                    &format!("/api/v1/resource_{i}"),
                    Obj::Func((defs.len() + i) as u32),
                ));
            } else {
                router.push_route(RouteEntry::new(
                    "GET",
                    &format!("/api/v1/item_{i}/:id"),
                    Obj::Func((defs.len() + i) as u32),
                ));
            }
        }

        let probes = [
            ("GET", "/users/me"),
            ("GET", "/users/42"),
            ("POST", "/users/42"),
            ("GET", "/health"),
            ("POST", "/health"),
            ("GET", "/other"),
            ("GET", "/files"),
            ("GET", "/files/"),
            ("GET", "/files/static"),
            ("GET", "/files/static/nested/a.png"),
            ("GET", "/files/img/meta"),
            ("GET", "/any-first"),
            ("POST", "/any-first"),
            ("*", "/any-first"),
            ("*", "/health"),
            ("GET", "/post-first"),
            ("POST", "/post-first"),
            ("GET", "/dup"),
            ("patch", "/orgs/acme/repos/conduit/issues/99"),
            ("GET", "/d/1/2/3/4/5/6/7/8/9/10/11/12/13/14/15/16/17/end"),
            ("GET", "/orgs//repos/conduit/issues/99"),
            ("propfind", "/dav/folder"),
            ("MKCOL", "/dav/folder"),
            ("PROPFIND", "/dav-static"),
            ("GET", "/dav-static"),
            ("GET", "/"),
            ("DELETE", "/"),
            ("GET", "/api/v1/resource_980"),
            ("GET", "/api/v1/item_981/xyz"),
            ("POST", "/api/v1/item_981/xyz"),
            ("GET", "/does/not/exist"),
        ];

        for &(method, path) in &probes {
            let expected = lookup_route(&router.routes, method, path);
            let actual = router.lookup_route(method, path);
            match (expected, actual) {
                (Ok((er, ep)), Ok((ar, ap))) => {
                    assert!(
                        std::ptr::eq(er, ar),
                        "route mismatch for {method} {path}: expected {} vs actual {}",
                        er.trimmed_pattern,
                        ar.trimmed_pattern
                    );
                    assert_eq!(params_vec(ep), params_vec(ap), "params mismatch for {method} {path}");
                }
                (Err(es), Err(as_)) => {
                    assert_eq!(es, as_, "status mismatch for {method} {path}");
                }
                (l, r) => panic!(
                    "result mismatch for {method} {path}: linear={} indexed={}",
                    l.is_ok(),
                    r.is_ok()
                ),
            }
        }
    }

    #[test]
    #[ignore]
    fn bench_lookup_route_linear_vs_indexed() {
        use std::hint::black_box;

        fn measure_median_us(mut f: impl FnMut() -> usize) -> f64 {
            const SERIES: usize = 9;
            const ITERS: usize = 50_000;
            let mut medians_us = Vec::with_capacity(SERIES);
            let mut sink = 0usize;
            for _ in 0..SERIES {
                let start = Instant::now();
                for _ in 0..ITERS {
                    sink = sink.wrapping_add(f());
                }
                let per_call_us = start.elapsed().as_secs_f64() * 1_000_000.0 / (ITERS as f64);
                medians_us.push(per_call_us);
            }
            black_box(sink);
            medians_us.sort_by(|a, b| a.total_cmp(b));
            medians_us[SERIES / 2]
        }

        for &n_routes in &[10usize, 100, 1000] {
            let mut router = RouterState::default();
            for i in 0..n_routes {
                router.push_route(RouteEntry::new(
                    "GET",
                    &format!("/api/v1/route_{i}"),
                    Obj::Func(i as u32),
                ));
            }
            let target = format!("/api/v1/route_{}", n_routes - 1);
            let linear_us = measure_median_us(|| {
                match lookup_route(black_box(&router.routes), black_box("GET"), black_box(&target)) {
                    Ok((r, _)) => r.trimmed_pattern.len(),
                    Err(s) => s as usize,
                }
            });
            let indexed_us = measure_median_us(|| {
                match router.lookup_route(black_box("GET"), black_box(&target)) {
                    Ok((r, _)) => r.trimmed_pattern.len(),
                    Err(s) => s as usize,
                }
            });
            eprintln!(
                "[bench static routes N={n_routes}] linear median={linear_us:.3}us | indexed median={indexed_us:.3}us"
            );
        }

        let mut mixed = RouterState::default();
        for i in 0..500usize {
            mixed.push_route(RouteEntry::new(
                "GET",
                &format!("/s/v1/item_{i}"),
                Obj::Func(i as u32),
            ));
        }
        for i in 0..500usize {
            mixed.push_route(RouteEntry::new(
                "GET",
                &format!("/d/v1/group_{i}/:id"),
                Obj::Func((500 + i) as u32),
            ));
        }
        let target_dyn = "/d/v1/group_499/12345";
        let linear_mixed_us = measure_median_us(|| {
            match lookup_route(black_box(&mixed.routes), black_box("GET"), black_box(target_dyn)) {
                Ok((r, _)) => r.trimmed_pattern.len(),
                Err(s) => s as usize,
            }
        });
        let indexed_mixed_us = measure_median_us(|| {
            match mixed.lookup_route(black_box("GET"), black_box(target_dyn)) {
                Ok((r, _)) => r.trimmed_pattern.len(),
                Err(s) => s as usize,
            }
        });
        eprintln!(
            "[bench mixed routes N=1000] linear median={linear_mixed_us:.3}us | indexed median={indexed_mixed_us:.3}us"
        );
    }

    #[test]
    fn fuzz_http_parser_never_panics() {
        let seeds: &[&[u8]] = &[
            b"GET /hello?a=1&b=2 HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
            b"POST /items HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhello",
            b"GET /legacy HTTP/1.0\r\n\r\n",
            b"HEAD /assets/logo.png HTTP/1.1\r\nHost: a\r\nX-Custom: v\r\n\r\n",
            b"POST /bad HTTP/1.1\r\nHost: a\r\nContent-Length: 99999999999999999999999\r\n\r\n",
        ];

        for &edge in &[
            b"GET / HTTP/1.1\r\nHost: a\r\nContent-Length: -1\r\n\r\n".as_slice(),
            b"GET / HTTP/1.1\r\nHost: a\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"GET / HTTP/1.1\r\nHost: a\r\nContent-Length: 99999999999999999999999\r\n\r\n".as_slice(),
            b"GET / HTTP/1.1\nHost: a\r\n\r\n".as_slice(),
            b"\r\n\r\n".as_slice(),
            b"".as_slice(),
        ] {
            let _ = parse_http_head(edge);
            let mut br = BufReader::new(Cursor::new(edge));
            let _ = read_http_request(&mut br);
        }

        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut next_u64 = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let bytes_pool: &[u8] = b"\r\n :;/019aAzZ-_%?&=!\x00\xff";
        for iter in 0..2000usize {
            let mut data = seeds[iter % seeds.len()].to_vec();
            match next_u64() % 4 {
                0 => {
                    let cut = (next_u64() as usize) % (data.len() + 1);
                    data.truncate(cut);
                }
                1 => {
                    let pos = (next_u64() as usize) % data.len().max(1);
                    data[pos] = bytes_pool[(next_u64() as usize) % bytes_pool.len()];
                }
                2 => {
                    let other = seeds[(next_u64() as usize) % seeds.len()];
                    let mid = (next_u64() as usize) % (data.len() + 1);
                    let o_mid = (next_u64() as usize) % (other.len() + 1);
                    let mut spliced = data[..mid].to_vec();
                    spliced.extend_from_slice(&other[o_mid..]);
                    data = spliced;
                }
                _ => {
                    let flips = ((next_u64() % 4) + 1) as usize;
                    for _ in 0..flips {
                        let pos = (next_u64() as usize) % data.len().max(1);
                        data[pos] ^= 1u8 << ((next_u64() % 8) as u8);
                    }
                }
            }
            let ok = std::panic::catch_unwind(|| {
                let _ = parse_http_head(&data);
                let mut br = BufReader::new(Cursor::new(data.as_slice()));
                let _ = read_http_request(&mut br);
            });
            assert!(ok.is_ok(), "HTTP parser panicked on mutation iter {iter}");
        }
    }
}
