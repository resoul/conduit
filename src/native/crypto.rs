use std::cell::RefCell;
#[cfg(not(windows))]
use std::fs::File;
#[cfg(not(windows))]
use std::io::Read;
use std::sync::Arc;

use crate::native::b64_simd;
use crate::vm::value::{Obj, SCHEMA_CRYPTO_ERROR, StructData};

pub const CRYPTO_SHA256: u8 = 0;
pub const CRYPTO_HMAC_SHA256: u8 = 1;
pub const CRYPTO_B64_ENC: u8 = 2;
pub const CRYPTO_B64_DEC: u8 = 3;
pub const CRYPTO_HEX_ENC: u8 = 4;
pub const CRYPTO_HEX_DEC: u8 = 5;
pub const CRYPTO_UUID_V4: u8 = 6;
pub const CRYPTO_RAND_HEX: u8 = 7;
pub const CRYPTO_RAND_INT: u8 = 8;
pub const CRYPTO_HASH_PASSWORD: u8 = 9;
pub const CRYPTO_VERIFY_PASSWORD: u8 = 10;
pub const CRYPTO_CONST_TIME_EQ: u8 = 11;

pub const MAX_PBKDF2_ITERATIONS: u32 = 10_000_000;
const DEFAULT_PBKDF2_ITERATIONS: u32 = 600_000;

#[inline]
pub fn make_crypto_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_CRYPTO_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

// ── SHA-256 (FIPS 180-4) ─────────────────────────────────────────────────────

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[inline]
fn sha256_compress_scalar(h: &mut [u32; 8], blocks: &[[u8; 64]]) {
    for chunk in blocks {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
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

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "sha2")]
unsafe fn sha256_compress_aarch64(h: &mut [u32; 8], blocks: &[[u8; 64]]) {
    use std::arch::aarch64::*;
    unsafe {
        let mut abcd = vld1q_u32(h.as_ptr());
        let mut efgh = vld1q_u32(h.as_ptr().add(4));

        for block in blocks {
            let abcd_orig = abcd;
            let efgh_orig = efgh;

            let p = block.as_ptr();
            let mut w0 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p)));
            let mut w1 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(16))));
            let mut w2 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(32))));
            let mut w3 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(48))));

            macro_rules! round4 {
                ($w:expr, $k_off:expr) => {
                    let k = vld1q_u32(K256.as_ptr().add($k_off));
                    let tmp = vaddq_u32($w, k);
                    let abcd_save = abcd;
                    abcd = vsha256hq_u32(abcd, efgh, tmp);
                    efgh = vsha256h2q_u32(efgh, abcd_save, tmp);
                };
            }

            // Rounds 0..16
            round4!(w0, 0);
            round4!(w1, 4);
            round4!(w2, 8);
            round4!(w3, 12);

            // Rounds 16..64
            let mut k_off = 16usize;
            while k_off < 64 {
                w0 = vsha256su1q_u32(vsha256su0q_u32(w0, w1), w2, w3);
                round4!(w0, k_off);
                w1 = vsha256su1q_u32(vsha256su0q_u32(w1, w2), w3, w0);
                round4!(w1, k_off + 4);
                w2 = vsha256su1q_u32(vsha256su0q_u32(w2, w3), w0, w1);
                round4!(w2, k_off + 8);
                w3 = vsha256su1q_u32(vsha256su0q_u32(w3, w0), w1, w2);
                round4!(w3, k_off + 12);
                k_off += 16;
            }

            abcd = vaddq_u32(abcd, abcd_orig);
            efgh = vaddq_u32(efgh, efgh_orig);
        }

        vst1q_u32(h.as_mut_ptr(), abcd);
        vst1q_u32(h.as_mut_ptr().add(4), efgh);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse4.1,ssse3")]
