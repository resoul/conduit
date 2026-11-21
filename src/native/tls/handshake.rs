//! TLS handshake message encoding, decoding, and transcript hash helpers.

use std::net::IpAddr;
use std::str::FromStr;

use crate::native::tls::crypto::sha2::{Sha256, Sha384};
use crate::native::tls::error::TlsError;

pub const MAX_HANDSHAKE_MSG: usize = 64 * 1024;

// Handshake message types
pub const HT_CLIENT_HELLO: u8 = 1;
pub const HT_SERVER_HELLO: u8 = 2;
pub const HT_NEW_SESSION_TICKET: u8 = 4;
pub const HT_ENCRYPTED_EXTENSIONS: u8 = 8;
pub const HT_CERTIFICATE: u8 = 11;
pub const HT_SERVER_KEY_EXCHANGE: u8 = 12;
pub const HT_CERTIFICATE_REQUEST: u8 = 13;
pub const HT_SERVER_HELLO_DONE: u8 = 14;
pub const HT_CERTIFICATE_VERIFY: u8 = 15;
pub const HT_CLIENT_KEY_EXCHANGE: u8 = 16;
pub const HT_FINISHED: u8 = 20;
pub const HT_KEY_UPDATE: u8 = 24;
pub const HT_MESSAGE_HASH: u8 = 254;

// Named groups
pub const GROUP_SECP256R1: u16 = 0x0017;
pub const GROUP_X25519: u16 = 0x001d;

// Signature schemes
pub const SIG_RSA_PKCS1_SHA256: u16 = 0x0401;
pub const SIG_RSA_PKCS1_SHA384: u16 = 0x0501;
pub const SIG_RSA_PKCS1_SHA512: u16 = 0x0601;
pub const SIG_ECDSA_SECP256R1_SHA256: u16 = 0x0403;
pub const SIG_ECDSA_SECP384R1_SHA384: u16 = 0x0503;
pub const SIG_RSA_PSS_RSAE_SHA256: u16 = 0x0804;
pub const SIG_RSA_PSS_RSAE_SHA384: u16 = 0x0805;
pub const SIG_RSA_PSS_RSAE_SHA512: u16 = 0x0806;

// Extensions
pub const EXT_SERVER_NAME: u16 = 0x0000;
pub const EXT_SUPPORTED_GROUPS: u16 = 0x000a;
pub const EXT_EC_POINT_FORMATS: u16 = 0x000b;
pub const EXT_SIGNATURE_ALGORITHMS: u16 = 0x000d;
pub const EXT_ALPN: u16 = 0x0010;
pub const EXT_EXTENDED_MASTER_SECRET: u16 = 0x0017;
const EXT_PRE_SHARED_KEY: u16 = 0x0029;
const EXT_EARLY_DATA: u16 = 0x002a;
pub const EXT_SUPPORTED_VERSIONS: u16 = 0x002b;
const EXT_PSK_KEY_EXCHANGE_MODES: u16 = 0x002d;
pub const EXT_KEY_SHARE: u16 = 0x0033;
pub const EXT_RENEGOTIATION_INFO: u16 = 0xff01;

/// Special ServerHello.random indicating HelloRetryRequest (RFC 8446 §4.1.3).
pub const HELLO_RETRY_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    // TLS 1.3
    TlsAes128GcmSha256 = 0x1301,
    TlsAes256GcmSha384 = 0x1302,
    TlsChaCha20Poly1305Sha256 = 0x1303,
    // TLS 1.2 (ECDHE + AEAD)
    TlsEcdheEcdsaWithAes128GcmSha256 = 0xc02b,
    TlsEcdheEcdsaWithAes256GcmSha384 = 0xc02c,
    TlsEcdheRsaWithAes128GcmSha256 = 0xc02f,
    TlsEcdheRsaWithAes256GcmSha384 = 0xc030,
    TlsEcdheRsaWithChaCha20Poly1305Sha256 = 0xcca8,
    TlsEcdheEcdsaWithChaCha20Poly1305Sha256 = 0xcca9,
}

