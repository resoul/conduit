//! Zero-dependency non-blocking HTTP/1.1 reactor (`kqueue` on macOS/iOS, `epoll` on Linux).
//! Keeps idle keep-alive sockets and slow/partial header/body reads off the VM worker pool.

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
mod imp {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};
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
    const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
    const READ_CHUNK_SIZE: usize = 16 * 1024;
    const FILE_WRITE_CHUNK_SIZE: usize = 64 * 1024;

    pub(crate) struct ReactorJob {
        pub conn_id: u64,
        pub raw: RawHttpRequest,
        pub head_only: bool,
        pub keep_alive: bool,
    }

    pub(crate) struct ReactorFileBody {
        pub file: std::fs::File,
        pub remaining: u64,
    }

    pub(crate) struct ReactorResponse {
        pub conn_id: u64,
        pub bytes: Vec<u8>,
        pub file_body: Option<ReactorFileBody>,
        pub keep_alive: bool,
    }

    pub(crate) struct ReactorConfig {
        pub idle_timeout: Duration,
        pub header_timeout: Duration,
        pub body_timeout: Duration,
        pub write_timeout: Duration,
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

    // ── Connection State Machine & Deadline Queue ───────────────────────────

    #[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
    struct DeadlineEntry {
        deadline: Instant,
        token: u64,
        generation: u64,
    }

    struct DeadlineQueue {
        heap: BinaryHeap<Reverse<DeadlineEntry>>,
    }

    impl DeadlineQueue {
        fn new() -> Self {
            Self {
                heap: BinaryHeap::new(),
            }
        }

        fn push(&mut self, token: u64, generation: u64, deadline: Instant) {
            self.heap.push(Reverse(DeadlineEntry {
                deadline,
                token,
                generation,
            }));
        }

        fn clear(&mut self) {
            self.heap.clear();
        }

        #[cfg(test)]
        fn len(&self) -> usize {
            self.heap.len()
        }

        fn maybe_compact(
            &mut self,
            live_conns: usize,
            rebuild: impl FnOnce() -> Vec<Reverse<DeadlineEntry>>,
        ) {
            if self.heap.len() > live_conns.saturating_mul(4).max(256) {
                self.heap = BinaryHeap::from(rebuild());
            }
        }

        fn peek_earliest(
            &mut self,
            is_live: impl Fn(u64, u64, Instant) -> bool,
        ) -> Option<Instant> {
            while let Some(&Reverse(entry)) = self.heap.peek() {
                if is_live(entry.token, entry.generation, entry.deadline) {
                    return Some(entry.deadline);
                }
                self.heap.pop();
            }
            None
        }

        fn pop_next_expired(
            &mut self,
            now: Instant,
            is_live: impl Fn(u64, u64, Instant) -> bool,
        ) -> Option<u64> {
            while let Some(&Reverse(entry)) = self.heap.peek() {
                if entry.deadline > now {
                    return None;
                }
                self.heap.pop();
                if is_live(entry.token, entry.generation, entry.deadline) {
                    return Some(entry.token);
                }
            }
            None
        }
    }

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
            file_body: Option<ReactorFileBody>,
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
        deadline_gen: u64,
        phase: ConnPhase,
    }

    impl<P> ReactorConn<P> {
        #[inline]
        fn current_deadline(&self) -> Option<Instant> {
            match &self.phase {
                ConnPhase::ReadingHead { deadline, .. }
                | ConnPhase::ReadingBody { deadline, .. }
                | ConnPhase::Writing { deadline, .. } => Some(*deadline),
                ConnPhase::Dispatching => None,
            }
        }
    }

    struct Reactor<P> {
        poller: Poller,
        conns: HashMap<u64, ReactorConn<P>, FxBuildHasher>,
        deadlines: DeadlineQueue,
        cfg: ReactorConfig,
        next_token: u64,
        in_flight: usize,
    }

    impl<P> Reactor<P> {
        #[inline]
        fn is_deadline_live(conn: &ReactorConn<P>, generation: u64, deadline: Instant) -> bool {
            conn.deadline_gen == generation && conn.current_deadline() == Some(deadline)
        }

        fn maybe_compact_deadlines(&mut self) {
            let conns = &self.conns;
            self.deadlines.maybe_compact(conns.len(), || {
                conns
                    .iter()
                    .filter_map(|(&token, conn)| {
                        conn.current_deadline().map(|deadline| {
                            Reverse(DeadlineEntry {
                                deadline,
                                token,
                                generation: conn.deadline_gen,
                            })
                        })
                    })
                    .collect()
            });
        }

        fn arm_deadline(&mut self, token: u64, deadline: Instant) {
            if let Some(conn) = self.conns.get_mut(&token) {
                conn.deadline_gen = conn.deadline_gen.wrapping_add(1);
                self.deadlines.push(token, conn.deadline_gen, deadline);
                self.maybe_compact_deadlines();
            }
        }

        fn clear_deadline(&mut self, token: u64) {
            if let Some(conn) = self.conns.get_mut(&token) {
                conn.deadline_gen = conn.deadline_gen.wrapping_add(1);
            }
        }

        fn close_conn(&mut self, token: u64) {
            if let Some(conn) = self.conns.remove(&token) {
                self.poller
                    .deregister(conn.fd, conn.armed_read, conn.armed_write);
                if self.conns.is_empty() {
                    self.deadlines.clear();
                } else {
                    self.maybe_compact_deadlines();
                }
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

        #[allow(clippy::too_many_arguments)]
        fn start_writing(
            &mut self,
            token: u64,
            bytes: Vec<u8>,
            file_body: Option<ReactorFileBody>,
            keep_alive: bool,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let write_dl = now + self.cfg.write_timeout;
            let Some(conn) = self.conns.get_mut(&token) else {
                return;
            };
            conn.phase = ConnPhase::Writing {
                buf: bytes,
                written: 0,
                file_body,
                keep_alive,
                deadline: write_dl,
            };
            self.arm_deadline(token, write_dl);
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
            self.start_writing(token, bytes, None, false, now, stop_flag, job_tx);
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
            self.clear_deadline(token);
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
                Continue(Option<Instant>),
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
                            let mut armed_dl = None;
                            if idle_until.take().is_some() {
                                let dl = now + self.cfg.header_timeout;
                                *deadline = dl;
                                armed_dl = Some(dl);
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
                                                        let body_dl = now + self.cfg.body_timeout;
                                                        conn.phase = ConnPhase::ReadingBody {
                                                            head,
                                                            body,
                                                            remaining,
                                                            deadline: body_dl,
                                                        };
                                                        Step::Continue(Some(body_dl))
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
                                    Step::Continue(armed_dl)
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
                                Step::Continue(None)
                            }
                        }
                        _ => break,
                    }
                };

                match step {
                    Step::Continue(new_dl) => {
                        if let Some(dl) = new_dl {
                            self.arm_deadline(token, dl);
                        }
                    }
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

        fn finish_write(
            &mut self,
            token: u64,
            keep_alive: bool,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            let can_keep_alive = {
                let Some(conn) = self.conns.get(&token) else {
                    return;
                };
                keep_alive
                    && !stop_flag.load(Ordering::Acquire)
                    && conn.req_count < self.cfg.max_keepalive_requests
            };
            if can_keep_alive {
                let next_dl = if let Some(conn) = self.conns.get_mut(&token) {
                    let has_pending = !conn.pending_read.is_empty();
                    let deadline = if has_pending {
                        now + self.cfg.header_timeout
                    } else {
                        now + self.cfg.idle_timeout
                    };
                    conn.phase = ConnPhase::ReadingHead {
                        buf: Vec::new(),
                        idle_until: (!has_pending).then(|| now + self.cfg.idle_timeout),
                        deadline,
                    };
                    Some(deadline)
                } else {
                    None
                };
                if let Some(dl) = next_dl {
                    self.arm_deadline(token, dl);
                }
                self.drive_read(token, now, stop_flag, job_tx);
            } else {
                self.close_conn(token);
            }
        }

        fn drive_write(
            &mut self,
            token: u64,
            now: Instant,
            stop_flag: &AtomicBool,
            job_tx: &mpsc::SyncSender<ReactorJob>,
        ) {
            enum ChunkState {
                Ready,
                Done(bool),
                TruncatedOrErr,
            }

            enum WriteProgress {
                Continue(Option<Instant>),
                Done(bool),
                Close,
            }

            loop {
                let chunk_state = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    let ConnPhase::Writing {
                        buf,
                        written,
                        file_body,
                        keep_alive,
                        ..
                    } = &mut conn.phase
                    else {
                        return;
                    };
                    if *written >= buf.len() {
                        if let Some(fb) = file_body.as_mut() {
                            if fb.remaining == 0 {
                                *file_body = None;
                                ChunkState::Done(*keep_alive)
                            } else {
                                let want = (fb.remaining as usize).min(FILE_WRITE_CHUNK_SIZE);
                                buf.resize(want, 0);
                                let read_res = loop {
                                    match fb.file.read(&mut buf[..want]) {
                                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                                        other => break other,
                                    }
                                };
                                match read_res {
                                    Ok(0) | Err(_) => ChunkState::TruncatedOrErr,
                                    Ok(got) => {
                                        buf.truncate(got);
                                        fb.remaining -= got as u64;
                                        *written = 0;
                                        if fb.remaining == 0 {
                                            *file_body = None;
                                        }
                                        ChunkState::Ready
                                    }
                                }
                            }
                        } else {
                            ChunkState::Done(*keep_alive)
                        }
                    } else {
                        ChunkState::Ready
                    }
                };

                match chunk_state {
                    ChunkState::Ready => {}
                    ChunkState::Done(keep_alive) => {
                        self.finish_write(token, keep_alive, now, stop_flag, job_tx);
                        return;
                    }
                    ChunkState::TruncatedOrErr => {
                        self.close_conn(token);
                        return;
                    }
                }

                let res = {
                    let Some(conn) = self.conns.get_mut(&token) else {
                        return;
                    };
                    let ConnPhase::Writing { buf, written, .. } = &mut conn.phase else {
                        return;
                    };
                    conn.stream.write(&buf[*written..])
                };

                match res {
                    Ok(n) => {
                        let progress = {
                            let Some(conn) = self.conns.get_mut(&token) else {
                                return;
                            };
                            let ConnPhase::Writing {
                                buf,
                                written,
                                file_body,
                                keep_alive,
                                deadline,
                            } = &mut conn.phase
                            else {
                                return;
                            };
                            if n == 0 && *written < buf.len() {
                                WriteProgress::Close
                            } else {
                                *written += n;
                                let has_more_file =
                                    file_body.as_ref().is_some_and(|fb| fb.remaining > 0);
                                let mut refreshed_dl = None;
                                if n > 0
                                    && has_more_file
                                    && deadline.saturating_duration_since(now)
                                        < self.cfg.write_timeout / 2
                                {
                                    let dl = now + self.cfg.write_timeout;
                                    *deadline = dl;
                                    refreshed_dl = Some(dl);
                                }
                                if *written < buf.len() || has_more_file {
                                    WriteProgress::Continue(refreshed_dl)
                                } else {
                                    *file_body = None;
                                    WriteProgress::Done(*keep_alive)
                                }
                            }
                        };
                        match progress {
                            WriteProgress::Continue(dl) => {
                                if let Some(dl) = dl {
                                    self.arm_deadline(token, dl);
                                }
                                continue;
                            }
                            WriteProgress::Close => {
                                self.close_conn(token);
                                return;
                            }
                            WriteProgress::Done(keep_alive) => {
                                self.finish_write(token, keep_alive, now, stop_flag, job_tx);
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
            loop {
                let expired_token = {
                    let conns = &self.conns;
                    self.deadlines.pop_next_expired(now, |tok, generation, dl| {
                        conns
                            .get(&tok)
                            .is_some_and(|c| Self::is_deadline_live(c, generation, dl))
                    })
                };
                let Some(token) = expired_token else {
                    break;
                };
                let close_without_408 = match self.conns.get(&token).map(|c| &c.phase) {
                    Some(ConnPhase::ReadingHead { buf, idle_until, .. }) => {
                        idle_until.is_some() && buf.is_empty()
                    }
                    Some(ConnPhase::ReadingBody { .. }) => false,
                    Some(ConnPhase::Writing { .. }) => true,
                    Some(ConnPhase::Dispatching) | None => continue,
                };
                if close_without_408 {
                    self.close_conn(token);
                } else {
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

        fn next_poll_timeout_ms(
            &mut self,
            now: Instant,
            shutdown_deadline: Option<Instant>,
        ) -> Option<u64> {
            let earliest_conn = {
                let conns = &self.conns;
                self.deadlines.peek_earliest(|tok, generation, dl| {
                    conns
                        .get(&tok)
                        .is_some_and(|c| Self::is_deadline_live(c, generation, dl))
                })
            };
            let earliest = match (shutdown_deadline, earliest_conn) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            };
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
            deadlines: DeadlineQueue::new(),
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
                                    let accept_dl = now + reactor.cfg.header_timeout;
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
                                            deadline_gen: 1,
                                            phase: ConnPhase::ReadingHead {
                                                buf: Vec::new(),
                                                idle_until: None,
                                                deadline: accept_dl,
                                            },
                                        },
                                    );
                                    reactor.deadlines.push(token, 1, accept_dl);
                                    reactor.maybe_compact_deadlines();
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
                        resp.file_body,
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn deadline_queue_ordering_invalidation_and_compaction() {
            let t0 = Instant::now();
            let mut active: HashMap<u64, (u64, Instant), FxBuildHasher> =
                HashMap::with_hasher(FxBuildHasher);
            let mut q = DeadlineQueue::new();

            for (tok, offset_ms) in [(10u64, 500u64), (11, 100), (12, 300)] {
                let dl = t0 + Duration::from_millis(offset_ms);
                active.insert(tok, (1, dl));
                q.push(tok, 1, dl);
            }

            // Update token 11 deadline later; old generation 1 must be ignored.
            let updated_11 = t0 + Duration::from_millis(450);
            active.insert(11, (2, updated_11));
            q.push(11, 2, updated_11);

            // Clear token 12 deadline (e.g., closed/dispatching).
            active.remove(&12);

            let earliest = q
                .peek_earliest(|tok, g, dl| {
                    active.get(&tok).is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
                })
                .unwrap();
            assert_eq!(earliest, updated_11);

            let expired_at_450 = q.pop_next_expired(updated_11, |tok, g, dl| {
                active.get(&tok).is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
            });
            assert_eq!(expired_at_450, Some(11));
            active.remove(&11);

            assert_eq!(
                q.pop_next_expired(t0 + Duration::from_millis(499), |tok, g, dl| {
                    active.get(&tok).is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
                }),
                None
            );
            assert_eq!(
                q.pop_next_expired(t0 + Duration::from_millis(500), |tok, g, dl| {
                    active.get(&tok).is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
                }),
                Some(10)
            );
            active.remove(&10);
            assert!(q.peek_earliest(|tok, g, dl| {
                active.get(&tok).is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
            })
            .is_none());

            // Compaction bounds stale entries after many re-arms across 64 connections.
            let mut conns: HashMap<u64, (u64, Instant), FxBuildHasher> =
                HashMap::with_hasher(FxBuildHasher);
            for tok in 0..64u64 {
                let dl = t0 + Duration::from_millis(1000 + tok);
                conns.insert(tok, (1, dl));
                q.push(tok, 1, dl);
            }
            for step in 0..10_000u64 {
                let tok = step % 64;
                let entry = conns.get_mut(&tok).unwrap();
                entry.0 += 1;
                entry.1 = t0 + Duration::from_millis(2000 + step);
                q.push(tok, entry.0, entry.1);
                q.maybe_compact(conns.len(), || {
                    conns
                        .iter()
                        .map(|(&t, &(g, dl))| {
                            Reverse(DeadlineEntry {
                                deadline: dl,
                                token: t,
                                generation: g,
                            })
                        })
                        .collect()
                });
            }
            assert!(q.len() <= conns.len() * 4, "heap len={}", q.len());
        }

        #[test]
        #[ignore]
        fn bench_reactor_deadlines_scan_vs_heap() {
            let t0 = Instant::now();
            let iters = 20_000usize;
            for &n_conns in &[1_000usize, 4_000, 8_000] {
                let mut scan_conns: HashMap<u64, (u64, Instant), FxBuildHasher> =
                    HashMap::with_hasher(FxBuildHasher);
                let mut heap_conns: HashMap<u64, (u64, Instant), FxBuildHasher> =
                    HashMap::with_hasher(FxBuildHasher);
                let mut queue = DeadlineQueue::new();
                for i in 0..n_conns as u64 {
                    let dl = t0 + Duration::from_millis(3_000 + (i % 500));
                    scan_conns.insert(i, (1, dl));
                    heap_conns.insert(i, (1, dl));
                    queue.push(i, 1, dl);
                }

                // Baseline: double O(N) scan per iteration
                let mut scan_iter_ns = Vec::with_capacity(iters);
                let scan_start = Instant::now();
                let mut sink_scan = 0u64;
                for step in 0..iters {
                    let now = t0 + Duration::from_micros((step as u64) * 50);
                    let it_start = Instant::now();
                    let active_tok = (step as u64) % (n_conns as u64);
                    if let Some(v) = scan_conns.get_mut(&active_tok) {
                        v.0 += 1;
                        v.1 = now + Duration::from_millis(3_000);
                    }
                    let mut earliest: Option<Instant> = None;
                    for &(_, dl) in scan_conns.values() {
                        earliest = Some(match earliest {
                            Some(cur) => cur.min(dl),
                            None => dl,
                        });
                    }
                    if let Some(e) = earliest {
                        sink_scan = sink_scan.wrapping_add(
                            e.saturating_duration_since(now).as_millis() as u64,
                        );
                    }
                    let mut expired = Vec::new();
                    for (&tok, &(_, dl)) in &scan_conns {
                        if now >= dl {
                            expired.push(tok);
                        }
                    }
                    for tok in expired {
                        if let Some(v) = scan_conns.get_mut(&tok) {
                            v.1 = now + Duration::from_millis(3_000);
                        }
                    }
                    scan_iter_ns.push(it_start.elapsed().as_nanos() as u64);
                }
                let scan_total = scan_start.elapsed();

                // Heap: DeadlineQueue
                let mut heap_iter_ns = Vec::with_capacity(iters);
                let heap_start = Instant::now();
                let mut sink_heap = 0u64;
                for step in 0..iters {
                    let now = t0 + Duration::from_micros((step as u64) * 50);
                    let it_start = Instant::now();
                    let active_tok = (step as u64) % (n_conns as u64);
                    if let Some(v) = heap_conns.get_mut(&active_tok) {
                        v.0 += 1;
                        v.1 = now + Duration::from_millis(3_000);
                        queue.push(active_tok, v.0, v.1);
                        let count = heap_conns.len();
                        queue.maybe_compact(count, || {
                            heap_conns
                                .iter()
                                .map(|(&t, &(g, dl))| {
                                    Reverse(DeadlineEntry {
                                        deadline: dl,
                                        token: t,
                                        generation: g,
                                    })
                                })
                                .collect()
                        });
                    }
                    if let Some(e) = queue.peek_earliest(|t, g, dl| {
                        heap_conns
                            .get(&t)
                            .is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
                    }) {
                        sink_heap = sink_heap.wrapping_add(
                            e.saturating_duration_since(now).as_millis() as u64,
                        );
                    }
                    while let Some(tok) = queue.pop_next_expired(now, |t, g, dl| {
                        heap_conns
                            .get(&t)
                            .is_some_and(|&(cg, cdl)| cg == g && cdl == dl)
                    }) {
                        if let Some(v) = heap_conns.get_mut(&tok) {
                            v.0 += 1;
                            v.1 = now + Duration::from_millis(3_000);
                            queue.push(tok, v.0, v.1);
                        }
                    }
                    heap_iter_ns.push(it_start.elapsed().as_nanos() as u64);
                }
                let heap_total = heap_start.elapsed();

                scan_iter_ns.sort_unstable();
                heap_iter_ns.sort_unstable();
                let p50 = iters / 2;
                let p99 = (iters * 99) / 100;
                eprintln!(
                    "[bench deadlines N={n_conns}] scan total={:.2}ms (p50={:.3}us p99={:.3}us) | heap total={:.2}ms (p50={:.3}us p99={:.3}us) | sink={}/{}",
                    scan_total.as_secs_f64() * 1000.0,
                    scan_iter_ns[p50] as f64 / 1000.0,
                    scan_iter_ns[p99] as f64 / 1000.0,
                    heap_total.as_secs_f64() * 1000.0,
                    heap_iter_ns[p50] as f64 / 1000.0,
                    heap_iter_ns[p99] as f64 / 1000.0,
                    sink_scan,
                    sink_heap,
                );
            }
        }

        struct TestPermit(Arc<AtomicUsize>);
        impl Drop for TestPermit {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }

        struct TestReactorHarness {
            addr: std::net::SocketAddr,
            stop_flag: Arc<AtomicBool>,
            waker: ReactorWaker,
            live_permits: Arc<AtomicUsize>,
            total_accepted: Arc<AtomicUsize>,
            threads: Option<(std::thread::JoinHandle<()>, std::thread::JoinHandle<()>)>,
        }

        impl TestReactorHarness {
            fn spawn<F>(cfg: ReactorConfig, max_per_ip: usize, responder: F) -> Self
            where
                F: Fn(&ReactorJob) -> (Vec<u8>, Option<ReactorFileBody>, bool) + Send + 'static,
            {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = listener.local_addr().unwrap();
                let stop_flag = Arc::new(AtomicBool::new(false));
                let wake_pair = ReactorWakePair::new().unwrap();
                let waker = wake_pair.waker();
                let (job_tx, job_rx) = mpsc::sync_channel::<ReactorJob>(64);
                let (res_tx, res_rx) = mpsc::channel::<ReactorResponse>();

                let live_permits = Arc::new(AtomicUsize::new(0));
                let total_accepted = Arc::new(AtomicUsize::new(0));

                let worker_waker = waker.clone();
                let worker_handle = std::thread::spawn(move || {
                    while let Ok(job) = job_rx.recv() {
                        let (bytes, file_body, keep_alive) = responder(&job);
                        let _ = res_tx.send(ReactorResponse {
                            conn_id: job.conn_id,
                            bytes,
                            file_body,
                            keep_alive,
                        });
                        worker_waker.wake();
                    }
                });

                let r_stop = Arc::clone(&stop_flag);
                let r_permits = Arc::clone(&live_permits);
                let r_accepted = Arc::clone(&total_accepted);
                let reactor_handle = std::thread::spawn(move || {
                    run_http_reactor(
                        listener,
                        r_stop,
                        cfg,
                        move |_ip| {
                            let cur = r_permits.fetch_add(1, Ordering::SeqCst);
                            if max_per_ip > 0 && cur >= max_per_ip {
                                r_permits.fetch_sub(1, Ordering::SeqCst);
                                return None;
                            }
                            r_accepted.fetch_add(1, Ordering::SeqCst);
                            Some(TestPermit(Arc::clone(&r_permits)))
                        },
                        |mut s| {
                            let _ = s.set_nonblocking(true);
                            let _ = s.write_all(
                                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 19\r\nRetry-After: 1\r\nConnection: close\r\n\r\nService Unavailable",
                            );
                        },
                        |_e| false,
                        job_tx,
                        res_rx,
                        wake_pair,
                    );
                });

                Self {
                    addr,
                    stop_flag,
                    waker,
                    live_permits,
                    total_accepted,
                    threads: Some((reactor_handle, worker_handle)),
                }
            }

            fn total_accepted(&self) -> usize {
                self.total_accepted.load(Ordering::SeqCst)
            }

            fn wait_accepted_gt(&self, prev: usize) {
                for _ in 0..200 {
                    if self.total_accepted.load(Ordering::SeqCst) > prev {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                panic!(
                    "timed out waiting for total_accepted ({}) > {}",
                    self.total_accepted.load(Ordering::SeqCst),
                    prev
                );
            }

            fn wait_permits_zero(&self) {
                for _ in 0..300 {
                    if self.live_permits.load(Ordering::SeqCst) == 0 {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert_eq!(
                    self.live_permits.load(Ordering::SeqCst),
                    0,
                    "timed out waiting for active reactor permits to reach zero"
                );
            }
        }

        impl Drop for TestReactorHarness {
            fn drop(&mut self) {
                self.stop_flag.store(true, Ordering::Release);
                self.waker.wake();
                let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(50));
                if let Some((rh, wh)) = self.threads.take() {
                    let _ = rh.join();
                    let _ = wh.join();
                }
            }
        }

        fn build_ok_response(body: &[u8], keep_alive: bool, head_only: bool) -> Vec<u8> {
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
                body.len(),
                if keep_alive { "keep-alive" } else { "close" }
            )
            .into_bytes();
            if !head_only {
                out.extend_from_slice(body);
            }
            out
        }

        fn count_open_fds() -> usize {
            let dir = if cfg!(target_os = "linux") {
                "/proc/self/fd"
            } else {
                "/dev/fd"
            };
            std::fs::read_dir(dir).map(|it| it.count()).unwrap_or(0)
        }

        use std::sync::atomic::AtomicUsize;

        #[test]
        fn reactor_enforces_header_body_idle_and_write_timeouts() {
            let cfg = ReactorConfig {
                idle_timeout: Duration::from_millis(120),
                header_timeout: Duration::from_millis(140),
                body_timeout: Duration::from_millis(140),
                write_timeout: Duration::from_millis(160),
                max_conns: 32,
                max_keepalive_requests: 100,
            };
            let big_payload = Arc::new(vec![b'B'; 16 * 1024 * 1024]);
            let big_clone = Arc::clone(&big_payload);

            let srv = TestReactorHarness::spawn(cfg, 0, move |job| {
                if job.raw.path == "/big" {
                    let out = build_ok_response(&big_clone, false, job.head_only);
                    (out, None, false)
                } else {
                    (build_ok_response(b"ok", job.keep_alive, job.head_only), None, job.keep_alive)
                }
            });

            // 1. Header timeout -> 408 Request Timeout.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                c.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n").unwrap();
                let mut resp = String::new();
                c.read_to_string(&mut resp).unwrap();
                assert!(resp.starts_with("HTTP/1.1 408 Request Timeout\r\n"), "{resp}");
            }

            // 2. Body timeout -> 408 Request Timeout.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                c.write_all(
                    b"POST /hello HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nabcd",
                )
                .unwrap();
                let mut resp = String::new();
                c.read_to_string(&mut resp).unwrap();
                assert!(resp.starts_with("HTTP/1.1 408 Request Timeout\r\n"), "{resp}");
            }

            // 3. Keep-alive idle timeout -> clean EOF without 408.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                c.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut all = String::new();
                c.read_to_string(&mut all).unwrap();
                assert!(
                    all.starts_with("HTTP/1.1 200 OK\r\n") && all.ends_with("\r\n\r\nok"),
                    "{all}"
                );
                assert!(!all.contains("408 Request Timeout"), "{all}");
            }

            // 4. Write stall timeout: request 16 MiB and never drain socket until write deadline fires.
            {
                let accepted_before = srv.total_accepted();
                let mut stalled_writer = TcpStream::connect(srv.addr).unwrap();
                stalled_writer
                    .write_all(b"GET /big HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                    .unwrap();
                srv.wait_accepted_gt(accepted_before);
                srv.wait_permits_zero();
                stalled_writer
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut drained = Vec::new();
                let _ = stalled_writer.read_to_end(&mut drained);
                assert!(
                    drained.len() < big_payload.len(),
                    "stalled write should time out and close before delivering full 16 MiB (got {})",
                    drained.len()
                );
            }
            srv.wait_permits_zero();
        }

        #[test]
        fn reactor_handles_abrupt_disconnects_mid_read_write_and_file_stream() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let tmp_dir = std::env::temp_dir().join(format!(
                "conduit_reactor_abort_{}_{}",
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&tmp_dir).unwrap();
            let file_path = tmp_dir.join("blob.bin");
            let file_len = 2 * 1024 * 1024usize;
            let file_bytes: Vec<u8> = (0..file_len).map(|i| ((i * 31) ^ 0xA5) as u8).collect();
            std::fs::write(&file_path, &file_bytes).unwrap();

            let cfg = ReactorConfig {
                idle_timeout: Duration::from_millis(500),
                header_timeout: Duration::from_millis(500),
                body_timeout: Duration::from_millis(500),
                write_timeout: Duration::from_secs(2),
                max_conns: 64,
                max_keepalive_requests: 100,
            };
            let big_buf = Arc::new(vec![b'Z'; 4 * 1024 * 1024]);
            let big_buf_c = Arc::clone(&big_buf);
            let fp_c = file_path.clone();

            let srv = TestReactorHarness::spawn(cfg, 0, move |job| match job.raw.path.as_str() {
                "/big" => (build_ok_response(&big_buf_c, false, job.head_only), None, false),
                "/file" => {
                    let file = std::fs::File::open(&fp_c).unwrap();
                    let head = build_ok_response(&file_bytes, job.keep_alive, true);
                    (
                        head,
                        Some(ReactorFileBody {
                            file,
                            remaining: file_len as u64,
                        }),
                        job.keep_alive,
                    )
                }
                "/truncated_file" => {
                    let file = std::fs::File::open(&fp_c).unwrap();
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        file_len + 1024
                    )
                    .into_bytes();
                    (
                        header,
                        Some(ReactorFileBody {
                            file,
                            remaining: (file_len + 1024) as u64,
                        }),
                        false,
                    )
                }
                _ => (
                    build_ok_response(b"healthy", job.keep_alive, job.head_only),
                    None,
                    job.keep_alive,
                ),
            });

            // 1. Disconnect mid-header and mid-body.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.write_all(b"GET /healthy HTTP/1.1\r\n").unwrap();
            }
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.write_all(b"POST /healthy HTTP/1.1\r\nContent-Length: 4096\r\n\r\npart")
                    .unwrap();
            }

            // 2. Disconnect mid-buffered-write and mid-file-stream.
            for path in ["/big", "/file"] {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                c.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                    .unwrap();
                let mut head_sample = [0u8; 512];
                let n = c.read(&mut head_sample).unwrap();
                assert!(n > 0, "expected partial bytes for {path}");
                drop(c);
            }

            // 3. Truncated ReactorFileBody (`remaining > actual file size`) hits EOF cleanly.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                c.write_all(b"GET /truncated_file HTTP/1.1\r\nHost: x\r\n\r\n")
                    .unwrap();
                let mut partial = Vec::new();
                let _ = c.read_to_end(&mut partial);
                assert!(partial.len() < file_len + 1024);
            }

            // 4. Reactor remains completely healthy and serves full /file & /healthy responses.
            {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                c.write_all(
                    b"GET /file HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
                let mut full = Vec::new();
                c.read_to_end(&mut full).unwrap();
                let split = find_crlf_crlf(&full).unwrap();
                let expected: Vec<u8> = (0..file_len).map(|i| ((i * 31) ^ 0xA5) as u8).collect();
                assert_eq!(&full[split..], expected.as_slice());
            }

            srv.wait_permits_zero();
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }

        #[test]
        fn reactor_no_fd_leak_across_churn_timeouts_aborts_and_file_streams() {
            let fds_before_pollers = count_open_fds();
            for _ in 0..150 {
                let poller = Poller::new().unwrap();
                let wake = ReactorWakePair::new().unwrap();
                poller.register_read(wake.read_fd.0, TOKEN_WAKE).unwrap();
                poller.deregister(wake.read_fd.0, true, false);
            }
            let fds_after_pollers = count_open_fds();
            assert!(
                fds_after_pollers.saturating_sub(fds_before_pollers) < 32,
                "poller/wake-pipe create+drop leaked fds: before={fds_before_pollers}, after={fds_after_pollers}"
            );

            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let tmp_dir = std::env::temp_dir().join(format!(
                "conduit_reactor_fdleak_{}_{}",
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&tmp_dir).unwrap();
            let file_path = tmp_dir.join("asset.bin");
            let file_data = vec![0x42u8; 32 * 1024];
            std::fs::write(&file_path, &file_data).unwrap();

            let fds_before_srv = count_open_fds();
            let cfg = ReactorConfig {
                idle_timeout: Duration::from_millis(80),
                header_timeout: Duration::from_millis(80),
                body_timeout: Duration::from_millis(80),
                write_timeout: Duration::from_secs(2),
                max_conns: 12,
                max_keepalive_requests: 10,
            };
            let fp = file_path.clone();
            let fdata_len = file_data.len();

            let srv = TestReactorHarness::spawn(cfg, 0, move |job| {
                if job.raw.path == "/asset" {
                    let f = std::fs::File::open(&fp).unwrap();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
                        fdata_len,
                        if job.keep_alive { "keep-alive" } else { "close" }
                    )
                    .into_bytes();
                    (
                        head,
                        Some(ReactorFileBody {
                            file: f,
                            remaining: fdata_len as u64,
                        }),
                        job.keep_alive,
                    )
                } else {
                    (build_ok_response(b"ok", job.keep_alive, job.head_only), None, job.keep_alive)
                }
            });

            // 1. Keep-alive multi-request connections.
            for _ in 0..30 {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                c.write_all(
                    b"GET /a HTTP/1.1\r\nHost: x\r\n\r\nGET /b HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
                let mut buf = Vec::new();
                let _ = c.read_to_end(&mut buf);
                assert!(buf.windows(2).any(|w| w == b"ok"));
            }

            // 2. File streams & abrupt client drops.
            for idx in 0..24 {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                c.write_all(b"GET /asset HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                    .unwrap();
                if idx % 2 == 0 {
                    let mut one = [0u8; 64];
                    let _ = c.read(&mut one);
                } else {
                    let mut buf = Vec::new();
                    let _ = c.read_to_end(&mut buf);
                    let split = find_crlf_crlf(&buf).unwrap();
                    assert_eq!(buf.len() - split, fdata_len);
                }
            }

            // 3. Saturated max_conns (12) -> 503 rejections + 408 timeouts + instant drops.
            {
                let mut held = Vec::new();
                for _ in 0..12 {
                    let mut s = TcpStream::connect(srv.addr).unwrap();
                    let _ = s.write_all(b"GET /hold HTTP/1.1\r\n");
                    held.push(s);
                }
                for _ in 0..20 {
                    if let Ok(mut extra) = TcpStream::connect(srv.addr) {
                        let _ = extra.set_read_timeout(Some(Duration::from_millis(200)));
                        let _ = extra.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
                        let mut small = [0u8; 256];
                        let _ = extra.read(&mut small);
                    }
                }
                for mut h in held {
                    let _ = h.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut r = Vec::new();
                    let _ = h.read_to_end(&mut r);
                }
            }

            srv.wait_permits_zero();
            drop(srv);
            let fds_after_srv = count_open_fds();
            assert!(
                fds_after_srv.saturating_sub(fds_before_srv) < 32,
                "reactor churn leaked fds: before={fds_before_srv}, after={fds_after_srv}"
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }

        #[test]
        fn reactor_fuzz_incremental_framing_and_pipelining_never_panics() {
            let cfg = ReactorConfig {
                idle_timeout: Duration::from_millis(250),
                header_timeout: Duration::from_millis(250),
                body_timeout: Duration::from_millis(250),
                write_timeout: Duration::from_secs(2),
                max_conns: 64,
                max_keepalive_requests: 10,
            };
            let srv = TestReactorHarness::spawn(cfg, 0, |job| {
                let body = if job.raw.body.is_empty() {
                    b"ok".as_slice()
                } else {
                    job.raw.body.as_slice()
                };
                (
                    build_ok_response(body, job.keep_alive, job.head_only),
                    None,
                    job.keep_alive,
                )
            });

            let edge_cases: Vec<Vec<u8>> = vec![
                b"GET /1 HTTP/1.1\r\nHost: x\r\n\r\nGET /2 HTTP/1.1\r\nHost: x\r\n\r\nPOST /3 HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".to_vec(),
                b"GET /bare HTTP/1.1\nHost: x\r\n\r\n".to_vec(),
                b"GET /cr HTTP/1.1\rHost: x\r\n\r\n".to_vec(),
                b"POST /badcl HTTP/1.1\r\nHost: x\r\nContent-Length: nope\r\n\r\n".to_vec(),
                format!("GET /big HTTP/1.1\r\nHost: x\r\nX-Pad: {}\r\n\r\n", "a".repeat(MAX_HEADER_BYTES + 128)).into_bytes(),
                b"\r\n\r\n\xff\xfe\x00\x01".to_vec(),
            ];

            for payload in &edge_cases {
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let _ = c.write_all(payload);
                let mut out = Vec::new();
                let _ = c.read_to_end(&mut out);
            }

            // Byte-by-byte trickle across header CRLFCRLF and body split.
            {
                let msg = b"POST /trickle HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping";
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_nodelay(true).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                for chunk in msg.chunks(3) {
                    c.write_all(chunk).unwrap();
                    std::thread::sleep(Duration::from_millis(1));
                }
                let mut resp = String::new();
                c.read_to_string(&mut resp).unwrap();
                assert!(
                    resp.starts_with("HTTP/1.1 200 OK\r\n") && resp.ends_with("\r\n\r\nping"),
                    "{resp}"
                );
            }

            // 60 deterministic fuzzed frames.
            let mut rng = 0x243f_6a88_85a3_08d3u64;
            let mut next_u64 = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            for _ in 0..60 {
                let len = ((next_u64() as usize) % 512) + 1;
                let data: Vec<u8> = (0..len).map(|_| (next_u64() & 0xff) as u8).collect();
                let mut c = TcpStream::connect(srv.addr).unwrap();
                c.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
                let _ = c.write_all(&data);
                let _ = c.shutdown(std::net::Shutdown::Write);
                let mut sink = [0u8; 512];
                let _ = c.read(&mut sink);
            }

            // Verify reactor recovers and answers cleanly.
            let mut healthy = TcpStream::connect(srv.addr).unwrap();
            healthy.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            healthy
                .write_all(b"GET /ok HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut ok_resp = String::new();
            healthy.read_to_string(&mut ok_resp).unwrap();
            assert!(
                ok_resp.starts_with("HTTP/1.1 200 OK\r\n") && ok_resp.ends_with("\r\n\r\nok"),
                "{ok_resp}"
            );
            srv.wait_permits_zero();
        }
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
))]
pub(crate) use imp::{
    ReactorConfig, ReactorFileBody, ReactorJob, ReactorResponse, ReactorWakePair, run_http_reactor,
};
