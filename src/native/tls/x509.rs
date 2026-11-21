//! X.509 v3 DER certificate parser (RFC 5280).

use crate::native::tls::asn1::{
    DerReader, TAG_BOOLEAN, TAG_GENERALIZED_TIME, TAG_IA5_STRING, TAG_INTEGER, TAG_NULL,
    TAG_SEQUENCE, TAG_UTC_TIME,
};
use crate::native::tls::crypto::rsa::RsaHashAlg;
use crate::native::tls::error::TlsError;

// Signature / key OIDs (DER encoded body without tag/len)
const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_MD5_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x04];
const OID_SHA1_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05];
const OID_SHA256_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_SHA384_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_SHA512_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
const OID_RSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];

const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const OID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];

const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_SECP384R1: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];

const OID_ECDSA_SHA1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x01];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];

// Extension OIDs
const OID_EXT_SUBJECT_KEY_ID: &[u8] = &[0x55, 0x1d, 0x0e];
const OID_EXT_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_EXT_SAN: &[u8] = &[0x55, 0x1d, 0x11];
const OID_EXT_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_EXT_NAME_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x1e];
const OID_EXT_CERT_POLICIES: &[u8] = &[0x55, 0x1d, 0x20];
const OID_EXT_AUTH_KEY_ID: &[u8] = &[0x55, 0x1d, 0x23];
const OID_EXT_EKU: &[u8] = &[0x55, 0x1d, 0x25];

const OID_EKU_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_EKU_ANY: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigAlg {
    RsaPkcs1(RsaHashAlg),
    RsaPss { hash: RsaHashAlg, salt_len: usize },
    EcdsaSha256,
    EcdsaSha384,
    EcdsaSha512,
    WeakMd5OrSha1,
    Unsupported,
}

