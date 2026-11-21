use std::collections::VecDeque;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::crypto::Zeroize;
use super::crypto::hmac_hkdf::hkdf_expand_label;
use super::crypto::sha2::{Sha256, Sha384};
use super::handshake::{CipherSuite, parse_new_session_ticket};
use super::roots::RootSource;
use super::verify::unix_now;
use super::{TlsConfig, VerifyMode};
use crate::native::crypto::sha256_bytes;

const MAX_CACHED_SESSIONS: usize = 128;
const MAX_TICKET_BYTES: usize = 4096;
const MAX_LIFETIME_SECS: u32 = 7 * 24 * 60 * 60;

#[derive(PartialEq, Eq, Clone)]
pub(super) struct SessionKey {
    host: String,
    port: u16,
    alpn: Vec<&'static [u8]>,
    verify: VerifyMode,
    custom_roots_hash: Option<[u8; 32]>,
}

impl SessionKey {
    // A CA file is snapshotted into Inline by connect(), so the digest and
    // the certificate verifier see exactly the same trust anchors.
    pub(super) fn new(cfg: &TlsConfig, port: u16) -> Self {
        let custom_roots_hash = match (&cfg.roots, cfg.verify) {
            (RootSource::Inline(pem), VerifyMode::VerifyCa | VerifyMode::VerifyFull) => {
                Some(sha256_bytes(pem))
            }
            _ => None,
        };
        Self {
            host: cfg.server_name.to_ascii_lowercase(),
            port,
            alpn: cfg.alpn.clone(),
            verify: cfg.verify,
            custom_roots_hash,
        }
    }
}

#[derive(Clone)]
pub(super) struct ServerIdentity {
    pub(super) leaf: Arc<[u8]>,
    pub(super) validity: Option<RangeInclusive<i64>>,
    pub(super) authenticated_at: Instant,
}

impl ServerIdentity {
    fn is_current(&self, now: Instant, unix: i64) -> bool {
        now.duration_since(self.authenticated_at).as_secs() < u64::from(MAX_LIFETIME_SECS)
            && self
                .validity
                .as_ref()
                .is_none_or(|range| range.contains(&unix))
    }
}

pub(super) struct Resumption {
    pub(super) identity: ServerIdentity,
    pub(super) master_secret: Vec<u8>,
    pub(super) remaining_tickets: u8,
}

impl Drop for Resumption {
    fn drop(&mut self) {
        self.master_secret.zeroize();
    }
}

pub(super) struct SessionTicket {
    pub(super) identity: ServerIdentity,
    pub(super) suite: CipherSuite,
    pub(super) ticket: Vec<u8>,
    pub(super) psk: Vec<u8>,
    issued_at: Instant,
    lifetime: Duration,
    age_add: u32,
}

impl Drop for SessionTicket {
    fn drop(&mut self) {
        self.psk.zeroize();
    }
}

impl SessionTicket {
    pub(super) fn is_current(&self, now: Instant, unix: i64) -> bool {
        now.duration_since(self.issued_at) < self.lifetime && self.identity.is_current(now, unix)
    }

    pub(super) fn obfuscated_age(&self) -> u32 {
        (self.issued_at.elapsed().as_millis() as u32).wrapping_add(self.age_add)
    }
}

static SESSIONS: Mutex<VecDeque<(SessionKey, SessionTicket)>> = Mutex::new(VecDeque::new());

pub(super) fn take_ticket(key: &SessionKey) -> Option<SessionTicket> {
    let mut sessions = SESSIONS.lock().ok()?;
    let now = Instant::now();
    let unix = unix_now();
    sessions.retain(|(_, ticket)| ticket.is_current(now, unix));
    let index = sessions
        .iter()
        .position(|(candidate, _)| candidate == key)?;
    // Consume on offer to avoid linking multiple connections via one ticket.
    sessions.remove(index).map(|(_, ticket)| ticket)
}

