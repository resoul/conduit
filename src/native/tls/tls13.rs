//! TLS 1.3 (RFC 8446) client state machine and key schedule.

use std::io::{Read, Write};

use crate::native::crypto::{constant_time_eq, fill_random_bytes};
use crate::native::tls::crypto::Zeroize;
use crate::native::tls::crypto::chacha_poly::ChaCha20Poly1305;
use crate::native::tls::crypto::gcm::AesGcm;
use crate::native::tls::crypto::hmac_hkdf::{Hmac, derive_secret, hkdf_expand_label, hkdf_extract};
use crate::native::tls::crypto::p256::{ecdsa_p256_verify_der, p256_ecdh, p256_public_key};
use crate::native::tls::crypto::p384::ecdsa_p384_verify_der;
use crate::native::tls::crypto::rsa::{RsaHashAlg, RsaPublicKey};
use crate::native::tls::crypto::sha2::{Digest, Sha256, Sha384};
use crate::native::tls::crypto::x25519::{x25519_public, x25519_shared};
use crate::native::tls::error::{AlertDescription, TlsError};
use crate::native::tls::handshake::{
    CipherSuite, GROUP_SECP256R1, GROUP_X25519, HT_CERTIFICATE, HT_CERTIFICATE_REQUEST,
    HT_CERTIFICATE_VERIFY, HT_ENCRYPTED_EXTENSIONS, HT_FINISHED, HT_SERVER_HELLO, HandshakeReader,
    SIG_ECDSA_SECP256R1_SHA256, SIG_ECDSA_SECP384R1_SHA384, SIG_RSA_PSS_RSAE_SHA256,
    SIG_RSA_PSS_RSAE_SHA384, SIG_RSA_PSS_RSAE_SHA512, ServerHello, Transcript, build_client_hello,
    parse_certificate_verify, parse_encrypted_extensions, parse_server_hello,
    parse_tls13_certificate, wrap_handshake,
};
use crate::native::tls::record::{
    AeadCipher, CT_ALERT, CT_CHANGE_CIPHER_SPEC, CT_HANDSHAKE, CipherState, RecordLayer,
};
use crate::native::tls::roots::RootStore;
use crate::native::tls::verify::{unix_now, verify_server_cert};
use crate::native::tls::x509::{SubjectPublicKey, parse_x509};
use crate::native::tls::{TlsConfig, VerifyMode};

pub enum EphemeralKey {
    X25519([u8; 32]),
    P256([u8; 32]),
}

impl Drop for EphemeralKey {
    fn drop(&mut self) {
        match self {
            Self::X25519(k) | Self::P256(k) => k.zeroize(),
        }
    }
}

impl EphemeralKey {
    pub fn generate(group: u16) -> Result<(Self, Vec<u8>), TlsError> {
        let mut priv_bytes = [0u8; 32];
        fill_random_bytes(&mut priv_bytes).map_err(|_| TlsError::CryptoError("CSPRNG failure"))?;
        match group {
            GROUP_X25519 => {
                let pub_key = x25519_public(&priv_bytes);
                Ok((Self::X25519(priv_bytes), pub_key.to_vec()))
            }
            GROUP_SECP256R1 => {
                for _ in 0..8 {
                    if let Some(pub_key) = p256_public_key(&priv_bytes) {
                        return Ok((Self::P256(priv_bytes), pub_key.to_vec()));
                    }
                    fill_random_bytes(&mut priv_bytes)
                        .map_err(|_| TlsError::CryptoError("CSPRNG failure"))?;
                }
                Err(TlsError::CryptoError("failed to generate P-256 keypair"))
            }
            _ => Err(TlsError::ProtocolError(format!(
                "unsupported key_share group 0x{group:04x}"
            ))),
        }
    }

    pub fn ecdh(&self, group: u16, peer_pub: &[u8]) -> Result<Vec<u8>, TlsError> {
        match (self, group) {
            (Self::X25519(priv_k), GROUP_X25519) => {
                let peer: [u8; 32] = peer_pub
                    .try_into()
                    .map_err(|_| TlsError::DecodeError("invalid X25519 peer key length"))?;
                x25519_shared(priv_k, &peer)
                    .map(|s| s.to_vec())
                    .ok_or(TlsError::CryptoError("X25519 shared secret is all-zero"))
            }
            (Self::P256(priv_k), GROUP_SECP256R1) => p256_ecdh(priv_k, peer_pub)
                .map(|s| s.to_vec())
                .ok_or(TlsError::CryptoError("invalid P-256 peer public key")),
            _ => Err(TlsError::ProtocolError("key_share group mismatch".to_string())),
        }
    }
}

