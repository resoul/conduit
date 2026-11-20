//! PEM certificate decoder using Conduit's SIMD base64 decoder.

use crate::native::crypto::decode_base64_bytes;
use crate::native::tls::error::TlsError;

const MAX_PEM_BYTES: usize = 16 * 1024 * 1024;
const BEGIN_CERT: &str = "-----BEGIN CERTIFICATE-----";
const END_CERT: &str = "-----END CERTIFICATE-----";

/// Decode all `CERTIFICATE` blocks from PEM text into DER byte vectors.
pub fn decode_pem_certificates(pem: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    if pem.len() > MAX_PEM_BYTES {
        return Err(TlsError::CertificateError("PEM input exceeds 16 MiB limit".to_string()));
    }
    let text = std::str::from_utf8(pem)
        .map_err(|_| TlsError::CertificateError("PEM input is not valid UTF-8".to_string()))?;

    let mut certs = Vec::new();
    let mut in_cert = false;
    let mut b64 = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN_CERT {
            if in_cert {
                return Err(TlsError::CertificateError("nested BEGIN CERTIFICATE in PEM".to_string()));
            }
            in_cert = true;
            b64.clear();
        } else if trimmed == END_CERT {
            if !in_cert {
                return Err(TlsError::CertificateError("unexpected END CERTIFICATE in PEM".to_string()));
            }
            in_cert = false;
            let der = decode_base64_bytes(&b64)
                .map_err(|_| TlsError::CertificateError("invalid base64 in PEM certificate".to_string()))?;
            if !der.is_empty() {
                certs.push(der);
            }
            b64.clear();
        } else if in_cert && !trimmed.is_empty() {
            b64.push_str(trimmed);
        }
    }

    if in_cert {
        return Err(TlsError::CertificateError("unterminated PEM certificate block".to_string()));
    }

    Ok(certs)
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
}