unsafe fn sha256_compress_x86_64(h: &mut [u32; 8], blocks: &[[u8; 64]]) {
    use std::arch::x86_64::*;
    unsafe {
        let mut state0 = _mm_set_epi32(h[0] as i32, h[1] as i32, h[4] as i32, h[5] as i32);
        let mut state1 = _mm_set_epi32(h[2] as i32, h[3] as i32, h[6] as i32, h[7] as i32);
        let shuf_mask = _mm_set_epi64x(0x0c0d_0e0f_0809_0a0b, 0x0405_0607_0001_0203);

        for block in blocks {
            let state0_orig = state0;
            let state1_orig = state1;

            let p = block.as_ptr() as *const __m128i;
            let mut w0 = _mm_shuffle_epi8(_mm_loadu_si128(p), shuf_mask);
            let mut w1 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(1)), shuf_mask);
            let mut w2 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(2)), shuf_mask);
            let mut w3 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(3)), shuf_mask);

            macro_rules! x86_round4 {
                ($w:expr, $k_off:expr) => {
                    let mut msg = _mm_add_epi32(
                        $w,
                        _mm_loadu_si128(K256.as_ptr().add($k_off) as *const __m128i),
                    );
                    state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
                    msg = _mm_shuffle_epi32(msg, 0x0E);
                    state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
                };
            }

            macro_rules! x86_sched {
                ($w0:expr, $w1:expr, $w2:expr, $w3:expr) => {
                    let tmp = _mm_alignr_epi8($w3, $w2, 4);
                    $w0 = _mm_add_epi32(_mm_sha256msg1_epu32($w0, $w1), tmp);
                    $w0 = _mm_sha256msg2_epu32($w0, $w3);
                };
            }

            x86_round4!(w0, 0);
            x86_round4!(w1, 4);
            x86_round4!(w2, 8);
            x86_round4!(w3, 12);

            let mut k_off = 16usize;
            while k_off < 64 {
                x86_sched!(w0, w1, w2, w3);
                x86_round4!(w0, k_off);
                x86_sched!(w1, w2, w3, w0);
                x86_round4!(w1, k_off + 4);
                x86_sched!(w2, w3, w0, w1);
                x86_round4!(w2, k_off + 8);
                x86_sched!(w3, w0, w1, w2);
                x86_round4!(w3, k_off + 12);
                k_off += 16;
            }

            state0 = _mm_add_epi32(state0, state0_orig);
            state1 = _mm_add_epi32(state1, state1_orig);
        }

        h[0] = _mm_extract_epi32::<3>(state0) as u32;
        h[1] = _mm_extract_epi32::<2>(state0) as u32;
        h[2] = _mm_extract_epi32::<3>(state1) as u32;
        h[3] = _mm_extract_epi32::<2>(state1) as u32;
        h[4] = _mm_extract_epi32::<1>(state0) as u32;
        h[5] = _mm_extract_epi32::<0>(state0) as u32;
        h[6] = _mm_extract_epi32::<1>(state1) as u32;
        h[7] = _mm_extract_epi32::<0>(state1) as u32;
    }
}

