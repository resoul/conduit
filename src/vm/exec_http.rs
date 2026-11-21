use std::collections::HashMap;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::ir::Module;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
use crate::native::http::{format_error_response_bytes, write_http_response_head};
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
use crate::native::http_reactor::ReactorFileBody;
use crate::native::http::{
    HttpClientState, MatchedParams, RouteEntry, RouterServerOptions, RouterState,
    StaticFileBody, header_key, http_client_bytes_request,
    http_client_pooled_request_with_options, http_client_request,
    http_client_request_with_timeout, make_http_client_obj, make_http_error, make_response_obj,
    make_response_with_body_obj, make_router_obj, resolve_client_url, resolve_static_file,
    route_prefix_matches, write_http_response_ext,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
use crate::native::http::{
    ReadHttpError, deadline_reader_writer_pair, read_http_request_deadline, write_error_response,
    write_http_file_response,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::string::VmString;
use crate::vm::value::{
    FxBuildHasher, ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_HTTP_CLIENT,
    SCHEMA_HTTP_REQUEST, SCHEMA_HTTP_RESPONSE, SCHEMA_HTTP_ROUTER, StructData,
};
use crate::vm::{ExecResult, RuntimeError, Vm};

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BODY_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_MAX_KEEPALIVE_REQUESTS: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IpNet {
    V4 { addr: u32, mask: u32 },
    V6 { addr: u128, mask: u128 },
}

impl IpNet {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (ip_part, prefix_part) = match trimmed.split_once('/') {
            Some((ip, p)) => (ip.trim(), Some(p.trim())),
            None => (trimmed, None),
        };
        let ip = ip_part.parse::<IpAddr>().ok()?.to_canonical();
        match ip {
            IpAddr::V4(v4) => {
                let prefix = match prefix_part {
                    Some(p) => p.parse::<u32>().ok().filter(|&n| n <= 32)?,
                    None => 32,
                };
                let mask = if prefix == 0 { 0 } else { !0u32 << (32 - prefix) };
                Some(Self::V4 {
                    addr: u32::from(v4) & mask,
                    mask,
                })
            }
            IpAddr::V6(v6) => {
                let prefix = match prefix_part {
                    Some(p) => p.parse::<u32>().ok().filter(|&n| n <= 128)?,
                    None => 128,
                };
                let mask = if prefix == 0 { 0 } else { !0u128 << (128 - prefix) };
                Some(Self::V6 {
                    addr: u128::from(v6) & mask,
                    mask,
                })
            }
        }
    }

    fn contains(&self, raw_ip: IpAddr) -> bool {
        match (self, raw_ip.to_canonical()) {
            (Self::V4 { addr, mask }, IpAddr::V4(v4)) => (u32::from(v4) & *mask) == *addr,
            (Self::V6 { addr, mask }, IpAddr::V6(v6)) => (u128::from(v6) & *mask) == *addr,
            _ => false,
        }
    }
}

struct HttpServerConfig {
    workers: usize,
    idle_timeout: Duration,
    header_timeout: Duration,
    body_timeout: Duration,
    max_conns_per_ip: usize,
    #[cfg_attr(
        not(any(
            target_os = "macos",
            target_os = "ios",
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )
        )),
        allow(dead_code)
    )]
    max_conns: usize,
    trusted_proxies: Vec<IpNet>,
}

fn resolve_duration_ms<F: Fn(&str) -> Option<String>>(
    get_env: &F,
    var: &str,
    router_ms: Option<u64>,
    min_ms: u64,
    max_ms: u64,
    default: Duration,
) -> Duration {
    if let Some(val) = get_env(var)
        && let Ok(ms) = val.trim().parse::<u64>()
    {
        return Duration::from_millis(ms.clamp(min_ms, max_ms));
    }
    if let Some(ms) = router_ms {
        return Duration::from_millis(ms.clamp(min_ms, max_ms));
    }
    default
}

impl HttpServerConfig {
    fn resolve(opts: &RouterServerOptions) -> Self {
        Self::resolve_with_env(opts, |k| std::env::var(k).ok())
    }

    fn resolve_with_env<F: Fn(&str) -> Option<String>>(
        opts: &RouterServerOptions,
        get_env: F,
    ) -> Self {
        let default_workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(4, 16))
            .unwrap_or(8);
        let workers = get_env("CONDUIT_HTTP_WORKERS")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .or(opts.workers)
            .map(|n| n.clamp(1, 256))
            .unwrap_or(default_workers);
        let max_conns_per_ip = get_env("CONDUIT_HTTP_MAX_CONNS_PER_IP")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .or(opts.max_conns_per_ip)
            .unwrap_or_else(|| (workers / 2).max(4));
        let default_max_conns = (workers * 128).clamp(512, 8192);
        let max_conns = get_env("CONDUIT_HTTP_MAX_CONNS")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|n| n.clamp(16, 65_536))
            .unwrap_or(default_max_conns);
        let trusted_proxies = match get_env("CONDUIT_HTTP_TRUSTED_PROXIES") {
            Some(raw) => raw.split(',').filter_map(IpNet::parse).collect(),
            None => opts
                .trusted_proxies
                .iter()
                .filter_map(|s| IpNet::parse(s))
                .collect(),
        };
        Self {
            workers,
            idle_timeout: resolve_duration_ms(
                &get_env,
                "CONDUIT_HTTP_IDLE_TIMEOUT_MS",
                opts.idle_timeout_ms,
                100,
                300_000,
                DEFAULT_IDLE_TIMEOUT,
            ),
            header_timeout: resolve_duration_ms(
                &get_env,
                "CONDUIT_HTTP_HEADER_TIMEOUT_MS",
                opts.header_timeout_ms,
                500,
                300_000,
                DEFAULT_HEADER_TIMEOUT,
            ),
            body_timeout: resolve_duration_ms(
                &get_env,
                "CONDUIT_HTTP_BODY_TIMEOUT_MS",
                opts.body_timeout_ms,
                500,
                600_000,
                DEFAULT_BODY_TIMEOUT,
            ),
            max_conns_per_ip,
            max_conns,
            trusted_proxies,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum IpBucket {
    LoopbackV4,
    LoopbackV6,
    V4([u8; 4]),
    /// IPv6 clients grouped by /64 prefix to prevent trivial address rotation within a subnet.
    V6Prefix([u8; 8]),
}

struct IpConnLimiter {
    counts: Mutex<HashMap<IpBucket, usize, FxBuildHasher>>,
    max_remote_per_ip: usize,
    max_loopback_per_ip: usize,
    trusted_proxies: Vec<IpNet>,
}

struct IpConnPermit {
    limiter: Arc<IpConnLimiter>,
    bucket: Option<IpBucket>,
}

impl IpConnLimiter {
    #[cfg(test)]
    fn new(workers: usize) -> Arc<Self> {
        Self::with_config(workers, (workers / 2).max(4), Vec::new())
    }

    fn with_config(
        workers: usize,
        max_remote_per_ip: usize,
        trusted_proxies: Vec<IpNet>,
    ) -> Arc<Self> {
        Arc::new(Self {
            counts: Mutex::new(HashMap::with_hasher(FxBuildHasher)),
            max_remote_per_ip,
            max_loopback_per_ip: (workers * 4).max(32),
            trusted_proxies,
        })
    }

    fn try_acquire(self: &Arc<Self>, raw_ip: IpAddr) -> Option<IpConnPermit> {
        let ip = raw_ip.to_canonical();
        if !ip.is_loopback()
            && (self.max_remote_per_ip == 0
                || self.trusted_proxies.iter().any(|net| net.contains(ip)))
        {
            return Some(IpConnPermit {
                limiter: Arc::clone(self),
                bucket: None,
            });
        }
        let (bucket, limit) = match ip {
            IpAddr::V4(v4) if v4.is_loopback() => (IpBucket::LoopbackV4, self.max_loopback_per_ip),
            IpAddr::V4(v4) => (IpBucket::V4(v4.octets()), self.max_remote_per_ip),
            IpAddr::V6(v6) if v6.is_loopback() => (IpBucket::LoopbackV6, self.max_loopback_per_ip),
            IpAddr::V6(v6) => {
                let mut prefix = [0u8; 8];
                prefix.copy_from_slice(&v6.octets()[..8]);
                (IpBucket::V6Prefix(prefix), self.max_remote_per_ip)
            }
        };
        let mut guard = self.counts.lock().ok()?;
        let entry = guard.entry(bucket).or_insert(0);
        if *entry >= limit {
            return None;
        }
        *entry += 1;
        Some(IpConnPermit {
            limiter: Arc::clone(self),
            bucket: Some(bucket),
        })
    }
}

impl Drop for IpConnPermit {
    fn drop(&mut self) {
        let Some(bucket) = self.bucket else {
            return;
        };
        if let Ok(mut guard) = self.limiter.counts.lock()
            && let Some(count) = guard.get_mut(&bucket)
        {
            if *count <= 1 {
                guard.remove(&bucket);
            } else {
                *count -= 1;
            }
        }
    }
}

fn send_503_nonblocking(mut stream: TcpStream) {
    let _ = stream.set_nonblocking(true);
    let _ = std::io::Write::write_all(
        &mut stream,
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 19\r\nRetry-After: 1\r\nConnection: close\r\n\r\nService Unavailable",
    );
}

fn is_fatal_accept_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::InvalidInput {
        return true;
    }
    #[cfg(unix)]
    if matches!(err.raw_os_error(), Some(9 | 22)) {
        // EBADF (9), EINVAL (22)
        return true;
    }
    #[cfg(windows)]
    if matches!(err.raw_os_error(), Some(10009 | 10022 | 10038)) {
        // WSAEBADF, WSAEINVAL, WSAENOTSOCK
        return true;
    }
    false
}

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub const HTTP_TEXT: u8 = 0;
pub const HTTP_HTML: u8 = 1;
pub const HTTP_JSON: u8 = 2;
pub const HTTP_RESPONSE: u8 = 3;
pub const HTTP_ROUTER: u8 = 4;
pub const HTTP_ROUTE_GET: u8 = 5;
pub const HTTP_ROUTE_POST: u8 = 6;
pub const HTTP_ROUTE_PUT: u8 = 7;
pub const HTTP_ROUTE_DELETE: u8 = 8;
pub const HTTP_DISPATCH: u8 = 9;
pub const HTTP_SERVE: u8 = 10;
pub const HTTP_GET: u8 = 11;
pub const HTTP_POST: u8 = 12;
pub const HTTP_REQUEST: u8 = 13;
pub const HTTP_REQUEST_TIMEOUT: u8 = 14;
pub const HTTP_ROUTE_PATCH: u8 = 15;
pub const HTTP_ROUTE_OPTIONS: u8 = 16;
pub const HTTP_ROUTE_HEAD: u8 = 17;
pub const HTTP_ROUTE_ANY: u8 = 18;
pub const HTTP_ROUTE_BEFORE: u8 = 19;
pub const HTTP_ROUTE_AFTER: u8 = 20;
pub const HTTP_GET_BYTES: u8 = 21;
pub const HTTP_REQUEST_BYTES: u8 = 22;
pub const HTTP_STOP: u8 = 23;
pub const HTTP_ROUTE_TYPED_BODY: u8 = 24;
pub const HTTP_ROUTE_GROUP_BEFORE: u8 = 25;
pub const HTTP_ROUTE_GROUP_AFTER: u8 = 26;
pub const HTTP_ROUTE_STATIC: u8 = 27;
pub const HTTP_ROUTE_CORS: u8 = 28;
pub const HTTP_OK_JSON: u8 = 29;
pub const HTTP_STATUS_JSON: u8 = 30;
pub const HTTP_REDIRECT: u8 = 31;
pub const HTTP_NO_CONTENT: u8 = 32;
pub const HTTP_REQ_HEADER: u8 = 33;
pub const HTTP_REQ_PARAM: u8 = 34;
pub const HTTP_REQ_QUERY_PARAM: u8 = 35;
pub const HTTP_GET_JSON: u8 = 36;
pub const HTTP_POST_JSON: u8 = 37;
pub const HTTP_ROUTE_GUARD: u8 = 38;
pub const HTTP_ROUTE_TYPED_QUERY: u8 = 39;
pub const HTTP_ROUTE_AUTO_RESP: u8 = 40;
pub const HTTP_CLIENT_NEW: u8 = 41;
pub const HTTP_CLIENT_HEADER: u8 = 42;
pub const HTTP_CLIENT_TIMEOUT: u8 = 43;
pub const HTTP_CLIENT_BASE_URL: u8 = 44;
pub const HTTP_CLIENT_REQUEST: u8 = 45;
pub const HTTP_CLIENT_GET_JSON: u8 = 46;
pub const HTTP_CLIENT_POST_JSON: u8 = 47;
pub const HTTP_CLIENT_CLOSE: u8 = 48;
pub const HTTP_CLIENT_POOL_STATS: u8 = 49;
pub const HTTP_ROUTE_ON_START: u8 = 50;
pub const HTTP_ROUTE_ON_STOP: u8 = 51;
pub const HTTP_CLIENT_DENY_PRIVATE: u8 = 52;
pub const HTTP_ROUTER_WORKERS: u8 = 53;
pub const HTTP_ROUTER_TIMEOUTS: u8 = 54;
pub const HTTP_ROUTER_MAX_CONNS_PER_IP: u8 = 55;
pub const HTTP_ROUTER_TRUSTED_PROXIES: u8 = 56;

