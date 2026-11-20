//! Pure-Rust TLS 1.2 / 1.3 client implementation.

pub mod asn1;
pub mod crypto;
pub mod error;
pub mod handshake;
pub mod pem;
pub mod record;
pub mod roots;
pub mod tls12;
pub mod tls13;
pub mod verify;
pub mod x509;

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::native::tls::error::{AlertDescription, TlsError};
use crate::native::tls::handshake::{HT_KEY_UPDATE, HT_NEW_SESSION_TICKET, HandshakeReader, wrap_handshake};
use crate::native::tls::record::{CT_ALERT, CT_APPLICATION_DATA, CT_HANDSHAKE, RecordLayer};
use crate::native::tls::roots::RootSource;
use crate::native::tls::tls13::{
    Tls13Secrets, connect_tls_handshake, derive_next_app_secret, make_tls13_cipher_state,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    Disable,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub server_name: String,
    pub verify: VerifyMode,
    pub roots: RootSource,
    pub alpn: Vec<&'static [u8]>,
    pub min_version: TlsVersion,
    pub handshake_timeout: Duration,
}

impl TlsConfig {
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            verify: VerifyMode::VerifyFull,
            roots: RootSource::System,
            alpn: Vec::new(),
            min_version: TlsVersion::Tls12,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

pub struct TlsStream<S: Read + Write> {
    inner: S,
    record: RecordLayer,
    hs_reader: HandshakeReader,
    app_buf: Vec<u8>,
    app_pos: usize,
    eof_clean: bool,
    alpn: Option<Vec<u8>>,
    leaf_cert_der: Option<Vec<u8>>,
    tls13_secrets: Option<Tls13Secrets>,
}

impl<S: Read + Write> TlsStream<S> {
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.alpn.as_deref()
    }

    pub fn leaf_cert_der(&self) -> Option<&[u8]> {
        self.leaf_cert_der.as_deref()
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    fn handle_post_handshake_bytes(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.hs_reader.push_bytes(data)?;
        while let Some((mt, raw)) = self.hs_reader.next_message()? {
            match mt {
                HT_NEW_SESSION_TICKET => {
                    // Ignored (stateless client)
                }
                HT_KEY_UPDATE => {
                    if raw.len() != 5 {
                        return Err(TlsError::DecodeError("invalid KeyUpdate length"));
                    }
                    let req = raw[4];
                    if req > 1 {
                        return Err(TlsError::DecodeError("invalid KeyUpdate request_update"));
                    }
                    if let Some(secrets) = &mut self.tls13_secrets {
                        secrets.server_app_secret =
                            derive_next_app_secret(secrets.suite, &secrets.server_app_secret);
                        self.record.read_state =
                            make_tls13_cipher_state(secrets.suite, &secrets.server_app_secret);
                        if req == 1 {
                            let ku = wrap_handshake(HT_KEY_UPDATE, &[0]);
                            self.record.write_record(&mut self.inner, CT_HANDSHAKE, &ku)?;
                            secrets.client_app_secret =
                                derive_next_app_secret(secrets.suite, &secrets.client_app_secret);
                            self.record.write_state =
                                make_tls13_cipher_state(secrets.suite, &secrets.client_app_secret);
                        }
                    }
                }
                _ => {
                    return Err(TlsError::ProtocolError(format!(
                        "unexpected post-handshake message type {mt}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn send_key_update_if_needed(&mut self) -> Result<(), TlsError> {
        if self.record.write_state.needs_tls13_rekey()
            && let Some(secrets) = &mut self.tls13_secrets
        {
            let ku = wrap_handshake(HT_KEY_UPDATE, &[0]);
            self.record.write_record(&mut self.inner, CT_HANDSHAKE, &ku)?;
            secrets.client_app_secret =
                derive_next_app_secret(secrets.suite, &secrets.client_app_secret);
            self.record.write_state =
                make_tls13_cipher_state(secrets.suite, &secrets.client_app_secret);
        }
        Ok(())
    }

    pub fn send_close_notify(&mut self) {
        let _ = self.record.write_record(&mut self.inner, CT_ALERT, &[1, 0]);
    }
}

impl<S: Read + Write> Read for TlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.app_pos < self.app_buf.len() {
                let avail = self.app_buf.len() - self.app_pos;
                let n = avail.min(buf.len());
                buf[..n].copy_from_slice(&self.app_buf[self.app_pos..self.app_pos + n]);
                self.app_pos += n;
                if self.app_pos == self.app_buf.len() {
                    self.app_buf.clear();
                    self.app_pos = 0;
                }
                return Ok(n);
            }
            if self.eof_clean {
                return Ok(0);
            }

            let (ct, payload) = self
                .record
                .read_record(&mut self.inner)
                .map_err(tls_to_io_error)?;
            match ct {
                CT_APPLICATION_DATA => {
                    if !payload.is_empty() {
                        self.app_buf.clear();
                        self.app_pos = 0;
                        self.app_buf.extend_from_slice(payload);
                    }
                }
                CT_HANDSHAKE => {
                    let owned = payload.to_vec();
                    self.handle_post_handshake_bytes(&owned)
                        .map_err(tls_to_io_error)?;
                }
                CT_ALERT => {
                    if payload.len() >= 2
                        && AlertDescription::from_u8(payload[1]) == AlertDescription::CloseNotify
                    {
                        self.eof_clean = true;
                        return Ok(0);
                    }
                    let desc = payload
                        .get(1)
                        .copied()
                        .map(AlertDescription::from_u8)
                        .unwrap_or(AlertDescription::DecodeError);
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        format!("TLS alert: {}", desc.as_str()),
                    ));
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unexpected TLS record type {ct}"),
                    ));
                }
            }
        }
    }
}