#[inline]
fn sha256_compress(h: &mut [u32; 8], blocks: &[[u8; 64]]) {
    if blocks.is_empty() {
        return;
    }
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("sha2") {
        // SAFETY: sha2 feature verified at runtime.
        unsafe { sha256_compress_aarch64(h, blocks) };
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sha")
        && std::arch::is_x86_feature_detected!("ssse3")
        && std::arch::is_x86_feature_detected!("sse4.1")
    {
        // SAFETY: sha + ssse3 + sse4.1 features verified at runtime.
        unsafe { sha256_compress_x86_64(h, blocks) };
        return;
    }
    sha256_compress_scalar(h, blocks);
}

pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let (full_blocks, rem) = data.as_chunks::<64>();
    sha256_compress(&mut h, full_blocks);

    let mut tail = [0u8; 128];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let total_tail = if rem.len() < 56 { 64 } else { 128 };
    let bit_len = (data.len() as u64).wrapping_mul(8);
    tail[total_tail - 8..total_tail].copy_from_slice(&bit_len.to_be_bytes());
    let tail_blocks = tail[..total_tail].as_chunks::<64>().0;
    sha256_compress(&mut h, tail_blocks);

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Streaming SHA-256 hasher reusing hardware-accelerated compression.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: sha256_init_state(),
            buf: [0u8; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                sha256_compress(&mut self.state, std::slice::from_ref(&self.buf));
                self.buf_len = 0;
            }
        }
        let (full_blocks, rem) = data.as_chunks::<64>();
        if !full_blocks.is_empty() {
            sha256_compress(&mut self.state, full_blocks);
        }
        if !rem.is_empty() {
            self.buf[..rem.len()].copy_from_slice(rem);
            self.buf_len = rem.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let mut tail = [0u8; 128];
        tail[..self.buf_len].copy_from_slice(&self.buf[..self.buf_len]);
        tail[self.buf_len] = 0x80;
        let total_tail = if self.buf_len < 56 { 64 } else { 128 };
        let bit_len = self.total_bytes.wrapping_mul(8);
        tail[total_tail - 8..total_tail].copy_from_slice(&bit_len.to_be_bytes());
        let tail_blocks = tail[..total_tail].as_chunks::<64>().0;
        sha256_compress(&mut self.state, tail_blocks);

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

pub fn hmac_sha256_bytes(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = sha256_bytes(key);
        k_block[..32].copy_from_slice(&hashed);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = sha256_bytes(&inner);

    let mut outer = [0u8; 96];
    outer[..64].copy_from_slice(&opad);
    outer[64..].copy_from_slice(&inner_hash);
    sha256_bytes(&outer)
}

// ── Hex & Base64 ─────────────────────────────────────────────────────────────

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX_CHARS[(b >> 4) as usize] as char);
        out.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn decode_hex(s: &str) -> Obj {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return make_crypto_error("invalid_hex", "Hex string must have even length");
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = match hex_val(pair[0]) {
            Some(v) => v,
            None => return make_crypto_error("invalid_hex", "Invalid hex character"),
        };
        let lo = match hex_val(pair[1]) {
            Some(v) => v,
            None => return make_crypto_error("invalid_hex", "Invalid hex character"),
        };
        out.push((hi << 4) | lo);
    }
    match String::from_utf8(out) {
        Ok(text) => Obj::str(text),
        Err(e) => make_crypto_error("invalid_utf8", e.to_string()),
    }
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

const B64_ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut consumed = 0usize;
    if bytes.len() >= 28 && b64_simd::available() {
        // SAFETY: CPU feature verified by `b64_simd::available()`.
        consumed = unsafe { b64_simd::encode_prefix(bytes, &mut out) };
    }
    let (chunks, rem) = bytes[consumed..].as_chunks::<3>();
    for &[b0, b1, b2] in chunks {
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.extend_from_slice(&[
            B64_ALPHA[((n >> 18) & 63) as usize],
            B64_ALPHA[((n >> 12) & 63) as usize],
            B64_ALPHA[((n >> 6) & 63) as usize],
            B64_ALPHA[(n & 63) as usize],
        ]);
    }
    if !rem.is_empty() {
        let b0 = rem[0] as u32;
        let b1 = if rem.len() > 1 { rem[1] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8);
        out.extend_from_slice(&[
            B64_ALPHA[((n >> 18) & 63) as usize],
            B64_ALPHA[((n >> 12) & 63) as usize],
            if rem.len() > 1 { B64_ALPHA[((n >> 6) & 63) as usize] } else { b'=' },
            b'=',
        ]);
    }
    // SAFETY: `B64_ALPHA` and `'='` are strictly ASCII.
    unsafe { String::from_utf8_unchecked(out) }
}

const B64_LUT: [i8; 256] = {
    let mut lut = [-1i8; 256];
    let mut i = 0usize;
    while i < 64 {
        lut[B64_ALPHA[i] as usize] = i as i8;
        i += 1;
    }
    lut
};