impl CipherSuite {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x1301 => Some(Self::TlsAes128GcmSha256),
            0x1302 => Some(Self::TlsAes256GcmSha384),
            0x1303 => Some(Self::TlsChaCha20Poly1305Sha256),
            0xc02b => Some(Self::TlsEcdheEcdsaWithAes128GcmSha256),
            0xc02c => Some(Self::TlsEcdheEcdsaWithAes256GcmSha384),
            0xc02f => Some(Self::TlsEcdheRsaWithAes128GcmSha256),
            0xc030 => Some(Self::TlsEcdheRsaWithAes256GcmSha384),
            0xcca8 => Some(Self::TlsEcdheRsaWithChaCha20Poly1305Sha256),
            0xcca9 => Some(Self::TlsEcdheEcdsaWithChaCha20Poly1305Sha256),
            _ => None,
        }
    }

    pub fn is_tls13(self) -> bool {
        matches!(
            self,
            Self::TlsAes128GcmSha256 | Self::TlsAes256GcmSha384 | Self::TlsChaCha20Poly1305Sha256
        )
    }

    pub fn uses_sha384(self) -> bool {
        matches!(
            self,
            Self::TlsAes256GcmSha384
                | Self::TlsEcdheEcdsaWithAes256GcmSha384
                | Self::TlsEcdheRsaWithAes256GcmSha384
        )
    }

    pub fn key_len(self) -> usize {
        match self {
            Self::TlsAes128GcmSha256
            | Self::TlsEcdheEcdsaWithAes128GcmSha256
            | Self::TlsEcdheRsaWithAes128GcmSha256 => 16,
            _ => 32,
        }
    }

    pub fn is_chacha(self) -> bool {
        matches!(
            self,
            Self::TlsChaCha20Poly1305Sha256
                | Self::TlsEcdheRsaWithChaCha20Poly1305Sha256
                | Self::TlsEcdheEcdsaWithChaCha20Poly1305Sha256
        )
    }

    pub fn is_ecdsa(self) -> bool {
        matches!(
            self,
            Self::TlsEcdheEcdsaWithAes128GcmSha256
                | Self::TlsEcdheEcdsaWithAes256GcmSha384
                | Self::TlsEcdheEcdsaWithChaCha20Poly1305Sha256
        )
    }
}

#[derive(Clone)]
pub enum Transcript {
    Both {
        sha256: Sha256,
        sha384: Sha384,
    },
    Sha256(Sha256),
    Sha384(Sha384),
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self::Both {
            sha256: Sha256::new(),
            sha384: Sha384::new(),
        }
    }

    pub fn update(&mut self, msg: &[u8]) {
        match self {
            Self::Both { sha256, sha384 } => {
                sha256.update(msg);
                sha384.update(msg);
            }
            Self::Sha256(h) => h.update(msg),
            Self::Sha384(h) => h.update(msg),
        }
    }

    pub fn lock_to_sha384(&mut self, use_sha384: bool) {
        if let Self::Both { sha256, sha384 } = self {
            *self = if use_sha384 {
                Self::Sha384(sha384.clone())
            } else {
                Self::Sha256(sha256.clone())
            };
        }
    }

    /// Replace transcript with `message_hash` for HelloRetryRequest (RFC 8446 §4.4.1).
    pub fn replace_with_message_hash(&mut self) {
        let h = self.current_hash();
        let mut hdr = [HT_MESSAGE_HASH, 0, 0, h.len() as u8];
        let _ = &mut hdr;
        match self {
            Self::Sha256(s) => {
                let mut ns = Sha256::new();
                ns.update(&hdr);
                ns.update(&h);
                *s = ns;
            }
            Self::Sha384(s) => {
                let mut ns = Sha384::new();
                ns.update(&hdr);
                ns.update(&h);
                *s = ns;
            }
            Self::Both { .. } => unreachable!("must lock transcript before HRR"),
        }
    }

    pub fn current_hash(&self) -> Vec<u8> {
        match self {
            Self::Sha256(h) => h.clone().finalize().to_vec(),
            Self::Sha384(h) => h.clone().finalize().to_vec(),
            Self::Both { sha256, .. } => sha256.clone().finalize().to_vec(),
        }
    }
}

