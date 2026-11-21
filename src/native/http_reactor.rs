//! Zero-dependency non-blocking HTTP/1.1 reactor (`kqueue` on macOS/iOS, `epoll` on Linux).
//! Keeps idle keep-alive sockets and slow/partial header/body reads off the VM worker pool.

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
mod imp {
    use std::collections::HashMap;
    use std::io::{self, ErrorKind, Read, Write};
    use std::net::{IpAddr, TcpListener, TcpStream};
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use crate::native::http::{
        BODY_INITIAL_CAP, MAX_HEADER_BYTES, ParsedHttpHead, RawHttpRequest, ReadHttpError,
        find_crlf_crlf, format_error_response_bytes, has_bare_lf, parse_http_head,
    };
    use crate::vm::value::FxBuildHasher;

    const TOKEN_LISTENER: u64 = 0;
    const TOKEN_WAKE: u64 = 1;
    const FIRST_CLIENT_TOKEN: u64 = 2;
    const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
    const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
    const READ_CHUNK_SIZE: usize = 16 * 1024;

    pub(crate) struct ReactorJob {
        pub conn_id: u64,
        pub raw: RawHttpRequest,
        pub head_only: bool,
        pub keep_alive: bool,
    }

    pub(crate) struct ReactorResponse {
        pub conn_id: u64,
        pub bytes: Vec<u8>,
        pub keep_alive: bool,
    }

    pub(crate) struct ReactorConfig {
        pub idle_timeout: Duration,
        pub header_timeout: Duration,
        pub body_timeout: Duration,
        pub max_conns: usize,
        pub max_keepalive_requests: usize,
    }

    #[derive(Clone, Copy)]
    struct PollEvent {
        token: u64,
        readable: bool,
        writable: bool,
        error_or_eof: bool,
    }