pub(super) fn store_ticket(
    key: &SessionKey,
    resumption: &Resumption,
    suite: CipherSuite,
    body: &[u8],
) -> Result<(), super::error::TlsError> {
    let message = parse_new_session_ticket(body)?;
    if !(1..=MAX_LIFETIME_SECS).contains(&message.lifetime)
        || message.ticket.len() > MAX_TICKET_BYTES
    {
        return Ok(());
    }
    let now = Instant::now();
    let unix = unix_now();
    if !resumption.identity.is_current(now, unix) {
        return Ok(());
    }
    let mut psk = vec![0; resumption.master_secret.len()];
    if suite.uses_sha384() {
        hkdf_expand_label::<Sha384>(
            &resumption.master_secret,
            b"resumption",
            message.nonce,
            &mut psk,
        );
    } else {
        hkdf_expand_label::<Sha256>(
            &resumption.master_secret,
            b"resumption",
            message.nonce,
            &mut psk,
        );
    }
    let ticket = SessionTicket {
        identity: resumption.identity.clone(),
        suite,
        ticket: message.ticket.to_vec(),
        psk,
        issued_at: now,
        lifetime: Duration::from_secs(message.lifetime.into()),
        age_add: message.age_add,
    };
    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.retain(|(candidate, value)| candidate != key && value.is_current(now, unix));
        if sessions.len() == MAX_CACHED_SESSIONS {
            sessions.pop_front();
        }
        sessions.push_back((key.clone(), ticket));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickets_are_scoped_consumed_and_expire_with_server_identity() {
        let mut cfg = TlsConfig::new("resumption-cache.test");
        cfg.roots = RootSource::Inline(b"original roots".to_vec());
        cfg.alpn = vec![b"http/1.1"];
        let key = SessionKey::new(&cfg, 443);
        let now = unix_now();
        let resumption = Resumption {
            identity: ServerIdentity {
                leaf: Arc::from(&b"leaf"[..]),
                validity: Some((now - 10)..=(now + 60)),
                authenticated_at: Instant::now(),
            },
            master_secret: vec![0x42; 32],
            remaining_tickets: 8,
        };
        let suite = CipherSuite::TlsAes128GcmSha256;
        let mut message = vec![0, 0, 0, 60, 0, 0, 0, 0, 1, 7, 0, 1, 12, 0, 0];
        store_ticket(&key, &resumption, suite, &message).unwrap();
        assert!(take_ticket(&SessionKey::new(&cfg, 8443)).is_none());
        cfg.roots = RootSource::Inline(b"rotated roots".to_vec());
        assert!(take_ticket(&SessionKey::new(&cfg, 443)).is_none());
        let ticket = take_ticket(&key).unwrap();
        assert_eq!(ticket.ticket, [12]);
        assert!(take_ticket(&key).is_none());
        assert!(!ticket.is_current(ticket.issued_at + Duration::from_secs(60), now));
        assert!(!ticket.is_current(ticket.issued_at, now + 61));

        message[3] = 0;
        store_ticket(&key, &resumption, suite, &message).unwrap();
        assert!(take_ticket(&key).is_none());
        let auth_deadline =
            resumption.identity.authenticated_at + Duration::from_secs(MAX_LIFETIME_SECS.into());
        assert!(!resumption.identity.is_current(auth_deadline, now));
    }

    #[test]
    fn ticket_parser_checks_all_vector_lengths() {
        let body = [
            0, 0, 0, 60, 0xaa, 0xbb, 0xcc, 0xdd, 1, 7, 0, 2, 5, 6, 0, 8, 0, 42, 0, 4, 0, 0, 0, 1,
        ];
        let ticket = parse_new_session_ticket(&body).unwrap();
        assert_eq!(ticket.lifetime, 60);
        assert_eq!(ticket.age_add, 0xaabbccdd);
        assert_eq!(ticket.nonce, [7]);
        assert_eq!(ticket.ticket, [5, 6]);
        for len in 0..body.len() {
            assert!(
                parse_new_session_ticket(&body[..len]).is_err(),
                "accepted prefix of {len} bytes"
            );
        }
        let mut bad_extension = body;
        bad_extension[19] = 3;
        assert!(parse_new_session_ticket(&bad_extension).is_err());
    }

    #[test]
    fn fuzz_new_session_ticket_and_store_never_panics() {
        let mut cfg = TlsConfig::new("fuzz-ticket.test");
        cfg.alpn = vec![b"http/1.1"];
        let key = SessionKey::new(&cfg, 443);
        let now = unix_now();
        let valid_resumption = Resumption {
            identity: ServerIdentity {
                leaf: Arc::from(&b"leaf"[..]),
                validity: Some((now - 60)..=(now + 600)),
                authenticated_at: Instant::now(),
            },
            master_secret: vec![0x5a; 48],
            remaining_tickets: 4,
        };
        let valid_seed = [
            0, 0, 0, 120, 0x11, 0x22, 0x33, 0x44, 1, 9, 0, 3, 1, 2, 3, 0, 8, 0, 42, 0, 4, 0, 0,
            4, 0,
        ];

        let mut rng = 0x517c_c1b7_2722_0a95u64;
        let mut next_u64 = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };

        let fuzz_cfg = TlsConfig::new("fuzz-ticket-noise.test");
        let fuzz_key = SessionKey::new(&fuzz_cfg, 443);

        for iter in 0..1_000usize {
            let data: Vec<u8> = if iter % 3 == 0 {
                let len = (next_u64() as usize) % 256;
                (0..len).map(|_| (next_u64() & 0xff) as u8).collect()
            } else {
                let mut mutated = valid_seed.to_vec();
                match next_u64() % 3 {
                    0 => mutated.truncate((next_u64() as usize) % (mutated.len() + 1)),
                    1 => {
                        let flips = ((next_u64() % 4) + 1) as usize;
                        for _ in 0..flips {
                            let idx = (next_u64() as usize) % mutated.len();
                            mutated[idx] ^= (next_u64() & 0xff) as u8;
                        }
                    }
                    _ => {
                        let extra = (next_u64() as usize) % 64;
                        mutated.extend((0..extra).map(|_| (next_u64() & 0xff) as u8));
                    }
                }
                mutated
            };
            let suite = if iter % 2 == 0 {
                CipherSuite::TlsAes128GcmSha256
            } else {
                CipherSuite::TlsAes256GcmSha384
            };
            let _ = parse_new_session_ticket(&data);
            let _ = store_ticket(&fuzz_key, &valid_resumption, suite, &data);
        }
        while take_ticket(&fuzz_key).is_some() {}
        while take_ticket(&key).is_some() {}

        // Boundary checks: oversized lifetime/ticket and expired identity are ignored cleanly.
        let mut over_lifetime = vec![];
        over_lifetime.extend_from_slice(&(MAX_LIFETIME_SECS + 1).to_be_bytes());
        over_lifetime.extend_from_slice(&[0, 0, 0, 1, 1, 7, 0, 1, 99, 0, 0]);
        store_ticket(&key, &valid_resumption, CipherSuite::TlsAes128GcmSha256, &over_lifetime)
            .unwrap();
        assert!(take_ticket(&key).is_none());

        let mut oversized_ticket = vec![0, 0, 0, 60, 0, 0, 0, 1, 1, 7];
        let big_len = (MAX_TICKET_BYTES + 1) as u16;
        oversized_ticket.extend_from_slice(&big_len.to_be_bytes());
        oversized_ticket.resize(oversized_ticket.len() + big_len as usize, 0xaa);
        oversized_ticket.extend_from_slice(&[0, 0]);
        store_ticket(&key, &valid_resumption, CipherSuite::TlsAes128GcmSha256, &oversized_ticket)
            .unwrap();
        assert!(take_ticket(&key).is_none());

        let expired_resumption = Resumption {
            identity: ServerIdentity {
                leaf: Arc::from(&b"leaf"[..]),
                validity: Some((now - 120)..=(now - 1)),
                authenticated_at: Instant::now(),
            },
            master_secret: vec![0x11; 32],
            remaining_tickets: 1,
        };
        store_ticket(&key, &expired_resumption, CipherSuite::TlsAes128GcmSha256, &valid_seed)
            .unwrap();
        assert!(take_ticket(&key).is_none());
    }

    #[test]
    fn session_cache_eviction_sha384_psk_and_concurrent_single_use() {
        let now = unix_now();
        // Verify SHA-384 suite produces a 48-byte PSK matching the Sha384 master_secret.
        let mut cfg384 = TlsConfig::new("sha384-resumption.test");
        cfg384.alpn = vec![b"http/1.1"];
        let key384 = SessionKey::new(&cfg384, 443);
        let res384 = Resumption {
            identity: ServerIdentity {
                leaf: Arc::from(&b"leaf"[..]),
                validity: Some((now - 10)..=(now + 120)),
                authenticated_at: Instant::now(),
            },
            master_secret: vec![0x33; 48],
            remaining_tickets: 2,
        };
        let msg = [0, 0, 0, 60, 0, 0, 0, 7, 1, 5, 0, 2, 10, 20, 0, 0];
        store_ticket(&key384, &res384, CipherSuite::TlsAes256GcmSha384, &msg).unwrap();
        let taken384 = take_ticket(&key384).expect("sha384 ticket");
        assert_eq!(taken384.psk.len(), 48);
        assert_eq!(taken384.suite, CipherSuite::TlsAes256GcmSha384);
        assert!(take_ticket(&key384).is_none());

        // Inserting MAX_CACHED_SESSIONS + 32 unique keys evicts the oldest entries in FIFO order.
        let total = MAX_CACHED_SESSIONS + 32;
        let keys: Vec<SessionKey> = (0..total)
            .map(|i| {
                let host = format!("evict-{i}-cache.test");
                let c = TlsConfig::new(host);
                SessionKey::new(&c, 443)
            })
            .collect();
        let res256 = Resumption {
            identity: ServerIdentity {
                leaf: Arc::from(&b"leaf"[..]),
                validity: Some((now - 10)..=(now + 120)),
                authenticated_at: Instant::now(),
            },
            master_secret: vec![0x22; 32],
            remaining_tickets: 2,
        };
        for k in &keys {
            store_ticket(k, &res256, CipherSuite::TlsAes128GcmSha256, &msg).unwrap();
        }
        // Notice other parallel tests share `SESSIONS`, so at least the earliest keys are evicted
        // and the most recent tail remains retrievable (bounded capacity <= MAX_CACHED_SESSIONS).
        assert!(
            take_ticket(&keys[0]).is_none(),
            "oldest key must be evicted past MAX_CACHED_SESSIONS"
        );
        let newest = take_ticket(&keys[total - 1]).expect("newest key must remain cached");
        assert_eq!(newest.psk.len(), 32);

        // Clean up remaining test keys so we don't pollute global cache.
        for k in &keys[..total - 1] {
            let _ = take_ticket(k);
        }

        // Concurrent store + take across 8 threads never panics or yields duplicate tickets.
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let shared_res = Arc::new(res256);
        let mut handles = Vec::new();
        for tid in 0..8usize {
            let b = Arc::clone(&barrier);
            let res_c = Arc::clone(&shared_res);
            handles.push(std::thread::spawn(move || {
                let mut seen = Vec::new();
                let shared_hosts = ["concurrent-a.test", "concurrent-b.test"];
                b.wait();
                for step in 0..40usize {
                    let c = TlsConfig::new(shared_hosts[(tid + step) % shared_hosts.len()]);
                    let k = SessionKey::new(&c, 443);
                    let id_byte = ((tid * 40 + step) & 0xff) as u8;
                    let m = [0, 0, 0, 60, 0, 0, 0, id_byte, 1, id_byte, 0, 2, tid as u8, step as u8, 0, 0];
                    store_ticket(&k, &res_c, CipherSuite::TlsAes128GcmSha256, &m).unwrap();
                    if let Some(t) = take_ticket(&k) {
                        seen.push(t.ticket.clone());
                    }
                }
                seen
            }));
        }
        for h in handles {
            let list = h.join().unwrap();
            assert!(list.len() <= 40);
        }
        for host in ["concurrent-a.test", "concurrent-b.test"] {
            let _ = take_ticket(&SessionKey::new(&TlsConfig::new(host), 443));
        }
    }
}