pub static SIGNAL_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_os_signal(_sig: i32) {
    SIGNAL_STOP.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
pub fn install_signal_handlers() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe {
        signal(2, on_os_signal);
        signal(15, on_os_signal);
    }
}

#[cfg(windows)]
pub fn install_signal_handlers() {
    type HandlerRoutine = unsafe extern "system" fn(ctrl_type: u32) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }
    unsafe extern "system" fn on_ctrl(ctrl_type: u32) -> i32 {
        match ctrl_type {
            0 | 1 | 2 | 5 | 6 => {
                SIGNAL_STOP.store(true, Ordering::SeqCst);
                1
            }
            _ => 0,
        }
    }
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

#[cfg(not(any(unix, windows)))]
pub fn install_signal_handlers() {}

fn wake_socket_addr(mut addr: std::net::SocketAddr) {
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        });
    }
    let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(50));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_http_listener(
    module: Arc<Module>,
    routers: Vec<RouterState>,
    http_clients: Vec<HttpClientState>,
    db_pools: Vec<crate::vm::DbPoolSlot>,
    db_pool_free: Vec<usize>,
    spawn_tracker: Arc<crate::vm::SpawnTracker>,
    script_args: Option<Vec<String>>,
    rid: usize,
    listener: TcpListener,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    watch_signals: bool,
) {
    let bound_addr = listener.local_addr().ok();
    if let Some(local) = bound_addr
        && let Ok(mut guard) = routers[rid].listen_addr.lock()
    {
        *guard = Some(local);
    }
    stop_flag.store(false, Ordering::Release);
    if watch_signals {
        SIGNAL_STOP.store(false, Ordering::Release);
        install_signal_handlers();
    }
    let cfg = HttpServerConfig::resolve(&routers[rid].server_opts);
    serve_http_listener_impl(
        module,
        routers.clone(),
        http_clients,
        db_pools,
        db_pool_free,
        spawn_tracker,
        script_args,
        rid,
        listener,
        bound_addr,
        stop_flag,
        watch_signals,
        cfg,
    );
    if let Ok(mut guard) = routers[rid].listen_addr.lock() {
        *guard = None;
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
fn dispatch_one_reactor_job(
    vm: &mut Vm,
    module: &Module,
    rid: usize,
    req_obj: Obj,
    head_only: bool,
    keep_alive: bool,
) -> (Vec<u8>, Option<ReactorFileBody>, bool) {
    let Ok(dispatched) = vm.dispatch_router_request_inner(module, 0, 0, rid, req_obj, true) else {
        return (
            format_error_response_bytes(500, "Internal Server Error"),
            None,
            false,
        );
    };
    match dispatched {
        DispatchedHttpResponse::Obj(resp_obj) => {
            let Ok((status, body, headers)) = vm.extract_response_fields(&resp_obj) else {
                return (
                    format_error_response_bytes(500, "Internal Server Error"),
                    None,
                    false,
                );
            };
            let mut out = Vec::with_capacity(256 + body.len());
            if write_http_response_ext(&mut out, status, body, headers, keep_alive, head_only)
                .is_err()
            {
                return (
                    format_error_response_bytes(500, "Internal Server Error"),
                    None,
                    false,
                );
            }
            (out, None, keep_alive)
        }
        DispatchedHttpResponse::StaticFile {
            status,
            headers,
            file,
            len,
        } => {
            let file_body = if head_only || len == 0 {
                None
            } else {
                let Some(file) = file else {
                    return (
                        format_error_response_bytes(500, "Internal Server Error"),
                        None,
                        false,
                    );
                };
                Some(ReactorFileBody {
                    file,
                    remaining: len,
                })
            };
            let mut out = Vec::with_capacity(256);
            if write_http_response_head(&mut out, status, len, Some(&headers), keep_alive).is_err()
            {
                return (
                    format_error_response_bytes(500, "Internal Server Error"),
                    None,
                    false,
                );
            }
            (out, file_body, keep_alive)
        }
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
#[allow(clippy::too_many_arguments)]
fn serve_http_listener_impl(
    module: Arc<Module>,
    routers: Vec<RouterState>,
    http_clients: Vec<HttpClientState>,
    db_pools: Vec<crate::vm::DbPoolSlot>,
    db_pool_free: Vec<usize>,
    spawn_tracker: Arc<crate::vm::SpawnTracker>,
    script_args: Option<Vec<String>>,
    rid: usize,
    listener: TcpListener,
    bound_addr: Option<std::net::SocketAddr>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    watch_signals: bool,
    cfg: HttpServerConfig,
) {
    use crate::native::http_reactor::{
        ReactorConfig, ReactorJob, ReactorResponse, ReactorWakePair, run_http_reactor,
    };

    let workers = cfg.workers;
    let ip_limiter = IpConnLimiter::with_config(workers, cfg.max_conns_per_ip, cfg.trusted_proxies);
    let reactor_cfg = ReactorConfig {
        idle_timeout: cfg.idle_timeout,
        header_timeout: cfg.header_timeout,
        body_timeout: cfg.body_timeout,
        max_conns: cfg.max_conns,
        max_keepalive_requests: HTTP_MAX_KEEPALIVE_REQUESTS,
    };
    let Ok(wake_pair) = ReactorWakePair::new() else {
        return;
    };
    let (job_tx, job_rx) = mpsc::sync_channel::<ReactorJob>(workers * 8);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (res_tx, res_rx) = mpsc::channel::<ReactorResponse>();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let job_rx = Arc::clone(&job_rx);
            let res_tx = res_tx.clone();
            let waker = wake_pair.waker();
            let routers = routers.clone();
            let http_clients = http_clients.clone();
            let db_pools = db_pools.clone();
            let db_pool_free = db_pool_free.clone();
            let spawn_tracker = Arc::clone(&spawn_tracker);
            let mod_worker = Arc::clone(&module);
            let script_args = script_args.clone();
            scope.spawn(move || {
                let mut worker_vm = Vm::new();
                worker_vm.script_args = script_args;
                worker_vm.host.routers = routers;
                worker_vm.host.http_clients = http_clients;
                worker_vm.host.db_pools = db_pools;
                worker_vm.host.db_pool_free = db_pool_free;
                worker_vm.host.spawn_tracker = spawn_tracker;
                worker_vm.host.shared_module = Some(Arc::clone(&mod_worker));
                loop {
                    let job = {
                        let Ok(guard) = job_rx.lock() else { break };
                        match guard.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    let conn_id = job.conn_id;
                    let head_only = job.head_only;
                    let keep_alive = job.keep_alive;
                    let req_obj = job.raw.into_request_obj();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        dispatch_one_reactor_job(
                            &mut worker_vm,
                            &mod_worker,
                            rid,
                            req_obj,
                            head_only,
                            keep_alive,
                        )
                    }));
                    let (bytes, file_body, keep_alive) = match result {
                        Ok(triple) => triple,
                        Err(_) => {
                            eprintln!("[http] worker recovered from request panic");
                            worker_vm.frame_depth = 0;
                            let obj_len = worker_vm.objs.len();
                            worker_vm.clear_obj_range(0, obj_len);
                            (
                                format_error_response_bytes(500, "Internal Server Error"),
                                None,
                                false,
                            )
                        }
                    };
                    let _ = res_tx.send(ReactorResponse {
                        conn_id,
                        bytes,
                        file_body,
                        keep_alive,
                    });
                    waker.wake();
                    crate::vm::exec_map::flush_stdout();
                }
            });
        }
        drop(res_tx);

        let watch_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = if watch_signals {
            let watch_stop = Arc::clone(&stop_flag);
            let watch_done_flag = Arc::clone(&watch_done);
            let watch_waker = wake_pair.waker();
            Some(scope.spawn(move || {
                while !watch_done_flag.load(Ordering::Acquire)
                    && !watch_stop.load(Ordering::Acquire)
                {
                    if SIGNAL_STOP.load(Ordering::Acquire) {
                        watch_stop.store(true, Ordering::Release);
                        watch_waker.wake();
                        if let Some(wake_addr) = bound_addr {
                            wake_socket_addr(wake_addr);
                        }
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(50));
                }
            }))
        } else {
            None
        };

        run_http_reactor(
            listener,
            Arc::clone(&stop_flag),
            reactor_cfg,
            |ip| ip_limiter.try_acquire(ip),
            send_503_nonblocking,
            is_fatal_accept_error,
            job_tx,
            res_rx,
            wake_pair,
        );

        if let Some(w) = watcher {
            watch_done.store(true, Ordering::Release);
            w.thread().unpark();
        }
    });
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
#[allow(clippy::too_many_arguments)]
fn serve_http_listener_impl(
    module: Arc<Module>,
    routers: Vec<RouterState>,
    http_clients: Vec<HttpClientState>,
    db_pools: Vec<crate::vm::DbPoolSlot>,
    db_pool_free: Vec<usize>,
    spawn_tracker: Arc<crate::vm::SpawnTracker>,
    script_args: Option<Vec<String>>,
    rid: usize,
    listener: TcpListener,
    bound_addr: Option<std::net::SocketAddr>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    watch_signals: bool,
    cfg: HttpServerConfig,
) {
    let workers = cfg.workers;
    let idle_timeout = cfg.idle_timeout;
    let header_timeout = cfg.header_timeout;
    let body_timeout = cfg.body_timeout;
    let ip_limiter = IpConnLimiter::with_config(workers, cfg.max_conns_per_ip, cfg.trusted_proxies);
    let (tx, rx) = mpsc::sync_channel::<(TcpStream, IpConnPermit)>(workers * 8);
    let rx = Arc::new(Mutex::new(rx));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let rx = Arc::clone(&rx);
            let routers = routers.clone();
            let http_clients = http_clients.clone();
            let db_pools = db_pools.clone();
            let db_pool_free = db_pool_free.clone();
            let spawn_tracker = Arc::clone(&spawn_tracker);
            let mod_worker = Arc::clone(&module);
            let script_args = script_args.clone();
            scope.spawn(move || {
                let mut worker_vm = Vm::new();
                worker_vm.script_args = script_args;
                worker_vm.host.routers = routers;
                worker_vm.host.http_clients = http_clients;
                worker_vm.host.db_pools = db_pools;
                worker_vm.host.db_pool_free = db_pool_free;
                worker_vm.host.spawn_tracker = spawn_tracker;
                worker_vm.host.shared_module = Some(Arc::clone(&mod_worker));
                loop {
                    let (stream, permit) = {
                        let Ok(guard) = rx.lock() else { break };
                        match guard.recv() {
                            Ok(conn) => conn,
                            Err(_) => break,
                        }
                    };
                    serve_connection_isolated(
                        &mut worker_vm,
                        &mod_worker,
                        rid,
                        stream,
                        idle_timeout,
                        header_timeout,
                        body_timeout,
                    );
                    drop(permit);
                    crate::vm::exec_map::flush_stdout();
                }
            });
        }
        let watch_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = if watch_signals {
            let watch_stop = Arc::clone(&stop_flag);
            let watch_done_flag = Arc::clone(&watch_done);
            Some(scope.spawn(move || {
                while !watch_done_flag.load(Ordering::Acquire)
                    && !watch_stop.load(Ordering::Acquire)
                {
                    if SIGNAL_STOP.load(Ordering::Acquire) {
                        watch_stop.store(true, Ordering::Release);
                        if let Some(wake_addr) = bound_addr {
                            wake_socket_addr(wake_addr);
                        }
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(50));
                }
            }))
        } else {
            None
        };
        let mut backoff_ms = 5u64;
        while !stop_flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, peer_addr)) => {
                    backoff_ms = 5;
                    if stop_flag.load(Ordering::Acquire) {
                        break;
                    }
                    let _ = stream.set_nodelay(true);
                    let Some(permit) = ip_limiter.try_acquire(peer_addr.ip()) else {
                        send_503_nonblocking(stream);
                        continue;
                    };
                    match tx.try_send((stream, permit)) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full((stream, _permit))) => {
                            send_503_nonblocking(stream);
                            continue;
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(e) => {
                    if stop_flag.load(Ordering::Acquire) || is_fatal_accept_error(&e) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(backoff_ms));
                    backoff_ms = (backoff_ms * 2).min(50);
                }
            }
        }
        drop(tx);
        if let Some(w) = watcher {
            watch_done.store(true, Ordering::Release);
            w.thread().unpark();
        }
    });
}