#[derive(Debug, Clone)]
pub enum SubjectPublicKey {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    EcP256(Vec<u8>),
    EcP384(Vec<u8>),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanEntry {
    Dns(String),
    Ip(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct NameConstraints {
    pub permitted_dns: Vec<String>,
    pub excluded_dns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct X509Cert<'a> {
    pub tbs_raw: &'a [u8],
    pub version: u8,
    pub sig_alg: SigAlg,
    pub signature: &'a [u8],
    pub issuer_raw: &'a [u8],
    pub subject_raw: &'a [u8],
    pub not_before: i64,
    pub not_after: i64,
    pub spki: SubjectPublicKey,
    pub san: Vec<SanEntry>,
    pub is_ca: bool,
    pub path_len: Option<u32>,
    pub has_key_usage: bool,
    pub key_cert_sign: bool,
    pub eku_present: bool,
    pub eku_server_auth: bool,
    pub name_constraints: Option<NameConstraints>,
}

/// Fast extraction of `subject_raw` from DER certificate for lazy root indexing.
pub fn extract_subject_raw(der: &[u8]) -> Option<&[u8]> {
    let mut top = DerReader::new(der).read_sequence().ok()?;
    let mut tbs = top.read_sequence().ok()?;
    if tbs.peek_tag() == Some(0xa0) {
        let _ = tbs.read_tlv().ok()?; // version
    }
    let _ = tbs.read_tlv().ok()?; // serial
    let _ = tbs.read_tlv().ok()?; // signature
    let _ = tbs.read_tlv().ok()?; // issuer
    let _ = tbs.read_tlv().ok()?; // validity
    let subject = tbs.read_tag(TAG_SEQUENCE).ok()?;
    Some(subject.raw)
}

pub fn parse_x509<'a>(der: &'a [u8]) -> Result<X509Cert<'a>, TlsError> {
    let mut top_reader = DerReader::new(der);
    let mut cert_seq = top_reader.read_sequence()?;
    if !top_reader.is_empty() {
        return Err(TlsError::CertificateError("trailing bytes after X.509 Certificate".to_string()));
    }

    let tbs_tlv = cert_seq.read_tag(TAG_SEQUENCE)?;
    let tbs_raw = tbs_tlv.raw;
    let mut tbs = tbs_tlv.reader()?;

    let sig_alg_outer = parse_algorithm_id(&mut cert_seq)?;
    let (unused_bits, sig_bytes) = cert_seq.read_bit_string()?;
    if unused_bits != 0 {
        return Err(TlsError::CertificateError("non-zero unused bits in signatureValue".to_string()));
    }
    if !cert_seq.is_empty() {
        return Err(TlsError::CertificateError("trailing bytes in Certificate".to_string()));
    }

    // Parse TBSCertificate
    let version = if tbs.peek_tag() == Some(0xa0) {
        let v_tlv = tbs.read_tlv()?;
        let mut v_reader = v_tlv.reader()?;
        let v_bytes = v_reader.read_integer_bytes()?;
        if !v_reader.is_empty() || v_bytes.len() != 1 {
            return Err(TlsError::CertificateError("invalid X.509 version".to_string()));
        }
        v_bytes[0]
    } else {
        0 // v1
    };
    if version > 2 {
        return Err(TlsError::CertificateError(format!("unsupported X.509 version {}", version + 1)));
    }

    let _serial = tbs.read_integer_bytes()?;
    let sig_alg_inner = parse_algorithm_id(&mut tbs)?;
    if sig_alg_outer != sig_alg_inner {
        return Err(TlsError::CertificateError(
            "Certificate signatureAlgorithm mismatch with TBSCertificate".to_string(),
        ));
    }

    let issuer_raw = tbs.read_tag(TAG_SEQUENCE)?.raw;

    let mut validity_seq = tbs.read_sequence()?;
    let not_before = parse_time(&mut validity_seq)?;
    let not_after = parse_time(&mut validity_seq)?;
    if !validity_seq.is_empty() {
        return Err(TlsError::CertificateError("trailing bytes in Validity".to_string()));
    }

    let subject_raw = tbs.read_tag(TAG_SEQUENCE)?.raw;
    let spki = parse_spki(&mut tbs)?;

    // Optional issuerUniqueID [1], subjectUniqueID [2]
    if tbs.peek_tag() == Some(0x81) {
        let _ = tbs.read_tlv()?;
    }
    if tbs.peek_tag() == Some(0x82) {
        let _ = tbs.read_tlv()?;
    }

    let mut san = Vec::new();
    let mut is_ca = false;
    let mut path_len = None;
    let mut has_key_usage = false;
    let mut key_cert_sign = false;
    let mut eku_present = false;
    let mut eku_server_auth = false;
    let mut name_constraints = None;

    if tbs.peek_tag() == Some(0xa3) {
        if version < 2 {
            return Err(TlsError::CertificateError("extensions present in non-v3 certificate".to_string()));
        }
        let ext_outer = tbs.read_tlv()?;
        let mut ext_wrap = ext_outer.reader()?;
        let mut exts = ext_wrap.read_sequence()?;
        if !ext_wrap.is_empty() {
            return Err(TlsError::CertificateError("trailing bytes after extensions".to_string()));
        }
        let mut seen_oids: Vec<&[u8]> = Vec::new();
        while !exts.is_empty() {
            let mut ext = exts.read_sequence()?;
            let oid = ext.read_oid()?;
            if seen_oids.contains(&oid) {
                return Err(TlsError::CertificateError("duplicate X.509 extension".to_string()));
            }
            seen_oids.push(oid);
            let critical = if ext.peek_tag() == Some(TAG_BOOLEAN) {
                ext.read_boolean()?
            } else {
                false
            };
            let val = ext.read_octet_string()?;
            if !ext.is_empty() {
                return Err(TlsError::CertificateError("trailing bytes in X.509 extension".to_string()));
            }
            match oid {
                OID_EXT_SAN => {
                    san = parse_san(val)?;
                }
                OID_EXT_BASIC_CONSTRAINTS => {
                    let (ca, plen) = parse_basic_constraints(val)?;
                    is_ca = ca;
                    path_len = plen;
                }
                OID_EXT_KEY_USAGE => {
                    has_key_usage = true;
                    key_cert_sign = parse_key_usage_cert_sign(val)?;
                }
                OID_EXT_EKU => {
                    eku_present = true;
                    eku_server_auth = parse_eku_server_auth(val)?;
                }
                OID_EXT_NAME_CONSTRAINTS => {
                    name_constraints = Some(parse_name_constraints(val, critical)?);
                }
                OID_EXT_SUBJECT_KEY_ID | OID_EXT_AUTH_KEY_ID | OID_EXT_CERT_POLICIES => {
                    // Safe to ignore when non-critical; if certPolicies is critical we only
                    // reject truly unknown critical extensions.
                    if critical && oid != OID_EXT_CERT_POLICIES {
                        return Err(TlsError::CertificateError(
                            "critical key identifier extension not allowed".to_string(),
                        ));
                    }
                }
                _ => {
                    if critical {
                        return Err(TlsError::CertificateError(
                            "unsupported critical X.509 extension".to_string(),
                        ));
                    }
                }
            }
        }
    }

    if !tbs.is_empty() {
        return Err(TlsError::CertificateError("trailing bytes in TBSCertificate".to_string()));
    }

    Ok(X509Cert {
        tbs_raw,
        version,
        sig_alg: sig_alg_outer,
        signature: sig_bytes,
        issuer_raw,
        subject_raw,
        not_before,
        not_after,
        spki,
        san,
        is_ca,
        path_len,
        has_key_usage,
        key_cert_sign,
        eku_present,
        eku_server_auth,
        name_constraints,
    })
}

fn parse_algorithm_id(reader: &mut DerReader<'_>) -> Result<SigAlg, TlsError> {
    let mut seq = reader.read_sequence()?;
    let oid = seq.read_oid()?;
    let alg = match oid {
        OID_MD5_RSA | OID_SHA1_RSA | OID_ECDSA_SHA1 => SigAlg::WeakMd5OrSha1,
        OID_SHA256_RSA => SigAlg::RsaPkcs1(RsaHashAlg::Sha256),
        OID_SHA384_RSA => SigAlg::RsaPkcs1(RsaHashAlg::Sha384),
        OID_SHA512_RSA => SigAlg::RsaPkcs1(RsaHashAlg::Sha512),
        OID_ECDSA_SHA256 => SigAlg::EcdsaSha256,
        OID_ECDSA_SHA384 => SigAlg::EcdsaSha384,
        OID_ECDSA_SHA512 => SigAlg::EcdsaSha512,
        OID_RSA_PSS => {
            if seq.is_empty() {
                SigAlg::Unsupported
            } else {
                let pss = parse_pss_params(&mut seq)?;
                if !seq.is_empty() {
                    return Err(TlsError::CertificateError(
                        "trailing data in RSASSA-PSS AlgorithmIdentifier".to_string(),
                    ));
                }
                pss
            }
        }
        _ => SigAlg::Unsupported,
    };
    Ok(alg)
}

fn parse_sha2_alg_id(reader: &mut DerReader<'_>) -> Result<Option<RsaHashAlg>, TlsError> {
    let mut alg_seq = reader.read_sequence()?;
    let oid = alg_seq.read_oid()?;
    if !alg_seq.is_empty() {
        alg_seq.read_tag(TAG_NULL)?;
        if !alg_seq.is_empty() {
            return Err(TlsError::CertificateError(
                "trailing parameters in PSS HashAlgorithm".to_string(),
            ));
        }
    }
    Ok(match oid {
        OID_SHA256 => Some(RsaHashAlg::Sha256),
        OID_SHA384 => Some(RsaHashAlg::Sha384),
        OID_SHA512 => Some(RsaHashAlg::Sha512),
        _ => None,
    })
}

pub(crate) fn parse_pss_params(reader: &mut DerReader<'_>) -> Result<SigAlg, TlsError> {
    let mut params = reader.read_sequence()?;
    let mut hash_alg: Option<RsaHashAlg> = None;
    let mut mgf_hash: Option<RsaHashAlg> = None;
    let mut saw_mgf = false;
    let mut salt_len: usize = 20; // RFC 4055 default saltLength
    let mut expected_min_tag: u8 = 0xa0;

    while !params.is_empty() {
        let tlv = params.read_tlv()?;
        if !matches!(tlv.tag, 0xa0..=0xa3) || tlv.tag < expected_min_tag {
            return Err(TlsError::CertificateError(
                "invalid or out-of-order RSASSA-PSS-params field".to_string(),
            ));
        }
        expected_min_tag = tlv.tag + 1;
        let mut inner = tlv.reader()?;
        match tlv.tag {
            0xa0 => {
                let Some(h) = parse_sha2_alg_id(&mut inner)? else {
                    return Ok(SigAlg::WeakMd5OrSha1);
                };
                hash_alg = Some(h);
            }
            0xa1 => {
                saw_mgf = true;
                let mut mgf_seq = inner.read_sequence()?;
                let mgf_oid = mgf_seq.read_oid()?;
                if mgf_oid != OID_MGF1 {
                    return Ok(SigAlg::Unsupported);
                }
                let Some(h) = parse_sha2_alg_id(&mut mgf_seq)? else {
                    return Ok(SigAlg::WeakMd5OrSha1);
                };
                if !mgf_seq.is_empty() {
                    return Err(TlsError::CertificateError(
                        "trailing data in MGF1 AlgorithmIdentifier".to_string(),
                    ));
                }
                mgf_hash = Some(h);
            }
            0xa2 => {
                let int_bytes = inner.read_positive_integer_be()?;
                if int_bytes.len() > 2 {
                    return Err(TlsError::CertificateError("invalid PSS saltLength".to_string()));
                }
                let mut v = 0usize;
                for &b in int_bytes {
                    v = (v << 8) | (b as usize);
                }
                if v > 512 {
                    return Err(TlsError::CertificateError("PSS saltLength exceeds 512".to_string()));
                }
                salt_len = v;
            }
            0xa3 => {
                let int_bytes = inner.read_positive_integer_be()?;
                if int_bytes != [1] {
                    return Err(TlsError::CertificateError(
                        "unsupported PSS trailerField".to_string(),
                    ));
                }
            }
            _ => unreachable!(),
        }
        if !inner.is_empty() {
            return Err(TlsError::CertificateError(
                "trailing data in RSASSA-PSS-params field".to_string(),
            ));
        }
    }

    let Some(hash) = hash_alg else {
        // RFC 4055 default hash is SHA-1, which we reject
        return Ok(SigAlg::WeakMd5OrSha1);
    };
    if !saw_mgf {
        // RFC 4055 default maskGenAlgorithm is mgf1SHA1, which is weak / mismatched
        return Ok(SigAlg::WeakMd5OrSha1);
    }
    if mgf_hash != Some(hash) {
        return Err(TlsError::CertificateError(
            "RSASSA-PSS MGF1 hash does not match hashAlgorithm".to_string(),
        ));
    }
    Ok(SigAlg::RsaPss { hash, salt_len })
}

pub(crate) fn parse_spki(reader: &mut DerReader<'_>) -> Result<SubjectPublicKey, TlsError> {
    let mut spki_seq = reader.read_sequence()?;
    let mut alg_seq = spki_seq.read_sequence()?;
    let alg_oid = alg_seq.read_oid()?;
    let curve_oid = if alg_oid == OID_EC_PUBLIC_KEY {
        Some(alg_seq.read_oid()?)
    } else {
        if alg_seq.peek_tag() == Some(TAG_NULL) {
            let _ = alg_seq.read_tlv()?;
        }
        None
    };

    let (unused, key_bytes) = spki_seq.read_bit_string()?;
    if unused != 0 || !spki_seq.is_empty() {
        return Err(TlsError::CertificateError("invalid SubjectPublicKeyInfo".to_string()));
    }

    match alg_oid {
        OID_RSA_ENCRYPTION => {
            let mut rsa_seq = DerReader::new(key_bytes).read_sequence()?;
            let n = rsa_seq.read_positive_integer_be()?.to_vec();
            let e = rsa_seq.read_positive_integer_be()?.to_vec();
            if !rsa_seq.is_empty() {
                return Err(TlsError::CertificateError("trailing bytes in RSA public key".to_string()));
            }
            Ok(SubjectPublicKey::Rsa { n, e })
        }
        OID_EC_PUBLIC_KEY => match curve_oid {
            Some(OID_PRIME256V1) => Ok(SubjectPublicKey::EcP256(key_bytes.to_vec())),
            Some(OID_SECP384R1) => Ok(SubjectPublicKey::EcP384(key_bytes.to_vec())),
            _ => Ok(SubjectPublicKey::Unsupported),
        },
        _ => Ok(SubjectPublicKey::Unsupported),
    }
}

fn parse_san(val: &[u8]) -> Result<Vec<SanEntry>, TlsError> {
    let mut seq = DerReader::new(val).read_sequence()?;
    let mut entries = Vec::new();
    while !seq.is_empty() {
        let tlv = seq.read_tlv()?;
        match tlv.tag {
            0x82 => {
                // dNSName [2] IA5String
                let s = std::str::from_utf8(tlv.value).map_err(|_| {
                    TlsError::CertificateError("invalid UTF-8 in SAN dNSName".to_string())
                })?;
                if !s.is_ascii() {
                    return Err(TlsError::CertificateError("non-ASCII dNSName in SAN".to_string()));
                }
                entries.push(SanEntry::Dns(s.to_ascii_lowercase()));
            }
            0x87 => {
                // iPAddress [7] OCTET STRING (4 or 16 bytes)
                if tlv.value.len() == 4 || tlv.value.len() == 16 {
                    entries.push(SanEntry::Ip(tlv.value.to_vec()));
                } else {
                    return Err(TlsError::CertificateError("invalid SAN iPAddress length".to_string()));
                }
            }
            _ => {}
        }
    }
    Ok(entries)
}

fn parse_basic_constraints(val: &[u8]) -> Result<(bool, Option<u32>), TlsError> {
    let mut seq = DerReader::new(val).read_sequence()?;
    let mut is_ca = false;
    let mut path_len = None;
    if seq.peek_tag() == Some(TAG_BOOLEAN) {
        is_ca = seq.read_boolean()?;
    }
    if seq.peek_tag() == Some(TAG_INTEGER) {
        let b = seq.read_positive_integer_be()?;
        if b.len() > 4 {
            return Err(TlsError::CertificateError("pathLenConstraint too large".to_string()));
        }
        let mut v = 0u32;
        for &byte in b {
            v = (v << 8) | u32::from(byte);
        }
        path_len = Some(v);
    }
    if !seq.is_empty() {
        return Err(TlsError::CertificateError("trailing bytes in BasicConstraints".to_string()));
    }
    Ok((is_ca, path_len))
}

fn parse_key_usage_cert_sign(val: &[u8]) -> Result<bool, TlsError> {
    let mut r = DerReader::new(val);
    let (_unused, bits) = r.read_bit_string()?;
    if !r.is_empty() || bits.is_empty() {
        return Err(TlsError::CertificateError("invalid KeyUsage extension".to_string()));
    }
    // keyCertSign is bit 5 (0x04 in first byte, big-endian bit numbering: bit 0 = 0x80)
    Ok((bits[0] & 0x04) != 0)
}

fn parse_eku_server_auth(val: &[u8]) -> Result<bool, TlsError> {
    let mut seq = DerReader::new(val).read_sequence()?;
    let mut server_auth = false;
    while !seq.is_empty() {
        let oid = seq.read_oid()?;
        if oid == OID_EKU_SERVER_AUTH || oid == OID_EKU_ANY {
            server_auth = true;
        }
    }
    Ok(server_auth)
}

fn parse_name_constraints(val: &[u8], critical: bool) -> Result<NameConstraints, TlsError> {
    let mut seq = DerReader::new(val).read_sequence()?;
    let mut nc = NameConstraints::default();
    while !seq.is_empty() {
        let tlv = seq.read_tlv()?;
        let is_permitted = match tlv.tag {
            0xa0 => true,
            0xa1 => false,
            _ => continue,
        };
        let mut subtrees = tlv.reader()?;
        while !subtrees.is_empty() {
            let mut subtree = subtrees.read_sequence()?;
            let base = subtree.read_tlv()?;
            if base.tag == 0x82
                && let Ok(s) = std::str::from_utf8(base.value)
                && s.is_ascii()
            {
                let lower = s.to_ascii_lowercase();
                if is_permitted {
                    nc.permitted_dns.push(lower);
                } else {
                    nc.excluded_dns.push(lower);
                }
            } else if critical {
                return Err(TlsError::CertificateError(
                    "unsupported critical nameConstraints subtree type".to_string(),
                ));
            }
        }
    }
    Ok(nc)
}

fn parse_time(reader: &mut DerReader<'_>) -> Result<i64, TlsError> {
    let tlv = reader.read_tlv()?;
    let s = std::str::from_utf8(tlv.value)
        .map_err(|_| TlsError::CertificateError("invalid UTF-8 in certificate time".to_string()))?;
    match tlv.tag {
        TAG_UTC_TIME => {
            // YYMMDDHHMMSSZ (13 bytes)
            if s.len() != 13 || !s.ends_with('Z') {
                return Err(TlsError::CertificateError("invalid UTCTime format".to_string()));
            }
            let yy: i64 = parse_2d(&s[0..2])?;
            let year = if yy >= 50 { 1900 + yy } else { 2000 + yy };
            let month = parse_2d(&s[2..4])?;
            let day = parse_2d(&s[4..6])?;
            let hour = parse_2d(&s[6..8])?;
            let min = parse_2d(&s[8..10])?;
            let sec = parse_2d(&s[10..12])?;
            civil_to_unix(year, month, day, hour, min, sec)
        }
        TAG_GENERALIZED_TIME => {
            // YYYYMMDDHHMMSSZ (15 bytes)
            if s.len() != 15 || !s.ends_with('Z') {
                return Err(TlsError::CertificateError("invalid GeneralizedTime format".to_string()));
            }
            let year = parse_4d(&s[0..4])?;
            let month = parse_2d(&s[4..6])?;
            let day = parse_2d(&s[6..8])?;
            let hour = parse_2d(&s[8..10])?;
            let min = parse_2d(&s[10..12])?;
            let sec = parse_2d(&s[12..14])?;
            civil_to_unix(year, month, day, hour, min, sec)
        }
        _ => Err(TlsError::CertificateError("unexpected ASN.1 time tag".to_string())),
    }
}

fn parse_2d(s: &str) -> Result<i64, TlsError> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TlsError::CertificateError("non-digit in certificate time".to_string()));
    }
    s.parse::<i64>()
        .map_err(|_| TlsError::CertificateError("invalid digit in certificate time".to_string()))
}

