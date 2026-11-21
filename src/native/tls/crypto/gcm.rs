//! AES-GCM (128 & 256) AEAD (NIST SP 800-38D).

use crate::native::crypto::constant_time_eq;

use super::Zeroize;
use super::aes::AesKey;
use super::ghash::Ghash;

#[derive(Clone)]
pub struct AesGcm {
    aes: AesKey,
    h: [u8; 16],
}

impl Drop for AesGcm {
    fn drop(&mut self) {
        self.h.zeroize();
    }
}

impl AesGcm {
    pub fn new(key: &[u8]) -> Option<Self> {
        let aes = AesKey::new(key)?;
        let mut h = [0u8; 16];
        aes.encrypt_block(&mut h);
        Some(Self { aes, h })
    }

    pub fn new_portable(key: &[u8]) -> Option<Self> {
        let aes = AesKey::new_portable(key)?;
        let mut h = [0u8; 16];
        aes.encrypt_block(&mut h);
        Some(Self { aes, h })
    }

    fn ctr_xor(&self, iv: &[u8; 12], initial_ctr: u32, buf: &mut [u8]) {
        let mut counter_block = [0u8; 16];
        counter_block[..12].copy_from_slice(iv);
        let mut ctr = initial_ctr;
        for chunk in buf.chunks_mut(16) {
            counter_block[12..16].copy_from_slice(&ctr.to_be_bytes());
            let mut keystream = counter_block;
            self.aes.encrypt_block(&mut keystream);
            for (b, ks) in chunk.iter_mut().zip(keystream.iter()) {
                *b ^= *ks;
            }
            ctr = ctr.wrapping_add(1);
        }
    }

    fn compute_tag(&self, iv: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
        let mut ghash = Ghash::new(&self.h);
        ghash.update_padded(aad);
        ghash.update_padded(ciphertext);

        let mut lens = [0u8; 16];
        lens[..8].copy_from_slice(&((aad.len() as u64).wrapping_mul(8)).to_be_bytes());
        lens[8..].copy_from_slice(&((ciphertext.len() as u64).wrapping_mul(8)).to_be_bytes());
        ghash.update_block(&lens);

        let mut tag = ghash.finalize();
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(iv);
        j0[15] = 1;
        let mut ej0 = j0;
        self.aes.encrypt_block(&mut ej0);
        for (t, &e) in tag.iter_mut().zip(ej0.iter()) {
            *t ^= e;
        }
        tag
    }

    pub fn seal_in_place(&self, iv: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
        self.ctr_xor(iv, 2, buf);
        self.compute_tag(iv, aad, buf)
    }

    pub fn open_in_place(
        &self,
        iv: &[u8; 12],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; 16],
    ) -> bool {
        let expected = self.compute_tag(iv, aad, buf);
        if !constant_time_eq(&expected, tag) {
            return false;
        }
        self.ctr_xor(iv, 2, buf);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn wycheproof_aes_gcm_known_answers() {
        // C2SP Wycheproof aes_gcm_test.json v1, test cases 1 and 2.
        // https://github.com/C2SP/wycheproof/blob/main/testvectors_v1/aes_gcm_test.json
        let hex_to_bytes = |value: &str| {
            (0..value.len()).step_by(2)
                .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let key = hex_to_bytes("5b9604fe14eadba931b0ccf34843dab9");
        let plaintext = hex_to_bytes("001d0c231287c1182784554ca3a21908");
        for (iv, aad, ciphertext, tag) in [
            ("028318abc1824029138141a2", "", "26073cc1d851beff176384dc9896d5ff",
                "0a3ea7a5487cb5f7d70fb6c58d038554"),
            ("921d2507fa8007b7bd067d34", "00112233445566778899aabbccddeeff",
                "49d8b9783e911913d87094d1f63cc765", "1e348ba07cca2cf04c618cb4d43a5b92"),
        ] {
            let iv = hex_to_bytes(iv).try_into().unwrap();
            let aad = hex_to_bytes(aad);
            let expected = hex_to_bytes(ciphertext);
            let tag: [u8; 16] = hex_to_bytes(tag).try_into().unwrap();
            for cipher in [AesGcm::new(&key).unwrap(), AesGcm::new_portable(&key).unwrap()] {
                let mut sealed = plaintext.clone();
                assert_eq!(cipher.seal_in_place(&iv, &aad, &mut sealed), tag);
                assert_eq!(sealed, expected);
                assert!(cipher.open_in_place(&iv, &aad, &mut sealed, &tag));
                assert_eq!(sealed, plaintext);
            }
        }
    }

    #[test]
    fn aes128_gcm_nist_test_case_2_and_4() {
        // Test Case 2: 16-byte plaintext, empty AAD
        let key = [0u8; 16];
        let iv = [0u8; 12];
        let mut buf = [0u8; 16];
        let gcm = AesGcm::new(&key).unwrap();
        let tag = gcm.seal_in_place(&iv, &[], &mut buf);
        assert_eq!(encode_hex(&buf), "0388dace60b6a392f328c2b971b2fe78");
        assert_eq!(encode_hex(&tag), "ab6e47d42cec13bdf53a67b21257bddf");

        assert!(gcm.open_in_place(&iv, &[], &mut buf, &tag));
        assert_eq!(buf, [0u8; 16]);

        // Bad tag rejection
        let mut bad_tag = tag;
        bad_tag[0] ^= 1;
        assert!(!gcm.open_in_place(&iv, &[], &mut buf, &bad_tag));

        // Empty plaintext + empty AAD (Test Case 1)
        let mut empty = [];
        let tag1 = gcm.seal_in_place(&iv, &[], &mut empty);
        assert_eq!(encode_hex(&tag1), "58e2fccefa7e3061367f1d57a4e7455a");
    }

    #[test]
    fn aes256_gcm_nist_vector() {
        let key = [0u8; 32];
        let iv = [0u8; 12];
        let mut buf = [0u8; 16];
        let gcm = AesGcm::new(&key).unwrap();
        let tag = gcm.seal_in_place(&iv, &[], &mut buf);
        assert_eq!(encode_hex(&buf), "cea7403d4d606b6e074ec5d3baf39d18");
        assert_eq!(encode_hex(&tag), "d0d1c8a799996bf0265b98b5d48ab919");
        assert!(gcm.open_in_place(&iv, &[], &mut buf, &tag));
        assert_eq!(buf, [0u8; 16]);
    }
}