impl<S: Read + Write> Write for TlsStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.send_key_update_if_needed().map_err(tls_to_io_error)?;
        self.record
            .write_record(&mut self.inner, CT_APPLICATION_DATA, buf)
            .map_err(tls_to_io_error)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl TlsStream<TcpStream> {
    pub fn is_alive_probe(&mut self) -> bool {
        if self.eof_clean || self.app_pos < self.app_buf.len() {
            return false;
        }
        if self.inner.set_nonblocking(true).is_err() {
            return false;
        }
        let mut alive = true;
        loop {
            match self.record.try_read_record_nonblocking(&mut self.inner) {
                Ok(None) => break,
                Ok(Some((CT_HANDSHAKE, payload))) => {
                    let owned = payload.to_vec();
                    if self.handle_post_handshake_bytes(&owned).is_err() {
                        alive = false;
                        break;
                    }
                }
                Ok(Some((CT_ALERT, _))) | Ok(Some((CT_APPLICATION_DATA, _))) | Ok(Some(_)) | Err(_) => {
                    alive = false;
                    break;
                }
            }
        }
        let _ = self.inner.set_nonblocking(false);
        alive
    }
}

pub fn connect(tcp: TcpStream, cfg: &TlsConfig) -> Result<TlsStream<TcpStream>, TlsError> {
    let prev_read = tcp.read_timeout().ok().flatten();
    let prev_write = tcp.write_timeout().ok().flatten();
    let _ = tcp.set_read_timeout(Some(cfg.handshake_timeout));
    let _ = tcp.set_write_timeout(Some(cfg.handshake_timeout));

    let res = connect_stream(tcp, cfg);
    if let Ok(ref s) = res {
        let _ = s.get_ref().set_read_timeout(prev_read);
        let _ = s.get_ref().set_write_timeout(prev_write);
    }
    res
}

pub fn connect_stream<S: Read + Write>(mut stream: S, cfg: &TlsConfig) -> Result<TlsStream<S>, TlsError> {
    let allow_tls12 = cfg.min_version == TlsVersion::Tls12;
    let outcome = connect_tls_handshake(&mut stream, cfg, allow_tls12)?;
    Ok(TlsStream {
        inner: stream,
        record: outcome.record,
        hs_reader: HandshakeReader::new(),
        app_buf: Vec::with_capacity(4096),
        app_pos: 0,
        eof_clean: false,
        alpn: outcome.alpn,
        leaf_cert_der: outcome.leaf_cert_der,
        tls13_secrets: outcome.tls13_secrets,
    })
}

fn tls_to_io_error(e: TlsError) -> io::Error {
    match e {
        TlsError::Io(io_err) => io_err,
        TlsError::UnexpectedEof => io::Error::new(io::ErrorKind::UnexpectedEof, e.to_string()),
        TlsError::Timeout => io::Error::new(io::ErrorKind::TimedOut, e.to_string()),
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    }
}