enum DispatchedHttpResponse {
    Obj(Obj),
    StaticFile {
        status: i64,
        headers: OrderedMap,
        file: Option<std::fs::File>,
        len: u64,
    },
}

#[inline]
fn set_matched_params_on_record(req: &mut StructData, params: MatchedParams) {
    let params = match params {
        MatchedParams::None => Obj::Map(Arc::new(OrderedMap::new())),
        MatchedParams::One(key, value) => Obj::deferred_map_entry(key, value),
        MatchedParams::Many(entries) => Obj::deferred_field(move || {
            Obj::Map(Arc::new(OrderedMap::from_vec(entries)))
        }),
    };
    req.set_obj(5, params);
}

impl Vm {
    pub(crate) fn wake_router_listener(&self, rid: usize) {
        if let Some(router) = self.host.routers.get(rid)
            && let Ok(guard) = router.listen_addr.lock()
            && let Some(addr) = *guard
        {
            wake_socket_addr(addr);
        }
    }

    fn invoke_http_fn_raw(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        handler: &Obj,
        args: &[Obj],
    ) -> Result<ExecResult, RuntimeError> {
        let (func_id, has_closure) = match handler {
            Obj::Func(fid) => (*fid, false),
            Obj::Closure(cd) => (cd.func_id, true),
            _ => return Err(err_msg("Expected HTTP handler function")),
        };
        let closure_slot = if has_closure {
            self.ensure_capacity(s_top, o_top + 1);
            self.objs[o_top] = handler.clone();
            o_top
        } else {
            usize::MAX
        };
        let child_s = s_top;
        let child_o = if has_closure { o_top + 1 } else { o_top };
        let func = &module.functions[func_id as usize];
        let used_objs = (func.num_objs as usize).max(args.len()).max(1);
        self.ensure_capacity(
            child_s + func.num_scalars.max(1) as usize,
            child_o + used_objs,
        );
        for (i, arg) in args.iter().enumerate() {
            self.objs[child_o + i] = arg.clone();
        }
        let res = self.exec_from(module, func_id, child_s, child_o, closure_slot);
        if has_closure {
            self.objs[closure_slot] = Obj::Empty;
        }
        match res {
            Ok(val) => {
                if used_objs > func.num_objs as usize {
                    self.clear_obj_range(
                        child_o + func.num_objs as usize,
                        used_objs - func.num_objs as usize,
                    );
                }
                Ok(val)
            }
            Err(e) => {
                self.clear_obj_range(child_o, used_objs);
                Err(e)
            }
        }
    }

    fn invoke_http_fn(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        handler: &Obj,
        args: &[Obj],
    ) -> Result<Obj, RuntimeError> {
        match self.invoke_http_fn_raw(module, s_top, o_top, handler, args)? {
            ExecResult::Obj(o) => Ok(o),
            _ => Err(err_msg("HTTP handler must return an object")),
        }
    }