pub fn wrap_handshake(msg_type: u8, body: &[u8]) -> Vec<u8> {
    let len = body.len();
    let mut out = Vec::with_capacity(4 + len);
    out.push(msg_type);
    out.push(((len >> 16) & 0xff) as u8);
    out.push(((len >> 8) & 0xff) as u8);
    out.push((len & 0xff) as u8);
    out.extend_from_slice(body);
    out
}

pub fn build_client_hello(
    random: &[u8; 32],
    legacy_session_id: &[u8],
    server_name: &str,
    alpn: &[&[u8]],
    key_share_group: u16,
    key_share_pub: &[u8],
    allow_tls12: bool,
) -> Vec<u8> {
    build_client_hello_inner(
        random,
        legacy_session_id,
        server_name,
        alpn,
        key_share_group,
        key_share_pub,
        allow_tls12,
        false,
        None,
    )
}

pub(super) struct PskOffer<'a> {
    pub(super) identity: &'a [u8],
    pub(super) obfuscated_age: u32,
    pub(super) binder_len: usize,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_client_hello_inner(
    random: &[u8; 32],
    legacy_session_id: &[u8],
    server_name: &str,
    alpn: &[&[u8]],
    key_share_group: u16,
    key_share_pub: &[u8],
    allow_tls12: bool,
    enable_resumption: bool,
    psk: Option<PskOffer<'_>>,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(256);
    body.extend_from_slice(&0x0303u16.to_be_bytes()); // legacy_version = TLS 1.2
    body.extend_from_slice(random);
    body.push(legacy_session_id.len() as u8);
    body.extend_from_slice(legacy_session_id);

    // Cipher suites
    let mut suites = vec![
        CipherSuite::TlsAes128GcmSha256 as u16,
        CipherSuite::TlsAes256GcmSha384 as u16,
        CipherSuite::TlsChaCha20Poly1305Sha256 as u16,
    ];
    if allow_tls12 {
        suites.extend_from_slice(&[
            CipherSuite::TlsEcdheEcdsaWithAes128GcmSha256 as u16,
            CipherSuite::TlsEcdheEcdsaWithAes256GcmSha384 as u16,
            CipherSuite::TlsEcdheRsaWithAes128GcmSha256 as u16,
            CipherSuite::TlsEcdheRsaWithAes256GcmSha384 as u16,
            CipherSuite::TlsEcdheEcdsaWithChaCha20Poly1305Sha256 as u16,
            CipherSuite::TlsEcdheRsaWithChaCha20Poly1305Sha256 as u16,
        ]);
    }
    body.extend_from_slice(&((suites.len() * 2) as u16).to_be_bytes());
    for s in suites {
        body.extend_from_slice(&s.to_be_bytes());
    }

    // legacy_compression_methods: [0] (null)
    body.push(1);
    body.push(0);

    // Extensions
    let mut exts = Vec::with_capacity(160);

    // 1. server_name (only for hostnames, never IP literals per RFC 6066)
    if !server_name.is_empty() && IpAddr::from_str(server_name).is_err() {
        let name_bytes = server_name.as_bytes();
        let entry_len = 1 + 2 + name_bytes.len();
        let list_len = entry_len;
        push_ext(&mut exts, EXT_SERVER_NAME, |b| {
            b.extend_from_slice(&(list_len as u16).to_be_bytes());
            b.push(0); // host_name
            b.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
            b.extend_from_slice(name_bytes);
        });
    }

    // 2. supported_versions
    push_ext(&mut exts, EXT_SUPPORTED_VERSIONS, |b| {
        if allow_tls12 {
            b.push(4);
            b.extend_from_slice(&0x0304u16.to_be_bytes());
            b.extend_from_slice(&0x0303u16.to_be_bytes());
        } else {
            b.push(2);
            b.extend_from_slice(&0x0304u16.to_be_bytes());
        }
    });

    // 3. supported_groups
    push_ext(&mut exts, EXT_SUPPORTED_GROUPS, |b| {
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(&GROUP_X25519.to_be_bytes());
        b.extend_from_slice(&GROUP_SECP256R1.to_be_bytes());
    });

    // 4. signature_algorithms
    let sig_algs = [
        SIG_ECDSA_SECP256R1_SHA256,
        SIG_ECDSA_SECP384R1_SHA384,
        SIG_RSA_PSS_RSAE_SHA256,
        SIG_RSA_PSS_RSAE_SHA384,
        SIG_RSA_PSS_RSAE_SHA512,
        SIG_RSA_PKCS1_SHA256,
        SIG_RSA_PKCS1_SHA384,
        SIG_RSA_PKCS1_SHA512,
    ];
    push_ext(&mut exts, EXT_SIGNATURE_ALGORITHMS, |b| {
        b.extend_from_slice(&((sig_algs.len() * 2) as u16).to_be_bytes());
        for s in sig_algs {
            b.extend_from_slice(&s.to_be_bytes());
        }
    });

    // 5. key_share
    push_ext(&mut exts, EXT_KEY_SHARE, |b| {
        let entry_len = 2 + 2 + key_share_pub.len();
        b.extend_from_slice(&(entry_len as u16).to_be_bytes());
        b.extend_from_slice(&key_share_group.to_be_bytes());
        b.extend_from_slice(&(key_share_pub.len() as u16).to_be_bytes());
        b.extend_from_slice(key_share_pub);
    });

    // 6. ALPN
    if !alpn.is_empty() {
        push_ext(&mut exts, EXT_ALPN, |b| {
            let total: usize = alpn.iter().map(|p| 1 + p.len()).sum();
            b.extend_from_slice(&(total as u16).to_be_bytes());
            for proto in alpn {
                b.push(proto.len() as u8);
                b.extend_from_slice(proto);
            }
        });
    }

    // 7. TLS 1.2 safety extensions (EMS, ec_point_formats, renegotiation_info)
    if allow_tls12 {
        push_ext(&mut exts, EXT_EC_POINT_FORMATS, |b| {
            b.push(1);
            b.push(0); // uncompressed
        });
        push_ext(&mut exts, EXT_EXTENDED_MASTER_SECRET, |_| {});
        push_ext(&mut exts, EXT_RENEGOTIATION_INFO, |b| {
            b.push(0);
        });
    }

    if enable_resumption {
        // Offer psk_dhe_ke. Every resumed handshake uses a fresh key share.
        push_ext(&mut exts, EXT_PSK_KEY_EXCHANGE_MODES, |b| {
            b.extend_from_slice(&[1, 1])
        });
    }
    if let Some(psk) = psk {
        // pre_shared_key goes last. The final ClientHello lengths already
        // include the binder when the caller hashes the prefix.
        push_ext(&mut exts, EXT_PRE_SHARED_KEY, |b| {
            b.extend_from_slice(&((psk.identity.len() + 6) as u16).to_be_bytes());
            b.extend_from_slice(&(psk.identity.len() as u16).to_be_bytes());
            b.extend_from_slice(psk.identity);
            b.extend_from_slice(&psk.obfuscated_age.to_be_bytes());
            b.extend_from_slice(&((psk.binder_len + 1) as u16).to_be_bytes());
            b.push(psk.binder_len as u8);
            b.resize(b.len() + psk.binder_len, 0);
        });
    }

    body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    body.extend_from_slice(&exts);

    wrap_handshake(HT_CLIENT_HELLO, &body)
}

