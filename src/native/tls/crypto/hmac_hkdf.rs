//! Generic HMAC (RFC 2104) and HKDF (RFC 5869) + TLS 1.3 HKDF-Expand-Label (RFC 8446).

use super::Zeroize;
use super::sha2::Digest;

/// Maximum block size among SHA-256 (64) and SHA-384/512 (128).
const MAX_BLOCK: usize = 128;

pub struct Hmac<D: Digest> {
    inner: D,
    opad: [u8; MAX_BLOCK],
}

impl<D: Digest> Hmac<D> {
    pub fn new(key: &[u8]) -> Self {
        let mut k_block = [0u8; MAX_BLOCK];
        if key.len() > D::BLOCK_LEN {
            let mut h = D::new();
            h.update(key);
            h.finalize_into(&mut k_block[..D::OUTPUT_LEN]);
        } else {
            k_block[..key.len()].copy_from_slice(key);
        }

        let mut ipad = [0x36u8; MAX_BLOCK];
        let mut opad = [0x5cu8; MAX_BLOCK];
        for i in 0..D::BLOCK_LEN {
            ipad[i] ^= k_block[i];
            opad[i] ^= k_block[i];
        }
        k_block.zeroize();

        let mut inner = D::new();
        inner.update(&ipad[..D::BLOCK_LEN]);
        ipad.zeroize();

        Self { inner, opad }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub fn finalize_into(mut self, out: &mut [u8]) {
        let mut inner_hash = [0u8; 64];
        self.inner.finalize_into(&mut inner_hash[..D::OUTPUT_LEN]);

        let mut outer = D::new();
        outer.update(&self.opad[..D::BLOCK_LEN]);
        outer.update(&inner_hash[..D::OUTPUT_LEN]);
        outer.finalize_into(&mut out[..D::OUTPUT_LEN]);

        inner_hash.zeroize();
        self.opad.zeroize();
    }

    pub fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut h = Self::new(key);
        h.update(data);
        let mut out = vec![0u8; D::OUTPUT_LEN];
        h.finalize_into(&mut out);
        out
    }
}

pub fn hkdf_extract<D: Digest>(salt: &[u8], ikm: &[u8]) -> Vec<u8> {
    if salt.is_empty() {
        let zeros = [0u8; 64];
        Hmac::<D>::mac(&zeros[..D::OUTPUT_LEN], ikm)
    } else {
        Hmac::<D>::mac(salt, ikm)
    }
}

pub fn hkdf_expand<D: Digest>(prk: &[u8], info: &[u8], out: &mut [u8]) {
    let hash_len = D::OUTPUT_LEN;
    debug_assert!(out.len() <= 255 * hash_len);
    let mut t = [0u8; 64];
    let mut t_len = 0usize;
    let mut counter: u8 = 1;
    let mut offset = 0usize;

    while offset < out.len() {
        let mut mac = Hmac::<D>::new(prk);
        if t_len > 0 {
            mac.update(&t[..t_len]);
        }
        mac.update(info);
        mac.update(&[counter]);
        mac.finalize_into(&mut t[..hash_len]);
        t_len = hash_len;

        let copy_len = (out.len() - offset).min(hash_len);
        out[offset..offset + copy_len].copy_from_slice(&t[..copy_len]);
        offset += copy_len;
        counter = counter.wrapping_add(1);
    }
    t.zeroize();
}

/// TLS 1.3 HKDF-Expand-Label(Secret, Label, Context, Length) (RFC 8446 §7.1).
pub fn hkdf_expand_label<D: Digest>(secret: &[u8], label: &[u8], context: &[u8], out: &mut [u8]) {
    let mut hkdf_label = Vec::with_capacity(2 + 1 + 6 + label.len() + 1 + context.len());
    let len16 = (out.len() as u16).to_be_bytes();
    hkdf_label.extend_from_slice(&len16);
    hkdf_label.push((6 + label.len()) as u8);
    hkdf_label.extend_from_slice(b"tls13 ");
    hkdf_label.extend_from_slice(label);
    hkdf_label.push(context.len() as u8);
    hkdf_label.extend_from_slice(context);

    hkdf_expand::<D>(secret, &hkdf_label, out);
    hkdf_label.zeroize();
}

/// TLS 1.3 Derive-Secret(Secret, Label, MessagesHash) (RFC 8446 §7.1).
pub fn derive_secret<D: Digest>(secret: &[u8], label: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; D::OUTPUT_LEN];
    hkdf_expand_label::<D>(secret, label, transcript_hash, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;
    use crate::native::tls::crypto::sha2::{Sha256, Sha384};

    #[test]
    fn hmac_rfc4231_vectors() {
        // Test Case 1
        let mac = Hmac::<Sha256>::mac(&[0x0bu8; 20], b"Hi There");
        assert_eq!(
            encode_hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Test Case 2 (Jefe)
        let mac = Hmac::<Sha256>::mac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            encode_hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test Case 1 SHA-384
        let mac384 = Hmac::<Sha384>::mac(&[0x0bu8; 20], b"Hi There");
        assert_eq!(
            encode_hex(&mac384),
            "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6"
        );
    }

    #[test]
    fn hkdf_rfc5869_vectors() {
        // Test Case 1
        let ikm = [0x0bu8; 22];
        let salt = [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c];
        let info = [0xf0u8, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let prk = hkdf_extract::<Sha256>(&salt, &ikm);
        assert_eq!(
            encode_hex(&prk),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
        let mut okm = [0u8; 42];
        hkdf_expand::<Sha256>(&prk, &info, &mut okm);
        assert_eq!(
            encode_hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );

        // Test Case 3 (zero-length salt & info)
        let prk3 = hkdf_extract::<Sha256>(&[], &[0x0bu8; 22]);
        assert_eq!(
            encode_hex(&prk3),
            "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04"
        );
        let mut okm3 = [0u8; 42];
        hkdf_expand::<Sha256>(&prk3, &[], &mut okm3);
        assert_eq!(
            encode_hex(&okm3),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
    }

    #[test]
    fn hkdf_expand_label_rfc8448_vector() {
        // RFC 8448 §3 Simple 1-RTT Handshake: Early Secret & Derived Secret
        let early_secret = hkdf_extract::<Sha256>(&[0u8; 32], &[0u8; 32]);
        assert_eq!(
            encode_hex(&early_secret),
            "33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a"
        );
        let empty_hash = Sha256::digest_vec(b"");
        let derived = derive_secret::<Sha256>(&early_secret, b"derived", &empty_hash);
        assert_eq!(
            encode_hex(&derived),
            "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba"
        );
    }
}
