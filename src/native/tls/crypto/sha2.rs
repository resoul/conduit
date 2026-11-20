//! SHA-2 family: streaming SHA-256 (via native::crypto), SHA-384, and SHA-512 (FIPS 180-4).

use crate::native::crypto::Sha256 as BaseSha256;

/// Unified streaming hash interface for HMAC, HKDF, and transcript hashing.
pub trait Digest: Clone {
    const BLOCK_LEN: usize;
    const OUTPUT_LEN: usize;

    fn new() -> Self;
    fn update(&mut self, data: &[u8]);
    fn finalize_into(self, out: &mut [u8]);

    fn digest_vec(data: &[u8]) -> Vec<u8> {
        let mut h = Self::new();
        h.update(data);
        let mut out = vec![0u8; Self::OUTPUT_LEN];
        h.finalize_into(&mut out);
        out
    }
}

#[derive(Clone)]
pub struct Sha256(BaseSha256);

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self(BaseSha256::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 32] {
        self.0.finalize()
    }
}

impl Digest for Sha256 {
    const BLOCK_LEN: usize = 64;
    const OUTPUT_LEN: usize = 32;

    fn new() -> Self {
        Self::new()
    }

    fn update(&mut self, data: &[u8]) {
        self.update(data);
    }

    fn finalize_into(self, out: &mut [u8]) {
        out[..32].copy_from_slice(&self.finalize());
    }
}

// ── SHA-512 / SHA-384 (FIPS 180-4) ───────────────────────────────────────────

const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

const SHA384_INIT: [u64; 8] = [
    0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
    0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
];

const SHA512_INIT: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

fn sha512_compress(h: &mut [u64; 8], blocks: &[[u8; 128]]) {
    for chunk in blocks {
        let mut w = [0u64; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 8;
            *word = u64::from_be_bytes([
                chunk[j],
                chunk[j + 1],
                chunk[j + 2],
                chunk[j + 3],
                chunk[j + 4],
                chunk[j + 5],
                chunk[j + 6],
                chunk[j + 7],
            ]);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K512[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

#[derive(Clone)]
struct Sha512Core {
    state: [u64; 8],
    buf: [u8; 128],
    buf_len: usize,
    total_bytes: u128,
}

impl Sha512Core {
    fn new(init: [u64; 8]) -> Self {
        Self {
            state: init,
            buf: [0u8; 128],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u128);
        if self.buf_len > 0 {
            let take = (128 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 128 {
                sha512_compress(&mut self.state, std::slice::from_ref(&self.buf));
                self.buf_len = 0;
            }
        }
        let (full_blocks, rem) = data.as_chunks::<128>();
        if !full_blocks.is_empty() {
            sha512_compress(&mut self.state, full_blocks);
        }
        if !rem.is_empty() {
            self.buf[..rem.len()].copy_from_slice(rem);
            self.buf_len = rem.len();
        }
    }

    fn finalize_words(mut self) -> [u64; 8] {
        let mut tail = [0u8; 256];
        tail[..self.buf_len].copy_from_slice(&self.buf[..self.buf_len]);
        tail[self.buf_len] = 0x80;
        let total_tail = if self.buf_len < 112 { 128 } else { 256 };
        let bit_len = self.total_bytes.wrapping_mul(8);
        tail[total_tail - 16..total_tail].copy_from_slice(&bit_len.to_be_bytes());
        let tail_blocks = tail[..total_tail].as_chunks::<128>().0;
        sha512_compress(&mut self.state, tail_blocks);
        self.state
    }
}

#[derive(Clone)]
pub struct Sha384(Sha512Core);

impl Default for Sha384 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha384 {
    pub fn new() -> Self {
        Self(Sha512Core::new(SHA384_INIT))
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 48] {
        let words = self.0.finalize_words();
        let mut out = [0u8; 48];
        for (i, w) in words.iter().take(6).enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

impl Digest for Sha384 {
    const BLOCK_LEN: usize = 128;
    const OUTPUT_LEN: usize = 48;

    fn new() -> Self {
        Self::new()
    }

    fn update(&mut self, data: &[u8]) {
        self.update(data);
    }

    fn finalize_into(self, out: &mut [u8]) {
        out[..48].copy_from_slice(&self.finalize());
    }
}

#[derive(Clone)]
pub struct Sha512(Sha512Core);

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha512 {
    pub fn new() -> Self {
        Self(Sha512Core::new(SHA512_INIT))
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 64] {
        let words = self.0.finalize_words();
        let mut out = [0u8; 64];
        for (i, w) in words.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

impl Digest for Sha512 {
    const BLOCK_LEN: usize = 128;
    const OUTPUT_LEN: usize = 64;

    fn new() -> Self {
        Self::new()
    }

    fn update(&mut self, data: &[u8]) {
        self.update(data);
    }

    fn finalize_into(self, out: &mut [u8]) {
        out[..64].copy_from_slice(&self.finalize());
    }
}

pub fn sha384_bytes(data: &[u8]) -> [u8; 48] {
    let mut h = Sha384::new();
    h.update(data);
    h.finalize()
}

pub fn sha512_bytes(data: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn sha256_streaming_cavp_vectors() {
        // Empty
        let h = Sha256::new();
        assert_eq!(
            encode_hex(&h.finalize()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc"
        let mut h = Sha256::new();
        h.update(b"a");
        h.update(b"bc");
        assert_eq!(
            encode_hex(&h.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 448 bits ("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
        let mut h = Sha256::new();
        h.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            encode_hex(&h.finalize()),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // 1,000,000 'a'
        let mut h = Sha256::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            encode_hex(&h.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha384_cavp_vectors() {
        assert_eq!(
            encode_hex(&sha384_bytes(b"")),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(
            encode_hex(&sha384_bytes(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            encode_hex(&sha384_bytes(
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
            )),
            "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039"
        );
        let mut h = Sha384::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            encode_hex(&h.finalize()),
            "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"
        );
    }

    #[test]
    fn sha512_cavp_vectors() {
        assert_eq!(
            encode_hex(&sha512_bytes(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            encode_hex(&sha512_bytes(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        let mut h = Sha512::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            encode_hex(&h.finalize()),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
    }
}