fn push_ext(out: &mut Vec<u8>, ext_type: u16, fill: impl FnOnce(&mut Vec<u8>)) {
    out.extend_from_slice(&ext_type.to_be_bytes());
    let len_pos = out.len();
    out.extend_from_slice(&[0, 0]);
    fill(out);
    let len = out.len() - len_pos - 2;
    out[len_pos..len_pos + 2].copy_from_slice(&(len as u16).to_be_bytes());
}

pub struct ServerHello {
    pub random: [u8; 32],
    pub session_id: Vec<u8>,
    pub cipher_suite: CipherSuite,
    pub is_hrr: bool,
    pub selected_version: u16,
    pub key_share: Option<(u16, Vec<u8>)>,
    pub hrr_selected_group: Option<u16>,
    pub extended_master_secret: bool,
    pub renegotiation_info_empty: bool,
}

pub fn parse_server_hello(body: &[u8]) -> Result<ServerHello, TlsError> {
    parse_server_hello_with_psk(body).map(|(hello, _)| hello)
}

pub(super) fn parse_server_hello_with_psk(
    body: &[u8],
) -> Result<(ServerHello, Option<u16>), TlsError> {
    if body.len() < 38 {
        return Err(TlsError::DecodeError("ServerHello too short"));
    }
    let legacy_ver = u16::from_be_bytes([body[0], body[1]]);
    if legacy_ver != 0x0303 {
        return Err(TlsError::ProtocolError(format!(
            "unexpected ServerHello legacy_version 0x{legacy_ver:04x}"
        )));
    }
    let random: [u8; 32] = body[2..34].try_into().unwrap();
    let is_hrr = random == HELLO_RETRY_RANDOM;
    let sid_len = body[34] as usize;
    if sid_len > 32 || 35 + sid_len + 3 > body.len() {
        return Err(TlsError::DecodeError("invalid ServerHello session_id"));
    }
    let session_id = body[35..35 + sid_len].to_vec();
    let mut pos = 35 + sid_len;
    let suite_raw = u16::from_be_bytes([body[pos], body[pos + 1]]);
    let cipher_suite = CipherSuite::from_u16(suite_raw).ok_or_else(|| {
        TlsError::ProtocolError(format!("unsupported cipher suite 0x{suite_raw:04x}"))
    })?;
    let comp = body[pos + 2];
    if comp != 0 {
        return Err(TlsError::ProtocolError("non-null compression method".to_string()));
    }
    pos += 3;

    let mut selected_version = 0x0303u16;
    let mut key_share = None;
    let mut hrr_selected_group = None;
    let mut extended_master_secret = false;
    let mut renegotiation_info_empty = false;
    let mut selected_psk = None;

    if pos < body.len() {
        if pos + 2 > body.len() {
            return Err(TlsError::DecodeError("truncated ServerHello extensions"));
        }
        let exts_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2;
        if pos + exts_len != body.len() {
            return Err(TlsError::DecodeError("invalid ServerHello extensions length"));
        }
        let end = pos + exts_len;
        while pos < end {
            if pos + 4 > end {
                return Err(TlsError::DecodeError("truncated extension header"));
            }
            let ext_type = u16::from_be_bytes([body[pos], body[pos + 1]]);
            let ext_len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
            pos += 4;
            if pos + ext_len > end {
                return Err(TlsError::DecodeError("truncated extension body"));
            }
            let ext = &body[pos..pos + ext_len];
            pos += ext_len;

            match ext_type {
                EXT_PRE_SHARED_KEY => {
                    if is_hrr || selected_psk.is_some() || ext.len() != 2 {
                        return Err(TlsError::DecodeError("invalid ServerHello pre_shared_key"));
                    }
                    selected_psk = Some(u16::from_be_bytes([ext[0], ext[1]]));
                }
                EXT_SUPPORTED_VERSIONS => {
                    if ext.len() != 2 {
                        return Err(TlsError::DecodeError("invalid supported_versions"));
                    }
                    selected_version = u16::from_be_bytes([ext[0], ext[1]]);
                }
                EXT_KEY_SHARE => {
                    if is_hrr {
                        if ext.len() != 2 {
                            return Err(TlsError::DecodeError("invalid HRR key_share"));
                        }
                        hrr_selected_group = Some(u16::from_be_bytes([ext[0], ext[1]]));
                    } else {
                        if ext.len() < 4 {
                            return Err(TlsError::DecodeError("invalid ServerHello key_share"));
                        }
                        let group = u16::from_be_bytes([ext[0], ext[1]]);
                        let klen = u16::from_be_bytes([ext[2], ext[3]]) as usize;
                        if 4 + klen != ext.len() {
                            return Err(TlsError::DecodeError("invalid key_share key length"));
                        }
                        key_share = Some((group, ext[4..].to_vec()));
                    }
                }
                EXT_EXTENDED_MASTER_SECRET => {
                    if !ext.is_empty() {
                        return Err(TlsError::DecodeError("non-empty extended_master_secret"));
                    }
                    extended_master_secret = true;
                }
                EXT_RENEGOTIATION_INFO => {
                    if ext == [0] {
                        renegotiation_info_empty = true;
                    } else {
                        return Err(TlsError::ProtocolError("non-empty renegotiation_info".to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    if selected_psk.is_some() && selected_version != 0x0304 {
        return Err(TlsError::ProtocolError(
            "pre_shared_key outside TLS 1.3 ServerHello".to_string(),
        ));
    }
    let hello = ServerHello {
        random,
        session_id,
        cipher_suite,
        is_hrr,
        selected_version,
        key_share,
        hrr_selected_group,
        extended_master_secret,
        renegotiation_info_empty,
    };
    Ok((hello, selected_psk))
}

pub fn parse_encrypted_extensions(body: &[u8]) -> Result<Option<Vec<u8>>, TlsError> {
    if body.len() < 2 {
        return Err(TlsError::DecodeError("EncryptedExtensions too short"));
    }
    let len = u16::from_be_bytes([body[0], body[1]]) as usize;
    if 2 + len != body.len() {
        return Err(TlsError::DecodeError("invalid EncryptedExtensions length"));
    }
    let mut pos = 2;
    let mut alpn = None;
    while pos < body.len() {
        if pos + 4 > body.len() {
            return Err(TlsError::DecodeError("truncated EE extension"));
        }
        let ext_type = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let ext_len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_len > body.len() {
            return Err(TlsError::DecodeError("truncated EE extension body"));
        }
        let ext = &body[pos..pos + ext_len];
        pos += ext_len;
        if ext_type == EXT_EARLY_DATA {
            return Err(TlsError::ProtocolError(
                "server offered unrequested early_data".to_string(),
            ));
        }
        if ext_type == EXT_ALPN {
            if alpn.is_some() || ext.len() < 4 {
                return Err(TlsError::DecodeError("invalid ALPN extension"));
            }
            let list_len = u16::from_be_bytes([ext[0], ext[1]]) as usize;
            let plen = ext[2] as usize;
            if 2 + list_len != ext.len() || 3 + plen != ext.len() {
                return Err(TlsError::DecodeError("invalid ALPN server selection"));
            }
            alpn = Some(ext[3..3 + plen].to_vec());
        }
    }
    Ok(alpn)
}

pub(super) struct NewSessionTicket<'a> {
    pub lifetime: u32,
    pub age_add: u32,
    pub nonce: &'a [u8],
    pub ticket: &'a [u8],
}

pub(super) fn parse_new_session_ticket(body: &[u8]) -> Result<NewSessionTicket<'_>, TlsError> {
    let invalid = TlsError::DecodeError("invalid NewSessionTicket");
    let (header, rest) = body.split_first_chunk::<9>().ok_or(invalid)?;
    let lifetime = u32::from_be_bytes(header[..4].try_into().unwrap());
    let age_add = u32::from_be_bytes(header[4..8].try_into().unwrap());
    let (nonce, rest) = rest
        .split_at_checked(header[8] as usize)
        .ok_or(TlsError::DecodeError("truncated ticket nonce"))?;
    let (length, rest) = rest
        .split_first_chunk::<2>()
        .ok_or(TlsError::DecodeError("missing ticket length"))?;
    let (ticket, rest) = rest
        .split_at_checked(u16::from_be_bytes(*length) as usize)
        .ok_or(TlsError::DecodeError("truncated ticket"))?;
    let (length, extensions) = rest
        .split_first_chunk::<2>()
        .ok_or(TlsError::DecodeError("missing ticket extensions"))?;
    if ticket.is_empty() || extensions.len() != u16::from_be_bytes(*length) as usize {
        return Err(TlsError::DecodeError("invalid ticket or extensions length"));
    }
    let mut body = extensions;
    let mut saw_early_data = false;
    while !body.is_empty() {
        let (header, rest) = body
            .split_first_chunk::<4>()
            .ok_or(TlsError::DecodeError("truncated ticket extension"))?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let len = u16::from_be_bytes([header[2], header[3]]) as usize;
        let (_, rest) = rest
            .split_at_checked(len)
            .ok_or(TlsError::DecodeError("truncated ticket extension body"))?;
        if kind == EXT_EARLY_DATA {
            if len != 4 || saw_early_data {
                return Err(TlsError::DecodeError("invalid ticket early_data extension"));
            }
            saw_early_data = true;
        }
        body = rest;
    }
    Ok(NewSessionTicket {
        lifetime,
        age_add,
        nonce,
        ticket,
    })
}

pub fn parse_tls13_certificate(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    if body.len() < 4 {
        return Err(TlsError::DecodeError("TLS 1.3 Certificate too short"));
    }
    let ctx_len = body[0] as usize;
    let mut pos = 1 + ctx_len;
    if pos + 3 > body.len() {
        return Err(TlsError::DecodeError("truncated TLS 1.3 Certificate list"));
    }
    let list_len = ((body[pos] as usize) << 16)
        | ((body[pos + 1] as usize) << 8)
        | (body[pos + 2] as usize);
    pos += 3;
    if pos + list_len != body.len() {
        return Err(TlsError::DecodeError("invalid TLS 1.3 Certificate list length"));
    }
    let mut certs = Vec::new();
    while pos < body.len() {
        if certs.len() >= 10 {
            return Err(TlsError::CertificateError("certificate chain exceeds 10 certs".to_string()));
        }
        if pos + 3 > body.len() {
            return Err(TlsError::DecodeError("truncated CertificateEntry length"));
        }
        let cert_len = ((body[pos] as usize) << 16)
            | ((body[pos + 1] as usize) << 8)
            | (body[pos + 2] as usize);
        pos += 3;
        if pos + cert_len + 2 > body.len() {
            return Err(TlsError::DecodeError("truncated CertificateEntry data"));
        }
        certs.push(body[pos..pos + cert_len].to_vec());
        pos += cert_len;
        let ext_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2;
        if pos + ext_len > body.len() {
            return Err(TlsError::DecodeError("truncated CertificateEntry extensions"));
        }
        pos += ext_len;
    }
    Ok(certs)
}

pub fn parse_certificate_verify(body: &[u8]) -> Result<(u16, &[u8]), TlsError> {
    if body.len() < 4 {
        return Err(TlsError::DecodeError("CertificateVerify too short"));
    }
    let scheme = u16::from_be_bytes([body[0], body[1]]);
    let sig_len = u16::from_be_bytes([body[2], body[3]]) as usize;
    if 4 + sig_len != body.len() {
        return Err(TlsError::DecodeError("invalid CertificateVerify signature length"));
    }
    Ok((scheme, &body[4..]))
}

/// Reader for a stream of handshake messages (which may span multiple records or share one record).
pub struct HandshakeReader {
    buf: Vec<u8>,
}

impl Default for HandshakeReader {
    fn default() -> Self {
        Self::new()
    }
}

impl HandshakeReader {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
        }
    }

    pub fn push_bytes(&mut self, data: &[u8]) -> Result<(), TlsError> {
        if self.buf.len() + data.len() > MAX_HANDSHAKE_MSG * 2 {
            return Err(TlsError::ProtocolError("handshake buffer exceeded limit".to_string()));
        }
        self.buf.extend_from_slice(data);
        Ok(())
    }

    pub fn next_message(&mut self) -> Result<Option<(u8, Vec<u8>)>, TlsError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let msg_type = self.buf[0];
        let len = ((self.buf[1] as usize) << 16)
            | ((self.buf[2] as usize) << 8)
            | (self.buf[3] as usize);
        if len > MAX_HANDSHAKE_MSG {
            return Err(TlsError::ProtocolError(format!(
                "handshake message size {len} exceeds 64 KiB limit"
            )));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let raw_msg = self.buf[..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some((msg_type, raw_msg)))
    }
}