fn decode_base64_bytes_slice(clean: &[u8]) -> Result<Result<Vec<u8>, (&'static str, &'static str)>, bool> {
    if !clean.len().is_multiple_of(4) {
        let has_ws = clean.iter().any(|b| b.is_ascii_whitespace());
        if has_ws {
            return Err(true);
        }
        return Ok(Err((
            "invalid_base64",
            "Base64 length must be a multiple of 4",
        )));
    }
    let mut out = Vec::with_capacity((clean.len() / 4) * 3 + 8);
    let mut simd_chars = 0usize;
    if clean.len() >= 68 && b64_simd::available() {
        // SAFETY: CPU feature verified by `b64_simd::available()`.
        simd_chars = unsafe { b64_simd::decode_prefix(&clean[..clean.len() - 4], &mut out) };
    }
    let chunks = clean[simd_chars..].as_chunks::<4>().0;
    if let Some((last, head)) = chunks.split_last() {
        for &[c0, c1, c2, c3] in head {
            let a = B64_LUT[c0 as usize];
            let b = B64_LUT[c1 as usize];
            let c = B64_LUT[c2 as usize];
            let d = B64_LUT[c3 as usize];
            if (a | b | c | d) < 0 {
                if c0.is_ascii_whitespace()
                    || c1.is_ascii_whitespace()
                    || c2.is_ascii_whitespace()
                    || c3.is_ascii_whitespace()
                {
                    return Err(true);
                }
                return Ok(Err(("invalid_base64", "Invalid Base64 character")));
            }
            let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
            out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        }
        let a = B64_LUT[last[0] as usize];
        let b = B64_LUT[last[1] as usize];
        if a < 0 || b < 0 {
            if last.iter().any(|x| x.is_ascii_whitespace()) {
                return Err(true);
            }
            return Ok(Err(("invalid_base64", "Invalid Base64 character")));
        }
        let c = if last[2] == b'=' {
            -2
        } else {
            let v = B64_LUT[last[2] as usize];
            if v < 0 {
                if last[2].is_ascii_whitespace() || last[3].is_ascii_whitespace() {
                    return Err(true);
                }
                return Ok(Err(("invalid_base64", "Invalid Base64 character")));
            }
            v
        };
        let d = if last[3] == b'=' {
            -2
        } else {
            let v = B64_LUT[last[3] as usize];
            if v < 0 {
                if last[3].is_ascii_whitespace() {
                    return Err(true);
                }
                return Ok(Err(("invalid_base64", "Invalid Base64 character")));
            }
            v
        };
        if c == -2 && d != -2 {
            return Ok(Err(("invalid_base64", "Invalid Base64 padding")));
        }
        out.push(((a as u8) << 2) | ((b as u8) >> 4));
        if c >= 0 {
            out.push(((b as u8) << 4) | ((c as u8) >> 2));
            if d >= 0 {
                out.push(((c as u8) << 6) | (d as u8));
            }
        }
    }
    Ok(Ok(out))
}

pub fn decode_base64_bytes(s: &str) -> Result<Vec<u8>, (&'static str, &'static str)> {
    let raw = s.as_bytes();
    match decode_base64_bytes_slice(raw) {
        Ok(res) => res,
        Err(_) => {
            let clean: Vec<u8> = raw.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            match decode_base64_bytes_slice(&clean) {
                Ok(res) => res,
                Err(_) => Err(("invalid_base64", "Invalid Base64 character")),
            }
        }
    }
}

pub fn decode_base64(s: &str) -> Obj {
    match decode_base64_bytes(s) {
        Ok(out) => match String::from_utf8(out) {
            Ok(text) => Obj::str(text),
            Err(e) => make_crypto_error("invalid_utf8", e.to_string()),
        },
        Err((kind, msg)) => make_crypto_error(kind, msg),
    }
}

// ── OS CSPRNG / UUID v4 ──────────────────────────────────────────────────────

#[cfg(windows)]
fn os_fill_random(buf: &mut [u8], _cur_pid: u32) -> Result<(), String> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            h_algorithm: *mut core::ffi::c_void,
            pb_buffer: *mut u8,
            cb_buffer: u32,
            dw_flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        let status = unsafe {
            BCryptGenRandom(
                core::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(format!("BCryptGenRandom failed with NTSTATUS 0x{:08x}", status));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn os_fill_random(buf: &mut [u8], cur_pid: u32) -> Result<(), String> {
    thread_local! {
        static URANDOM_FILE: RefCell<(Option<File>, u32)> = const { RefCell::new((None, 0)) };
    }
    URANDOM_FILE.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.0.is_none() || guard.1 != cur_pid {
            guard.0 = Some(
                File::open("/dev/urandom")
                    .map_err(|e| format!("Failed to open /dev/urandom: {}", e))?,
            );
            guard.1 = cur_pid;
        }
        guard
            .0
            .as_mut()
            .unwrap()
            .read_exact(buf)
            .map_err(|e| format!("Failed to read /dev/urandom: {}", e))
    })
}

