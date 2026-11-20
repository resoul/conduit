//! TLS 1.2 (RFC 5246 / RFC 7627 / RFC 8422) client state machine (ECDHE + AEAD only).

use std::io::{Read, Write};

use crate::native::crypto::constant_time_eq;
use crate::native::tls::crypto::Zeroize;
use crate::native::tls::crypto::chacha_poly::ChaCha20Poly1305;
use crate::native::tls::crypto::gcm::AesGcm;
use crate::native::tls::crypto::hmac_hkdf::Hmac;
use crate::native::tls::crypto::p256::ecdsa_p256_verify_der;
use crate::native::tls::crypto::p384::ecdsa_p384_verify_der;
use crate::native::tls::crypto::rsa::{RsaHashAlg, RsaPublicKey};
use crate::native::tls::crypto::sha2::{Digest, Sha256, Sha384, Sha512};
use crate::native::tls::error::TlsError;
use crate::native::tls::handshake::{
    CipherSuite, HT_CERTIFICATE, HT_CERTIFICATE_REQUEST, HT_CLIENT_KEY_EXCHANGE, HT_FINISHED,
    HT_SERVER_HELLO_DONE, HT_SERVER_KEY_EXCHANGE, HandshakeReader, SIG_ECDSA_SECP256R1_SHA256,
    SIG_ECDSA_SECP384R1_SHA384, SIG_RSA_PKCS1_SHA256, SIG_RSA_PKCS1_SHA384,
    SIG_RSA_PKCS1_SHA512, SIG_RSA_PSS_RSAE_SHA256, SIG_RSA_PSS_RSAE_SHA384,
    SIG_RSA_PSS_RSAE_SHA512, ServerHello, Transcript, wrap_handshake,
};
use crate::native::tls::record::{
    AeadCipher, CT_CHANGE_CIPHER_SPEC, CT_HANDSHAKE, CipherState, RecordLayer,
};
use crate::native::tls::roots::RootStore;
use crate::native::tls::tls13::{EphemeralKey, HandshakeOutcome, read_next_handshake};
use crate::native::tls::verify::{unix_now, verify_server_cert};
use crate::native::tls::x509::{SubjectPublicKey, parse_x509};
use crate::native::tls::{TlsConfig, VerifyMode};

fn p_hash<D: Digest>(secret: &[u8], seed: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    let mut a = Hmac::<D>::mac(secret, seed);
    while out.len() < out_len {
        let mut mac = Hmac::<D>::new(secret);
        mac.update(&a);
        mac.update(seed);
        let mut block = vec![0u8; D::OUTPUT_LEN];
        mac.finalize_into(&mut block);
        let take = (out_len - out.len()).min(block.len());
        out.extend_from_slice(&block[..take]);
        a = Hmac::<D>::mac(secret, &a);
    }
    out
}

pub fn tls12_prf(use_sha384: bool, secret: &[u8], label: &[u8], seed: &[u8], out_len: usize) -> Vec<u8> {
    let mut combined = Vec::with_capacity(label.len() + seed.len());
    combined.extend_from_slice(label);
    combined.extend_from_slice(seed);
    if use_sha384 {
        p_hash::<Sha384>(secret, &combined, out_len)
    } else {
        p_hash::<Sha256>(secret, &combined, out_len)
    }
}