pub struct Tls13Secrets {
    pub suite: CipherSuite,
    pub client_app_secret: Vec<u8>,
    pub server_app_secret: Vec<u8>,
}

impl Drop for Tls13Secrets {
    fn drop(&mut self) {
        self.client_app_secret.zeroize();
        self.server_app_secret.zeroize();
    }
}

pub struct HandshakeOutcome {
    pub record: RecordLayer,
    pub alpn: Option<Vec<u8>>,
    pub leaf_cert_der: Option<Vec<u8>>,
    pub tls13_secrets: Option<Tls13Secrets>,
}

pub fn make_tls13_cipher_state(suite: CipherSuite, traffic_secret: &[u8]) -> CipherState {
    let key_len = suite.key_len();
    let mut key = vec![0u8; key_len];
    let mut iv = [0u8; 12];
    if suite.uses_sha384() {
        hkdf_expand_label::<Sha384>(traffic_secret, b"key", &[], &mut key);
        hkdf_expand_label::<Sha384>(traffic_secret, b"iv", &[], &mut iv);
    } else {
        hkdf_expand_label::<Sha256>(traffic_secret, b"key", &[], &mut key);
        hkdf_expand_label::<Sha256>(traffic_secret, b"iv", &[], &mut iv);
    }

    let cipher = if suite.is_chacha() {
        let mut k32 = [0u8; 32];
        k32.copy_from_slice(&key);
        AeadCipher::ChaCha20Poly1305(ChaCha20Poly1305::new(k32))
    } else {
        AeadCipher::AesGcm(Box::new(AesGcm::new(&key).expect("valid AES key length")))
    };
    key.zeroize();

    CipherState::Tls13 {
        cipher,
        iv,
        seq: 0,
    }
}

pub fn derive_next_app_secret(suite: CipherSuite, current: &[u8]) -> Vec<u8> {
    if suite.uses_sha384() {
        let mut next = vec![0u8; 48];
        hkdf_expand_label::<Sha384>(current, b"traffic upd", &[], &mut next);
        next
    } else {
        let mut next = vec![0u8; 32];
        hkdf_expand_label::<Sha256>(current, b"traffic upd", &[], &mut next);
        next
    }
}