fn parse_4d(s: &str) -> Result<i64, TlsError> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TlsError::CertificateError("non-digit in certificate time".to_string()));
    }
    s.parse::<i64>()
        .map_err(|_| TlsError::CertificateError("invalid digit in certificate time".to_string()))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            if leap { 29 } else { 28 }
        }
        _ => 0,
    }
}

fn civil_to_unix(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> Result<i64, TlsError> {
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&min)
        || !(0..=59).contains(&sec)
    {
        return Err(TlsError::CertificateError("out-of-range certificate timestamp".to_string()));
    }
    // Howard Hinnant's days_from_civil algorithm
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Ok(days * 86_400 + hour * 3_600 + min * 60 + sec)
}

#[allow(dead_code)]
fn _keep_tag_ia5_used() -> u8 {
    TAG_IA5_STRING
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        assert!(body.len() < 128);
        out.push(body.len() as u8);
        out.extend_from_slice(body);
        out
    }

    fn alg_id(oid: &[u8]) -> Vec<u8> {
        let mut body = wrap_tlv(0x06, oid);
        body.extend_from_slice(&[0x05, 0x00]); // NULL
        wrap_tlv(0x30, &body)
    }

    fn mgf1(hash_oid: &[u8]) -> Vec<u8> {
        let mut body = wrap_tlv(0x06, OID_MGF1);
        body.extend_from_slice(&alg_id(hash_oid));
        wrap_tlv(0x30, &body)
    }

    #[test]
    fn parse_pss_params_validates_hash_mgf_salt_and_trailer() {
        // Valid SHA-256 + MGF1-SHA256 + saltLength 32 + trailerField 1
        let mut valid_body = Vec::new();
        valid_body.extend_from_slice(&wrap_tlv(0xa0, &alg_id(OID_SHA256)));
        valid_body.extend_from_slice(&wrap_tlv(0xa1, &mgf1(OID_SHA256)));
        valid_body.extend_from_slice(&wrap_tlv(0xa2, &wrap_tlv(0x02, &[32])));
        valid_body.extend_from_slice(&wrap_tlv(0xa3, &wrap_tlv(0x02, &[1])));
        let valid = wrap_tlv(0x30, &valid_body);
        assert_eq!(
            parse_pss_params(&mut DerReader::new(&valid)).unwrap(),
            SigAlg::RsaPss {
                hash: RsaHashAlg::Sha256,
                salt_len: 32,
            }
        );

        // Missing MGF1 (defaults to SHA-1) rejected as weak
        let mut no_mgf_body = Vec::new();
        no_mgf_body.extend_from_slice(&wrap_tlv(0xa0, &alg_id(OID_SHA256)));
        let no_mgf = wrap_tlv(0x30, &no_mgf_body);
        assert_eq!(
            parse_pss_params(&mut DerReader::new(&no_mgf)).unwrap(),
            SigAlg::WeakMd5OrSha1
        );

        // Mismatched MGF1 hash (SHA-256 + MGF1-SHA384) rejected
        let mut mismatch_body = Vec::new();
        mismatch_body.extend_from_slice(&wrap_tlv(0xa0, &alg_id(OID_SHA256)));
        mismatch_body.extend_from_slice(&wrap_tlv(0xa1, &mgf1(OID_SHA384)));
        let mismatch = wrap_tlv(0x30, &mismatch_body);
        assert!(parse_pss_params(&mut DerReader::new(&mismatch)).is_err());

        // Invalid trailerField != 1 rejected
        let mut bad_trailer_body = Vec::new();
        bad_trailer_body.extend_from_slice(&wrap_tlv(0xa0, &alg_id(OID_SHA256)));
        bad_trailer_body.extend_from_slice(&wrap_tlv(0xa1, &mgf1(OID_SHA256)));
        bad_trailer_body.extend_from_slice(&wrap_tlv(0xa3, &wrap_tlv(0x02, &[2])));
        let bad_trailer = wrap_tlv(0x30, &bad_trailer_body);
        assert!(parse_pss_params(&mut DerReader::new(&bad_trailer)).is_err());
    }
}