thread_local! {
    static RNG_BUF: RefCell<([u8; 4096], usize, u32)> = const { RefCell::new(([0u8; 4096], 4096, 0)) };
}

pub fn fill_random_bytes(buf: &mut [u8]) -> Result<(), String> {
    if buf.is_empty() {
        return Ok(());
    }
    let cur_pid = std::process::id();
    if buf.len() > 256 {
        return os_fill_random(buf, cur_pid);
    }
    RNG_BUF.with(|cell| {
        let (pool, pos, cached_pid) = &mut *cell.borrow_mut();
        if *cached_pid != cur_pid || *pos + buf.len() > pool.len() {
            os_fill_random(pool, cur_pid)?;
            *pos = 0;
            *cached_pid = cur_pid;
        }
        buf.copy_from_slice(&pool[*pos..*pos + buf.len()]);
        pool[*pos..*pos + buf.len()].fill(0);
        *pos += buf.len();
        Ok(())
    })
}

pub fn uuid_v4() -> Obj {
    let mut b = [0u8; 16];
    if let Err(e) = fill_random_bytes(&mut b) {
        return Obj::str(e);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let mut out = [0u8; 36];
    let mut pos = 0usize;
    for (i, &byte) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out[pos] = b'-';
            pos += 1;
        }
        out[pos] = HEX_CHARS[(byte >> 4) as usize];
        out[pos + 1] = HEX_CHARS[(byte & 0x0f) as usize];
        pos += 2;
    }
    let s = std::str::from_utf8(&out).expect("hex ascii");
    Obj::str(s)
}

pub fn random_bytes_hex(len: i64) -> Obj {
    if !(0..=1_048_576).contains(&len) {
        return make_crypto_error("invalid_input", "random_bytes_hex(len) must be between 0 and 1048576");
    }
    let mut buf = vec![0u8; len as usize];
    if let Err(e) = fill_random_bytes(&mut buf) {
        return make_crypto_error("rng_failed", e);
    }
    Obj::str(encode_hex(&buf))
}

pub fn random_int(min: i64, max: i64) -> Obj {
    if min >= max {
        return make_crypto_error("invalid_input", "random_int(min, max) requires min < max");
    }
    let span = (max as i128 - min as i128) as u128;
    let mut b = [0u8; 8];
    if let Err(e) = fill_random_bytes(&mut b) {
        return make_crypto_error("rng_failed", e);
    }
    let v = u64::from_le_bytes(b) as u128;
    let offset = (v % span) as i128;
    Obj::boxed_int((min as i128 + offset) as i64)
}

#[inline]
fn sha256_init_state() -> [u32; 8] {
    [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ]
}