    fn extract_router_id(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected Router"));
        };
        if rec.schema_id != SCHEMA_HTTP_ROUTER {
            return Err(err_msg("Expected Router"));
        }
        let id = rec.get_scalar(0) as usize;
        if id >= self.host.routers.len() {
            return Err(err_msg("Invalid Router handle"));
        }
        Ok(id)
    }

    fn extract_client_id(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected http.Client"));
        };
        if rec.schema_id != SCHEMA_HTTP_CLIENT {
            return Err(err_msg("Expected http.Client"));
        }
        let cid = rec.get_scalar(0) as usize;
        if cid >= self.host.http_clients.len() {
            return Err(err_msg("Invalid http.Client handle"));
        }
        Ok(cid)
    }

    fn extract_response_fields<'a>(
        &self,
        obj: &'a Obj,
    ) -> Result<(i64, &'a str, Option<&'a OrderedMap>), RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected Response"));
        };
        if rec.schema_id != SCHEMA_HTTP_RESPONSE {
            return Err(err_msg("Expected Response"));
        }
        let status = rec.get_scalar(0) as i64;
        let body = match rec.get_obj(0) {
            Obj::Str(s) => s.as_str(),
            _ => "",
        };
        let headers = match rec.get_obj(1) {
            Obj::Map(m) => Some(m.as_ref()),
            _ => None,
        };
        Ok((status, body, headers))
    }

    fn dispatch_router_request(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        router_id: usize,
        req_obj: Obj,
    ) -> Result<Obj, RuntimeError> {
        match self.dispatch_router_request_inner(module, s_top, o_top, router_id, req_obj, false)? {
            DispatchedHttpResponse::Obj(obj) => Ok(obj),
            DispatchedHttpResponse::StaticFile { .. } => unreachable!(),
        }
    }

    fn dispatch_router_request_inner(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        router_id: usize,
        mut req_obj: Obj,
        allow_file_stream: bool,
    ) -> Result<DispatchedHttpResponse, RuntimeError> {
        let source = match &req_obj {
            Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_REQUEST => Arc::clone(rec),
            _ => return Err(err_msg("Expected Request")),
        };
        let req_method = match source.get_obj(0) {
            Obj::Str(s) => s.as_str(),
            _ => "GET",
        };
        let req_path = match source.get_obj(1) {
            Obj::Str(s) => s.as_str(),
            _ => "/",
        };

        let cors_origin = self.host.routers[router_id].cors_origin.clone();
        let mut resp = if req_method == "OPTIONS"
            && cors_origin.is_some()
            && self.host.routers[router_id]
                .lookup_route(req_method, req_path)
                .is_err()
        {
            let mut hdrs = OrderedMap::with_capacity(3);
            hdrs.set(
                MapKey::Str(header_key("access-control-allow-methods")),
                MapVal::Obj(Obj::str("GET, POST, PUT, PATCH, DELETE, OPTIONS")),
            );
            hdrs.set(
                MapKey::Str(header_key("access-control-allow-headers")),
                MapVal::Obj(Obj::str("Content-Type, Authorization")),
            );
            let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
            s.set_scalar(0, 204);
            s.set_obj(0, Obj::str(""));
            s.set_obj(1, Obj::Map(Arc::new(hdrs)));
            Some(Obj::Record(Arc::new(s)))
        } else {
            None
        };
        if resp.is_none() {
            let before_len = self.host.routers[router_id].before.len();
            for idx in 0..before_len {
                let hook = {
                    let (prefix, h) = &self.host.routers[router_id].before[idx];
                    if let Some(p) = prefix
                        && !route_prefix_matches(p, req_path)
                    {
                        continue;
                    }
                    h.clone()
                };
                let out = self.invoke_http_fn(
                    module,
                    s_top,
                    o_top,
                    &hook,
                    std::slice::from_ref(&req_obj),
                )?;
                if out != Obj::Empty {
                    resp = Some(out);
                    break;
                }
            }
        }

        let mut cached_to_restore: Option<(Arc<StructData>, Arc<StructData>)> = None;
        let mut static_passthrough: Option<(Arc<VmString>, Option<std::fs::File>, u64)> = None;
        let mut resp = match resp {
            Some(r) => r,
            None => {
                let matched = match self.host.routers[router_id].lookup_route(req_method, req_path) {
                    Ok((entry, params)) => Ok((
                        entry.handler.clone(),
                        entry.query_plan,
                        entry.body_json_plan,
                        entry.guard.clone(),
                        entry.auto_resp_kind,
                        params,
                    )),
                    Err(status) => Err(status),
                };
                match matched {
                    Ok((handler, query_plan, body_plan, guard, auto_resp_kind, MatchedParams::None)) => self
                        .invoke_http_route_handler(
                            module,
                            s_top,
                            o_top,
                            &handler,
                            query_plan,
                            body_plan,
                            guard.as_ref(),
                            auto_resp_kind,
                            &req_obj,
                        )?,
                    Ok((handler, query_plan, body_plan, guard, auto_resp_kind, matched_params)) => {
                        let mut cached = self.host.cached_http_req.take();
                        let Obj::Record(ref mut rec_arc) = req_obj else {
                            return Err(err_msg("Expected Request"));
                        };
                        if let Some((ref mut last_src, ref mut cached_rec)) = cached
                            && cached_rec.obj_len == rec_arc.obj_len
                            && let Some(cached_mut) = Arc::get_mut(cached_rec)
                        {
                            if !Arc::ptr_eq(last_src, rec_arc) {
                                for idx in 0..rec_arc.obj_len as usize {
                                    if idx != 5 {
                                        cached_mut.set_obj(idx, rec_arc.raw_obj(idx).clone());
                                    }
                                }
                                *last_src = Arc::clone(rec_arc);
                            }
                            set_matched_params_on_record(cached_mut, matched_params);
                            req_obj = Obj::Record(Arc::clone(cached_rec));
                        } else {
                            let src_arc = Arc::clone(rec_arc);
                            let req_mut = Arc::make_mut(rec_arc);
                            set_matched_params_on_record(req_mut, matched_params);
                            if let Obj::Record(new_arc) = &req_obj {
                                cached = Some((src_arc, Arc::clone(new_arc)));
                            }
                        }
                        cached_to_restore = cached;
                        self.invoke_http_route_handler(
                            module,
                            s_top,
                            o_top,
                            &handler,
                            query_plan,
                            body_plan,
                            guard.as_ref(),
                            auto_resp_kind,
                            &req_obj,
                        )?
                    }
                    Err(status) => {
                        if let Some(resolved) = resolve_static_file(
                            &self.host.routers[router_id].statics,
                            req_method,
                            req_path,
                        ) {
                            let len = resolved.len();
                            let has_matching_after =
                                self.host.routers[router_id].after.iter().any(|(prefix, _)| {
                                    prefix
                                        .as_ref()
                                        .is_none_or(|p| route_prefix_matches(p, req_path))
                                });
                            if allow_file_stream && !has_matching_after {
                                let mut headers = OrderedMap::with_capacity(if cors_origin.is_some() { 2 } else { 1 });
                                headers.set(
                                    MapKey::Str(header_key("content-type")),
                                    MapVal::Obj(Obj::str(resolved.content_type)),
                                );
                                if let Some(origin) = cors_origin {
                                    headers.set(
                                        MapKey::Str(header_key("access-control-allow-origin")),
                                        MapVal::Obj(Obj::str(origin.as_ref())),
                                    );
                                }
                                if cached_to_restore.is_some() {
                                    self.host.cached_http_req = cached_to_restore;
                                }
                                let file = match resolved.body {
                                    StaticFileBody::File { file, .. } => Some(file),
                                    StaticFileBody::HeadOnly { .. } => None,
                                };
                                return Ok(DispatchedHttpResponse::StaticFile {
                                    status: 200,
                                    headers,
                                    file,
                                    len,
                                });
                            } else if allow_file_stream {
                                let file_opt = match resolved.body {
                                    StaticFileBody::HeadOnly { .. } => None,
                                    StaticFileBody::File { file, .. } => Some(file),
                                };
                                let sentinel_str = Arc::new(VmString::from(""));
                                static_passthrough = Some((Arc::clone(&sentinel_str), file_opt, len));
                                make_response_with_body_obj(
                                    200,
                                    Obj::Str(sentinel_str),
                                    Some(resolved.content_type),
                                )
                            } else {
                                match resolved.body {
                                    StaticFileBody::HeadOnly { .. } => {
                                        make_response_obj(200, "", Some(resolved.content_type))
                                    }
                                    StaticFileBody::File { mut file, len } => {
                                        let mut bytes = Vec::with_capacity(
                                            (len as usize).min(crate::native::http::MAX_BODY),
                                        );
                                        if std::io::Read::read_to_end(&mut file, &mut bytes).is_ok() {
                                            let body = String::from_utf8_lossy(&bytes).into_owned();
                                            make_response_obj(200, body, Some(resolved.content_type))
                                        } else {
                                            make_response_obj(
                                                404,
                                                "Not Found",
                                                Some("text/plain; charset=utf-8"),
                                            )
                                        }
                                    }
                                }
                            }
                        } else if status == 405 {
                            make_response_obj(405, "Method Not Allowed", Some("text/plain; charset=utf-8"))
                        } else {
                            make_response_obj(404, "Not Found", Some("text/plain; charset=utf-8"))
                        }
                    }
                }
            }
        };

        let after_len = self.host.routers[router_id].after.len();
        for idx in 0..after_len {
            let hook = {
                let (prefix, h) = &self.host.routers[router_id].after[idx];
                if let Some(p) = prefix
                    && !route_prefix_matches(p, req_path)
                {
                    continue;
                }
                h.clone()
            };
            resp = self.invoke_http_fn(
                module,
                s_top,
                o_top,
                &hook,
                &[req_obj.clone(), resp],
            )?;
        }
        if let Some(origin) = cors_origin
            && let Obj::Record(ref mut rec_arc) = resp
            && rec_arc.schema_id == SCHEMA_HTTP_RESPONSE
        {
            let rec_mut = Arc::make_mut(rec_arc);
            let mut hdrs = match rec_mut.get_obj(1) {
                Obj::Map(m) => (**m).clone(),
                _ => OrderedMap::with_capacity(1),
            };
            hdrs.set(
                MapKey::Str(header_key("access-control-allow-origin")),
                MapVal::Obj(Obj::str(origin.as_ref())),
            );
            rec_mut.set_obj(1, Obj::Map(Arc::new(hdrs)));
        }
        if cached_to_restore.is_some() {
            self.host.cached_http_req = cached_to_restore;
        }
        if let Some((sentinel_str, file_opt, static_len)) = static_passthrough
            && let Obj::Record(ref rec) = resp
            && rec.schema_id == SCHEMA_HTTP_RESPONSE
            && matches!(rec.raw_obj(0), Obj::Str(s) if Arc::ptr_eq(s, &sentinel_str))
        {
            let status = rec.get_scalar(0) as i64;
            let headers = match rec.get_obj(1) {
                Obj::Map(m) => (**m).clone(),
                _ => OrderedMap::new(),
            };
            if status == 204 || status == 304 {
                return Ok(DispatchedHttpResponse::StaticFile {
                    status,
                    headers,
                    file: None,
                    len: 0,
                });
            }
            return Ok(DispatchedHttpResponse::StaticFile {
                status,
                headers,
                file: file_opt,
                len: static_len,
            });
        }
        Ok(DispatchedHttpResponse::Obj(resp))
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_http_route_handler(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        handler: &Obj,
        query_plan: Option<u16>,
        body_json_plan: Option<u16>,
        guard: Option<&Obj>,
        auto_resp_kind: Option<u8>,
        req_obj: &Obj,
    ) -> Result<Obj, RuntimeError> {
        if guard.is_none() && query_plan.is_none() && body_json_plan.is_none() {
            if auto_resp_kind.is_none()
                && let Obj::Func(fid) = handler
                && let [
                    crate::vm::opcode::Op::LoadConstObj { dst_obj: r0, idx },
                    crate::vm::opcode::Op::ExtOp {
                        kind: crate::vm::opcode::EXT_HTTP_OP,
                        aux,
                        a: r1,
                        b: r0_b,
                        c: 0,
                    },
                    crate::vm::opcode::Op::ReturnObj { src: r1_b },
                ] = module.functions[*fid as usize].ops.as_ref()
                && r0 == r0_b
                && r1 == r1_b
                && matches!(*aux, HTTP_TEXT | HTTP_HTML | HTTP_JSON)
                && let Some(body @ Obj::Str(_)) = module.obj_consts.get(*idx as usize)
            {
                let ct = match *aux {
                    HTTP_TEXT => Some("text/plain; charset=utf-8"),
                    HTTP_HTML => Some("text/html; charset=utf-8"),
                    _ => Some("application/json"),
                };
                return Ok(make_response_with_body_obj(200, body.clone(), ct));
            }
            let raw_res = self.invoke_http_fn_raw(
                module,
                s_top,
                o_top,
                handler,
                std::slice::from_ref(req_obj),
            )?;
            return convert_auto_http_response(module, raw_res, auto_resp_kind);
        }

        let guard_ctx = if let Some(g) = guard {
            let res = self.invoke_http_fn(module, s_top, o_top, g, std::slice::from_ref(req_obj))?;
            if matches!(&res, Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_RESPONSE) {
                return Ok(res);
            }
            Some(res)
        } else {
            None
        };

        let mut call_args = Vec::with_capacity(4);
        call_args.push(req_obj.clone());

        if let Some(q_idx) = query_plan {
            let decoded_q = match decode_http_query_params(&module.http_query_plans[q_idx as usize], req_obj) {
                Ok(v) => v,
                Err(msg) => {
                    return Ok(make_response_obj(
                        400,
                        format!(r#"{{"error":"invalid_query","message":"{}"}}"#, json_escape_str(&msg)),
                        Some("application/json; charset=utf-8"),
                    ));
                }
            };
            let mut violations = Vec::new();
            self.validate_dyn_val(module, s_top, o_top, &decoded_q, "", &mut violations, 0)?;
            if !violations.is_empty() {
                return Ok(format_validation_error_response(&violations));
            }
            call_args.push(decoded_q);
        }

        if let Some(plan_idx) = body_json_plan {
            let body_str = match req_obj {
                Obj::Record(rec) => {
                    if rec.obj_len >= 7
                        && let Obj::Bytes(bytes) = rec.get_obj(6)
                        && std::str::from_utf8(bytes).is_err()
                    {
                        return Ok(make_response_obj(
                            400,
                            r#"{"error":"invalid_json","message":"Request body is not valid UTF-8"}"#,
                            Some("application/json; charset=utf-8"),
                        ));
                    }
                    match rec.get_obj(2) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    }
                }
                _ => "",
            };
            let decoded = match crate::native::json_decode::decode_json_with_plan(
                body_str,
                &module.json_plans[plan_idx as usize],
            ) {
                Ok(v) => v,
                Err(e) => {
                    let msg = json_escape_str(&e.message);
                    return Ok(make_response_obj(
                        400,
                        format!(r#"{{"error":"invalid_json","message":"{}"}}"#, msg),
                        Some("application/json; charset=utf-8"),
                    ));
                }
            };
            let mut violations = Vec::new();
            self.validate_dyn_val(module, s_top, o_top, &decoded, "", &mut violations, 0)?;
            if !violations.is_empty() {
                return Ok(format_validation_error_response(&violations));
            }
            call_args.push(decoded);
        }

        if let Some(ctx) = guard_ctx {
            call_args.push(ctx);
        }

        let raw_res = self.invoke_http_fn_raw(module, s_top, o_top, handler, &call_args)?;
        convert_auto_http_response(module, raw_res, auto_resp_kind)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_http_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            HTTP_TEXT => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.text()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("text/plain; charset=utf-8"));
            }
            HTTP_HTML => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.html()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("text/html; charset=utf-8"));
            }
            HTTP_JSON => {
                let body = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.json()")),
                };
                self.objs[o_base + a as usize] =
                    make_response_with_body_obj(200, body, Some("application/json"));
            }
            HTTP_RESPONSE => {
                let status = self.scalars[s_base + b as usize] as i64;
                let body = match &self.objs[o_base + c as usize] {
                    Obj::Str(_) => self.objs[o_base + c as usize].clone(),
                    _ => return Err(err_msg("Expected String in http.response()")),
                };
                self.objs[o_base + a as usize] = make_response_with_body_obj(status, body, None);
            }
            HTTP_ROUTER => {
                let id = self.host.routers.len();
                self.host.routers.push(RouterState::default());
                self.objs[o_base + a as usize] = make_router_obj(id as i64);
            }
            HTTP_ROUTE_GET
            | HTTP_ROUTE_POST
            | HTTP_ROUTE_PUT
            | HTTP_ROUTE_DELETE
            | HTTP_ROUTE_PATCH
            | HTTP_ROUTE_OPTIONS
            | HTTP_ROUTE_HEAD
            | HTTP_ROUTE_ANY => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let pattern = self.sys_str(o_base, b, "router.route()")?;
                let handler = self.objs[o_base + c as usize].clone();
                let method = match aux {
                    HTTP_ROUTE_GET => "GET",
                    HTTP_ROUTE_POST => "POST",
                    HTTP_ROUTE_PUT => "PUT",
                    HTTP_ROUTE_DELETE => "DELETE",
                    HTTP_ROUTE_PATCH => "PATCH",
                    HTTP_ROUTE_OPTIONS => "OPTIONS",
                    HTTP_ROUTE_HEAD => "HEAD",
                    _ => "*",
                };
                self.host.routers[rid]
                    .push_route(RouteEntry::new(method, &pattern, handler));
            }
            HTTP_ROUTE_BEFORE => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].before.push((None, handler));
            }
            HTTP_ROUTE_AFTER => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].after.push((None, handler));
            }
            HTTP_ROUTE_ON_START => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].on_start.push(handler);
            }
            HTTP_ROUTE_ON_STOP => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let handler = self.objs[o_base + b as usize].clone();
                self.host.routers[rid].on_stop.push(handler);
            }
            HTTP_ROUTE_GROUP_BEFORE => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let prefix = self.sys_str(o_base, b, "router group before")?.as_str().into();
                let handler = self.objs[o_base + c as usize].clone();
                self.host.routers[rid].before.push((Some(prefix), handler));
            }
            HTTP_ROUTE_GROUP_AFTER => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let prefix = self.sys_str(o_base, b, "router group after")?.as_str().into();
                let handler = self.objs[o_base + c as usize].clone();
                self.host.routers[rid].after.push((Some(prefix), handler));
            }
            HTTP_ROUTE_STATIC => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let url_prefix = self.sys_str(o_base, b, "router static")?.as_str().into();
                let dir_path = self.sys_str(o_base, c, "router static")?.as_str().into();
                self.host.routers[rid].statics.push((url_prefix, dir_path));
            }
            HTTP_ROUTE_CORS => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let origin = self.sys_str(o_base, b, "router cors")?.as_str().into();
                self.host.routers[rid].cors_origin = Some(origin);
            }
            HTTP_ROUTER_WORKERS => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let count = self.scalars[s_base + b as usize] as i64;
                self.host.routers[rid].server_opts.workers = if count > 0 {
                    Some((count as usize).clamp(1, 256))
                } else {
                    None
                };
            }
            HTTP_ROUTER_TIMEOUTS => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed args for Router.timeouts()"));
                };
                let idle_ms = packed.get_scalar(0) as i64;
                let header_ms = packed.get_scalar(1) as i64;
                let body_ms = packed.get_scalar(2) as i64;
                let opts = &mut self.host.routers[rid].server_opts;
                opts.idle_timeout_ms =
                    (idle_ms > 0).then(|| (idle_ms as u64).clamp(100, 300_000));
                opts.header_timeout_ms =
                    (header_ms > 0).then(|| (header_ms as u64).clamp(500, 300_000));
                opts.body_timeout_ms =
                    (body_ms > 0).then(|| (body_ms as u64).clamp(500, 600_000));
            }
            HTTP_ROUTER_MAX_CONNS_PER_IP => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let limit = self.scalars[s_base + b as usize] as i64;
                self.host.routers[rid].server_opts.max_conns_per_ip = if limit >= 0 {
                    Some(limit as usize)
                } else {
                    None
                };
            }
            HTTP_ROUTER_TRUSTED_PROXIES => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let Obj::List(ListStorage::Obj(items)) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected [String] in Router.trusted_proxies()"));
                };
                let mut cidrs = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let Obj::Str(s) = item else {
                        return Err(err_msg("Expected [String] in Router.trusted_proxies()"));
                    };
                    cidrs.push(s.as_str().to_string());
                }
                self.host.routers[rid].server_opts.trusted_proxies = cidrs;
            }
            HTTP_ROUTE_TYPED_BODY => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                if let Some(last) = self.host.routers[rid].routes.last_mut() {
                    last.body_json_plan = Some(c);
                }
            }
            HTTP_ROUTE_GUARD => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                let g = self.objs[o_base + b as usize].clone();
                if let Some(last) = self.host.routers[rid].routes.last_mut() {
                    last.guard = Some(g);
                }
            }
            HTTP_ROUTE_TYPED_QUERY => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                if let Some(last) = self.host.routers[rid].routes.last_mut() {
                    last.query_plan = Some(c);
                }
            }
            HTTP_ROUTE_AUTO_RESP => {
                let rid = self.extract_router_id(&self.objs[o_base + a as usize])?;
                if let Some(last) = self.host.routers[rid].routes.last_mut() {
                    last.auto_resp_kind = Some(c as u8);
                }
            }
            HTTP_OK_JSON | HTTP_STATUS_JSON => {
                let (status, val) = if aux == HTTP_OK_JSON {
                    (200i64, &self.objs[o_base + b as usize])
                } else {
                    (
                        self.scalars[s_base + b as usize] as i64,
                        &self.objs[o_base + c as usize],
                    )
                };
                let resp = match crate::native::json_encode::encode_json_obj(val, &module.struct_schemas) {
                    Ok(json_str) => make_response_obj(status, json_str, Some("application/json; charset=utf-8")),
                    Err(e) => make_response_obj(
                        500,
                        format!(r#"{{"error":"encode_error","message":"{}"}}"#, json_escape_str(&e.message)),
                        Some("application/json; charset=utf-8"),
                    ),
                };
                self.objs[o_base + a as usize] = resp;
            }
            HTTP_REDIRECT => {
                let status = self.scalars[s_base + b as usize] as i64;
                let url = self.sys_str(o_base, c, "http.redirect()")?;
                let mut hdrs = OrderedMap::with_capacity(1);
                hdrs.set(
                    MapKey::Str(header_key("location")),
                    MapVal::Obj(Obj::Str(url)),
                );
                let mut s = StructData::new(SCHEMA_HTTP_RESPONSE, 1, 2);
                s.set_scalar(0, status as u64);
                s.set_obj(0, Obj::str(""));
                s.set_obj(1, Obj::Map(Arc::new(hdrs)));
                self.objs[o_base + a as usize] = Obj::Record(Arc::new(s));
            }
            HTTP_NO_CONTENT => {
                self.objs[o_base + a as usize] = make_response_obj(204, "", None);
            }
            HTTP_REQ_HEADER | HTTP_REQ_PARAM | HTTP_REQ_QUERY_PARAM => {
                let Obj::Record(rec) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected http.Request"));
                };
                let key = self.sys_str(o_base, c, "Request lookup")?;
                let field_idx = match aux {
                    HTTP_REQ_HEADER => 3,
                    HTTP_REQ_QUERY_PARAM => 4,
                    _ => 5,
                };
                let found = match rec.raw_obj(field_idx) {
                    Obj::DeferredField(df) if aux != HTTP_REQ_HEADER => {
                        if let Some((ek, ev)) = df.as_single_map_entry() {
                            if ek.as_str() == key.as_str() {
                                Some(ev.clone())
                            } else {
                                None
                            }
                        } else if let Obj::Map(map) = df.value() {
                            map.get(&MapKey::Str(key)).and_then(|v| match v {
                                MapVal::Obj(o) => Some(o.clone()),
                                _ => None,
                            })
                        } else {
                            None
                        }
                    }
                    _ => match rec.get_obj(field_idx) {
                        Obj::Map(map) => {
                            if aux == HTTP_REQ_HEADER {
                                let lower = if key.bytes().any(|b| b.is_ascii_uppercase()) {
                                    std::borrow::Cow::Owned(key.as_str().to_ascii_lowercase())
                                } else {
                                    std::borrow::Cow::Borrowed(key.as_str())
                                };
                                map.get_str(&lower)
                                    .or_else(|| {
                                        map.entries.iter().find_map(|(k, v)| match k {
                                            MapKey::Str(ks) if ks.as_str().eq_ignore_ascii_case(key.as_str()) => Some(v),
                                            _ => None,
                                        })
                                    })
                                    .and_then(|v| match v {
                                        MapVal::Obj(o) => Some(o.clone()),
                                        _ => None,
                                    })
                            } else {
                                map.get(&MapKey::Str(key)).and_then(|v| match v {
                                    MapVal::Obj(o) => Some(o.clone()),
                                    _ => None,
                                })
                            }
                        }
                        _ => None,
                    },
                };
                self.objs[o_base + a as usize] = found.unwrap_or(Obj::Empty);
            }
            HTTP_GET_JSON => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed get_json args"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let mut url = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String URL in http.get_json()")),
                };
                if let Obj::Map(qmap) = packed.get_obj(1)
                    && !qmap.is_empty()
                    && let Obj::Str(qs) = crate::native::url::encode_query_map(qmap)
                {
                    if url.contains('?') {
                        url.push('&');
                    } else {
                        url.push('?');
                    }
                    url.push_str(qs.as_str());
                }
                let hdrs = vec![("Accept".to_string(), "application/json".to_string())];
                let resp = http_client_request("GET", &url, "", &hdrs);
                let out = match &resp {
                    Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_RESPONSE => {
                        let body = match rec.get_obj(0) {
                            Obj::Str(s) => s.as_str(),
                            _ => "",
                        };
                        match crate::native::json_decode::decode_json_with_plan(
                            body,
                            &module.json_plans[plan_idx],
                        ) {
                            Ok(v) => v,
                            Err(e) => make_http_error("decode_error", &e.message),
                        }
                    }
                    _ => resp,
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_POST_JSON => {
                let url = self.sys_str(o_base, b, "http.post_json()")?;
                let val = &self.objs[o_base + c as usize];
                let out = match crate::native::json_encode::encode_json_obj(val, &module.struct_schemas) {
                    Ok(body) => {
                        let hdrs = vec![
                            ("Content-Type".to_string(), "application/json".to_string()),
                            ("Accept".to_string(), "application/json".to_string()),
                        ];
                        http_client_request("POST", url.as_str(), &body, &hdrs)
                    }
                    Err(e) => make_http_error("encode_error", &e.message),
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_CLIENT_NEW => {
                let base_url = self.sys_str(o_base, b, "http.client()")?.as_str().to_string();
                let cid = self.host.http_clients.len();
                self.host.http_clients.push(HttpClientState::new(base_url));
                self.objs[o_base + a as usize] = make_http_client_obj(cid);
            }
            HTTP_CLIENT_HEADER => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed Client.header args"));
                };
                let name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.header name must be String")),
                };
                let value = match packed.get_obj(1) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.header value must be String")),
                };
                let mut next = self.host.http_clients[cid].clone();
                next.set_header(name, value);
                let new_cid = self.host.http_clients.len();
                self.host.http_clients.push(next);
                self.objs[o_base + a as usize] = make_http_client_obj(new_cid);
            }
            HTTP_CLIENT_TIMEOUT => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let ms = self.scalars[s_base + c as usize] as i64;
                let mut next = self.host.http_clients[cid].clone();
                next.timeout_ms = ms;
                let new_cid = self.host.http_clients.len();
                self.host.http_clients.push(next);
                self.objs[o_base + a as usize] = make_http_client_obj(new_cid);
            }
            HTTP_CLIENT_BASE_URL => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let base_url = self.sys_str(o_base, c, "Client.base_url()")?.as_str().to_string();
                let mut next = self.host.http_clients[cid].clone();
                next.base_url = base_url;
                let new_cid = self.host.http_clients.len();
                self.host.http_clients.push(next);
                self.objs[o_base + a as usize] = make_http_client_obj(new_cid);
            }
            HTTP_CLIENT_DENY_PRIVATE => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let deny = self.scalars[s_base + c as usize] != 0;
                let mut next = self.host.http_clients[cid].clone();
                next.deny_private_addrs = deny;
                let new_cid = self.host.http_clients.len();
                self.host.http_clients.push(next);
                self.objs[o_base + a as usize] = make_http_client_obj(new_cid);
            }
            HTTP_CLIENT_REQUEST => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed Client.request args"));
                };
                let method = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.request method must be String")),
                };
                let path = match packed.get_obj(1) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.request path must be String")),
                };
                let body = match packed.get_obj(2) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => String::new(),
                };
                let mut hdrs = self.host.http_clients[cid].default_headers.clone();
                if let Obj::Map(extra) = packed.get_obj(3) {
                    for (k, v) in &extra.entries {
                        if let (MapKey::Str(ks), MapVal::Obj(Obj::Str(vs))) = (k, v) {
                            if let Some((_, existing)) = hdrs
                                .iter_mut()
                                .find(|(hk, _)| hk.eq_ignore_ascii_case(ks.as_str()))
                            {
                                *existing = vs.as_str().to_string();
                            } else {
                                hdrs.push((ks.as_str().to_string(), vs.as_str().to_string()));
                            }
                        }
                    }
                }
                let full_url = resolve_client_url(&self.host.http_clients[cid].base_url, &path);
                let timeout_ms = self.host.http_clients[cid].timeout_ms;
                let deny_private = self.host.http_clients[cid].deny_private_addrs;
                let pool = Arc::clone(&self.host.http_clients[cid].pool);
                let resp = http_client_pooled_request_with_options(
                    &method,
                    &full_url,
                    &body,
                    &hdrs,
                    timeout_ms,
                    &pool,
                    deny_private,
                );
                self.objs[o_base + a as usize] = resp;
            }
            HTTP_CLIENT_GET_JSON => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed Client.get_json args"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let path = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.get_json path must be String")),
                };
                let mut full_url = resolve_client_url(&self.host.http_clients[cid].base_url, &path);
                if let Obj::Map(qmap) = packed.get_obj(1)
                    && !qmap.is_empty()
                    && let Obj::Str(qs) = crate::native::url::encode_query_map(qmap)
                {
                    if full_url.contains('?') {
                        full_url.push('&');
                    } else {
                        full_url.push('?');
                    }
                    full_url.push_str(qs.as_str());
                }
                let mut hdrs = self.host.http_clients[cid].default_headers.clone();
                if !hdrs.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
                    hdrs.push(("Accept".to_string(), "application/json".to_string()));
                }
                let timeout_ms = self.host.http_clients[cid].timeout_ms;
                let deny_private = self.host.http_clients[cid].deny_private_addrs;
                let pool = Arc::clone(&self.host.http_clients[cid].pool);
                let resp = http_client_pooled_request_with_options(
                    "GET",
                    &full_url,
                    "",
                    &hdrs,
                    timeout_ms,
                    &pool,
                    deny_private,
                );
                let out = match &resp {
                    Obj::Record(rec) if rec.schema_id == SCHEMA_HTTP_RESPONSE => {
                        let body = match rec.get_obj(0) {
                            Obj::Str(s) => s.as_str(),
                            _ => "",
                        };
                        match crate::native::json_decode::decode_json_with_plan(
                            body,
                            &module.json_plans[plan_idx],
                        ) {
                            Ok(v) => v,
                            Err(e) => make_http_error("decode_error", &e.message),
                        }
                    }
                    _ => resp,
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_CLIENT_POST_JSON => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed Client.post_json args"));
                };
                let path = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Client.post_json path must be String")),
                };
                let val = packed.get_obj(1);
                let out = match crate::native::json_encode::encode_json_obj(val, &module.struct_schemas) {
                    Ok(body) => {
                        let mut hdrs = self.host.http_clients[cid].default_headers.clone();
                        if !hdrs.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                            hdrs.push(("Content-Type".to_string(), "application/json".to_string()));
                        }
                        if !hdrs.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
                            hdrs.push(("Accept".to_string(), "application/json".to_string()));
                        }
                        let full_url = resolve_client_url(&self.host.http_clients[cid].base_url, &path);
                        let timeout_ms = self.host.http_clients[cid].timeout_ms;
                        let deny_private = self.host.http_clients[cid].deny_private_addrs;
                        let pool = Arc::clone(&self.host.http_clients[cid].pool);
                        http_client_pooled_request_with_options(
                            "POST",
                            &full_url,
                            &body,
                            &hdrs,
                            timeout_ms,
                            &pool,
                            deny_private,
                        )
                    }
                    Err(e) => make_http_error("encode_error", &e.message),
                };
                self.objs[o_base + a as usize] = out;
            }
            HTTP_CLIENT_CLOSE => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                self.host.http_clients[cid].pool.clear();
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            HTTP_CLIENT_POOL_STATS => {
                let cid = self.extract_client_id(&self.objs[o_base + b as usize])?;
                let idle = self.host.http_clients[cid].pool.idle_count() as i64;
                let mut m = OrderedMap::with_capacity(1);
                m.set(
                    MapKey::Str(Arc::new("idle".into())),
                    MapVal::Scalar(idle as u64),
                );
                self.objs[o_base + a as usize] = Obj::Map(Arc::new(m));
            }
            HTTP_DISPATCH => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                let req_obj = self.objs[o_base + c as usize].clone();
                let resp = self.dispatch_router_request(module, s_top, o_top, rid, req_obj)?;
                self.objs[o_base + a as usize] = resp;
            }
            HTTP_SERVE => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                let addr = self.sys_str(o_base, c, "router.serve()")?;
                if !module.on_start_funcs.is_empty()
                    && !self.host.lifecycle_started
                    && let Err(msg) = self.run_module_on_start(module, s_top, o_top)
                {
                    self.objs[o_base + a as usize] = make_http_error("startup_error", msg);
                    return Ok(());
                }
                let on_start_hooks = self.host.routers[rid].on_start.clone();
                for hook in on_start_hooks {
                    match self.invoke_zero_arg_cb(module, s_top, o_top, &hook, 0) {
                        Ok(obj)
                            if crate::native::validation::is_err_or_nil_obj(module, &obj)
                                && !matches!(obj, Obj::Empty) =>
                        {
                            self.objs[o_base + a as usize] = make_http_error(
                                "startup_error",
                                format!("router on_start hook failed: {}", obj.format_obj()),
                            );
                            return Ok(());
                        }
                        Ok(_) => {}
                        Err(e) => {
                            self.objs[o_base + a as usize] =
                                make_http_error("startup_error", e.message);
                            return Ok(());
                        }
                    }
                }
                crate::vm::exec_map::flush_stdout();
                let listener = match TcpListener::bind(addr.as_str()) {
                    Ok(l) => l,
                    Err(e) => {
                        self.objs[o_base + a as usize] = make_http_error("bind_error", e.to_string());
                        return Ok(());
                    }
                };
                let stop_flag = Arc::clone(&self.host.routers[rid].stop_flag);
                let shared_mod = self
                    .host
                    .shared_module
                    .get_or_insert_with(|| Arc::new(module.clone()))
                    .clone();
                serve_http_listener(
                    shared_mod,
                    self.host.routers.clone(),
                    self.host.http_clients.clone(),
                    self.host.db_pools.clone(),
                    self.host.db_pool_free.clone(),
                    Arc::clone(&self.host.spawn_tracker),
                    self.script_args.clone(),
                    rid,
                    listener,
                    stop_flag,
                    true,
                );
                if self.host.spawn_tracker.active_count() > 0 {
                    self.host
                        .spawn_tracker
                        .wait_idle(Duration::from_secs(5));
                }
                let mut stop_err: Option<String> = None;
                let on_stop_hooks = self.host.routers[rid].on_stop.clone();
                for hook in on_stop_hooks {
                    match self.invoke_zero_arg_cb(module, s_top, o_top, &hook, 0) {
                        Ok(obj)
                            if crate::native::validation::is_err_or_nil_obj(module, &obj)
                                && !matches!(obj, Obj::Empty) =>
                        {
                            if stop_err.is_none() {
                                stop_err = Some(format!(
                                    "router on_stop hook failed: {}",
                                    obj.format_obj()
                                ));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            if stop_err.is_none() {
                                stop_err = Some(e.message);
                            }
                        }
                    }
                }
                if !module.on_stop_funcs.is_empty()
                    && !self.host.lifecycle_stopped
                    && let Err(msg) = self.run_module_on_stop(module, s_top, o_top)
                    && stop_err.is_none()
                {
                    stop_err = Some(msg);
                }
                crate::vm::exec_map::flush_stdout();
                self.objs[o_base + a as usize] = match stop_err {
                    Some(msg) => make_http_error("shutdown_error", msg),
                    None => Obj::Empty,
                };
            }
            HTTP_STOP => {
                let rid = self.extract_router_id(&self.objs[o_base + b as usize])?;
                self.host.routers[rid].stop_flag.store(true, Ordering::Release);
                if let Ok(guard) = self.host.routers[rid].listen_addr.lock()
                    && let Some(mut addr) = *guard
                {
                    if addr.ip().is_unspecified() {
                        addr.set_ip(if addr.is_ipv4() {
                            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                        } else {
                            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
                        });
                    }
                    let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(50));
                }
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            HTTP_GET => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.get expects String url")),
                };
                self.objs[o_base + a as usize] = http_client_request("GET", &url, "", &[]);
            }
            HTTP_POST => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.post expects String url")),
                };
                let body = match &self.objs[o_base + c as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.post expects String body")),
                };
                self.objs[o_base + a as usize] = http_client_request("POST", &url, &body, &[]);
            }
            HTTP_REQUEST | HTTP_REQUEST_TIMEOUT => {
                let req = extract_packed_request(&self.objs[o_base + b as usize])?;
                self.objs[o_base + a as usize] = if aux == HTTP_REQUEST_TIMEOUT {
                    let timeout_ms = self.scalars[s_base + c as usize] as i64;
                    http_client_request_with_timeout(
                        &req.method,
                        &req.url,
                        &req.body,
                        &req.headers,
                        timeout_ms,
                    )
                } else {
                    http_client_request(&req.method, &req.url, &req.body, &req.headers)
                };
            }
            HTTP_GET_BYTES => {
                let url = match &self.objs[o_base + b as usize] {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.get_bytes expects String url")),
                };
                self.objs[o_base + a as usize] =
                    http_client_bytes_request("GET", &url, &[], &[], 30_000);
            }
            HTTP_REQUEST_BYTES => {
                let Obj::Record(sd) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("http.request_bytes expects packed request args"));
                };
                if sd.schema_id != SCHEMA_HTTP_REQUEST {
                    return Err(err_msg("http.request_bytes expects packed request args"));
                }
                let method = match sd.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.request_bytes expects String method")),
                };
                let url = match sd.get_obj(1) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("http.request_bytes expects String url")),
                };
                let converted;
                let body_bytes = match sd.get_obj(2) {
                    Obj::Bytes(bytes) => bytes.as_slice(),
                    Obj::List(ListStorage::Int(items)) => {
                        let mut bytes = Vec::with_capacity(items.len());
                        for &v in items.iter() {
                            match u8::try_from(v) {
                                Ok(byte) => bytes.push(byte),
                                Err(_) => {
                                    self.objs[o_base + a as usize] = make_http_error(
                                        "invalid_body",
                                        format!("byte value {} out of range 0..=255", v),
                                    );
                                    return Ok(());
                                }
                            }
                        }
                        converted = bytes;
                        &converted
                    }
                    _ => return Err(err_msg("http.request_bytes expects Bytes or [Int] body")),
                };
                let hdrs = extract_header_pairs(sd.get_obj(3));
                self.objs[o_base + a as usize] =
                    http_client_bytes_request(&method, &url, body_bytes, &hdrs, 30_000);
            }
            _ => return Err(err_msg("Unknown http sub-op")),
        }
        Ok(())
    }
}