/// Transport abstraction for DB and network drivers (zero overhead for Plain TCP).
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Read for Transport {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Transport {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

impl Transport {
    pub fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.set_read_timeout(d),
            Self::Tls(s) => s.get_ref().set_read_timeout(d),
        }
    }

    pub fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.set_write_timeout(d),
            Self::Tls(s) => s.get_ref().set_write_timeout(d),
        }
    }

    pub fn set_nodelay(&self, on: bool) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.set_nodelay(on),
            Self::Tls(s) => s.get_ref().set_nodelay(on),
        }
    }

    pub fn leaf_cert_der(&self) -> Option<&[u8]> {
        match self {
            Self::Plain(_) => None,
            Self::Tls(s) => s.leaf_cert_der(),
        }
    }

    pub fn shutdown(&mut self) {
        match self {
            Self::Plain(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Self::Tls(s) => {
                s.send_close_notify();
                let _ = s.get_ref().shutdown(std::net::Shutdown::Both);
            }
        }
    }

    pub fn is_alive_probe(&mut self) -> bool {
        match self {
            Self::Plain(s) => {
                if s.set_nonblocking(true).is_err() {
                    return false;
                }
                let mut probe = [0u8; 1];
                let alive = match s.peek(&mut probe) {
                    Ok(_) => false,
                    Err(e) => e.kind() == io::ErrorKind::WouldBlock,
                };
                let _ = s.set_nonblocking(false);
                alive
            }
            Self::Tls(s) => s.is_alive_probe(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct MockPipe {
        rx: Cursor<Vec<u8>>,
        tx: Vec<u8>,
    }

    impl Read for MockPipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.rx.read(buf)
        }
    }

    impl Write for MockPipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.tx.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn negative_record_overflow_and_downgrade_sentinel() {
        // 1. Record overflow (> 16 KiB + 256)
        let mut overflow_rec = vec![CT_HANDSHAKE, 0x03, 0x03, 0x45, 0x00];
        overflow_rec.resize(5 + 0x4500, 0);
        let pipe = MockPipe {
            rx: Cursor::new(overflow_rec),
            tx: Vec::new(),
        };
        let cfg = TlsConfig::new("example.com");
        assert!(matches!(
            connect_stream(pipe, &cfg),
            Err(TlsError::RemoteAlert {
                description: AlertDescription::RecordOverflow,
                ..
            })
        ));

        // 2. TLS 1.3 -> TLS 1.2 downgrade sentinel in ServerHello.random
        let mut sh_body = Vec::new();
        sh_body.extend_from_slice(&0x0303u16.to_be_bytes());
        let mut sh_rand = [0x11u8; 32];
        sh_rand[24..].copy_from_slice(b"DOWNGRD\x01");
        sh_body.extend_from_slice(&sh_rand);
        sh_body.push(0); // session_id len
        sh_body.extend_from_slice(&0xc02fu16.to_be_bytes()); // TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
        sh_body.push(0); // compression
        let hs = wrap_handshake(crate::native::tls::handshake::HT_SERVER_HELLO, &sh_body);
        let mut rec = vec![CT_HANDSHAKE, 0x03, 0x03, (hs.len() >> 8) as u8, (hs.len() & 0xff) as u8];
        rec.extend_from_slice(&hs);

        let pipe2 = MockPipe {
            rx: Cursor::new(rec),
            tx: Vec::new(),
        };
        assert!(matches!(
            connect_stream(pipe2, &cfg),
            Err(TlsError::RemoteAlert {
                description: AlertDescription::IllegalParameter,
                ..
            })
        ));

        // 3. Truncated / garbage handshake never panics
        for len in 0..16 {
            let garbage = vec![CT_HANDSHAKE; len];
            let p = MockPipe {
                rx: Cursor::new(garbage),
                tx: Vec::new(),
            };
            assert!(std::panic::catch_unwind(|| connect_stream(p, &cfg).is_err()).is_ok());
        }
    }

    #[test]
    fn negative_bad_record_mac_and_missing_close_notify() {
        use crate::native::tls::crypto::gcm::AesGcm;
        use crate::native::tls::record::AeadCipher;
        use crate::native::tls::record::CipherState;

        let key = [0x42u8; 16];
        let iv = [0x11u8; 12];
        let cipher = AeadCipher::AesGcm(Box::new(AesGcm::new(&key).unwrap()));

        // 1. Truncation attack: EOF without close_notify
        let pipe_eof = MockPipe {
            rx: Cursor::new(Vec::new()),
            tx: Vec::new(),
        };
        let mut tls_eof = TlsStream {
            inner: pipe_eof,
            record: RecordLayer::new(),
            hs_reader: HandshakeReader::new(),
            app_buf: Vec::new(),
            app_pos: 0,
            eof_clean: false,
            alpn: None,
            leaf_cert_der: None,
            tls13_secrets: None,
        };
        tls_eof.record.read_state = CipherState::Tls13 {
            cipher: cipher.clone(),
            iv,
            seq: 0,
        };
        let mut buf = [0u8; 16];
        let err = tls_eof.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        // 2. Corrupted AEAD tag -> BadRecordMac
        let mut bad_rec = vec![CT_APPLICATION_DATA, 0x03, 0x03, 0x00, 0x15];
        bad_rec.extend_from_slice(&[0xaa; 21]);
        let pipe_mac = MockPipe {
            rx: Cursor::new(bad_rec),
            tx: Vec::new(),
        };
        let mut tls_mac = TlsStream {
            inner: pipe_mac,
            record: RecordLayer::new(),
            hs_reader: HandshakeReader::new(),
            app_buf: Vec::new(),
            app_pos: 0,
            eof_clean: false,
            alpn: None,
            leaf_cert_der: None,
            tls13_secrets: None,
        };
        tls_mac.record.read_state = CipherState::Tls13 {
            cipher,
            iv,
            seq: 0,
        };
        let err_mac = tls_mac.read(&mut buf).unwrap_err();
        assert_eq!(err_mac.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn fuzz_like_parsers_never_panic() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next_byte = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed & 0xff) as u8
        };

        for iter in 0..500 {
            let len = (iter % 128) + 1;
            let data: Vec<u8> = (0..len).map(|_| next_byte()).collect();
            assert!(std::panic::catch_unwind(|| {
                let _ = crate::native::tls::handshake::parse_server_hello(&data);
                let _ = crate::native::tls::handshake::parse_encrypted_extensions(&data);
                let _ = crate::native::tls::handshake::parse_tls13_certificate(&data);
                let _ = crate::native::tls::handshake::parse_certificate_verify(&data);
                let _ = crate::native::tls::x509::parse_x509(&data);
                let mut dr = crate::native::tls::asn1::DerReader::new(&data);
                let _ = dr.read_tlv();
            })
            .is_ok());
        }
    }

    #[test]
    fn openssl_s_server_tls13_and_tls12_interop() {
        use std::process::{Command, Stdio};
        if Command::new("openssl").arg("version").output().is_err() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("conduit_tls_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let key_path = tmp.join("srv.key");
        let crt_path = tmp.join("srv.crt");
        let ext_path = tmp.join("srv.ext");
        std::fs::write(&ext_path, "subjectAltName=DNS:localhost,IP:127.0.0.1\n").unwrap();

        let ok = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                "-nodes", "-keyout", key_path.to_str().unwrap(),
                "-out", crt_path.to_str().unwrap(),
                "-days", "1", "-subj", "/CN=localhost",
                "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }

        let Ok(probe) = std::net::TcpListener::bind("127.0.0.1:0") else {
            return; // Loopback sockets disabled in sandbox
        };
        drop(probe);

        for (ver_flag, min_ver, cipher_flag) in [
            ("-tls1_3", TlsVersion::Tls13, ("-ciphersuites", "TLS_AES_128_GCM_SHA256")),
            ("-tls1_3", TlsVersion::Tls13, ("-ciphersuites", "TLS_CHACHA20_POLY1305_SHA256")),
            ("-tls1_2", TlsVersion::Tls12, ("-cipher", "ECDHE-ECDSA-AES128-GCM-SHA256")),
            ("-tls1_2", TlsVersion::Tls12, ("-cipher", "ECDHE-ECDSA-CHACHA20-POLY1305")),
        ] {
            let port = {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port()
            };
            let mut child = Command::new("openssl")
                .args([
                    "s_server",
                    "-accept", &format!("127.0.0.1:{port}"),
                    "-cert", crt_path.to_str().unwrap(),
                    "-key", key_path.to_str().unwrap(),
                    "-www",
                    ver_flag,
                    cipher_flag.0, cipher_flag.1,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn openssl s_server");

            // Wait briefly for s_server to bind
            let mut tcp = None;
            let mut perm_denied = false;
            for _ in 0..30 {
                match TcpStream::connect(format!("127.0.0.1:{port}")) {
                    Ok(s) => {
                        tcp = Some(s);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        perm_denied = true;
                        break;
                    }
                    Err(_) => {}
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if perm_denied {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&tmp);
                return;
            }
            let tcp = tcp.expect("connect to openssl s_server");
            let mut cfg = TlsConfig::new("localhost");
            cfg.roots = RootSource::File(crt_path.clone());
            cfg.min_version = min_ver;
            let mut tls = connect(tcp, &cfg)
                .unwrap_or_else(|e| panic!("handshake failed for {ver_flag} {}: {e}", cipher_flag.1));
            tls.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
            let mut resp = Vec::new();
            let _ = tls.read_to_end(&mut resp);
            let text = String::from_utf8_lossy(&resp);
            assert!(
                text.contains("HTTP/1.0 200 ok"),
                "unexpected response for {ver_flag} {}: {text}",
                cipher_flag.1
            );
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires network access"]
    fn live_https_tls_smoke_test() {
        for host in ["example.com", "www.cloudflare.com", "github.com"] {
            let addr = format!("{host}:443");
            let tcp = TcpStream::connect(&addr).expect("connect");
            let mut cfg = TlsConfig::new(host);
            cfg.alpn = vec![b"http/1.1"];
            let mut tls = connect(tcp, &cfg).expect("TLS handshake");
            let req = format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
            tls.write_all(req.as_bytes()).unwrap();
            let mut resp = Vec::new();
            let _ = tls.read_to_end(&mut resp);
            let head = String::from_utf8_lossy(&resp);
            assert!(head.starts_with("HTTP/1."), "unexpected response from {host}: {head}");
        }
    }
}