#[inline]
fn sha256_finish_with_prefix(mut h: [u32; 8], prefix_bytes: u64, data: &[u8]) -> [u8; 32] {
    let (full_blocks, rem) = data.as_chunks::<64>();
    sha256_compress(&mut h, full_blocks);

    let mut tail = [0u8; 128];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let total_tail = if rem.len() < 56 { 64 } else { 128 };
    let bit_len = prefix_bytes.wrapping_add(data.len() as u64).wrapping_mul(8);
    tail[total_tail - 8..total_tail].copy_from_slice(&bit_len.to_be_bytes());
    let tail_blocks = tail[..total_tail].as_chunks::<64>().0;
    sha256_compress(&mut h, tail_blocks);

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let mut k_block = [0u8; 64];
    if password.len() > 64 {
        let hashed = sha256_bytes(password);
        k_block[..32].copy_from_slice(&hashed);
    } else {
        k_block[..password.len()].copy_from_slice(password);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }
    let mut h_inner = sha256_init_state();
    sha256_compress(&mut h_inner, std::slice::from_ref(&ipad));
    let mut h_outer = sha256_init_state();
    sha256_compress(&mut h_outer, std::slice::from_ref(&opad));

    let iters = iterations.max(1);
    for (index, chunk) in out.chunks_mut(32).enumerate() {
        let mut salted = Vec::with_capacity(salt.len() + 4);
        salted.extend_from_slice(salt);
        salted.extend_from_slice(&(index as u32 + 1).to_be_bytes());

        let inner_h = sha256_finish_with_prefix(h_inner, 64, &salted);
        let mut u = sha256_finish_with_prefix(h_outer, 64, &inner_h);
        let mut t = u;
        for _ in 1..iters {
            let ih = sha256_finish_with_prefix(h_inner, 64, &u);
            u = sha256_finish_with_prefix(h_outer, 64, &ih);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

pub fn hash_password(password: &str) -> Obj {
    let mut salt = [0u8; 16];
    if let Err(e) = fill_random_bytes(&mut salt) {
        return make_crypto_error("rng_failed", e);
    }
    let mut dk = [0u8; 32];
    pbkdf2_sha256(password.as_bytes(), &salt, DEFAULT_PBKDF2_ITERATIONS, &mut dk);
    Obj::str(format!(
        "pbkdf2-sha256${}${}${}",
        DEFAULT_PBKDF2_ITERATIONS,
        encode_hex(&salt),
        encode_hex(&dk)
    ))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let valid_parts = match (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("pbkdf2-sha256"), Some(iter_s), Some(salt_hex), Some(hash_hex), None) => {
            if let Ok(iters) = iter_s.parse::<u32>()
                && (1..=MAX_PBKDF2_ITERATIONS).contains(&iters)
                && let (Some(salt), Some(expected)) = (decode_hex_bytes(salt_hex), decode_hex_bytes(hash_hex))
                && expected.len() == 32
            {
                Some((iters, salt, expected))
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some((iters, salt, expected)) = valid_parts {
        let mut dk = [0u8; 32];
        pbkdf2_sha256(password.as_bytes(), &salt, iters, &mut dk);
        constant_time_eq(&dk, &expected)
    } else {
        // Burn constant-ish work even on malformed input to avoid timing leaks.
        let mut dk = [0u8; 32];
        pbkdf2_sha256(password.as_bytes(), &[0u8; 16], 1, &mut dk);
        false
    }
}

fn decode_hex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = match pair[0] {
            b'0'..=b'9' => pair[0] - b'0',
            b'a'..=b'f' => pair[0] - b'a' + 10,
            b'A'..=b'F' => pair[0] - b'A' + 10,
            _ => return None,
        };
        let lo = match pair[1] {
            b'0'..=b'9' => pair[1] - b'0',
            b'a'..=b'f' => pair[1] - b'a' + 10,
            b'A'..=b'F' => pair[1] - b'A' + 10,
            _ => return None,
        };
        out.push((hi << 4) | lo);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_rfc6070_vector() {
        let mut out = [0u8; 32];
        pbkdf2_sha256(b"password", b"salt", 4096, &mut out);
        assert_eq!(
            encode_hex(&out),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn password_hash_and_verify_roundtrip() {
        let Obj::Str(h) = hash_password("s3cr3t!") else {
            panic!("expected hashed string");
        };
        assert!(verify_password("s3cr3t!", h.as_str()));
        assert!(!verify_password("wrong", h.as_str()));
        assert!(!verify_password("s3cr3t!", "bad$format"));
    }

    #[test]
    fn rng_pool_resets_on_pid_mismatch_and_zeroes_consumed_bytes() {
        let mut out = [0u8; 32];
        fill_random_bytes(&mut out).unwrap();
        RNG_BUF.with(|cell| {
            let (pool, pos, pid) = &mut *cell.borrow_mut();
            assert!(*pos >= 32);
            assert!(pool[*pos - 32..*pos].iter().all(|&b| b == 0));
            assert_eq!(*pid, std::process::id());
            *pid = 0;
            *pos = 16;
            pool[16..48].fill(0xAA);
        });
        let mut after_fork = [0xAAu8; 32];
        fill_random_bytes(&mut after_fork).unwrap();
        RNG_BUF.with(|cell| {
            let (_, pos, pid) = *cell.borrow();
            assert_eq!(pos, 32);
            assert_eq!(pid, std::process::id());
        });
        assert_ne!(after_fork, [0xAAu8; 32]);
    }
}
