//! ChaCha20-Poly1305 AEAD (RFC 8439 §2.8).

use crate::native::crypto::constant_time_eq;

use super::Zeroize;
use super::chacha20::{chacha20_block, chacha20_xor};
use super::poly1305::Poly1305;

#[derive(Clone)]
pub struct ChaCha20Poly1305 {
    key: [u8; 32],
}

impl Drop for ChaCha20Poly1305 {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl ChaCha20Poly1305 {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    fn compute_tag(&self, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
        let mut block0 = chacha20_block(&self.key, 0, nonce);
        let mut otk = [0u8; 32];
        otk.copy_from_slice(&block0[..32]);
        block0.zeroize();

        let mut poly = Poly1305::new(&otk);
        otk.zeroize();

        poly.update(aad);
        let aad_pad = (16 - (aad.len() % 16)) % 16;
        if aad_pad > 0 {
            poly.update(&[0u8; 16][..aad_pad]);
        }

        poly.update(ciphertext);
        let ct_pad = (16 - (ciphertext.len() % 16)) % 16;
        if ct_pad > 0 {
            poly.update(&[0u8; 16][..ct_pad]);
        }

        poly.update(&(aad.len() as u64).to_le_bytes());
        poly.update(&(ciphertext.len() as u64).to_le_bytes());
        poly.finalize()
    }

    pub fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
        chacha20_xor(&self.key, 1, nonce, buf);
        self.compute_tag(nonce, aad, buf)
    }

    pub fn open_in_place(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; 16],
    ) -> bool {
        let expected = self.compute_tag(nonce, aad, buf);
        if !constant_time_eq(&expected, tag) {
            return false;
        }
        chacha20_xor(&self.key, 1, nonce, buf);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn chacha20_poly1305_rfc8439_section_2_8_2() {
        let key: [u8; 32] = [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
            0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b,
            0x9c, 0x9d, 0x9e, 0x9f,
        ];
        let nonce: [u8; 12] = [
            0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        ];
        let aad = [
            0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
        ];
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let mut buf = pt.to_vec();
        let aead = ChaCha20Poly1305::new(key);
        let tag = aead.seal_in_place(&nonce, &aad, &mut buf);

        assert_eq!(
            encode_hex(&buf[..16]),
            "d31a8d34648e60db7b86afbc53ef7ec2"
        );
        assert_eq!(encode_hex(&tag), "1ae10b594f09e26a7e902ecbd0600691");

        assert!(aead.open_in_place(&nonce, &aad, &mut buf, &tag));
        assert_eq!(buf.as_slice(), pt);

        let mut bad_tag = tag;
        bad_tag[15] ^= 0x80;
        assert!(!aead.open_in_place(&nonce, &aad, &mut buf, &bad_tag));
    }

    #[test]
    fn wycheproof_chacha20_poly1305_edge_cases_and_rejections() {
        let key: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(11).wrapping_add(1));
        let nonce: [u8; 12] = [0x01, 0x02, 0x03, 0x04, 0, 0, 0, 1, 0xaa, 0xbb, 0xcc, 0xdd];
        let aead = ChaCha20Poly1305::new(key);

        for &aad_len in &[0usize, 1, 15, 16, 17, 63, 64, 65, 130] {
            for &pt_len in &[0usize, 1, 15, 16, 17, 63, 64, 65, 130] {
                let aad: Vec<u8> = (0..aad_len).map(|i| (i * 5 + 1) as u8).collect();
                let pt: Vec<u8> = (0..pt_len).map(|i| (i * 9 + 2) as u8).collect();
                let mut ct = pt.clone();
                let tag = aead.seal_in_place(&nonce, &aad, &mut ct);

                // Bit flip in tag leaves buffer untouched and rejects.
                let mut bad_tag = tag;
                bad_tag[0] ^= 1;
                let mut scratch = ct.clone();
                assert!(!aead.open_in_place(&nonce, &aad, &mut scratch, &bad_tag));
                assert_eq!(scratch, ct);

                // Bit flip in AAD or nonce rejects without decrypting.
                let mut bad_aad = aad.clone();
                if bad_aad.is_empty() {
                    bad_aad.push(0);
                } else {
                    bad_aad[aad_len / 2] ^= 0x04;
                }
                assert!(!aead.open_in_place(&nonce, &bad_aad, &mut scratch, &tag));
                assert_eq!(scratch, ct);

                let mut bad_nonce = nonce;
                bad_nonce[0] ^= 0x01;
                assert!(!aead.open_in_place(&bad_nonce, &aad, &mut scratch, &tag));
                assert_eq!(scratch, ct);

                // AAD/CT boundary move attack (padding/length block collision guard) must reject.
                if !aad.is_empty() {
                    let mut moved_ct = Vec::with_capacity(ct.len() + 1);
                    moved_ct.push(*aad.last().unwrap());
                    moved_ct.extend_from_slice(&ct);
                    let expected = moved_ct.clone();
                    assert!(!aead.open_in_place(&nonce, &aad[..aad.len() - 1], &mut moved_ct, &tag));
                    assert_eq!(moved_ct, expected);
                }
                if !ct.is_empty() {
                    let mut bad_ct = ct.clone();
                    bad_ct[ct.len() - 1] ^= 0x01;
                    let expected = bad_ct.clone();
                    assert!(!aead.open_in_place(&nonce, &aad, &mut bad_ct, &tag));
                    assert_eq!(bad_ct, expected);
                }

                assert!(aead.open_in_place(&nonce, &aad, &mut scratch, &tag));
                assert_eq!(scratch, pt);
            }
        }
    }
}
