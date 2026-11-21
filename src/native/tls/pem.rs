//! PEM certificate decoder using Conduit's SIMD base64 decoder.

use crate::native::crypto::decode_base64_bytes;
use crate::native::tls::error::TlsError;

const MAX_PEM_BYTES: usize = 16 * 1024 * 1024;

/// Decode all blocks matching `expected_label` (e.g. `"CERTIFICATE"`, `"PUBLIC KEY"`)
/// from PEM text into DER byte vectors.
pub(crate) fn decode_pem_blocks(pem: &[u8], expected_label: &str) -> Result<Vec<Vec<u8>>, TlsError> {
    if pem.len() > MAX_PEM_BYTES {
        return Err(TlsError::CertificateError("PEM input exceeds 16 MiB limit".to_string()));
    }
    let text = std::str::from_utf8(pem)
        .map_err(|_| TlsError::CertificateError("PEM input is not valid UTF-8".to_string()))?;

    let begin_marker = format!("-----BEGIN {expected_label}-----");
    let end_marker = format!("-----END {expected_label}-----");

    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut b64 = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == begin_marker {
            if in_block {
                return Err(TlsError::CertificateError(format!(
                    "nested BEGIN {expected_label} in PEM"
                )));
            }
            in_block = true;
            b64.clear();
        } else if trimmed == end_marker {
            if !in_block {
                return Err(TlsError::CertificateError(format!(
                    "unexpected END {expected_label} in PEM"
                )));
            }
            in_block = false;
            let der = decode_base64_bytes(&b64).map_err(|_| {
                TlsError::CertificateError(format!("invalid base64 in PEM {expected_label}"))
            })?;
            if !der.is_empty() {
                blocks.push(der);
            }
            b64.clear();
        } else if in_block && !trimmed.is_empty() {
            if trimmed.starts_with("-----") {
                return Err(TlsError::CertificateError(format!(
                    "unexpected PEM boundary while decoding {expected_label}"
                )));
            }
            b64.push_str(trimmed);
        }
    }

    if in_block {
        return Err(TlsError::CertificateError(format!(
            "unterminated PEM {expected_label} block"
        )));
    }

    Ok(blocks)
}

/// Decode all `CERTIFICATE` blocks from PEM text into DER byte vectors.
pub fn decode_pem_certificates(pem: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    decode_pem_blocks(pem, "CERTIFICATE")
}

/// Decode the first PEM block matching `expected_label`, if present.
pub(crate) fn decode_pem_block(pem: &[u8], expected_label: &str) -> Result<Option<Vec<u8>>, TlsError> {
    Ok(decode_pem_blocks(pem, expected_label)?.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_decodes_multiple_certs_and_ignores_comments() {
        let pem = b"# Comment line\n-----BEGIN CERTIFICATE-----\nAQID\nBA==\n-----END CERTIFICATE-----\n\n-----BEGIN CERTIFICATE-----\n//8=\n-----END CERTIFICATE-----\n";
        let certs = decode_pem_certificates(pem).unwrap();
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0], vec![1, 2, 3, 4]);
        assert_eq!(certs[1], vec![0xff, 0xff]);
    }

    #[test]
    fn pem_decodes_named_public_key_blocks_and_rejects_malformed() {
        let spki_pem = b"-----BEGIN EC PARAMETERS-----\nBggqhkjOPQMBBw==\n-----END EC PARAMETERS-----\n-----BEGIN PUBLIC KEY-----\nAQIDBA==\n-----END PUBLIC KEY-----\n";
        assert_eq!(decode_pem_block(spki_pem, "PUBLIC KEY").unwrap(), Some(vec![1, 2, 3, 4]));
        assert_eq!(decode_pem_block(spki_pem, "RSA PUBLIC KEY").unwrap(), None);

        let rsa_pem = b"-----BEGIN RSA PUBLIC KEY-----\n//8=\n-----END RSA PUBLIC KEY-----\n";
        assert_eq!(decode_pem_block(rsa_pem, "RSA PUBLIC KEY").unwrap(), Some(vec![0xff, 0xff]));

        let unterminated = b"-----BEGIN PUBLIC KEY-----\nAQID\n";
        assert!(decode_pem_block(unterminated, "PUBLIC KEY").is_err());
    }
}
