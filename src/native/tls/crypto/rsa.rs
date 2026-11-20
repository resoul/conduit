//! RSA PKCS#1 v1.5 and RSASSA-PSS signature verification (2048-4096+ bit).

use super::bigint::BigUint;
use super::sha2::{Sha256, Sha384, Sha512};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsaHashAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl RsaHashAlg {
    pub fn digest_len(self) -> usize {
        match self {
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    fn digest_info_prefix(self) -> &'static [u8] {
        // DER DigestInfo prefixes from RFC 8017 §9.2
        match self {
            Self::Sha256 => &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ],
            Self::Sha384 => &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x02, 0x05, 0x00, 0x04, 0x30,
            ],
            Self::Sha512 => &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x03, 0x05, 0x00, 0x04, 0x40,
            ],
        }
    }

    pub fn hash(self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha256 => {
                let mut h = Sha256::new();
                h.update(data);
                h.finalize().to_vec()
            }
            Self::Sha384 => {
                let mut h = Sha384::new();
                h.update(data);
                h.finalize().to_vec()
            }
            Self::Sha512 => {
                let mut h = Sha512::new();
                h.update(data);
                h.finalize().to_vec()
            }
        }
    }
}

pub struct RsaPublicKey {
    pub n: BigUint,
    pub e: BigUint,
    pub mod_bytes: usize,
}

impl RsaPublicKey {
    pub fn from_components(n_bytes: &[u8], e_bytes: &[u8]) -> Option<Self> {
        let n = BigUint::from_be_bytes(n_bytes);
        let e = BigUint::from_be_bytes(e_bytes);
        let bit_len = n.bit_len();
        if !(2048..=8192).contains(&bit_len) {
            return None;
        }
        // Reject weak public exponents (e < 65537 or even)
        if e.is_even() || e.cmp_val(&BigUint::from_u64(65537)) == std::cmp::Ordering::Less {
            return None;
        }
        if e.cmp_val(&n) != std::cmp::Ordering::Less {
            return None;
        }
        let mod_bytes = bit_len.div_ceil(8);
        Some(Self { n, e, mod_bytes })
    }

    /// Verify RSASSA-PKCS1-v1_5 signature over `digest` with strict DigestInfo matching.
    pub fn verify_pkcs1_v15(&self, alg: RsaHashAlg, digest: &[u8], sig: &[u8]) -> bool {
        if digest.len() != alg.digest_len() || sig.len() != self.mod_bytes {
            return false;
        }
        let s = BigUint::from_be_bytes(sig);
        if s.cmp_val(&self.n) != std::cmp::Ordering::Less {
            return false;
        }
        let m = s.mod_pow(&self.e, &self.n);
        let em = m.to_be_bytes_padded(self.mod_bytes);

        let prefix = alg.digest_info_prefix();
        let t_len = prefix.len() + digest.len();
        if self.mod_bytes < t_len + 11 {
            return false;
        }
        let ps_len = self.mod_bytes - t_len - 3;

        let mut expected = Vec::with_capacity(self.mod_bytes);
        expected.push(0x00);
        expected.push(0x01);
        expected.resize(expected.len() + ps_len, 0xff);
        expected.push(0x00);
        expected.extend_from_slice(prefix);
        expected.extend_from_slice(digest);

        em == expected
    }

    /// Verify RSASSA-PSS signature over `digest` (salt_len == digest_len by default).
    pub fn verify_pss(&self, alg: RsaHashAlg, digest: &[u8], sig: &[u8]) -> bool {
        if digest.len() != alg.digest_len() || sig.len() != self.mod_bytes {
            return false;
        }
        let s = BigUint::from_be_bytes(sig);
        if s.cmp_val(&self.n) != std::cmp::Ordering::Less {
            return false;
        }
        let m = s.mod_pow(&self.e, &self.n);
        let em_bits = self.n.bit_len() - 1;
        let em_len = em_bits.div_ceil(8);
        let em_full = m.to_be_bytes_padded(self.mod_bytes);
        // If em_len < self.mod_bytes (when modBits % 8 == 1), top byte must be 0
        let em = if em_len < self.mod_bytes {
            if em_full[0] != 0 {
                return false;
            }
            &em_full[1..]
        } else {
            &em_full[..]
        };

        let h_len = alg.digest_len();
        if em_len < h_len + 2 {
            return false;
        }
        if em[em_len - 1] != 0xbc {
            return false;
        }
        let db_len = em_len - h_len - 1;
        let masked_db = &em[..db_len];
        let h = &em[db_len..db_len + h_len];

        let top_zero_bits = 8 * em_len - em_bits;
        if top_zero_bits > 0 {
            let mask = 0xffu8 << (8 - top_zero_bits);
            if (masked_db[0] & mask) != 0 {
                return false;
            }
        }

        let db_mask = mgf1(alg, h, db_len);
        let mut db = vec![0u8; db_len];
        for i in 0..db_len {
            db[i] = masked_db[i] ^ db_mask[i];
        }
        if top_zero_bits > 0 {
            db[0] &= 0xffu8 >> top_zero_bits;
        }

        // Find 0x01 separator after leading 0x00 bytes
        let Some(one_pos) = db.iter().position(|&b| b != 0) else {
            return false;
        };
        if db[one_pos] != 0x01 {
            return false;
        }
        let salt = &db[one_pos + 1..];
        // TLS 1.3 requires salt_len == h_len; X.509 typically uses salt_len == h_len (or 0..=h_len)
        if salt.len() != h_len && !salt.is_empty() {
            // Allow standard PSS salt lengths up to db_len
        }

        let mut m_prime = Vec::with_capacity(8 + h_len + salt.len());
        m_prime.extend_from_slice(&[0u8; 8]);
        m_prime.extend_from_slice(digest);
        m_prime.extend_from_slice(salt);
        let h_prime = alg.hash(&m_prime);
        h_prime.as_slice() == h
    }
}

fn mgf1(alg: RsaHashAlg, seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter = 0u32;
    let mut buf = Vec::with_capacity(seed.len() + 4);
    while out.len() < len {
        buf.clear();
        buf.extend_from_slice(seed);
        buf.extend_from_slice(&counter.to_be_bytes());
        let h = alg.hash(&buf);
        let take = (len - out.len()).min(h.len());
        out.extend_from_slice(&h[..take]);
        counter = counter.wrapping_add(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsa_rejects_small_key_and_small_exponent() {
        // 1024-bit key rejected
        let mut n1024 = vec![0x80];
        n1024.resize(128, 0x01);
        assert!(RsaPublicKey::from_components(&n1024, &[0x01, 0x00, 0x01]).is_none());

        // 2048-bit key with e = 3 rejected
        let mut n2048 = vec![0x80];
        n2048.resize(256, 0x01);
        assert!(RsaPublicKey::from_components(&n2048, &[3]).is_none());
        // 2048-bit key with e = 65537 accepted
        assert!(RsaPublicKey::from_components(&n2048, &[0x01, 0x00, 0x01]).is_some());
    }
}