    // ── Platform Poller & Pipe Waker ─────────────────────────────────────────

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    mod sys_poller {
        use super::PollEvent;
        use core::ffi::c_void;
        use std::io::{self, ErrorKind};
        use std::os::unix::io::RawFd;

        const EVFILT_READ: i16 = -1;
        const EVFILT_WRITE: i16 = -2;
        const EV_ADD: u16 = 0x0001;
        const EV_DELETE: u16 = 0x0002;
        const EV_ENABLE: u16 = 0x0004;
        const EV_ERROR: u16 = 0x4000;
        const EV_EOF: u16 = 0x8000;
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        const F_GETFL: i32 = 3;
        const F_SETFL: i32 = 4;
        const O_NONBLOCK: i32 = 0x0004;
        const ENOENT: i32 = 2;
        const EBADF: i32 = 9;

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Kevent {
            ident: usize,
            filter: i16,
            flags: u16,
            fflags: u32,
            data: isize,
            udata: *mut c_void,
        }

        const _: () = assert!(std::mem::size_of::<Kevent>() == 32);

        #[repr(C)]
        struct Timespec {
            tv_sec: isize,
            tv_nsec: isize,
        }

        unsafe extern "C" {
            fn kqueue() -> i32;
            fn kevent(
                kq: i32,
                changelist: *const Kevent,
                nchanges: i32,
                eventlist: *mut Kevent,
                nevents: i32,
                timeout: *const Timespec,
            ) -> i32;
            fn pipe(fds: *mut i32) -> i32;
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
            fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
            fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
            fn close(fd: i32) -> i32;
        }

        impl Kevent {
            const fn zeroed() -> Self {
                Self {
                    ident: 0,
                    filter: 0,
                    flags: 0,
                    fflags: 0,
                    data: 0,
                    udata: core::ptr::null_mut(),
                }
            }
        }

        pub(super) struct Poller {
            kq: RawFd,
        }

        impl Drop for Poller {
            fn drop(&mut self) {
                unsafe {
                    close(self.kq);
                }
            }
        }

        fn set_cloexec_nonblock(fd: RawFd) -> io::Result<()> {
            unsafe {
                if fcntl(fd, F_SETFD, FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let fl = fcntl(fd, F_GETFL, 0);
                if fl < 0 {
                    return Err(io::Error::last_os_error());
                }
                if fcntl(fd, F_SETFL, fl | O_NONBLOCK) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }

        pub(super) fn create_wake_pipe() -> io::Result<(RawFd, RawFd)> {
            let mut fds = [0i32; 2];
            if unsafe { pipe(fds.as_mut_ptr()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            if let Err(e) = set_cloexec_nonblock(fds[0]).and_then(|()| set_cloexec_nonblock(fds[1]))
            {
                unsafe {
                    close(fds[0]);
                    close(fds[1]);
                }
                return Err(e);
            }
            Ok((fds[0], fds[1]))
        }

        pub(super) fn close_fd(fd: RawFd) {
            unsafe {
                close(fd);
            }
        }

        pub(super) fn write_one_byte(fd: RawFd) {
            let b = 1u8;
            unsafe {
                let _ = write(fd, (&b as *const u8).cast::<c_void>(), 1);
            }
        }

        pub(super) fn drain_pipe(fd: RawFd) {
            let mut buf = [0u8; 64];
            loop {
                let rc = unsafe { read(fd, buf.as_mut_ptr().cast::<c_void>(), buf.len()) };
                if rc > 0 {
                    continue;
                }
                if rc == 0 {
                    break;
                }
                let err = io::Error::last_os_error();
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        }

        impl Poller {
            pub(super) fn new() -> io::Result<Self> {
                let kq = unsafe { kqueue() };
                if kq < 0 {
                    return Err(io::Error::last_os_error());
                }
                if unsafe { fcntl(kq, F_SETFD, FD_CLOEXEC) } < 0 {
                    let err = io::Error::last_os_error();
                    unsafe {
                        close(kq);
                    }
                    return Err(err);
                }
                Ok(Self { kq })
            }

            fn change(&self, ev: Kevent, ignore_missing: bool) -> io::Result<()> {
                let rc = unsafe {
                    kevent(
                        self.kq,
                        &ev as *const Kevent,
                        1,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null(),
                    )
                };
                if rc < 0 {
                    let err = io::Error::last_os_error();
                    if ignore_missing && matches!(err.raw_os_error(), Some(ENOENT | EBADF)) {
                        return Ok(());
                    }
                    return Err(err);
                }
                Ok(())
            }

            pub(super) fn register_read(&self, fd: RawFd, token: u64) -> io::Result<()> {
                self.change(
                    Kevent {
                        ident: fd as usize,
                        filter: EVFILT_READ,
                        flags: EV_ADD | EV_ENABLE,
                        fflags: 0,
                        data: 0,
                        udata: (token as usize) as *mut c_void,
                    },
                    false,
                )
            }

            pub(super) fn set_interest(
                &self,
                fd: RawFd,
                token: u64,
                cur_read: &mut bool,
                cur_write: &mut bool,
                want_read: bool,
                want_write: bool,
            ) -> io::Result<()> {
                if *cur_read != want_read {
                    let flags = if want_read {
                        EV_ADD | EV_ENABLE
                    } else {
                        EV_DELETE
                    };
                    self.change(
                        Kevent {
                            ident: fd as usize,
                            filter: EVFILT_READ,
                            flags,
                            fflags: 0,
                            data: 0,
                            udata: (token as usize) as *mut c_void,
                        },
                        !want_read,
                    )?;
                    *cur_read = want_read;
                }
                if *cur_write != want_write {
                    let flags = if want_write {
                        EV_ADD | EV_ENABLE
                    } else {
                        EV_DELETE
                    };
                    self.change(
                        Kevent {
                            ident: fd as usize,
                            filter: EVFILT_WRITE,
                            flags,
                            fflags: 0,
                            data: 0,
                            udata: (token as usize) as *mut c_void,
                        },
                        !want_write,
                    )?;
                    *cur_write = want_write;
                }
                Ok(())
            }

            pub(super) fn deregister(
                &self,
                fd: RawFd,
                armed_read: bool,
                armed_write: bool,
            ) {
                if armed_read {
                    let _ = self.change(
                        Kevent {
                            ident: fd as usize,
                            filter: EVFILT_READ,
                            flags: EV_DELETE,
                            fflags: 0,
                            data: 0,
                            udata: core::ptr::null_mut(),
                        },
                        true,
                    );
                }
                if armed_write {
                    let _ = self.change(
                        Kevent {
                            ident: fd as usize,
                            filter: EVFILT_WRITE,
                            flags: EV_DELETE,
                            fflags: 0,
                            data: 0,
                            udata: core::ptr::null_mut(),
                        },
                        true,
                    );
                }
            }

            pub(super) fn wait(
                &self,
                out: &mut [PollEvent],
                timeout_ms: Option<u64>,
            ) -> io::Result<usize> {
                let mut raw = [Kevent::zeroed(); 256];
                let max = out.len().min(raw.len());
                let ts_storage;
                let ts_ptr = match timeout_ms {
                    Some(ms) => {
                        ts_storage = Timespec {
                            tv_sec: (ms / 1000) as isize,
                            tv_nsec: ((ms % 1000) * 1_000_000) as isize,
                        };
                        &ts_storage as *const Timespec
                    }
                    None => core::ptr::null(),
                };
                loop {
                    let n = unsafe {
                        kevent(
                            self.kq,
                            core::ptr::null(),
                            0,
                            raw.as_mut_ptr(),
                            max as i32,
                            ts_ptr,
                        )
                    };
                    if n < 0 {
                        let err = io::Error::last_os_error();
                        if err.kind() == ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(err);
                    }
                    let count = n as usize;
                    for i in 0..count {
                        let ev = raw[i];
                        out[i] = PollEvent {
                            token: (ev.udata as usize) as u64,
                            readable: ev.filter == EVFILT_READ,
                            writable: ev.filter == EVFILT_WRITE,
                            error_or_eof: (ev.flags & (EV_EOF | EV_ERROR)) != 0,
                        };
                    }
                    return Ok(count);
                }
            }
        }
    }

    #[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
    mod sys_poller {
        use super::PollEvent;
        use core::ffi::c_void;
        use std::io::{self, ErrorKind};
        use std::os::unix::io::RawFd;

        const EPOLL_CLOEXEC: i32 = 0x80000;
        const EPOLL_CTL_ADD: i32 = 1;
        const EPOLL_CTL_DEL: i32 = 2;
        const EPOLL_CTL_MOD: i32 = 3;
        const EPOLLIN: u32 = 0x001;
        const EPOLLOUT: u32 = 0x004;
        const EPOLLERR: u32 = 0x008;
        const EPOLLHUP: u32 = 0x010;
        const EPOLLRDHUP: u32 = 0x2000;
        const O_NONBLOCK: i32 = 0x800;
        const O_CLOEXEC: i32 = 0x80000;
        const ENOENT: i32 = 2;
        const EBADF: i32 = 9;

        #[cfg_attr(target_arch = "x86_64", repr(C, packed))]
        #[cfg_attr(not(target_arch = "x86_64"), repr(C))]
        #[derive(Clone, Copy)]
        struct EpollEvent {
            events: u32,
            data: u64,
        }

        const _: () = assert!(
            std::mem::size_of::<EpollEvent>() == if cfg!(target_arch = "x86_64") { 12 } else { 16 }
        );

        unsafe extern "C" {
            fn epoll_create1(flags: i32) -> i32;
            fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;
            fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
            fn pipe2(fds: *mut i32, flags: i32) -> i32;
            fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
            fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
            fn close(fd: i32) -> i32;
        }

        pub(super) struct Poller {
            epfd: RawFd,
        }

        impl Drop for Poller {
            fn drop(&mut self) {
                unsafe {
                    close(self.epfd);
                }
            }
        }

        pub(super) fn create_wake_pipe() -> io::Result<(RawFd, RawFd)> {
            let mut fds = [0i32; 2];
            if unsafe { pipe2(fds.as_mut_ptr(), O_NONBLOCK | O_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((fds[0], fds[1]))
        }

        pub(super) fn close_fd(fd: RawFd) {
            unsafe {
                close(fd);
            }
        }

        pub(super) fn write_one_byte(fd: RawFd) {
            let b = 1u8;
            unsafe {
                let _ = write(fd, (&b as *const u8).cast::<c_void>(), 1);
            }
        }

        pub(super) fn drain_pipe(fd: RawFd) {
            let mut buf = [0u8; 64];
            loop {
                let rc = unsafe { read(fd, buf.as_mut_ptr().cast::<c_void>(), buf.len()) };
                if rc > 0 {
                    continue;
                }
                if rc == 0 {
                    break;
                }
                let err = io::Error::last_os_error();
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        }

        impl Poller {
            pub(super) fn new() -> io::Result<Self> {
                let epfd = unsafe { epoll_create1(EPOLL_CLOEXEC) };
                if epfd < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self { epfd })
            }

            pub(super) fn register_read(&self, fd: RawFd, token: u64) -> io::Result<()> {
                let mut ev = EpollEvent {
                    events: EPOLLIN | EPOLLRDHUP,
                    data: token,
                };
                if unsafe { epoll_ctl(self.epfd, EPOLL_CTL_ADD, fd, &mut ev as *mut EpollEvent) }
                    < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }

            pub(super) fn set_interest(
                &self,
                fd: RawFd,
                token: u64,
                cur_read: &mut bool,
                cur_write: &mut bool,
                want_read: bool,
                want_write: bool,
            ) -> io::Result<()> {
                if *cur_read == want_read && *cur_write == want_write {
                    return Ok(());
                }
                let mut mask = EPOLLRDHUP;
                if want_read {
                    mask |= EPOLLIN;
                }
                if want_write {
                    mask |= EPOLLOUT;
                }
                let mut ev = EpollEvent {
                    events: mask,
                    data: token,
                };
                if unsafe { epoll_ctl(self.epfd, EPOLL_CTL_MOD, fd, &mut ev as *mut EpollEvent) }
                    < 0
                {
                    return Err(io::Error::last_os_error());
                }
                *cur_read = want_read;
                *cur_write = want_write;
                Ok(())
            }

            pub(super) fn deregister(&self, fd: RawFd, _armed_read: bool, _armed_write: bool) {
                let rc = unsafe {
                    epoll_ctl(self.epfd, EPOLL_CTL_DEL, fd, core::ptr::null_mut())
                };
                if rc < 0 {
                    let err = io::Error::last_os_error();
                    let _ = matches!(err.raw_os_error(), Some(ENOENT | EBADF));
                }
            }

            pub(super) fn wait(
                &self,
                out: &mut [PollEvent],
                timeout_ms: Option<u64>,
            ) -> io::Result<usize> {
                let mut raw = [EpollEvent { events: 0, data: 0 }; 256];
                let max = out.len().min(raw.len());
                let timeout_arg = match timeout_ms {
                    Some(ms) => ms.min(i32::MAX as u64) as i32,
                    None => -1,
                };
                loop {
                    let n = unsafe { epoll_wait(self.epfd, raw.as_mut_ptr(), max as i32, timeout_arg) };
                    if n < 0 {
                        let err = io::Error::last_os_error();
                        if err.kind() == ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(err);
                    }
                    let count = n as usize;
                    for i in 0..count {
                        let events = raw[i].events;
                        let token = raw[i].data;
                        out[i] = PollEvent {
                            token,
                            readable: (events & EPOLLIN) != 0,
                            writable: (events & EPOLLOUT) != 0,
                            error_or_eof: (events & (EPOLLERR | EPOLLHUP | EPOLLRDHUP)) != 0,
                        };
                    }
                    return Ok(count);
                }
            }
        }
    }

    use sys_poller::{Poller, close_fd, create_wake_pipe, drain_pipe, write_one_byte};

    pub(crate) struct OwnedFd(RawFd);

    impl Drop for OwnedFd {
        fn drop(&mut self) {
            close_fd(self.0);
        }
    }

    #[derive(Clone)]
    pub(crate) struct ReactorWaker {
        write_fd: Arc<OwnedFd>,
        woken: Arc<AtomicBool>,
    }

    pub(crate) struct ReactorWakePair {
        read_fd: OwnedFd,
        waker: ReactorWaker,
    }

    impl ReactorWakePair {
        pub(crate) fn new() -> io::Result<Self> {
            let (wake_r, wake_w) = create_wake_pipe()?;
            Ok(Self {
                read_fd: OwnedFd(wake_r),
                waker: ReactorWaker {
                    write_fd: Arc::new(OwnedFd(wake_w)),
                    woken: Arc::new(AtomicBool::new(false)),
                },
            })
        }

        pub(crate) fn waker(&self) -> ReactorWaker {
            self.waker.clone()
        }
    }

    impl ReactorWaker {
        pub(crate) fn wake(&self) {
            if !self.woken.swap(true, Ordering::SeqCst) {
                write_one_byte(self.write_fd.0);
            }
        }
    }

    // ── Connection State Machine ─────────────────────────────────────────────

    enum ConnPhase {
        ReadingHead {
            buf: Vec<u8>,
            idle_until: Option<Instant>,
            deadline: Instant,
        },
        ReadingBody {
            head: ParsedHttpHead,
            body: Vec<u8>,
            remaining: usize,
            deadline: Instant,
        },
        Dispatching,
        Writing {
            buf: Vec<u8>,
            written: usize,
            keep_alive: bool,
            deadline: Instant,
        },
    }

    struct ReactorConn<P> {
        stream: TcpStream,
        fd: RawFd,
        _permit: P,
        req_count: usize,
        pending_read: Vec<u8>,
        armed_read: bool,
        armed_write: bool,
        phase: ConnPhase,
    }

    struct Reactor<P> {
        poller: Poller,
        conns: HashMap<u64, ReactorConn<P>, FxBuildHasher>,
        cfg: ReactorConfig,
        next_token: u64,
        in_flight: usize,
    }

    impl<P> Reactor<P> {
        fn close_conn(&mut self, token: u64) {
            if let Some(conn) = self.conns.remove(&token) {
                self.poller
                    .deregister(conn.fd, conn.armed_read, conn.armed_write);
            }
        }

        fn set_interest(&mut self, token: u64, want_read: bool, want_write: bool) -> bool {
            let Some(conn) = self.conns.get_mut(&token) else {
                return false;
            };
            if self
                .poller
                .set_interest(
                    conn.fd,
                    token,
                    &mut conn.armed_read,
                    &mut conn.armed_write,
                    want_read,
                    want_write,
                )
                .is_err()
            {
                self.close_conn(token);
                return false;
            }
            true
        }

        fn start_writing(
            &mut self,
            token: u64,
            bytes: Vec<u8>,
            keep_alive: bool,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let Some(conn) = self.conns.get_mut(&token) else {
                return;
            };
            conn.phase = ConnPhase::Writing {
                buf: bytes,
                written: 0,
                keep_alive,
                deadline: now + WRITE_TIMEOUT,
            };
            self.drive_write(token, now, stop_flag, job_tx);
        }

        fn queue_http_error(
            &mut self,
            token: u64,
            status: i64,
            body: &str,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let bytes = format_error_response_bytes(status, body);
            self.start_writing(token, bytes, false, now, stop_flag, job_tx);
        }

        fn dispatch_request(
            &mut self,
            token: u64,
            head: ParsedHttpHead,
            body: Vec<u8>,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let Some(conn) = self.conns.get_mut(&token) else {
                return;
            };
            conn.req_count += 1;
            let head_only = head.method.eq_ignore_ascii_case("HEAD");
            let keep_alive = head.keep_alive && conn.req_count < self.cfg.max_keepalive_requests;
            let raw = RawHttpRequest {
                method: head.method,
                path: head.path,
                query: head.query,
                headers: head.headers,
                body,
                keep_alive,
            };
            if !self.set_interest(token, false, false) {
                return;
            }
            if let Some(conn) = self.conns.get_mut(&token) {
                conn.phase = ConnPhase::Dispatching;
            }
            match job_tx.try_send(ReactorJob {
                conn_id: token,
                raw,
                head_only,
                keep_alive,
            }) {
                Ok(()) => {
                    self.in_flight += 1;
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    self.queue_http_error(
                        token,
                        503,
                        "Service Unavailable",
                        now,
                        stop_flag,
                        job_tx,
                    );
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.close_conn(token);
                }
            }
        }

        fn feed_bytes(
            &mut self,
            token: u64,
            chunk: &[u8],
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            enum Step {
                Continue,
                Close,
                Error(i64, String),
                Dispatch(ParsedHttpHead, Vec<u8>),
            }

            let mut slice = chunk;
            while !slice.is_empty() {
                let step = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    match &mut conn.phase {
                        ConnPhase::ReadingHead {
                            buf,
                            idle_until,
                            deadline,
                        } => {
                            if idle_until.take().is_some() {
                                *deadline = now + self.cfg.header_timeout;
                            }
                            let prev_len = buf.len();
                            let prev_byte = buf.last().copied();
                            let cap_left = MAX_HEADER_BYTES.saturating_sub(prev_len);
                            if cap_left == 0 {
                                Step::Error(413, "Payload Too Large".to_string())
                            } else {
                                let take = slice.len().min(cap_left);
                                buf.extend_from_slice(&slice[..take]);
                                let rest = &slice[take..];

                                let search_from = prev_len.saturating_sub(3);
                                if let Some(rel_end) = find_crlf_crlf(&buf[search_from..]) {
                                    let head_end = search_from + rel_end;
                                    if has_bare_lf(&buf[prev_len..head_end], prev_byte) {
                                        Step::Error(
                                            400,
                                            "Bare LF in HTTP request".to_string(),
                                        )
                                    } else {
                                        match parse_http_head(&buf[..head_end - 4]) {
                                            Ok(head) => {
                                                let mut extra = buf.split_off(head_end);
                                                if !rest.is_empty() {
                                                    extra.extend_from_slice(rest);
                                                }
                                                slice = &[];
                                                if head.content_length == 0 {
                                                    if !extra.is_empty() {
                                                        conn.pending_read.extend_from_slice(&extra);
                                                    }
                                                    Step::Dispatch(head, Vec::new())
                                                } else {
                                                    let mut body = Vec::with_capacity(
                                                        head.content_length.min(BODY_INITIAL_CAP),
                                                    );
                                                    let body_take =
                                                        extra.len().min(head.content_length);
                                                    body.extend_from_slice(&extra[..body_take]);
                                                    if extra.len() > body_take {
                                                        conn.pending_read
                                                            .extend_from_slice(&extra[body_take..]);
                                                    }
                                                    let remaining =
                                                        head.content_length - body.len();
                                                    if remaining == 0 {
                                                        Step::Dispatch(head, body)
                                                    } else {
                                                        conn.phase = ConnPhase::ReadingBody {
                                                            head,
                                                            body,
                                                            remaining,
                                                            deadline: now + self.cfg.body_timeout,
                                                        };
                                                        Step::Continue
                                                    }
                                                }
                                            }
                                            Err(ReadHttpError::TooLarge) => Step::Error(
                                                413,
                                                "Payload Too Large".to_string(),
                                            ),
                                            Err(ReadHttpError::Bad(msg)) => Step::Error(400, msg),
                                            Err(_) => Step::Close,
                                        }
                                    }
                                } else if has_bare_lf(&buf[prev_len..], prev_byte) {
                                    Step::Error(400, "Bare LF in HTTP request".to_string())
                                } else if buf.len() >= MAX_HEADER_BYTES {
                                    Step::Error(413, "Payload Too Large".to_string())
                                } else {
                                    slice = rest;
                                    Step::Continue
                                }
                            }
                        }
                        ConnPhase::ReadingBody {
                            head: _,
                            body,
                            remaining,
                            deadline: _,
                        } => {
                            let take = slice.len().min(*remaining);
                            body.extend_from_slice(&slice[..take]);
                            *remaining -= take;
                            let rest = &slice[take..];
                            slice = rest;
                            if !rest.is_empty() {
                                conn.pending_read.extend_from_slice(rest);
                                slice = &[];
                            }
                            if *remaining == 0 {
                                let old = std::mem::replace(&mut conn.phase, ConnPhase::Dispatching);
                                match old {
                                    ConnPhase::ReadingBody { head, body, .. } => {
                                        Step::Dispatch(head, body)
                                    }
                                    _ => unreachable!(),
                                }
                            } else {
                                Step::Continue
                            }
                        }
                        _ => break,
                    }
                };

                match step {
                    Step::Continue => {}
                    Step::Close => {
                        self.close_conn(token);
                        return;
                    }
                    Step::Error(status, msg) => {
                        self.queue_http_error(token, status, &msg, now, stop_flag, job_tx);
                        return;
                    }
                    Step::Dispatch(head, body) => {
                        self.dispatch_request(token, head, body, now, stop_flag, job_tx);
                        return;
                    }
                }
            }
        }

        fn drive_read(
            &mut self,
            token: u64,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let pending_bytes = {
                let Some(conn) = self.conns.get_mut(&token) else {
                    return;
                };
                if !matches!(
                    conn.phase,
                    ConnPhase::ReadingHead { .. } | ConnPhase::ReadingBody { .. }
                ) || conn.pending_read.is_empty()
                {
                    None
                } else {
                    Some(std::mem::take(&mut conn.pending_read))
                }
            };
            if let Some(pending) = pending_bytes {
                self.feed_bytes(token, &pending, now, stop_flag, job_tx);
            }

            let mut buf = [0u8; READ_CHUNK_SIZE];
            loop {
                let read_len = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    match &conn.phase {
                        ConnPhase::ReadingHead { .. } => buf.len(),
                        ConnPhase::ReadingBody { remaining, .. } => (*remaining).min(buf.len()),
                        _ => return,
                    }
                };
                if read_len == 0 {
                    return;
                }
                let res = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    conn.stream.read(&mut buf[..read_len])
                };
                match res {
                    Ok(0) => {
                        self.close_conn(token);
                        return;
                    }
                    Ok(n) => {
                        self.feed_bytes(token, &buf[..n], now, stop_flag, job_tx);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        let _ = self.set_interest(token, true, false);
                        return;
                    }
                    Err(_) => {
                        self.close_conn(token);
                        return;
                    }
                }
            }
        }

        fn drive_write(
            &mut self,
            token: u64,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            loop {
                let res = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    let ConnPhase::Writing { buf, written, .. } = &mut conn.phase else {
                        return;
                    };
                    if *written >= buf.len() {
                        Ok(0usize)
                    } else {
                        conn.stream.write(&buf[*written..])
                    }
                };

                match res {
                    Ok(n) => {
                        let done = {
                            let Some(conn) = self.conns.get_mut(&token) else {
                                return;
                            };
                            let ConnPhase::Writing {
                                buf,
                                written,
                                keep_alive,
                                ..
                            } = &mut conn.phase
                            else {
                                return;
                            };
                            if n == 0 && *written < buf.len() {
                                None
                            } else {
                                *written += n;
                                if *written >= buf.len() {
                                    Some(*keep_alive)
                                } else {
                                    continue;
                                }
                            }
                        };
                        match done {
                            None => {
                                self.close_conn(token);
                                return;
                            }
                            Some(keep_alive) => {
                                let can_keep_alive = {
                                    let Some(conn) = self.conns.get(&token) else {
                                        return;
                                    };
                                    keep_alive
                                        && !stop_flag.load(Ordering::Acquire)
                                        && conn.req_count < self.cfg.max_keepalive_requests
                                };
                                if can_keep_alive {
                                    if let Some(conn) = self.conns.get_mut(&token) {
                                        let has_pending = !conn.pending_read.is_empty();
                                        conn.phase = ConnPhase::ReadingHead {
                                            buf: Vec::new(),
                                            idle_until: (!has_pending)
                                                .then(|| now + self.cfg.idle_timeout),
                                            deadline: if has_pending {
                                                now + self.cfg.header_timeout
                                            } else {
                                                now + self.cfg.idle_timeout
                                            },
                                        };
                                    }
                                    self.drive_read(token, now, stop_flag, job_tx);
                                } else {
                                    self.close_conn(token);
                                }
                                return;
                            }
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        let _ = self.set_interest(token, false, true);
                        return;
                    }
                    Err(_) => {
                        self.close_conn(token);
                        return;
                    }
                }
            }
        }

        fn sweep_timeouts(
            &mut self,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            enum TimeoutAction {
                Close,
                Send408,
            }
            let mut expired: Vec<(u64, TimeoutAction)> = Vec::new();
            for (&token, conn) in &self.conns {
                match &conn.phase {
                    ConnPhase::ReadingHead {
                        buf,
                        idle_until,
                        deadline,
                    } => {
                        if idle_until.is_some() && buf.is_empty() && now >= *deadline {
                            expired.push((token, TimeoutAction::Close));
                        } else if now >= *deadline {
                            expired.push((token, TimeoutAction::Send408));
                        }
                    }
                    ConnPhase::ReadingBody { deadline, .. } if now >= *deadline => {
                        expired.push((token, TimeoutAction::Send408));
                    }
                    ConnPhase::Writing { deadline, .. } if now >= *deadline => {
                        expired.push((token, TimeoutAction::Close));
                    }
                    _ => {}
                }
            }

            for (token, action) in expired {
                match action {
                    TimeoutAction::Close => self.close_conn(token),
                    TimeoutAction::Send408 => {
                        self.queue_http_error(
                            token,
                            408,
                            "Request Timeout",
                            now,
                            stop_flag,
                            job_tx,
                        );
                    }
                }
            }
        }

        fn next_poll_timeout_ms(
            &self,
            now: Instant,
            shutdown_deadline: Option<Instant>,
        ) -> Option<u64> {
            let mut earliest = shutdown_deadline;
            for conn in self.conns.values() {
                let dl = match &conn.phase {
                    ConnPhase::ReadingHead { deadline, .. }
                    | ConnPhase::ReadingBody { deadline, .. }
                    | ConnPhase::Writing { deadline, .. } => Some(*deadline),
                    ConnPhase::Dispatching => None,
                };
                if let Some(d) = dl {
                    earliest = Some(match earliest {
                        Some(cur) => cur.min(d),
                        None => d,
                    });
                }
            }
            let cap_ms = if shutdown_deadline.is_some() { 50 } else { 250 };
            let ms = match earliest {
                Some(dl) => (dl.saturating_duration_since(now).as_millis() as u64).min(cap_ms),
                None => cap_ms,
            };
            Some(ms.max(1))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_http_reactor<P, FAcquire, FReject>(
        listener: TcpListener,
        stop_flag: Arc<AtomicBool>,
        cfg: ReactorConfig,
        acquire_permit: FAcquire,
        reject_503: FReject,
        is_fatal_accept: fn(&io::Error) -> bool,
        job_tx: mpsc::SyncSender<ReactorJob>,
        res_rx: mpsc::Receiver<ReactorResponse>,
        wake_pair: ReactorWakePair,
    ) where
        FAcquire: Fn(IpAddr) -> Option<P>,
        FReject: Fn(TcpStream),
    {
        let Ok(poller) = Poller::new() else {
            return;
        };
        let wake_r = wake_pair.read_fd.0;
        let woken = Arc::clone(&wake_pair.waker.woken);

        if listener.set_nonblocking(true).is_err() {
            return;
        }
        let listener_fd = listener.as_raw_fd();
        if poller.register_read(listener_fd, TOKEN_LISTENER).is_err() {
            return;
        }
        if poller.register_read(wake_r, TOKEN_WAKE).is_err() {
            poller.deregister(listener_fd, true, false);
            return;
        }

        let mut reactor = Reactor {
            poller,
            conns: HashMap::with_hasher(FxBuildHasher),
            cfg,
            next_token: FIRST_CLIENT_TOKEN,
            in_flight: 0,
        };

        let mut events = [PollEvent {
            token: 0,
            readable: false,
            writable: false,
            error_or_eof: false,
        }; 256];
        let mut listener_active = true;
        let mut shutdown_deadline: Option<Instant> = None;

        loop {
            let now = Instant::now();
            if stop_flag.load(Ordering::Acquire) {
                if shutdown_deadline.is_none() {
                    shutdown_deadline = Some(now + SHUTDOWN_DRAIN_TIMEOUT);
                    if listener_active {
                        reactor.poller.deregister(listener_fd, true, false);
                        listener_active = false;
                    }
                    let idle_or_reading: Vec<u64> = reactor
                        .conns
                        .iter()
                        .filter_map(|(&tok, c)| match c.phase {
                            ConnPhase::ReadingHead { .. } | ConnPhase::ReadingBody { .. } => {
                                Some(tok)
                            }
                            ConnPhase::Dispatching | ConnPhase::Writing { .. } => None,
                        })
                        .collect();
                    for tok in idle_or_reading {
                        reactor.close_conn(tok);
                    }
                }
                if (reactor.in_flight == 0 && reactor.conns.is_empty())
                    || shutdown_deadline.is_some_and(|dl| now >= dl)
                {
                    break;
                }
            }

            let timeout_ms = reactor.next_poll_timeout_ms(now, shutdown_deadline);
            let n = match reactor.poller.wait(&mut events, timeout_ms) {
                Ok(n) => n,
                Err(_) => break,
            };

            let now = Instant::now();
            let mut fatal_accept = false;
            for ev in &events[..n] {
                match ev.token {
                    TOKEN_WAKE => {
                        drain_pipe(wake_r);
                        woken.store(false, Ordering::SeqCst);
                    }
                    TOKEN_LISTENER => {
                        loop {
                            if stop_flag.load(Ordering::Acquire) {
                                break;
                            }
                            match listener.accept() {
                                Ok((stream, peer_addr)) => {
                                    if reactor.conns.len() >= reactor.cfg.max_conns {
                                        reject_503(stream);
                                        continue;
                                    }
                                    let Some(permit) = acquire_permit(peer_addr.ip()) else {
                                        reject_503(stream);
                                        continue;
                                    };
                                    if stream.set_nonblocking(true).is_err() {
                                        continue;
                                    }
                                    let _ = stream.set_nodelay(true);
                                    let fd = stream.as_raw_fd();
                                    let token = reactor.next_token;
                                    reactor.next_token += 1;
                                    if reactor.poller.register_read(fd, token).is_err() {
                                        continue;
                                    }
                                    reactor.conns.insert(
                                        token,
                                        ReactorConn {
                                            stream,
                                            fd,
                                            _permit: permit,
                                            req_count: 0,
                                            pending_read: Vec::new(),
                                            armed_read: true,
                                            armed_write: false,
                                            phase: ConnPhase::ReadingHead {
                                                buf: Vec::new(),
                                                idle_until: None,
                                                deadline: now + reactor.cfg.header_timeout,
                                            },
                                        },
                                    );
                                }
                                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                                Err(e)
                                    if matches!(
                                        e.kind(),
                                        ErrorKind::ConnectionAborted
                                            | ErrorKind::ConnectionReset
                                            | ErrorKind::BrokenPipe
                                    ) =>
                                {
                                    continue;
                                }
                                Err(e) => {
                                    if stop_flag.load(Ordering::Acquire) || is_fatal_accept(&e) {
                                        fatal_accept = true;
                                    } else {
                                        std::thread::sleep(Duration::from_millis(5));
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    client_token => {
                        let phase_kind = reactor.conns.get(&client_token).map(|c| match c.phase {
                            ConnPhase::ReadingHead { .. } | ConnPhase::ReadingBody { .. } => 0u8,
                            ConnPhase::Writing { .. } => 1u8,
                            ConnPhase::Dispatching => 2u8,
                        });
                        match phase_kind {
                            Some(0) if ev.readable || ev.error_or_eof => {
                                reactor.drive_read(client_token, now, &stop_flag, &job_tx);
                            }
                            Some(1) if ev.writable || ev.error_or_eof => {
                                reactor.drive_write(client_token, now, &stop_flag, &job_tx);
                            }
                            Some(2) if ev.error_or_eof => {}
                            _ => {}
                        }
                    }
                }
            }

            if fatal_accept {
                break;
            }

            // Drain any worker responses unconditionally after draining the wake flag.
            while let Ok(resp) = res_rx.try_recv() {
                reactor.in_flight = reactor.in_flight.saturating_sub(1);
                if reactor.conns.contains_key(&resp.conn_id) {
                    reactor.start_writing(
                        resp.conn_id,
                        resp.bytes,
                        resp.keep_alive,
                        now,
                        &stop_flag,
                        &job_tx,
                    );
                }
            }

            reactor.sweep_timeouts(now, &stop_flag, &job_tx);
        }

        if listener_active {
            reactor.poller.deregister(listener_fd, true, false);
        }
        let leftover: Vec<u64> = reactor.conns.keys().copied().collect();
        for tok in leftover {
            reactor.close_conn(tok);
        }
        reactor.poller.deregister(wake_r, true, false);
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
pub(crate) use imp::{
    ReactorConfig, ReactorJob, ReactorResponse, ReactorWakePair, run_http_reactor,
};