pub fn finish_tls13_handshake<S: Read + Write>(
    stream: &mut S,
    cfg: &TlsConfig,
    mut record: RecordLayer,
    mut transcript: Transcript,
    mut hs_reader: HandshakeReader,
    sh: ServerHello,
    eph: EphemeralKey,
) -> Result<HandshakeOutcome, TlsError> {
    let suite = sh.cipher_suite;
    if !suite.is_tls13() {
        return Err(TlsError::ProtocolError("server picked non-TLS 1.3 cipher suite".to_string()));
    }
    let Some((peer_group, peer_pub)) = sh.key_share else {
        return Err(TlsError::ProtocolError("missing key_share in TLS 1.3 ServerHello".to_string()));
    };

    let mut shared_secret = eph.ecdh(peer_group, &peer_pub)?;

    let (client_hs_secret, server_hs_secret, master_secret) = if suite.uses_sha384() {
        let zeros = [0u8; 48];
        let empty_hash = Sha384::digest_vec(b"");
        let early = hkdf_extract::<Sha384>(&zeros, &zeros);
        let derived1 = derive_secret::<Sha384>(&early, b"derived", &empty_hash);
        let hs_secret = hkdf_extract::<Sha384>(&derived1, &shared_secret);
        let th_sh = transcript.current_hash();
        let chs = derive_secret::<Sha384>(&hs_secret, b"c hs traffic", &th_sh);
        let shs = derive_secret::<Sha384>(&hs_secret, b"s hs traffic", &th_sh);
        let derived2 = derive_secret::<Sha384>(&hs_secret, b"derived", &empty_hash);
        let ms = hkdf_extract::<Sha384>(&derived2, &zeros);
        (chs, shs, ms)
    } else {
        let zeros = [0u8; 32];
        let empty_hash = Sha256::digest_vec(b"");
        let early = hkdf_extract::<Sha256>(&zeros, &zeros);
        let derived1 = derive_secret::<Sha256>(&early, b"derived", &empty_hash);
        let hs_secret = hkdf_extract::<Sha256>(&derived1, &shared_secret);
        let th_sh = transcript.current_hash();
        let chs = derive_secret::<Sha256>(&hs_secret, b"c hs traffic", &th_sh);
        let shs = derive_secret::<Sha256>(&hs_secret, b"s hs traffic", &th_sh);
        let derived2 = derive_secret::<Sha256>(&hs_secret, b"derived", &empty_hash);
        let ms = hkdf_extract::<Sha256>(&derived2, &zeros);
        (chs, shs, ms)
    };
    shared_secret.zeroize();

    record.read_state = make_tls13_cipher_state(suite, &server_hs_secret);
    record.write_state = make_tls13_cipher_state(suite, &client_hs_secret);

    // 1. EncryptedExtensions
    let (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_ENCRYPTED_EXTENSIONS {
        return Err(TlsError::ProtocolError("expected EncryptedExtensions".to_string()));
    }
    let alpn = parse_encrypted_extensions(&raw[4..])?;
    transcript.update(&raw);

    // 2. Optional CertificateRequest, then Certificate
    let (mut mt, mut raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    let mut cert_requested = false;
    let mut cert_req_ctx: Vec<u8> = Vec::new();
    if mt == HT_CERTIFICATE_REQUEST {
        cert_requested = true;
        if raw.len() < 5 {
            return Err(TlsError::DecodeError("CertificateRequest too short"));
        }
        let ctx_len = raw[4] as usize;
        if 5 + ctx_len > raw.len() {
            return Err(TlsError::DecodeError("invalid CertificateRequest context"));
        }
        cert_req_ctx = raw[5..5 + ctx_len].to_vec();
        transcript.update(&raw);
        (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    }

    if mt != HT_CERTIFICATE {
        return Err(TlsError::ProtocolError("expected Certificate".to_string()));
    }
    let cert_chain = parse_tls13_certificate(&raw[4..])?;
    if cert_chain.is_empty() {
        return Err(TlsError::CertificateError("server sent empty Certificate".to_string()));
    }
    transcript.update(&raw);

    let leaf_der = cert_chain[0].clone();
    if cfg.verify != VerifyMode::Disable {
        let roots = RootStore::load(&cfg.roots)?;
        let check_host = cfg.verify == VerifyMode::VerifyFull;
        verify_server_cert(&cert_chain, &roots, &cfg.server_name, check_host, unix_now())?;
    }

    // 3. CertificateVerify
    let (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_CERTIFICATE_VERIFY {
        return Err(TlsError::ProtocolError("expected CertificateVerify".to_string()));
    }
    let (scheme, sig) = parse_certificate_verify(&raw[4..])?;
    let th_before_cv = transcript.current_hash();
    verify_tls13_cert_verify(&leaf_der, scheme, sig, &th_before_cv)?;
    transcript.update(&raw);

    // 4. Server Finished
    let (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_FINISHED {
        return Err(TlsError::ProtocolError("expected server Finished".to_string()));
    }
    let th_before_fin = transcript.current_hash();
    let expected_server_fin = compute_finished_verify_data(suite, &server_hs_secret, &th_before_fin);
    if !constant_time_eq(&raw[4..], &expected_server_fin) {
        return Err(TlsError::RemoteAlert {
            level: 2,
            description: AlertDescription::DecryptError,
        });
    }
    transcript.update(&raw);

    // Derive application traffic secrets
    let th_server_fin = transcript.current_hash();
    let (client_app_secret, server_app_secret) = if suite.uses_sha384() {
        (
            derive_secret::<Sha384>(&master_secret, b"c ap traffic", &th_server_fin),
            derive_secret::<Sha384>(&master_secret, b"s ap traffic", &th_server_fin),
        )
    } else {
        (
            derive_secret::<Sha256>(&master_secret, b"c ap traffic", &th_server_fin),
            derive_secret::<Sha256>(&master_secret, b"s ap traffic", &th_server_fin),
        )
    };

    // 5. Client optional empty Certificate + Finished
    if cert_requested {
        let mut body = Vec::with_capacity(1 + cert_req_ctx.len() + 3);
        body.push(cert_req_ctx.len() as u8);
        body.extend_from_slice(&cert_req_ctx);
        body.extend_from_slice(&[0, 0, 0]);
        let msg = wrap_handshake(HT_CERTIFICATE, &body);
        transcript.update(&msg);
        record.write_record(stream, CT_HANDSHAKE, &msg)?;
    }

    let th_client_fin = transcript.current_hash();
    let client_verify_data = compute_finished_verify_data(suite, &client_hs_secret, &th_client_fin);
    let client_fin_msg = wrap_handshake(HT_FINISHED, &client_verify_data);
    record.write_record(stream, CT_HANDSHAKE, &client_fin_msg)?;

    // Switch to application traffic keys
    record.read_state = make_tls13_cipher_state(suite, &server_app_secret);
    record.write_state = make_tls13_cipher_state(suite, &client_app_secret);

    Ok(HandshakeOutcome {
        record,
        alpn,
        leaf_cert_der: Some(leaf_der),
        tls13_secrets: Some(Tls13Secrets {
            suite,
            client_app_secret,
            server_app_secret,
        }),
    })
}

pub fn read_next_handshake<S: Read>(
    stream: &mut S,
    record: &mut RecordLayer,
    hs_reader: &mut HandshakeReader,
) -> Result<(u8, Vec<u8>), TlsError> {
    loop {
        if let Some(msg) = hs_reader.next_message()? {
            return Ok(msg);
        }
        let (ct, payload) = record.read_record(stream)?;
        match ct {
            CT_CHANGE_CIPHER_SPEC => continue,
            CT_HANDSHAKE => hs_reader.push_bytes(payload)?,
            CT_ALERT => {
                if payload.len() >= 2 {
                    return Err(TlsError::RemoteAlert {
                        level: payload[0],
                        description: AlertDescription::from_u8(payload[1]),
                    });
                }
                return Err(TlsError::DecodeError("malformed TLS alert"));
            }
            _ => {
                return Err(TlsError::ProtocolError(format!(
                    "unexpected record type {ct} during handshake"
                )));
            }
        }
    }
}

fn compute_finished_verify_data(suite: CipherSuite, base_key: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
    if suite.uses_sha384() {
        let mut fkey = [0u8; 48];
        hkdf_expand_label::<Sha384>(base_key, b"finished", &[], &mut fkey);
        let mac = Hmac::<Sha384>::mac(&fkey, transcript_hash);
        fkey.zeroize();
        mac
    } else {
        let mut fkey = [0u8; 32];
        hkdf_expand_label::<Sha256>(base_key, b"finished", &[], &mut fkey);
        let mac = Hmac::<Sha256>::mac(&fkey, transcript_hash);
        fkey.zeroize();
        mac
    }
}

fn verify_tls13_cert_verify(
    leaf_der: &[u8],
    scheme: u16,
    sig: &[u8],
    transcript_hash: &[u8],
) -> Result<(), TlsError> {
    let leaf = parse_x509(leaf_der)?;
    let mut content = Vec::with_capacity(64 + 33 + 1 + transcript_hash.len());
    content.resize(64, 0x20);
    content.extend_from_slice(b"TLS 1.3, server CertificateVerify");
    content.push(0x00);
    content.extend_from_slice(transcript_hash);

    let ok = match (scheme, &leaf.spki) {
        (SIG_ECDSA_SECP256R1_SHA256, SubjectPublicKey::EcP256(pub_bytes)) => {
            let mut h = Sha256::new();
            h.update(&content);
            ecdsa_p256_verify_der(pub_bytes, &h.finalize(), sig)
        }
        (SIG_ECDSA_SECP384R1_SHA384, SubjectPublicKey::EcP384(pub_bytes)) => {
            let mut h = Sha384::new();
            h.update(&content);
            ecdsa_p384_verify_der(pub_bytes, &h.finalize(), sig)
        }
        (
            SIG_RSA_PSS_RSAE_SHA256 | SIG_RSA_PSS_RSAE_SHA384 | SIG_RSA_PSS_RSAE_SHA512,
            SubjectPublicKey::Rsa { n, e },
        ) => {
            let alg = match scheme {
                SIG_RSA_PSS_RSAE_SHA256 => RsaHashAlg::Sha256,
                SIG_RSA_PSS_RSAE_SHA384 => RsaHashAlg::Sha384,
                _ => RsaHashAlg::Sha512,
            };
            let Some(rsa) = RsaPublicKey::from_components(n, e) else {
                return Err(TlsError::CertificateError(
                    "invalid or <2048-bit RSA leaf key".to_string(),
                ));
            };
            let digest = alg.hash(&content);
            rsa.verify_pss(alg, &digest, sig)
        }
        _ => {
            return Err(TlsError::CertificateError(format!(
                "unsupported or mismatched CertificateVerify scheme 0x{scheme:04x}"
            )));
        }
    };

    if ok {
        Ok(())
    } else {
        Err(TlsError::CertificateError(
            "TLS 1.3 CertificateVerify signature failed".to_string(),
        ))
    }
}

/// Perform initial ClientHello (+ optional HelloRetryRequest) and dispatch into TLS 1.3.
pub fn connect_tls_handshake<S: Read + Write>(
    stream: &mut S,
    cfg: &TlsConfig,
    allow_tls12: bool,
) -> Result<HandshakeOutcome, TlsError> {
    let mut client_random = [0u8; 32];
    fill_random_bytes(&mut client_random).map_err(|_| TlsError::CryptoError("CSPRNG failure"))?;
    let mut legacy_session_id = [0u8; 32];
    fill_random_bytes(&mut legacy_session_id)
        .map_err(|_| TlsError::CryptoError("CSPRNG failure"))?;

    let mut eph_group = GROUP_X25519;
    let (mut eph, mut pub_key) = EphemeralKey::generate(eph_group)?;

    let mut record = RecordLayer::new();
    let mut transcript = Transcript::new();
    let mut hs_reader = HandshakeReader::new();

    let alpn_refs: Vec<&[u8]> = cfg.alpn.to_vec();
    let ch1 = build_client_hello(
        &client_random,
        &legacy_session_id,
        &cfg.server_name,
        &alpn_refs,
        eph_group,
        &pub_key,
        allow_tls12,
    );
    transcript.update(&ch1);
    record.write_plain_with_legacy_ver(stream, CT_HANDSHAKE, 0x0301, &ch1)?;

    let (mt, raw_sh) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_SERVER_HELLO {
        return Err(TlsError::ProtocolError("expected ServerHello".to_string()));
    }
    let mut sh = parse_server_hello(&raw_sh[4..])?;

    if sh.is_hrr {
        transcript.lock_to_sha384(sh.cipher_suite.uses_sha384());
        transcript.update(&raw_sh);
        transcript.replace_with_message_hash();

        let next_group = sh.hrr_selected_group.ok_or_else(|| {
            TlsError::ProtocolError("HelloRetryRequest missing key_share group".to_string())
        })?;
        if next_group == eph_group {
            return Err(TlsError::ProtocolError(
                "HelloRetryRequest requested already-sent group".to_string(),
            ));
        }
        eph_group = next_group;
        let (new_eph, new_pub) = EphemeralKey::generate(eph_group)?;
        eph = new_eph;
        pub_key = new_pub;

        let ch2 = build_client_hello(
            &client_random,
            &legacy_session_id,
            &cfg.server_name,
            &alpn_refs,
            eph_group,
            &pub_key,
            allow_tls12,
        );
        transcript.update(&ch2);
        // Send dummy ChangeCipherSpec for middlebox compatibility
        record.write_plain_with_legacy_ver(stream, CT_CHANGE_CIPHER_SPEC, 0x0303, &[1])?;
        record.write_plain_with_legacy_ver(stream, CT_HANDSHAKE, 0x0303, &ch2)?;

        let (mt2, raw_sh2) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
        if mt2 != HT_SERVER_HELLO {
            return Err(TlsError::ProtocolError("expected second ServerHello".to_string()));
        }
        let sh2 = parse_server_hello(&raw_sh2[4..])?;
        if sh2.is_hrr {
            return Err(TlsError::ProtocolError("received second HelloRetryRequest".to_string()));
        }
        transcript.update(&raw_sh2);
        sh = sh2;
    } else if sh.selected_version == 0x0304 {
        transcript.lock_to_sha384(sh.cipher_suite.uses_sha384());
        transcript.update(&raw_sh);
    }

    if sh.selected_version == 0x0304 {
        return finish_tls13_handshake(stream, cfg, record, transcript, hs_reader, sh, eph);
    }

    // Downgrade sentinel check (RFC 8446 §4.1.3)
    if sh.random[24..] == *b"DOWNGRD\x01" || sh.random[24..] == *b"DOWNGRD\x00" {
        return Err(TlsError::RemoteAlert {
            level: 2,
            description: AlertDescription::IllegalParameter,
        });
    }

    if allow_tls12 && sh.selected_version == 0x0303 && !sh.is_hrr {
        transcript.update(&raw_sh);
        return crate::native::tls::tls12::finish_tls12_handshake(
            stream,
            cfg,
            record,
            transcript,
            hs_reader,
            &client_random,
            sh,
        );
    }

    Err(TlsError::RemoteAlert {
        level: 2,
        description: AlertDescription::ProtocolVersion,
    })
}