struct PackedRequestArgs {
    method: String,
    url: String,
    body: String,
    headers: Vec<(String, String)>,
}

fn extract_packed_request(obj: &Obj) -> Result<PackedRequestArgs, RuntimeError> {
    let Obj::Record(sd) = obj else {
        return Err(err_msg("http.request expects packed request args"));
    };
    if sd.schema_id != SCHEMA_HTTP_REQUEST {
        return Err(err_msg("http.request expects packed request args"));
    }
    let method = match sd.get_obj(0) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String method")),
    };
    let url = match sd.get_obj(1) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String url")),
    };
    let body = match sd.get_obj(2) {
        Obj::Str(s) => s.as_str().to_string(),
        _ => return Err(err_msg("http.request expects String body")),
    };
    let hdrs = extract_header_pairs(sd.get_obj(3));
    Ok(PackedRequestArgs {
        method,
        url,
        body,
        headers: hdrs,
    })
}

fn extract_header_pairs(obj: &Obj) -> Vec<(String, String)> {
    let mut hdrs = Vec::new();
    if let Obj::Map(m) = obj {
        for (k, v) in m.entries.iter() {
            if let (crate::vm::value::MapKey::Str(ks), crate::vm::value::MapVal::Obj(Obj::Str(vs))) = (k, v) {
                hdrs.push((ks.as_str().to_string(), vs.as_str().to_string()));
            }
        }
    }
    hdrs
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
fn serve_connection_isolated(
    vm: &mut Vm,
    module: &Module,
    rid: usize,
    stream: TcpStream,
    idle_timeout: Duration,
    header_timeout: Duration,
    body_timeout: Duration,
) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve_connection(vm, module, rid, stream, idle_timeout, header_timeout, body_timeout);
    }))
    .is_err()
    {
        eprintln!("[http] worker recovered from request panic");
        vm.frame_depth = 0;
        let obj_len = vm.objs.len();
        vm.clear_obj_range(0, obj_len);
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
fn serve_connection(
    vm: &mut Vm,
    module: &Module,
    rid: usize,
    stream: TcpStream,
    idle_timeout: Duration,
    header_timeout: Duration,
    body_timeout: Duration,
) {
    let _ = stream.set_read_timeout(Some(header_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Ok((mut reader, mut writer)) = deadline_reader_writer_pair(stream, header_timeout)
    else {
        return;
    };
    let mut req_count: usize = 0;
    loop {
        let idle = if req_count == 0 {
            header_timeout
        } else {
            idle_timeout
        };
        match read_http_request_deadline(&mut reader, idle, header_timeout, body_timeout)
        {
            Ok(raw) => {
                req_count += 1;
                let head_only = raw.method.eq_ignore_ascii_case("HEAD");
                let keep_alive = raw.keep_alive && req_count < HTTP_MAX_KEEPALIVE_REQUESTS;
                let req_obj = raw.into_request_obj();
                match vm.dispatch_router_request_inner(module, 0, 0, rid, req_obj, true) {
                    Ok(DispatchedHttpResponse::Obj(resp_obj)) => {
                        let Ok((status, body, headers)) = vm.extract_response_fields(&resp_obj) else {
                            write_error_response(&mut writer, 500, "Internal Server Error");
                            break;
                        };
                        if let Err(err) = write_http_response_ext(
                            &mut writer,
                            status,
                            body,
                            headers,
                            keep_alive,
                            head_only,
                        ) {
                            if err.kind() == std::io::ErrorKind::InvalidInput {
                                write_error_response(&mut writer, 500, "Internal Server Error");
                            }
                            break;
                        }
                    }
                    Ok(DispatchedHttpResponse::StaticFile {
                        status,
                        headers,
                        mut file,
                        len,
                    }) => {
                        if let Err(err) = write_http_file_response(
                            &mut writer,
                            status,
                            file.as_mut(),
                            len,
                            Some(&headers),
                            keep_alive,
                            head_only,
                        ) {
                            if err.kind() == std::io::ErrorKind::InvalidInput {
                                write_error_response(&mut writer, 500, "Internal Server Error");
                            }
                            break;
                        }
                    }
                    Err(_) => {
                        write_error_response(&mut writer, 500, "Internal Server Error");
                        break;
                    }
                }
                if !keep_alive || vm.host.routers[rid].stop_flag.load(Ordering::Acquire) {
                    break;
                }
            }
            Err(ReadHttpError::Closed) => break,
            Err(ReadHttpError::Timeout) => {
                write_error_response(&mut writer, 408, "Request Timeout");
                break;
            }
            Err(ReadHttpError::TooLarge) => {
                write_error_response(&mut writer, 413, "Payload Too Large");
                break;
            }
            Err(ReadHttpError::Bad(msg)) => {
                write_error_response(&mut writer, 400, &msg);
                break;
            }
            Err(ReadHttpError::Io(_)) => break,
        }
    }
}

fn format_validation_error_response(violations: &[Obj]) -> Obj {
    let mut body = String::from(r#"{"error":"validation_failed","issues":["#);
    for (idx, v) in violations.iter().enumerate() {
        if idx > 0 {
            body.push(',');
        }
        if let Obj::Record(rec) = v {
            let path = match rec.get_obj(0) {
                Obj::Str(s) => s.as_str(),
                _ => "",
            };
            let code = match rec.get_obj(1) {
                Obj::Str(s) => s.as_str(),
                _ => "",
            };
            let msg = match rec.get_obj(2) {
                Obj::Str(s) => s.as_str(),
                _ => "",
            };
            body.push_str(&format!(
                r#"{{"path":"{}","code":"{}","message":"{}"}}"#,
                json_escape_str(path),
                json_escape_str(code),
                json_escape_str(msg),
            ));
        }
    }
    body.push_str("]}");
    make_response_obj(422, &body, Some("application/json; charset=utf-8"))
}

fn lookup_req_str_map<'a>(map_obj: &'a Obj, key: &str) -> Option<&'a str> {
    let Obj::Map(m) = map_obj else {
        return None;
    };
    let val = m.get_str(key)?;
    match val {
        MapVal::Obj(Obj::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn decode_http_query_params(
    plan: &crate::ir::HttpQueryPlan,
    req_obj: &Obj,
) -> Result<Obj, String> {
    let Obj::Record(rec) = req_obj else {
        return Err("Expected Request".to_string());
    };
    let query_map = rec.get_obj(4);
    let params_map = rec.get_obj(5);
    let mut out = StructData::new(plan.schema_id, plan.scalar_len, plan.obj_len);

    for f in &plan.fields {
        let raw_opt = lookup_req_str_map(params_map, &f.param_name)
            .or_else(|| lookup_req_str_map(query_map, &f.param_name));

        match raw_opt {
            Some(raw) => {
                if f.is_optional && raw.is_empty() && f.kind != 0 {
                    out.set_obj(f.slot as usize, Obj::Empty);
                    continue;
                }
                match f.kind {
                    0 => out.set_obj(f.slot as usize, Obj::str(raw)),
                    1 => {
                        let n = raw.trim().parse::<i64>().map_err(|_| {
                            format!("Parameter '{}' must be an integer", f.param_name)
                        })?;
                        if f.is_optional {
                            out.set_obj(f.slot as usize, Obj::boxed_int(n));
                        } else {
                            out.set_scalar(f.slot as usize, n as u64);
                        }
                    }
                    2 => {
                        let v = raw.trim().parse::<f64>().map_err(|_| {
                            format!("Parameter '{}' must be a number", f.param_name)
                        })?;
                        if !v.is_finite() {
                            return Err(format!("Parameter '{}' must be finite", f.param_name));
                        }
                        if f.is_optional {
                            out.set_obj(f.slot as usize, Obj::boxed_float(v.to_bits()));
                        } else {
                            out.set_scalar(f.slot as usize, v.to_bits());
                        }
                    }
                    3 => {
                        let b = match raw.trim().to_ascii_lowercase().as_str() {
                            "true" | "1" => true,
                            "false" | "0" => false,
                            _ => {
                                return Err(format!(
                                    "Parameter '{}' must be a boolean",
                                    f.param_name
                                ));
                            }
                        };
                        if f.is_optional {
                            out.set_obj(f.slot as usize, Obj::boxed_bool(b));
                        } else {
                            out.set_scalar(f.slot as usize, b as u64);
                        }
                    }
                    _ => return Err("Unsupported query field kind".to_string()),
                }
            }
            None => match &f.default_val {
                Some(crate::ir::HttpQueryDefault::Nil) => {
                    out.set_obj(f.slot as usize, Obj::Empty);
                }
                Some(crate::ir::HttpQueryDefault::Str(s)) => {
                    out.set_obj(f.slot as usize, Obj::str(s.clone()));
                }
                Some(crate::ir::HttpQueryDefault::Int(n)) => {
                    if f.is_optional {
                        out.set_obj(f.slot as usize, Obj::boxed_int(*n));
                    } else {
                        out.set_scalar(f.slot as usize, *n as u64);
                    }
                }
                Some(crate::ir::HttpQueryDefault::Float(v)) => {
                    if f.is_optional {
                        out.set_obj(f.slot as usize, Obj::boxed_float(v.to_bits()));
                    } else {
                        out.set_scalar(f.slot as usize, v.to_bits());
                    }
                }
                Some(crate::ir::HttpQueryDefault::Bool(b)) => {
                    if f.is_optional {
                        out.set_obj(f.slot as usize, Obj::boxed_bool(*b));
                    } else {
                        out.set_scalar(f.slot as usize, *b as u64);
                    }
                }
                None if f.is_optional => {
                    out.set_obj(f.slot as usize, Obj::Empty);
                }
                None => {
                    return Err(format!("Missing required parameter '{}'", f.param_name));
                }
            },
        }
    }

    Ok(Obj::Record(Arc::new(out)))
}

fn convert_auto_http_response(
    module: &Module,
    raw: ExecResult,
    auto_resp_kind: Option<u8>,
) -> Result<Obj, RuntimeError> {
    let Some(kind) = auto_resp_kind else {
        return match raw {
            ExecResult::Obj(o) => Ok(o),
            _ => Err(err_msg("HTTP handler must return an object")),
        };
    };
    match raw {
        ExecResult::Void => Ok(make_response_obj(204, "", None)),
        ExecResult::Scalar(bits) => {
            let body = match kind {
                2 => {
                    let f = f64::from_bits(bits);
                    if f.is_finite() {
                        f.to_string()
                    } else {
                        "null".to_string()
                    }
                }
                3 => {
                    if bits != 0 {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                }
                4 => return Ok(make_response_obj(204, "", None)),
                _ => (bits as i64).to_string(),
            };
            Ok(make_response_obj(
                200,
                body,
                Some("application/json; charset=utf-8"),
            ))
        }
        ExecResult::Obj(obj) => {
            if let Obj::Record(rec) = &obj
                && rec.schema_id == SCHEMA_HTTP_RESPONSE
            {
                return Ok(obj);
            }
            if matches!(obj, Obj::Empty) {
                if kind == 5 {
                    return Ok(make_response_obj(
                        404,
                        r#"{"error":"not_found","message":"Not found"}"#,
                        Some("application/json; charset=utf-8"),
                    ));
                }
                return Ok(make_response_obj(204, "", None));
            }
            let status = if let Obj::Record(rec) = &obj {
                crate::native::find_schema(&module.struct_schemas, rec.schema_id)
                    .map(|s| {
                        s.http_status
                            .unwrap_or(if s.is_error { 500 } else { 200 })
                    })
                    .unwrap_or(200)
            } else {
                200
            };
            match crate::native::json_encode::encode_json_obj(&obj, &module.struct_schemas) {
                Ok(json_str) => Ok(make_response_obj(
                    status as i64,
                    json_str,
                    Some("application/json; charset=utf-8"),
                )),
                Err(e) => Ok(make_response_obj(
                    500,
                    format!(
                        r#"{{"error":"encode_error","message":"{}"}}"#,
                        json_escape_str(&e.message)
                    ),
                    Some("application/json; charset=utf-8"),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn ip_conn_limiter_enforces_remote_and_loopback_caps_and_releases() {
        let limiter = IpConnLimiter::new(4);
        assert_eq!(limiter.max_remote_per_ip, 4);
        assert_eq!(limiter.max_loopback_per_ip, 32);

        let remote = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));
        let mut remote_permits = Vec::new();
        for _ in 0..limiter.max_remote_per_ip {
            remote_permits.push(limiter.try_acquire(remote).expect("remote within cap"));
        }
        assert!(limiter.try_acquire(remote).is_none(), "remote cap reached");

        let mapped = IpAddr::V6(Ipv4Addr::new(203, 0, 113, 10).to_ipv6_mapped());
        assert!(limiter.try_acquire(mapped).is_none(), "IPv4-mapped IPv6 shares limit");

        drop(remote_permits.pop());
        let _p_extra = limiter.try_acquire(remote).expect("slot freed after drop");
        drop(remote_permits);
        drop(_p_extra);

        // IPv6 clients are grouped by /64 prefix to prevent trivial suffix rotation bypasses.
        let v6_a = IpAddr::V6("2001:db8:abcd:1234::1".parse::<Ipv6Addr>().unwrap());
        let v6_b = IpAddr::V6("2001:db8:abcd:1234:feed::9999".parse::<Ipv6Addr>().unwrap());
        let v6_other_prefix = IpAddr::V6("2001:db8:abcd:1235::1".parse::<Ipv6Addr>().unwrap());
        let mut v6_permits = Vec::new();
        for _ in 0..limiter.max_remote_per_ip {
            v6_permits.push(limiter.try_acquire(v6_a).expect("v6 /64 within cap"));
        }
        assert!(limiter.try_acquire(v6_b).is_none(), "same /64 shares cap");
        assert!(limiter.try_acquire(v6_other_prefix).is_some(), "different /64 has own bucket");
        drop(v6_permits);

        let loopback = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let mut loop_permits = Vec::new();
        for _ in 0..limiter.max_loopback_per_ip {
            loop_permits.push(limiter.try_acquire(loopback).expect("loopback within cap"));
        }
        let v4_mapped_loopback = IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped());
        assert!(
            limiter.try_acquire(v4_mapped_loopback).is_some(),
            "separate loopback IPv4 has own bucket"
        );
        assert!(limiter.try_acquire(loopback).is_none(), "v6 loopback cap enforced");
        drop(loop_permits);
        assert!(limiter.try_acquire(loopback).is_some());

        // Trusted proxies bypass per-IP remote caps.
        let proxies = vec![
            IpNet::parse("172.17.0.0/16").unwrap(),
            IpNet::parse("fd00:1::/48").unwrap(),
            IpNet::parse("10.0.0.5").unwrap(),
        ];
        assert!(proxies[0].contains("172.17.42.9".parse().unwrap()));
        assert!(proxies[0].contains("::ffff:172.17.42.9".parse().unwrap()));
        assert!(!proxies[0].contains("172.18.0.1".parse().unwrap()));
        assert!(proxies[1].contains("fd00:1::beef".parse().unwrap()));
        assert!(proxies[2].contains("10.0.0.5".parse().unwrap()));

        let proxied_limiter = IpConnLimiter::with_config(4, 2, proxies);
        let proxy_ip: IpAddr = "172.17.0.10".parse().unwrap();
        let mut proxy_permits = Vec::new();
        for _ in 0..20 {
            proxy_permits.push(proxied_limiter.try_acquire(proxy_ip).expect("trusted proxy unrestricted"));
        }
        let untrusted_ip: IpAddr = "198.51.100.7".parse().unwrap();
        let _u1 = proxied_limiter.try_acquire(untrusted_ip).unwrap();
        let _u2 = proxied_limiter.try_acquire(untrusted_ip).unwrap();
        assert!(proxied_limiter.try_acquire(untrusted_ip).is_none());
    }

    #[test]
    fn http_server_config_precedence_env_over_router_over_default() {
        let default_cfg = HttpServerConfig::resolve_with_env(&RouterServerOptions::default(), |_| None);
        assert_eq!(default_cfg.idle_timeout, DEFAULT_IDLE_TIMEOUT);
        assert_eq!(default_cfg.header_timeout, DEFAULT_HEADER_TIMEOUT);
        assert_eq!(default_cfg.body_timeout, DEFAULT_BODY_TIMEOUT);
        assert_eq!(
            default_cfg.max_conns,
            (default_cfg.workers * 128).clamp(512, 8192)
        );
        assert!(default_cfg.trusted_proxies.is_empty());

        let router_opts = RouterServerOptions {
            workers: Some(12),
            idle_timeout_ms: Some(1500),
            header_timeout_ms: Some(4000),
            body_timeout_ms: Some(12_000),
            max_conns_per_ip: Some(0),
            trusted_proxies: vec!["10.0.0.0/8".into(), "172.16.0.0/12".into()],
        };
        let router_cfg = HttpServerConfig::resolve_with_env(&router_opts, |_| None);
        assert_eq!(router_cfg.workers, 12);
        assert_eq!(router_cfg.idle_timeout, Duration::from_millis(1500));
        assert_eq!(router_cfg.header_timeout, Duration::from_millis(4000));
        assert_eq!(router_cfg.body_timeout, Duration::from_millis(12_000));
        assert_eq!(router_cfg.max_conns_per_ip, 0);
        assert_eq!(router_cfg.max_conns, 1536);
        assert_eq!(router_cfg.trusted_proxies.len(), 2);

        let env_cfg = HttpServerConfig::resolve_with_env(&router_opts, |k| match k {
            "CONDUIT_HTTP_WORKERS" => Some("24".into()),
            "CONDUIT_HTTP_IDLE_TIMEOUT_MS" => Some("7500".into()),
            "CONDUIT_HTTP_HEADER_TIMEOUT_MS" => Some("9000".into()),
            "CONDUIT_HTTP_BODY_TIMEOUT_MS" => Some("45000".into()),
            "CONDUIT_HTTP_MAX_CONNS_PER_IP" => Some("16".into()),
            "CONDUIT_HTTP_MAX_CONNS" => Some("2048".into()),
            "CONDUIT_HTTP_TRUSTED_PROXIES" => Some("".into()),
            _ => None,
        });
        assert_eq!(env_cfg.workers, 24);
        assert_eq!(env_cfg.idle_timeout, Duration::from_millis(7500));
        assert_eq!(env_cfg.header_timeout, Duration::from_millis(9000));
        assert_eq!(env_cfg.body_timeout, Duration::from_millis(45_000));
        assert_eq!(env_cfg.max_conns_per_ip, 16);
        assert_eq!(env_cfg.max_conns, 2048);
        assert!(env_cfg.trusted_proxies.is_empty());
    }

    #[test]
    fn http_reactor_slowloris_keepalive_pipelining_loopback_e2e() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let src = r#"
            import * as http from "cdt.http"

            export func setup_router() -> http.Router {
                r := http.router()
                r.workers(2)
                r.timeouts(250, 450, 450)
                r.get("/hello", (_req: http.Request) -> http.Response => {
                    return http.text("hi")
                })
                r.post("/echo", (req: http.Request) -> http.Response => {
                    return http.text(req.body)
                })
                return r
            }
        "#;

        let module = crate::frontend::compile_source(src).expect("compile reactor test router");
        let setup_fid = module.func_id("setup_router").unwrap();
        let mut init_vm = Vm::new();
        let router_obj = init_vm.call_obj(&module, setup_fid, &[], &[]).unwrap();
        let Obj::Record(sd) = router_obj else {
            panic!("expected Router");
        };
        let rid = sd.get_scalar(0) as usize;
        let stop_flag = Arc::clone(&init_vm.host.routers[rid].stop_flag);
        let routers = init_vm.host.routers.clone();
        let spawn_tracker = Arc::clone(&init_vm.host.spawn_tracker);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared_mod = Arc::new(module);

        let srv_stop = Arc::clone(&stop_flag);
        let srv_thread = std::thread::spawn(move || {
            serve_http_listener(
                shared_mod,
                routers,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                spawn_tracker,
                None,
                rid,
                listener,
                srv_stop,
                false,
            );
        });

        // 1. Open 8 Slowloris connections (4x worker count) and leave partial headers stalled.
        let mut slow_conns = Vec::new();
        for _ in 0..8 {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\n").unwrap();
            slow_conns.push(s);
        }

        // 2. While all 8 slow sockets are held open, a normal keep-alive connection must respond immediately.
        {
            let mut fast = TcpStream::connect(addr).unwrap();
            fast.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            for _ in 0..2 {
                fast.write_all(b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                    .unwrap();
                let mut buf = [0u8; 512];
                let n = fast.read(&mut buf).unwrap();
                let txt = String::from_utf8_lossy(&buf[..n]);
                assert!(txt.contains("HTTP/1.1 200 OK") && txt.ends_with("\r\n\r\nhi"), "got: {txt}");
            }

            // 3. HTTP pipelining: two requests sent in a single write arrive back in order.
            fast.write_all(
                b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\nPOST /echo HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping",
            )
            .unwrap();
            let mut all = Vec::new();
            fast.read_to_end(&mut all).unwrap();
            let combined = String::from_utf8_lossy(&all);
            assert!(combined.contains("\r\n\r\nhiHTTP/1.1 200 OK"), "pipelined resp 1: {combined}");
            assert!(combined.ends_with("\r\n\r\nping"), "pipelined resp 2: {combined}");
        }

        // 4. Split POST body sent across two writes with a short pause.
        {
            let mut split = TcpStream::connect(addr).unwrap();
            split.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            split
                .write_all(
                    b"POST /echo HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 6\r\nConnection: close\r\n\r\nab",
                )
                .unwrap();
            std::thread::sleep(Duration::from_millis(30));
            split.write_all(b"cdef").unwrap();
            let mut all = String::new();
            split.read_to_string(&mut all).unwrap();
            assert!(all.contains("200 OK") && all.ends_with("\r\n\r\nabcdef"), "split body: {all}");
        }

        // 5. Slowloris partial-header socket receives 408 Request Timeout after header_timeout (~450ms).
        slow_conns[0]
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut timeout_resp = String::new();
        slow_conns[0].read_to_string(&mut timeout_resp).unwrap();
        assert!(
            timeout_resp.starts_with("HTTP/1.1 408 Request Timeout"),
            "expected 408 timeout, got: {timeout_resp}"
        );
        drop(slow_conns);

        // 6. Stop signal shuts down reactor cleanly and promptly.
        let stopped_cleanly = Arc::new(AtomicBool::new(false));
        stop_flag.store(true, Ordering::Release);
        wake_socket_addr(addr);
        srv_thread.join().unwrap();
        stopped_cleanly.store(true, Ordering::Release);
        assert!(stopped_cleanly.load(Ordering::Acquire));
    }

    #[test]
    fn http_static_binary_streaming_and_head_loopback_e2e() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

        let tmp = std::env::temp_dir().join(format!("conduit_static_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut png_bytes: Vec<u8> = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\xff\xfe\xfd\x80".to_vec();
        let target_len = 140 * 1024;
        png_bytes.reserve(target_len);
        for i in 0..(target_len - png_bytes.len()) {
            png_bytes.push(((i * 73 + 0xA5) & 0xFF) as u8);
        }
        assert!(std::str::from_utf8(&png_bytes).is_err());

        let font_bytes: Vec<u8> = vec![b'w', b'O', b'F', b'2', 0x00, 0xFF, 0xFE, 0x81, 0x99, 0x00];
        std::fs::write(tmp.join("logo.png"), &png_bytes).unwrap();
        std::fs::write(tmp.join("font.woff2"), &font_bytes).unwrap();
        std::fs::write(tmp.join("blocked.png"), b"\x89PNGblocked").unwrap();

        let dir_str = tmp.to_string_lossy().replace('\\', "/");
        let src = format!(
            r#"
            import * as http from "cdt.http"

            func tag_static(req: http.Request, resp: http.Response) -> http.Response {{
                if req.path == "/assets/blocked.png" {{
                    return http.response(403, "Forbidden")
                }}
                hdrs := resp.headers
                hdrs.set("x-static-hook", "yes")
                return http.response_with(resp.status, resp.body, hdrs)
            }}

            router StaticApi {{
                cors "*"
                static "/assets" => "{dir_str}"
                after tag_static
            }}

            export func setup_static_router() -> http.Router {{
                r := http.build<StaticApi>()
                r.workers(2)
                return r
            }}
        "#
        );

        let module = crate::frontend::compile_source(&src).expect("compile static router");
        let setup_fid = module.func_id("setup_static_router").unwrap();
        let mut init_vm = Vm::new();
        let router_obj = init_vm.call_obj(&module, setup_fid, &[], &[]).unwrap();
        let Obj::Record(sd) = router_obj else {
            panic!("expected Router");
        };
        let rid = sd.get_scalar(0) as usize;
        let stop_flag = Arc::clone(&init_vm.host.routers[rid].stop_flag);
        let routers = init_vm.host.routers.clone();
        let spawn_tracker = Arc::clone(&init_vm.host.spawn_tracker);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared_mod = Arc::new(module);

        let srv_stop = Arc::clone(&stop_flag);
        let srv_thread = std::thread::spawn(move || {
            serve_http_listener(
                shared_mod,
                routers,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                spawn_tracker,
                None,
                rid,
                listener,
                srv_stop,
                false,
            );
        });

        // 1 & 2. Keep-alive HEAD followed by >128 KiB binary GET on the same socket.
        {
            let mut conn = TcpStream::connect(addr).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            conn.write_all(b"HEAD /assets/logo.png HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .unwrap();

            let mut head_buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let head_end = loop {
                let n = conn.read(&mut chunk).unwrap();
                assert!(n > 0, "unexpected EOF waiting for HEAD headers");
                head_buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = crate::native::http::find_crlf_crlf(&head_buf) {
                    break pos;
                }
            };
            assert_eq!(
                head_buf.len(),
                head_end,
                "HEAD response must not include body bytes"
            );
            let head_lower = String::from_utf8_lossy(&head_buf[..head_end]).to_ascii_lowercase();
            assert!(head_lower.starts_with("http/1.1 200 ok\r\n"), "{head_lower}");
            assert!(head_lower.contains("content-type: image/png\r\n"), "{head_lower}");
            assert!(
                head_lower.contains(&format!("content-length: {}\r\n", png_bytes.len())),
                "{head_lower}"
            );
            assert!(head_lower.contains("x-static-hook: yes\r\n"), "{head_lower}");
            assert!(
                head_lower.contains("access-control-allow-origin: *\r\n"),
                "{head_lower}"
            );

            conn.write_all(
                b"GET /assets/logo.png HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let mut raw_get = Vec::new();
            conn.read_to_end(&mut raw_get).unwrap();
            let split = crate::native::http::find_crlf_crlf(&raw_get).unwrap();
            let get_head_lower = String::from_utf8_lossy(&raw_get[..split]).to_ascii_lowercase();
            assert!(get_head_lower.starts_with("http/1.1 200 ok\r\n"), "{get_head_lower}");
            assert!(
                get_head_lower.contains(&format!("content-length: {}\r\n", png_bytes.len())),
                "{get_head_lower}"
            );
            assert!(get_head_lower.contains("x-static-hook: yes\r\n"), "{get_head_lower}");
            assert!(
                get_head_lower.contains("access-control-allow-origin: *\r\n"),
                "{get_head_lower}"
            );
            assert_eq!(&raw_get[split..], png_bytes.as_slice());
        }

        // 3. Binary woff2 font file preserves MIME type and exact bytes.
        {
            let mut conn = TcpStream::connect(addr).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            conn.write_all(
                b"GET /assets/font.woff2 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let mut raw = Vec::new();
            conn.read_to_end(&mut raw).unwrap();
            let split = crate::native::http::find_crlf_crlf(&raw).unwrap();
            let head_lower = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
            assert!(head_lower.starts_with("http/1.1 200 ok\r\n"), "{head_lower}");
            assert!(head_lower.contains("content-type: font/woff2\r\n"), "{head_lower}");
            assert_eq!(&raw[split..], font_bytes.as_slice());
        }

        // 4. `after` hook can override static response status/body cleanly.
        {
            let mut conn = TcpStream::connect(addr).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            conn.write_all(
                b"GET /assets/blocked.png HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let mut raw = Vec::new();
            conn.read_to_end(&mut raw).unwrap();
            let text = String::from_utf8(raw).unwrap();
            assert!(text.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{text}");
            assert!(text.ends_with("\r\n\r\nForbidden"), "{text}");
        }

        stop_flag.store(true, Ordering::Release);
        wake_socket_addr(addr);
        srv_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
