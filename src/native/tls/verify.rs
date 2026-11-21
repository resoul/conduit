//! X.509 certificate chain verification and RFC 6125 hostname/IP matching.
//! Note: CRL and OCSP revocation checking are explicitly out of scope.

use std::net::IpAddr;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::native::tls::crypto::p256::ecdsa_p256_verify_der;
use crate::native::tls::crypto::p384::ecdsa_p384_verify_der;
use crate::native::tls::crypto::rsa::{RsaHashAlg, RsaPublicKey};
use crate::native::tls::crypto::sha2::{Sha256, Sha384, Sha512};
use crate::native::tls::error::TlsError;
use crate::native::tls::roots::RootStore;
use crate::native::tls::x509::{
    NameConstraints, SanEntry, SigAlg, SubjectPublicKey, X509Cert, parse_x509,
};

pub const MAX_CHAIN_LEN: usize = 10;
pub const MAX_SIG_CHECKS: usize = 16;

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn verify_server_cert(
    cert_chain_der: &[Vec<u8>],
    roots: &RootStore,
    server_name: &str,
    check_hostname: bool,
    now: i64,
) -> Result<(), TlsError> {
    verify_server_cert_validity(cert_chain_der, roots, server_name, check_hostname, now).map(|_| ())
}

pub(super) fn verify_server_cert_validity(
    cert_chain_der: &[Vec<u8>],
    roots: &RootStore,
    server_name: &str,
    check_hostname: bool,
    now: i64,
) -> Result<std::ops::RangeInclusive<i64>, TlsError> {
    if cert_chain_der.is_empty() {
        return Err(TlsError::CertificateError("server sent empty certificate chain".to_string()));
    }
    if cert_chain_der.len() > MAX_CHAIN_LEN {
        return Err(TlsError::CertificateError(format!(
            "certificate chain exceeds maximum length of {MAX_CHAIN_LEN}"
        )));
    }

    let mut parsed = Vec::with_capacity(cert_chain_der.len());
    for der in cert_chain_der {
        parsed.push(parse_x509(der)?);
    }

    let leaf = &parsed[0];
    check_validity(leaf, now)?;
    if leaf.eku_present && !leaf.eku_server_auth {
        return Err(TlsError::CertificateError(
            "leaf certificate EKU does not allow serverAuth".to_string(),
        ));
    }

    if check_hostname {
        verify_hostname(leaf, server_name)?;
    }

    // Build and verify path from leaf up to a trust anchor
    let mut visited = vec![false; parsed.len()];
    visited[0] = true;
    let mut path_indices = vec![0usize];
    let mut sig_checks_left = MAX_SIG_CHECKS;
    build_and_verify_path(
        leaf,
        leaf,
        &parsed,
        roots,
        &mut visited,
        &mut path_indices,
        0,
        now,
        &mut sig_checks_left,
    )?
    .ok_or_else(|| {
        TlsError::CertificateError(
            "unable to verify certificate chain against any trusted root CA".to_string(),
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn build_and_verify_path<'a>(
    leaf: &X509Cert<'a>,
    current: &X509Cert<'a>,
    intermediates: &[X509Cert<'a>],
    roots: &RootStore,
    visited: &mut [bool],
    path_indices: &mut Vec<usize>,
    ca_depth: usize,
    now: i64,
    sig_checks_left: &mut usize,
) -> Result<Option<std::ops::RangeInclusive<i64>>, TlsError> {
    if path_indices.len() > MAX_CHAIN_LEN {
        return Ok(None);
    }

    // 1. Check if `current` is directly signed by a trust anchor in `roots`
    for root_der in roots.find_by_subject(current.issuer_raw) {
        let Ok(root) = parse_x509(root_der) else {
            continue;
        };
        if check_validity(&root, now).is_err() {
            continue;
        }
        if !check_ca_constraints(&root, ca_depth, true) {
            continue;
        }
        if *sig_checks_left == 0 {
            return Err(TlsError::CertificateError(
                "certificate chain signature verification limit exceeded".to_string(),
            ));
        }
        *sig_checks_left -= 1;
        if verify_signature(current, &root).is_ok() {
            if let Some(nc) = &root.name_constraints {
                check_name_constraints_on_leaf(nc, leaf.san.as_slice())?;
            }
            return Ok(Some(
                current.not_before.max(root.not_before)..=current.not_after.min(root.not_after),
            ));
        }
    }

    // 2. Otherwise try intermediate certificates provided by server
    for (idx, cand) in intermediates.iter().enumerate() {
        if visited[idx] || cand.subject_raw != current.issuer_raw {
            continue;
        }
        if check_validity(cand, now).is_err() {
            continue;
        }
        if !check_ca_constraints(cand, ca_depth, false) {
            continue;
        }
        if *sig_checks_left == 0 {
            return Err(TlsError::CertificateError(
                "certificate chain signature verification limit exceeded".to_string(),
            ));
        }
        *sig_checks_left -= 1;
        if verify_signature(current, cand).is_err() {
            continue;
        }
        if let Some(nc) = &cand.name_constraints {
            check_name_constraints_on_leaf(nc, leaf.san.as_slice())?;
        }
        visited[idx] = true;
        path_indices.push(idx);
        if let Some(validity) = build_and_verify_path(
            leaf,
            cand,
            intermediates,
            roots,
            visited,
            path_indices,
            ca_depth + 1,
            now,
            sig_checks_left,
        )? {
            return Ok(Some(
                current.not_before.max(*validity.start())..=current.not_after.min(*validity.end()),
            ));
        }
        path_indices.pop();
        visited[idx] = false;
    }

    Ok(None)
}

fn check_validity(cert: &X509Cert<'_>, now: i64) -> Result<(), TlsError> {
    if now < cert.not_before {
        return Err(TlsError::CertificateError("certificate is not yet valid".to_string()));
    }
    if now > cert.not_after {
        return Err(TlsError::CertificateError("certificate expired".to_string()));
    }
    Ok(())
}

fn check_ca_constraints(ca: &X509Cert<'_>, sub_ca_count: usize, is_trust_anchor: bool) -> bool {
    // Allow legacy X.509 v1 trust anchors without BasicConstraints
    if !ca.is_ca && !(is_trust_anchor && ca.version == 0) {
        return false;
    }
    if ca.has_key_usage && !ca.key_cert_sign {
        return false;
    }
    if let Some(max_len) = ca.path_len
        && sub_ca_count > max_len as usize
    {
        return false;
    }
    true
}

fn check_name_constraints_on_leaf(nc: &NameConstraints, san: &[SanEntry]) -> Result<(), TlsError> {
    for entry in san {
        if let SanEntry::Dns(dns) = entry {
            for excl in &nc.excluded_dns {
                if dns_matches_constraint(dns, excl) {
                    return Err(TlsError::CertificateError(format!(
                        "SAN '{dns}' excluded by CA nameConstraints '{excl}'"
                    )));
                }
            }
            if !nc.permitted_dns.is_empty()
                && !nc
                    .permitted_dns
                    .iter()
                    .any(|perm| dns_matches_constraint(dns, perm))
            {
                return Err(TlsError::CertificateError(format!(
                    "SAN '{dns}' not permitted by CA nameConstraints"
                )));
            }
        }
    }
    Ok(())
}

fn dns_matches_constraint(dns: &str, constraint: &str) -> bool {
    let d = dns.trim_end_matches('.').to_ascii_lowercase();
    let c = constraint
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if d.is_empty() || c.is_empty() {
        return false;
    }
    if d == c {
        return true;
    }
    d.ends_with(&format!(".{c}"))
}

pub fn verify_signature(child: &X509Cert<'_>, issuer: &X509Cert<'_>) -> Result<(), TlsError> {
    match child.sig_alg {
        SigAlg::WeakMd5OrSha1 => {
            return Err(TlsError::CertificateError(
                "weak signature algorithm (MD5/SHA-1) is forbidden".to_string(),
            ));
        }
        SigAlg::Unsupported => {
            return Err(TlsError::CertificateError(
                "unsupported certificate signature algorithm".to_string(),
            ));
        }
        _ => {}
    }

    let ok = match (&child.sig_alg, &issuer.spki) {
        (SigAlg::RsaPkcs1(alg), SubjectPublicKey::Rsa { n, e }) => {
            let Some(rsa) = RsaPublicKey::from_components(n, e) else {
                return Err(TlsError::CertificateError(
                    "invalid or too-small RSA public key (< 2048 bits)".to_string(),
                ));
            };
            let digest = alg.hash(child.tbs_raw);
            rsa.verify_pkcs1_v15(*alg, &digest, child.signature)
        }
        (SigAlg::RsaPss { hash, salt_len }, SubjectPublicKey::Rsa { n, e }) => {
            let Some(rsa) = RsaPublicKey::from_components(n, e) else {
                return Err(TlsError::CertificateError(
                    "invalid or <2048-bit RSA public key".to_string(),
                ));
            };
            let digest = hash.hash(child.tbs_raw);
            rsa.verify_pss_with_salt_len(*hash, *salt_len, &digest, child.signature)
        }
        (SigAlg::EcdsaSha256, SubjectPublicKey::EcP256(pub_bytes)) => {
            let mut h = Sha256::new();
            h.update(child.tbs_raw);
            ecdsa_p256_verify_der(pub_bytes, &h.finalize(), child.signature)
        }
        (SigAlg::EcdsaSha384, SubjectPublicKey::EcP256(pub_bytes)) => {
            let mut h = Sha384::new();
            h.update(child.tbs_raw);
            ecdsa_p256_verify_der(pub_bytes, &h.finalize(), child.signature)
        }
        (SigAlg::EcdsaSha256, SubjectPublicKey::EcP384(pub_bytes)) => {
            let mut h = Sha256::new();
            h.update(child.tbs_raw);
            ecdsa_p384_verify_der(pub_bytes, &h.finalize(), child.signature)
        }
        (SigAlg::EcdsaSha384, SubjectPublicKey::EcP384(pub_bytes)) => {
            let mut h = Sha384::new();
            h.update(child.tbs_raw);
            ecdsa_p384_verify_der(pub_bytes, &h.finalize(), child.signature)
        }
        (SigAlg::EcdsaSha512, SubjectPublicKey::EcP384(pub_bytes)) => {
            let mut h = Sha512::new();
            h.update(child.tbs_raw);
            ecdsa_p384_verify_der(pub_bytes, &h.finalize(), child.signature)
        }
        _ => false,
    };

    if ok {
        Ok(())
    } else {
        Err(TlsError::CertificateError("certificate signature verification failed".to_string()))
    }
}

/// Verify server hostname or IP against SAN entries (RFC 6125). CN fallback is never used.
pub fn verify_hostname(cert: &X509Cert<'_>, server_name: &str) -> Result<(), TlsError> {
    if let Ok(ip) = IpAddr::from_str(server_name) {
        let ip_bytes: Vec<u8> = match ip {
            IpAddr::V4(v4) => v4.octets().to_vec(),
            IpAddr::V6(v6) => v6.octets().to_vec(),
        };
        let matched = cert
            .san
            .iter()
            .any(|e| matches!(e, SanEntry::Ip(b) if *b == ip_bytes));
        return if matched {
            Ok(())
        } else {
            Err(TlsError::CertificateError(format!(
                "certificate SAN does not match IP '{server_name}'"
            )))
        };
    }

    let target = server_name.trim_end_matches('.').to_ascii_lowercase();
    if target.is_empty() {
        return Err(TlsError::CertificateError("empty server hostname".to_string()));
    }

    let matched = cert.san.iter().any(|e| match e {
        SanEntry::Dns(pattern) => dns_wildcard_match(pattern, &target),
        SanEntry::Ip(_) => false,
    });

    if matched {
        Ok(())
    } else {
        Err(TlsError::CertificateError(format!(
            "certificate SAN does not match hostname '{server_name}'"
        )))
    }
}

fn dns_wildcard_match(pattern: &str, hostname: &str) -> bool {
    let pattern = pattern.trim_end_matches('.');
    if pattern == hostname {
        return true;
    }
    let Some(rest_pat) = pattern.strip_prefix("*.") else {
        return false;
    };
    // Reject overly broad wildcards like `*.com` (must have >= 2 labels after `*.`)
    if !rest_pat.contains('.') || rest_pat.starts_with('.') {
        return false;
    }
    // Wildcard matches exactly one leftmost label
    let Some((first_label, rest_host)) = hostname.split_once('.') else {
        return false;
    };
    !first_label.is_empty() && rest_host == rest_pat
}

/// Helper for RSA hash digest calculation in CertificateVerify (TLS 1.2/1.3).
pub fn rsa_hash_alg_digest(alg: RsaHashAlg, data: &[u8]) -> Vec<u8> {
    alg.hash(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::tls::pem::decode_pem_certificates;
    use crate::native::tls::roots::RootSource;

    const TEST_ROOT_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----
MIIC/jCCAeagAwIBAgIUBn2W65B+s+wsdtpDre9zl74KGn4wDQYJKoZIhvcNAQEL
BQAwFzEVMBMGA1UEAwwMVGVzdCBSb290IENBMB4XDTI2MTEyMDA5NTkwNloXDTM2
MTExNzA5NTkwNlowFzEVMBMGA1UEAwwMVGVzdCBSb290IENBMIIBIjANBgkqhkiG
9w0BAQEFAAOCAQ8AMIIBCgKCAQEAle+1jDdpGFkOfflAkCahW60YyA7S8bPadOR7
Iozu5tAJCqiOD5sJoWC11o8D3miSt6eD5g0cUI7jXl6kTDDY+54IgiZpgfkLOsMW
4a2lbQbCzVd6XwhmJ2ERgYVkxAD2CaEwUBb3smcjGWlZ2IjseRRBKJT47mjd7qHk
dZZRbHopAoG0nKhjhnVecxGAt1e38taTTjkKRhiO/RYgeOz6pVbHTjMd8f+kVDxK
4JnghuOxfvTC74CnHStz3JeD3r4A13POi8kl+ynKE4pkx2d6nHAIC2H845W/HpV/
vN5h+fDHSKlSld5HTzLMv3/ZUUQR+lwKIRSu17tq2bikI9vzPwIDAQABo0IwQDAd
BgNVHQ4EFgQUnslksJSWuf6fuNHo9uSF3n2Qjx0wDwYDVR0TAQH/BAUwAwEB/zAO
BgNVHQ8BAf8EBAMCAQYwDQYJKoZIhvcNAQELBQADggEBABif+7sfeeEJANGsevXs
6TOECXCYMyVlxyWm1XbDnCnhGE18xCn8BrtbFHwxj0h8V2yq5/8n8besdnkSM/j/
N5KsGPCLzcpB+L/U17XWYjn7nb4Ywb6ZeHJPLaUdBZ18bknDMSiRjEzgEICUlOSY
OvqNxGJmeIZ8kJnpugQG/wa7UWQsYpU39/qAOLklh7Zgtbotzdwl4tXjYh7nRIzi
Y3ZTtBsLiSCA3DNdPihcOISYWlVDnj3/lvSe69lMPlQsDO8n9+/SBDMjQglS5+bh
dxVD3L1GJ7X1PNrQVKH4gQhJ9alRR/tBTJWIph37yD/1YwmF7bu5XK/FLxiat4GE
lUo=
-----END CERTIFICATE-----";

    const TEST_INTER_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----
MIIC6jCCAdKgAwIBAgIUNRi3rYoh5ReUS95bHvr7yoXfr40wDQYJKoZIhvcNAQEL
BQAwFzEVMBMGA1UEAwwMVGVzdCBSb290IENBMB4XDTI2MTEyMDA5NTkwNloXDTMx
MTExOTA5NTkwNlowHzEdMBsGA1UEAwwUVGVzdCBJbnRlcm1lZGlhdGUgQ0EwggEi
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQDc6MtT5a6j+13BTiMVqRHvhz7S
9tT4zaCTjDxjlFRm38iZqKANnM+YzSfnyazNGo3eGzQVDgUUsV3SkSEaDGBmOIIg
BT+KXFAqalKelamASOI0d6rlgxb6lFLTWFfy7nH7F0DGx47YkP4HK6n/frPeiczT
5lU5zHtQMKxcXDUBckm4zFL+kafz2N0WXbz9y01Me0x1BytzEybjp9AwSK0jMJGO
GT9uj1i2892+n5t/iRT6W8qdtJZBfE+i2wtkgN2d2BctQH+BJGo/Z3464SjpPUTw
G1NDw1ReoDRXMbNrZF4jarv+C5ixPUUVDxw94zcngUn1WAN7B3Upv85F63+7AgMB
AAGjJjAkMBIGA1UdEwEB/wQIMAYBAf8CAQAwDgYDVR0PAQH/BAQDAgIEMA0GCSqG
SIb3DQEBCwUAA4IBAQAdLzYOKYvuidOI/K+ropXr1besKFFX/Z9OFKviFJVstVIB
SloWFbODJ4SA0KjlE0PYkVMg4g3ABeuiuE8oWaDmXvYEaKewVV1yDBLgkhfaIbfm
rwL5R2phi02hN3xg7E/0bdo6MArPAX7kzaE5YEnUJmjjcTjP5VTN8B8FKylSw2Yx
bYzYtgjNOs5HLPGiUaL0sURdmgkdipKJsZ3NEnsPD2hCQhn954cVdO9SlyJAzY2Z
dL1em4kvh1rRNWSlxCfL7MQNCLwSMD8DTNaaR75sOGBsq6JLkcBiKur9rSlBPpJs
kEkHumb6fubt5TI2SITVXTnrsgdBm1IUvcw9g/zJ
-----END CERTIFICATE-----";

    const TEST_LEAF_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----
MIICTTCCATWgAwIBAgIUUyga0QIGoICm57u1SK6sHSMtwn0wDQYJKoZIhvcNAQEL
BQAwHzEdMBsGA1UEAwwUVGVzdCBJbnRlcm1lZGlhdGUgQ0EwHhcNMjYxMTIwMDk1
OTA2WhcNMjcxMTIwMDk1OTA2WjAfMR0wGwYDVQQDDBR3cm9uZy1jbi5leGFtcGxl
LmNvbTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABGLj42fKG35KYOPFQhAkMZHp
lTCFecF8CRImk646wW4ovPesV0z9EFUGeLk+L1c/d0kNwdILrBGj+jKl8FHfwS+j
TDBKMDMGA1UdEQQsMCqCD2FwaS5leGFtcGxlLmNvbYIRKi5zdmMuZXhhbXBsZS5j
b22HBH8AAAEwEwYDVR0lBAwwCgYIKwYBBQUHAwEwDQYJKoZIhvcNAQELBQADggEB
AJqhNeu4JKXhlib1OjpGZpWNsDfd2dc/69g3a57EylNQUo6dKHiAL256yaBugNzE
zR3bJkA3P2xABDNYIYvAAoH8wUWV50BbKxsk+hAya1MqkW9DPKLqmcoasq3oggRn
fecqvi104iPGM86+vV7/J1pq6nGehVws++FFQkIfSSkrpdrzO6jgoa2R2WbDDTuB
4RIf6gyHFqbQCHx4+pewdSysMEOeLIwb/FhfDFVLf9BfzArgGqbODEVbhYZPolLr
Q6AKUwv0rmkneLZwLAAzELbiMeCpnk4DBmDjcua3zkj9g9Xph2wZAmw9q5fZD2YR
WXzLvyx+jgyLFnWKSF7Qj3o=
-----END CERTIFICATE-----";

    #[test]
    fn wildcard_hostname_matching_rules() {
        assert!(dns_wildcard_match("*.example.com", "api.example.com"));
        assert!(!dns_wildcard_match("*.example.com", "a.b.example.com"));
        assert!(!dns_wildcard_match("*.example.com", "example.com"));
        assert!(!dns_wildcard_match("*.com", "example.com"));
        assert!(!dns_wildcard_match("foo*.example.com", "foobar.example.com"));
        assert!(dns_wildcard_match("db.internal", "db.internal"));

        assert!(dns_matches_constraint("EVIL.example.com", "example.com"));
        assert!(dns_matches_constraint("api.EXAMPLE.com.", ".example.com."));
        assert!(!dns_matches_constraint("badexample.com", "example.com"));
    }

    #[test]
    fn x509_chain_verification_positive_and_negative() {
        let roots = RootStore::from_pem(TEST_ROOT_PEM).unwrap();
        let inter_der = decode_pem_certificates(TEST_INTER_PEM).unwrap().pop().unwrap();
        let leaf_der = decode_pem_certificates(TEST_LEAF_PEM).unwrap().pop().unwrap();
        let chain = vec![leaf_der.clone(), inter_der.clone()];
        // Valid timestamp inside [2026-11-20, 2027-11-20]
        let valid_now = 1_795_200_000i64;

        assert!(verify_server_cert(&chain, &roots, "api.example.com", true, valid_now).is_ok());
        assert!(verify_server_cert(&chain, &roots, "v1.svc.example.com", true, valid_now).is_ok());
        assert!(verify_server_cert(&chain, &roots, "127.0.0.1", true, valid_now).is_ok());

        // CN fallback is forbidden when SAN is present
        assert!(verify_server_cert(&chain, &roots, "wrong-cn.example.com", true, valid_now).is_err());
        // Multi-level wildcard mismatch
        assert!(verify_server_cert(&chain, &roots, "a.b.svc.example.com", true, valid_now).is_err());
        // Expired / not yet valid
        assert!(verify_server_cert(&chain, &roots, "api.example.com", true, 2_000_000_000).is_err());
        assert!(verify_server_cert(&chain, &roots, "api.example.com", true, 1_500_000_000).is_err());
        // Missing intermediate
        assert!(verify_server_cert(std::slice::from_ref(&leaf_der), &roots, "api.example.com", true, valid_now).is_err());

        // Mutated signature in leaf fails
        let mut bad_leaf = leaf_der;
        let last_idx = bad_leaf.len() - 1;
        bad_leaf[last_idx] ^= 0xff;
        assert!(verify_server_cert(&[bad_leaf, inter_der], &roots, "api.example.com", true, valid_now).is_err());
    }

    #[test]
    fn system_roots_load_and_no_panics_on_garbage_der() {
        let store = RootStore::load(&RootSource::System).expect("system roots should load");
        assert!(!store.is_empty());

        let leaf_der = decode_pem_certificates(TEST_LEAF_PEM).unwrap().pop().unwrap();
        for len in 0..leaf_der.len().min(64) {
            let truncated = &leaf_der[..len];
            let res = std::panic::catch_unwind(|| parse_x509(truncated).is_ok());
            assert!(res.is_ok(), "parse_x509 panicked at len {len}");
        }
        let garbage = [0x30u8, 0x82, 0xff, 0xff, 0x01, 0x02, 0x03];
        assert!(std::panic::catch_unwind(|| parse_x509(&garbage).is_ok()).is_ok());
    }

    #[test]
    fn wycheproof_name_constraints_and_hostname_spoofing_cases() {
        let nc = NameConstraints {
            permitted_dns: vec!["example.com".to_string(), ".corp.local".to_string()],
            excluded_dns: vec!["evil.example.com".to_string(), "blocked.corp.local".to_string()],
        };
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("api.example.com".into())]).is_ok());
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("EXAMPLE.COM.".into())]).is_ok());
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("app.corp.local".into())]).is_ok());

        // Excluded subdomain or suffix spoofing must fail.
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("evil.example.com".into())]).is_err());
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("sub.EVIL.example.com.".into())]).is_err());
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("badexample.com".into())]).is_err());
        assert!(check_name_constraints_on_leaf(&nc, &[SanEntry::Dns("other.org".into())]).is_err());

        // Hostname wildcard / spoofing boundary checks.
        assert!(!dns_wildcard_match("*.*.example.com", "a.b.example.com"));
        assert!(!dns_wildcard_match("a*.example.com", "api.example.com"));
        assert!(!dns_wildcard_match("*", "localhost"));
        assert!(!dns_wildcard_match("*.1", "127.0.0.1"));
        assert!(!dns_matches_constraint("", "example.com"));

        // SAN IP vs DNS separation.
        let leaf_der = decode_pem_certificates(TEST_LEAF_PEM).unwrap().pop().unwrap();
        let leaf = parse_x509(&leaf_der).unwrap();
        assert!(verify_hostname(&leaf, "127.0.0.1").is_ok());
        assert!(verify_hostname(&leaf, "127.0.0.2").is_err());
        assert!(verify_hostname(&leaf, "::1").is_err());
        assert!(verify_hostname(&leaf, "").is_err());
    }
}
