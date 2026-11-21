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
}