pub fn finish_tls12_handshake<S: Read + Write>(
    stream: &mut S,
    cfg: &TlsConfig,
    mut record: RecordLayer,
    mut transcript: Transcript,
    mut hs_reader: HandshakeReader,
    client_random: &[u8; 32],
    sh: ServerHello,
) -> Result<HandshakeOutcome, TlsError> {
    let suite = sh.cipher_suite;
    if suite.is_tls13() {
        return Err(TlsError::ProtocolError("TLS 1.3 cipher suite in TLS 1.2 ServerHello".to_string()));
    }
    if !sh.extended_master_secret {
        return Err(TlsError::ProtocolError(
            "server does not support extended_master_secret (RFC 7627)".to_string(),
        ));
    }
    if !sh.renegotiation_info_empty {
        return Err(TlsError::ProtocolError(
            "server missing empty renegotiation_info (RFC 5746)".to_string(),
        ));
    }

    let use_sha384 = suite.uses_sha384();
    transcript.lock_to_sha384(use_sha384);

    // 1. Certificate
    let (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_CERTIFICATE {
        return Err(TlsError::ProtocolError("expected TLS 1.2 Certificate".to_string()));
    }
    transcript.update(&raw);
    let cert_chain = parse_tls12_certificate(&raw[4..])?;
    if cert_chain.is_empty() {
        return Err(TlsError::CertificateError("server sent empty Certificate".to_string()));
    }
    let leaf_der = cert_chain[0].clone();
    if cfg.verify != VerifyMode::Disable {
        let roots = RootStore::load(&cfg.roots)?;
        let check_host = cfg.verify == VerifyMode::VerifyFull;
        verify_server_cert(&cert_chain, &roots, &cfg.server_name, check_host, unix_now())?;
    }

    // 2. ServerKeyExchange (ECDHE)
    let (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_SERVER_KEY_EXCHANGE {
        return Err(TlsError::ProtocolError("expected TLS 1.2 ServerKeyExchange".to_string()));
    }
    transcript.update(&raw);
    let (group, peer_pub) = parse_and_verify_ske(&raw[4..], client_random, &sh.random, &leaf_der, suite)?;

    // 3. Optional CertificateRequest, then ServerHelloDone
    let (mut mt, mut raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    let mut cert_requested = false;
    if mt == HT_CERTIFICATE_REQUEST {
        cert_requested = true;
        transcript.update(&raw);
        (mt, raw) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    }
    if mt != HT_SERVER_HELLO_DONE || raw.len() != 4 {
        return Err(TlsError::ProtocolError("expected TLS 1.2 ServerHelloDone".to_string()));
    }
    transcript.update(&raw);

    // 4. ECDHE shared secret & ClientKeyExchange (RFC 7627: EMS session_hash includes CKE)
    let (eph, client_pub) = EphemeralKey::generate(group)?;
    let mut pms = eph.ecdh(group, &peer_pub)?;

    if cert_requested {
        let msg = wrap_handshake(HT_CERTIFICATE, &[0, 0, 0]);
        transcript.update(&msg);
        record.write_record(stream, CT_HANDSHAKE, &msg)?;
    }

    let mut cke_body = Vec::with_capacity(1 + client_pub.len());
    cke_body.push(client_pub.len() as u8);
    cke_body.extend_from_slice(&client_pub);
    let cke_msg = wrap_handshake(HT_CLIENT_KEY_EXCHANGE, &cke_body);
    transcript.update(&cke_msg);
    record.write_record(stream, CT_HANDSHAKE, &cke_msg)?;

    // 5. Extended master secret & key expansion
    let session_hash = transcript.current_hash();
    let mut master_secret = tls12_prf(use_sha384, &pms, b"extended master secret", &session_hash, 48);
    pms.zeroize();

    let mut rand_seed = [0u8; 64];
    rand_seed[..32].copy_from_slice(&sh.random);
    rand_seed[32..].copy_from_slice(client_random);
    let key_len = suite.key_len();
    let iv_len = if suite.is_chacha() { 12 } else { 4 };
    let kb_len = 2 * key_len + 2 * iv_len;
    let mut key_block = tls12_prf(use_sha384, &master_secret, b"key expansion", &rand_seed, kb_len);

    let cw_key = &key_block[..key_len];
    let sw_key = &key_block[key_len..2 * key_len];
    let cw_iv = &key_block[2 * key_len..2 * key_len + iv_len];
    let sw_iv = &key_block[2 * key_len + iv_len..kb_len];

    let client_write_state = make_tls12_cipher_state(suite, cw_key, cw_iv);
    let server_read_state = make_tls12_cipher_state(suite, sw_key, sw_iv);
    key_block.zeroize();

    // 6. Send ChangeCipherSpec + client Finished
    record.write_record(stream, CT_CHANGE_CIPHER_SPEC, &[1])?;
    record.write_state = client_write_state;

    let th_client_fin = transcript.current_hash();
    let client_vd = tls12_prf(use_sha384, &master_secret, b"client finished", &th_client_fin, 12);
    let client_fin_msg = wrap_handshake(HT_FINISHED, &client_vd);
    transcript.update(&client_fin_msg);
    record.write_record(stream, CT_HANDSHAKE, &client_fin_msg)?;

    // 7. Read server ChangeCipherSpec + Finished
    let (ct, payload) = record.read_record(stream)?;
    if ct != CT_CHANGE_CIPHER_SPEC || payload != [1] {
        master_secret.zeroize();
        return Err(TlsError::ProtocolError("expected server ChangeCipherSpec".to_string()));
    }
    record.read_state = server_read_state;

    let (mt, raw_fin) = read_next_handshake(stream, &mut record, &mut hs_reader)?;
    if mt != HT_FINISHED || raw_fin.len() != 16 {
        master_secret.zeroize();
        return Err(TlsError::ProtocolError("expected TLS 1.2 server Finished".to_string()));
    }
    let th_server_fin = transcript.current_hash();
    let expected_vd = tls12_prf(use_sha384, &master_secret, b"server finished", &th_server_fin, 12);
    master_secret.zeroize();

    if !constant_time_eq(&raw_fin[4..], &expected_vd) {
        let _ = record.write_record(stream, crate::native::tls::record::CT_ALERT, &[2, 51]);
        return Err(TlsError::ProtocolError("TLS 1.2 server Finished verify_data mismatch".to_string()));
    }

    Ok(HandshakeOutcome {
        record,
        alpn: None,
        leaf_cert_der: Some(leaf_der),
        tls13_secrets: None,
    })
}

fn make_tls12_cipher_state(suite: CipherSuite, key: &[u8], iv: &[u8]) -> CipherState {
    if suite.is_chacha() {
        let mut k32 = [0u8; 32];
        k32.copy_from_slice(key);
        let mut iv12 = [0u8; 12];
        iv12.copy_from_slice(iv);
        CipherState::Tls12 {
            cipher: AeadCipher::ChaCha20Poly1305(ChaCha20Poly1305::new(k32)),
            implicit_iv: [0; 4],
            seq: 0,
            is_chacha: true,
            chacha_iv: iv12,
        }
    } else {
        let mut iv4 = [0u8; 4];
        iv4.copy_from_slice(iv);
        CipherState::Tls12 {
            cipher: AeadCipher::AesGcm(Box::new(AesGcm::new(key).expect("valid AES key"))),
            implicit_iv: iv4,
            seq: 0,
            is_chacha: false,
            chacha_iv: [0; 12],
        }
    }
}

fn parse_tls12_certificate(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    if body.len() < 3 {
        return Err(TlsError::DecodeError("TLS 1.2 Certificate too short"));
    }
    let list_len = ((body[0] as usize) << 16) | ((body[1] as usize) << 8) | (body[2] as usize);
    if 3 + list_len != body.len() {
        return Err(TlsError::DecodeError("invalid TLS 1.2 Certificate list length"));
    }
    let mut pos = 3;
    let mut certs = Vec::new();
    while pos < body.len() {
        if certs.len() >= 10 {
            return Err(TlsError::CertificateError("certificate chain exceeds 10 certs".to_string()));
        }
        if pos + 3 > body.len() {
            return Err(TlsError::DecodeError("truncated Certificate length"));
        }
        let len = ((body[pos] as usize) << 16)
            | ((body[pos + 1] as usize) << 8)
            | (body[pos + 2] as usize);
        pos += 3;
        if pos + len > body.len() {
            return Err(TlsError::DecodeError("truncated Certificate DER"));
        }
        certs.push(body[pos..pos + len].to_vec());
        pos += len;
    }
    Ok(certs)
}

fn parse_and_verify_ske(
    body: &[u8],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
    leaf_der: &[u8],
    suite: CipherSuite,
) -> Result<(u16, Vec<u8>), TlsError> {
    if body.len() < 4 || body[0] != 3 {
        return Err(TlsError::DecodeError("invalid ECDHE ServerKeyExchange curve_type"));
    }
    let group = u16::from_be_bytes([body[1], body[2]]);
    let pub_len = body[3] as usize;
    let params_end = 4 + pub_len;
    if params_end + 4 > body.len() {
        return Err(TlsError::DecodeError("truncated ServerKeyExchange"));
    }
    let server_pub = body[4..params_end].to_vec();
    let params = &body[..params_end];

    let scheme = u16::from_be_bytes([body[params_end], body[params_end + 1]]);
    let sig_len = u16::from_be_bytes([body[params_end + 2], body[params_end + 3]]) as usize;
    if params_end + 4 + sig_len != body.len() {
        return Err(TlsError::DecodeError("invalid ServerKeyExchange signature length"));
    }
    let sig = &body[params_end + 4..];

    let mut signed = Vec::with_capacity(64 + params.len());
    signed.extend_from_slice(client_random);
    signed.extend_from_slice(server_random);
    signed.extend_from_slice(params);

    let leaf = parse_x509(leaf_der)?;
    let ok = match (scheme, &leaf.spki, suite.is_ecdsa()) {
        (SIG_ECDSA_SECP256R1_SHA256, SubjectPublicKey::EcP256(pub_bytes), true) => {
            let mut h = Sha256::new();
            h.update(&signed);
            ecdsa_p256_verify_der(pub_bytes, &h.finalize(), sig)
        }
        (SIG_ECDSA_SECP384R1_SHA384, SubjectPublicKey::EcP384(pub_bytes), true) => {
            let mut h = Sha384::new();
            h.update(&signed);
            ecdsa_p384_verify_der(pub_bytes, &h.finalize(), sig)
        }
        (
            SIG_RSA_PKCS1_SHA256 | SIG_RSA_PKCS1_SHA384 | SIG_RSA_PKCS1_SHA512,
            SubjectPublicKey::Rsa { n, e },
            false,
        ) => {
            let alg = match scheme {
                SIG_RSA_PKCS1_SHA256 => RsaHashAlg::Sha256,
                SIG_RSA_PKCS1_SHA384 => RsaHashAlg::Sha384,
                _ => RsaHashAlg::Sha512,
            };
            let Some(rsa) = RsaPublicKey::from_components(n, e) else {
                return Err(TlsError::CertificateError("invalid RSA key (<2048 bits)".to_string()));
            };
            let digest = alg.hash(&signed);
            rsa.verify_pkcs1_v15(alg, &digest, sig)
        }
        (
            SIG_RSA_PSS_RSAE_SHA256 | SIG_RSA_PSS_RSAE_SHA384 | SIG_RSA_PSS_RSAE_SHA512,
            SubjectPublicKey::Rsa { n, e },
            false,
        ) => {
            let alg = match scheme {
                SIG_RSA_PSS_RSAE_SHA256 => RsaHashAlg::Sha256,
                SIG_RSA_PSS_RSAE_SHA384 => RsaHashAlg::Sha384,
                _ => RsaHashAlg::Sha512,
            };
            let Some(rsa) = RsaPublicKey::from_components(n, e) else {
                return Err(TlsError::CertificateError("invalid RSA key (<2048 bits)".to_string()));
            };
            let digest = alg.hash(&signed);
            rsa.verify_pss(alg, &digest, sig)
        }
        _ => {
            return Err(TlsError::CertificateError(format!(
                "unsupported or mismatched TLS 1.2 SKE signature scheme 0x{scheme:04x}"
            )));
        }
    };

    if !ok {
        return Err(TlsError::CertificateError(
            "TLS 1.2 ServerKeyExchange signature verification failed".to_string(),
        ));
    }
    let _ = Sha512::new(); // keep Sha512 import clean if needed
    Ok((group, server_pub))
}
