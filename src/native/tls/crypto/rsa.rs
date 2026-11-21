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
        if n.is_even() {
            return None;
        }
        let bit_len = n.bit_len();
        if !(2048..=8192).contains(&bit_len) {
            return None;
        }
        // Reject weak public exponents (e < 65537 or even) and huge DoS exponents (e >= 2^33)
        if e.is_even()
            || e.cmp_val(&BigUint::from_u64(65537)) == std::cmp::Ordering::Less
            || e.bit_len() > 33
        {
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

    /// Verify RSASSA-PSS signature over `digest` requiring `salt_len == alg.digest_len()` (RFC 8446 §4.2.3).
    pub fn verify_pss(&self, alg: RsaHashAlg, digest: &[u8], sig: &[u8]) -> bool {
        self.verify_pss_with_salt_len(alg, alg.digest_len(), digest, sig)
    }

    /// Verify RSASSA-PSS signature over `digest` with an explicit expected `salt_len`.
    pub fn verify_pss_with_salt_len(
        &self,
        alg: RsaHashAlg,
        expected_salt_len: usize,
        digest: &[u8],
        sig: &[u8],
    ) -> bool {
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
        if em_len < h_len + expected_salt_len + 2 {
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
        if salt.len() != expected_salt_len {
            return false;
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

    #[test]
    fn wycheproof_rsa_pkcs1_v15_and_pss_vectors_and_rejections() {
        fn hex_to_bytes(s: &str) -> Vec<u8> {
            (0..s.len() / 2)
                .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
                .collect()
        }

        let n_bytes = hex_to_bytes(
            "a50558f8ac045a2cf46a3a8f58940d8da5309ad04f3d04a9e520b24ec34d65b4\
             86bbee388efd4fbe9e2c8f40ad9b60575baebfdb4834c713ef7fc4fe5cdf0834\
             38c599e6cdd9518cc72e330123173aafe059db9f6ac78d997c870c5c4b6b39f5\
             5c8690a238427ed490da7c5549645967bc98c60ad46b3f7a1a0a01194f5c1b7d\
             162330f65255e9b07ee583b8def81db7e3122ef01dacad1311f928b3371e34d3\
             4417dea51e9bd7a55765adb87c1ab4b27bcedc4dd8f582704d8860de2905d981\
             444e7096c8fa51ec3fde44e6416e9fc003c84d4fdf9a0a4dc6439baa1b463f99\
             b79e83427845143914ee40792e19b5271004ed6f6eadc12b8ce2970948973a13",
        );
        let e_bytes = [0x01, 0x00, 0x01];
        let pkcs1_sha256 = hex_to_bytes(
            "6497a128ba1a3dfd71d159f00e108a7b4d3325ea854b328a92851ec2b801e7f9\
             04832b57f458dd5d450f16a1886eb1cc7ce725333038f79bb865bb33b0dfdc5a\
             767dab1e565c669102ce0b922f8e52a3d0c3f644decb26ce5926b21c90cb1b2e\
             9155917245d5386b6db1e705a2d9f1f9a44baadf3782ee3d1f0a30366ec303cc\
             5d602da3154c3df96c6ed81e32592e35b34632159984bf02ea45fa22a0885acc\
             fe09262a7ccb36bbda56e2db2e916939f29420cd9f8ef30e5e7e70013eac940f\
             bf06b4f95f33cf8c0d90da578809861a39c591bb269571b4e007df567e9c2e06\
             7adc28a2857adb4d17fa420d414fe9fb406371568b847cb50c774bf0af8498eb",
        );
        let pss_sha256 = hex_to_bytes(
            "0806e0f7d13abf0751bdfd5e1dccb28c1b2ad58f4bb468f3afb2843cc88c70b1\
             8cfb7911a3d6f5dc8be2aa899c16355801a014fabb04e68470c913a6b87ab0fa\
             9c960c4e3ae8d095613523c1f1387b20d517a2160296b9cdf4a2b796738f2443\
             78418bfcb0c300b4077854b542fabfcad385c447d89e708c93f621d730d1e683\
             5be63d5d7ce7370fd1ef6b2017764502a1d85ca3dddf118881f0a99b2c7e80e2\
             cada635ab9b707cb59dafe06a81800e155a4e5fc056597e23acf898ac008e642\
             0a8ba9a3d6fb870af76c236efbdb2a463ed048bd60ffb9c3d75918f531ba50f1\
             27741044c7e7040c04f96803866ee223f9663fed4536cc49cbaea3c1e83ab145",
        );
        let pkcs1_sha384 = hex_to_bytes(
            "0663951f46d3940cc48c061a11ebfdc1115564e3af2ea10893d5abd03ddae5ca\
             c751b3c1996afdb947726068dcaf3c729e2245fff572625e9aae92d7ac31127f\
             1159feab060bc2fdc47afd3c36a57dabb4dda9339b158467dac435ec50c5a25a\
             17ebaf353a67d7c5c30169c253a6fab11cd8e9057320b182b8ba6d96740eda9d\
             9ca31d212fb31869bfcf23e3c3bbec446c2c9bbdf3f1f23a847b1bd06db6f8e0\
             57daae9b9187d3e1cb8e69987bc88d93db2bb40f8aa3e75252de67ef8fb25ea5\
             bd942fa5803b2db95df68a0bcd699f93a1de854041deb0f59e0d66f93c4cae43\
             d7c1e678387d567f18c60ad8ecae55190772d9f04e3aea41c96a3da0e241c537",
        );
        let pss_sha384 = hex_to_bytes(
            "1907227a073537b637415008a445cfd0bd4c6a2fdcedf36b8cfd3db1171d5b1c\
             f06155a809ead1e74eb03500cd8e557c43f0e8db910a10d2c455b4ce12873396\
             9dbf908e124a592bea6655f8909b99b6e39b812726b257902312610b4631d40b\
             92a4e8d07c4cf527d1d0ed0b9ebbc6443f43ef6c4e3e568cd35fe2c757587251\
             b65f7408dd5e90e5120187d2fdd9d10dd29e430d70643979acc7cc8375b3ffca\
             9a7fe77733d85c78950cb92ad605e11a2d977ce8bd1278803ab08f4e35dffc2e\
             a3afb58df7666f304a22d1766bece2c2b6eea7ba5302c7cd4f098e1d42d4adb2\
             fd8b614c9a0347f09e5e8efa85411d413883a29bd71cb8c1b107fa357cd24d38",
        );
        let pkcs1_sha512 = hex_to_bytes(
            "2758639f9f14e29e0ea7a490a860d3ee20b8d166bb124e88fbd44800cf588a6a\
             1c5578ac44b7e7ec2cf612162cc82e5770fb00904ea964f72c3a8fb9661c32b5\
             26cd6e385e0e37732ecdeb32d770d6fcb1d99049c35c218ba277eb35d90af355\
             5b5413cd3b07481a6359c62c53cfa20b93f1c452554d22d7feb5a812caf247f3\
             b4e19e7d16caf0d2f100a2ebce72a357dc6aa73027b6fadd072bd280b06be79a\
             866e160e27f57efb650b13cca3530bda1f6bdcd3f169b0b451d6b9d1891b291d\
             97847db2316b8116afee6d26c476208fce3080735ba2493ac5f65b209d51e40a\
             876d943504d1b96ce029086d40045028cf3031b58aacdc6cd32e69f8562ea5b7",
        );
        let pss_sha512 = hex_to_bytes(
            "2d5e76492af652ca1fb079646a25debde0c987585b85c09570414b10dcf7f6cc\
             acca9af5dd449828bc91a72e6a8f168ec2fdb14e4cdd325b7cec4f52ffcb8edd\
             e291cfe564041e2dad04c90bf2adbb67abefd9540a71165f94b92957f7b4bc4d\
             2d508c1a3a4baeb77c4c888a321dc34aaabb0911de5f17a7b4bc0185acfb1f06\
             ece4ca40240b88aa37f2ae3c057288a4905ec46eb628a451d026e9a68ded05ed\
             60a0c3010c2843f4067806a79bfdf85d58e5da2f40be601c2505935ff2185bd2\
             751fc407c6cebed4770a51de208a622b1e49cb6e2095ad1d4c66df2f14761835\
             834b03b24dbd042b03acece548547f218e2f13ff09a6ea8ea5688ae70dd03a34",
        );
        let pss_sha256_salt0 = hex_to_bytes(
            "0ec00121ab9126256a16cab1faa67a564c8142af94d24ab70116dd1936c11379\
             10112ce17b3561f82774c61c35c46f9225a8fb5deb8385dec811c6b40b665f1b\
             a1b5350ecea107f2dffb2d8cd9e02a34a475404360500af59464bd51cb4d4914\
             d680d997b3098b0d824560b0b2de626456cfd384bf6fdd3abe87f5cf7e379c51\
             af47ef4f283bd1723fc20aae983ffee5dd9a60739d8c16512229db39b0eaa83d\
             2f6f090b51dc90a81d66a1b4c8508c19a46f754a8355475a62f213534839da6a\
             9f67c405fbe80492e1a4bfce8fb4ddcd1295fc502d051f3b89c35dd5c80a998d\
             f6f443ba313bd811e8dd2c22971eb7e6b4f057bcbe27123614c02b872f9e0de4",
        );

        let key = RsaPublicKey::from_components(&n_bytes, &e_bytes).unwrap();
        let msg = b"conduit-wycheproof-rsa-test-message";
        let d256 = RsaHashAlg::Sha256.hash(msg);
        let d384 = RsaHashAlg::Sha384.hash(msg);
        let d512 = RsaHashAlg::Sha512.hash(msg);

        assert!(key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &pkcs1_sha256));
        assert!(key.verify_pss(RsaHashAlg::Sha256, &d256, &pss_sha256));
        assert!(key.verify_pkcs1_v15(RsaHashAlg::Sha384, &d384, &pkcs1_sha384));
        assert!(key.verify_pss(RsaHashAlg::Sha384, &d384, &pss_sha384));
        assert!(key.verify_pkcs1_v15(RsaHashAlg::Sha512, &d512, &pkcs1_sha512));
        assert!(key.verify_pss(RsaHashAlg::Sha512, &d512, &pss_sha512));
        assert!(key.verify_pss_with_salt_len(RsaHashAlg::Sha256, 0, &d256, &pss_sha256_salt0));

        // Reject even modulus, huge modulus (>8192 bits), and invalid/DoS exponents.
        let mut even_n = n_bytes.clone();
        *even_n.last_mut().unwrap() &= !1;
        assert!(RsaPublicKey::from_components(&even_n, &e_bytes).is_none());

        let mut huge_n = vec![0x80];
        huge_n.resize(1025, 0x01); // 8200 bits
        assert!(RsaPublicKey::from_components(&huge_n, &e_bytes).is_none());

        assert!(RsaPublicKey::from_components(&n_bytes, &[0x02, 0x00, 0x00, 0x00, 0x01]).is_none());
        assert!(RsaPublicKey::from_components(&n_bytes, &[0x01, 0x00, 0x00]).is_none());

        // Reject wrong lengths, edge values (0, 1, n, 0xff..ff), and bit flips.
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &pkcs1_sha256[..255]));
        let mut padded_zero = vec![0u8];
        padded_zero.extend_from_slice(&pkcs1_sha256);
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &padded_zero));

        let mut one_sig = vec![0u8; 256];
        one_sig[255] = 1;
        for edge_sig in [vec![0u8; 256], one_sig, n_bytes.clone(), vec![0xffu8; 256]] {
            assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &edge_sig));
            assert!(!key.verify_pss(RsaHashAlg::Sha256, &d256, &edge_sig));
        }

        let mut bad_pkcs1 = pkcs1_sha256.clone();
        bad_pkcs1[127] ^= 1;
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &bad_pkcs1));

        let mut bad_pss = pss_sha256.clone();
        bad_pss[255] ^= 1;
        assert!(!key.verify_pss(RsaHashAlg::Sha256, &d256, &bad_pss));

        let mut bad_d256 = d256.clone();
        bad_d256[0] ^= 0x80;
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &bad_d256, &pkcs1_sha256));
        assert!(!key.verify_pss(RsaHashAlg::Sha256, &bad_d256, &pss_sha256));

        // Cross-hash and cross-scheme / salt_len confusion must reject.
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha384, &d384, &pkcs1_sha256));
        assert!(!key.verify_pkcs1_v15(RsaHashAlg::Sha256, &d256, &pss_sha256));
        assert!(!key.verify_pss(RsaHashAlg::Sha256, &d256, &pkcs1_sha256));
        assert!(!key.verify_pss(RsaHashAlg::Sha256, &d256, &pss_sha256_salt0));
        assert!(!key.verify_pss_with_salt_len(RsaHashAlg::Sha256, 0, &d256, &pss_sha256));
    }
}
