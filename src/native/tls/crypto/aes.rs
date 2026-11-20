use super::Zeroize;

const RCON: [u8; 11] = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

#[inline]
fn bitslice_sbox_16(block: &mut [u8; 16]) {
    let mut u = [0u16; 8];
    for (byte_idx, &b) in block.iter().enumerate() {
        for bit in 0..8 {
            u[7 - bit] |= (((b >> bit) & 1) as u16) << byte_idx;
        }
    }

    let [x0, x1, x2, x3, x4, x5, x6, x7] = u;

    let y14 = x3 ^ x5;
    let y13 = x0 ^ x6;
    let y9 = x0 ^ x3;
    let y8 = x0 ^ x5;
    let t0 = x1 ^ x2;
    let y1 = t0 ^ x7;
    let y4 = y1 ^ x3;
    let y12 = y13 ^ y14;
    let y2 = y1 ^ x0;
    let y5 = y1 ^ x6;
    let y3 = y5 ^ y8;
    let t1 = x4 ^ y12;
    let y15 = t1 ^ x5;
    let y20 = t1 ^ x1;
    let y6 = y15 ^ x7;
    let y10 = y15 ^ t0;
    let y11 = y20 ^ y9;
    let y7 = x7 ^ y11;
    let y17 = y10 ^ y11;
    let y19 = y10 ^ y8;
    let y16 = t0 ^ y11;
    let y21 = y13 ^ y16;
    let y18 = x0 ^ y16;

    let t2 = y12 & y15;
    let t3 = y3 & y6;
    let t4 = t3 ^ t2;
    let t5 = y4 & x7;
    let t6 = t5 ^ t2;
    let t7 = y13 & y16;
    let t8 = y5 & y1;
    let t9 = t8 ^ t7;
    let t10 = y2 & y7;
    let t11 = t10 ^ t7;
    let t12 = y9 & y11;
    let t13 = y14 & y17;
    let t14 = t13 ^ t12;
    let t15 = y8 & y10;
    let t16 = t15 ^ t12;
    let t17 = t4 ^ y20;
    let t18 = t6 ^ t16;
    let t19 = t9 ^ t14;
    let t20 = t11 ^ t16;
    let t21 = t17 ^ t14;
    let t22 = t18 ^ y19;
    let t23 = t19 ^ y21;
    let t24 = t20 ^ y18;

    let t25 = t21 ^ t22;
    let t26 = t21 & t23;
    let t27 = t24 ^ t26;
    let t28 = t25 & t27;
    let t29 = t28 ^ t22;
    let t30 = t23 ^ t24;
    let t31 = t22 ^ t26;
    let t32 = t31 & t30;
    let t33 = t32 ^ t24;
    let t34 = t23 ^ t33;
    let t35 = t27 ^ t33;
    let t36 = t24 & t35;
    let t37 = t36 ^ t34;
    let t38 = t27 ^ t36;
    let t39 = t29 & t38;
    let t40 = t25 ^ t39;

    let t41 = t40 ^ t37;
    let t42 = t29 ^ t33;
    let t43 = t29 ^ t40;
    let t44 = t33 ^ t37;
    let t45 = t42 ^ t41;
    let z0 = t44 & y15;
    let z1 = t37 & y6;
    let z2 = t33 & x7;
    let z3 = t43 & y16;
    let z4 = t40 & y1;
    let z5 = t29 & y7;
    let z6 = t42 & y11;
    let z7 = t45 & y17;
    let z8 = t41 & y10;
    let z9 = t44 & y12;
    let z10 = t37 & y3;
    let z11 = t33 & y4;
    let z12 = t43 & y13;
    let z13 = t40 & y5;
    let z14 = t29 & y2;
    let z15 = t42 & y9;
    let z16 = t45 & y14;
    let z17 = t41 & y8;

    let t46 = z15 ^ z16;
    let t47 = z10 ^ z11;
    let t48 = z5 ^ z13;
    let t49 = z9 ^ z10;
    let t50 = z2 ^ z12;
    let t51 = z2 ^ z5;
    let t52 = z7 ^ z8;
    let t53 = z0 ^ z3;
    let t54 = z6 ^ z7;
    let t55 = z16 ^ z17;
    let t56 = z12 ^ t48;
    let t57 = t50 ^ t53;
    let t58 = z4 ^ t46;
    let t59 = z3 ^ t54;
    let t60 = t46 ^ t57;
    let t61 = z14 ^ t57;
    let t62 = t52 ^ t58;
    let t63 = t49 ^ t58;
    let t64 = z4 ^ t59;
    let t65 = t61 ^ t62;
    let t66 = z1 ^ t63;
    let s0 = t59 ^ t63;
    let s6 = t56 ^ (!t62);
    let s7 = t48 ^ (!t60);
    let t67 = t64 ^ t65;
    let s3 = t53 ^ t66;
    let s4 = t51 ^ t66;
    let s5 = t47 ^ t65;
    let s1 = t64 ^ (!s3);
    let s2 = t55 ^ (!t67);

    let out = [s0, s1, s2, s3, s4, s5, s6, s7];
    for (byte_idx, b) in block.iter_mut().enumerate() {
        let mut val = 0u8;
        for bit in 0..8 {
            val |= (((out[7 - bit] >> byte_idx) & 1) as u8) << bit;
        }
        *b = val;
    }
}

#[inline]
fn sub_word_ct(w: u32) -> u32 {
    let mut blk = [0u8; 16];
    blk[0..4].copy_from_slice(&w.to_be_bytes());
    bitslice_sbox_16(&mut blk);
    u32::from_be_bytes([blk[0], blk[1], blk[2], blk[3]])
}

#[inline]
fn xtime(x: u8) -> u8 {
    let hi = (x >> 7) & 1;
    let mask = 0u8.wrapping_sub(hi);
    (x << 1) ^ (0x1b & mask)
}

#[inline]
fn shift_rows(s: &mut [u8; 16]) {
    let b1 = s[1];
    s[1] = s[5];
    s[5] = s[9];
    s[9] = s[13];
    s[13] = b1;

    s.swap(2, 10);
    s.swap(6, 14);

    let b15 = s[15];
    s[15] = s[11];
    s[11] = s[7];
    s[7] = s[3];
    s[3] = b15;
}

#[inline]
fn mix_columns(s: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let a0 = s[i];
        let a1 = s[i + 1];
        let a2 = s[i + 2];
        let a3 = s[i + 3];
        let t = a0 ^ a1 ^ a2 ^ a3;
        s[i] ^= t ^ xtime(a0 ^ a1);
        s[i + 1] ^= t ^ xtime(a1 ^ a2);
        s[i + 2] ^= t ^ xtime(a2 ^ a3);
        s[i + 3] ^= t ^ xtime(a3 ^ a0);
    }
}

#[inline]
fn add_round_key(s: &mut [u8; 16], rk: &[u8; 16]) {
    for (a, &b) in s.iter_mut().zip(rk.iter()) {
        *a ^= b;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Portable,
    #[cfg(target_arch = "x86_64")]
    AesNi,
    #[cfg(target_arch = "aarch64")]
    Armv8,
}

#[derive(Clone)]
pub struct AesKey {
    round_keys: [[u8; 16]; 15],
    rounds: usize,
    backend: Backend,
}

impl Drop for AesKey {
    fn drop(&mut self) {
        for rk in &mut self.round_keys {
            rk.zeroize();
        }
    }
}

impl AesKey {
    pub fn new(key: &[u8]) -> Option<Self> {
        let backend = Self::detect_backend();
        Self::with_backend(key, backend)
    }

    pub fn new_portable(key: &[u8]) -> Option<Self> {
        Self::with_backend(key, Backend::Portable)
    }

    fn detect_backend() -> Backend {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("aes")
            && std::arch::is_x86_feature_detected!("sse2")
        {
            return Backend::AesNi;
        }
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("aes") {
            return Backend::Armv8;
        }
        Backend::Portable
    }

    fn with_backend(key: &[u8], backend: Backend) -> Option<Self> {
        let (nk, rounds) = match key.len() {
            16 => (4, 10),
            32 => (8, 14),
            _ => return None,
        };
        let mut w = [0u32; 60];
        for i in 0..nk {
            w[i] = u32::from_be_bytes([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]);
        }
        let total_words = 4 * (rounds + 1);
        for i in nk..total_words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                temp = sub_word_ct(temp.rotate_left(8)) ^ ((RCON[i / nk] as u32) << 24);
            } else if nk == 8 && i % nk == 4 {
                temp = sub_word_ct(temp);
            }
            w[i] = w[i - nk] ^ temp;
        }
        let mut round_keys = [[0u8; 16]; 15];
        for r in 0..=rounds {
            for c in 0..4 {
                let bytes = w[r * 4 + c].to_be_bytes();
                round_keys[r][c * 4..c * 4 + 4].copy_from_slice(&bytes);
            }
        }
        w.zeroize();
        Some(Self {
            round_keys,
            rounds,
            backend,
        })
    }

    #[inline]
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        match self.backend {
            Backend::Portable => self.encrypt_block_portable(block),
            #[cfg(target_arch = "x86_64")]
            Backend::AesNi => {
                // SAFETY: aes + sse2 detected at construction.
                unsafe { self.encrypt_block_aesni(block) }
            }
            #[cfg(target_arch = "aarch64")]
            Backend::Armv8 => {
                // SAFETY: aes feature detected at construction.
                unsafe { self.encrypt_block_armv8(block) }
            }
        }
    }

    fn encrypt_block_portable(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for rk in &self.round_keys[1..self.rounds] {
            bitslice_sbox_16(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, rk);
        }
        bitslice_sbox_16(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[self.rounds]);
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "aes,sse2")]
    unsafe fn encrypt_block_aesni(&self, block: &mut [u8; 16]) {
        use std::arch::x86_64::*;
        unsafe {
            let mut m = _mm_loadu_si128(block.as_ptr() as *const __m128i);
            let k0 = _mm_loadu_si128(self.round_keys[0].as_ptr() as *const __m128i);
            m = _mm_xor_si128(m, k0);
            for rk_bytes in &self.round_keys[1..self.rounds] {
                let rk = _mm_loadu_si128(rk_bytes.as_ptr() as *const __m128i);
                m = _mm_aesenc_si128(m, rk);
            }
            let k_last = _mm_loadu_si128(self.round_keys[self.rounds].as_ptr() as *const __m128i);
            m = _mm_aesenclast_si128(m, k_last);
            _mm_storeu_si128(block.as_mut_ptr() as *mut __m128i, m);
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "aes")]
    unsafe fn encrypt_block_armv8(&self, block: &mut [u8; 16]) {
        use std::arch::aarch64::*;
        unsafe {
            let mut m = vld1q_u8(block.as_ptr());
            for rk_bytes in &self.round_keys[..self.rounds - 1] {
                let rk = vld1q_u8(rk_bytes.as_ptr());
                m = vaesmcq_u8(vaeseq_u8(m, rk));
            }
            let rk_pen = vld1q_u8(self.round_keys[self.rounds - 1].as_ptr());
            let rk_last = vld1q_u8(self.round_keys[self.rounds].as_ptr());
            m = veorq_u8(vaeseq_u8(m, rk_pen), rk_last);
            vst1q_u8(block.as_mut_ptr(), m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn aes128_fips197_vectors() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let pt = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let expected = "69c4e0d86a7b0430d8cdb78070b4c55a";

        let mut b1 = pt;
        AesKey::new_portable(&key).unwrap().encrypt_block(&mut b1);
        assert_eq!(encode_hex(&b1), expected);

        let mut b2 = pt;
        AesKey::new(&key).unwrap().encrypt_block(&mut b2);
        assert_eq!(encode_hex(&b2), expected);
    }

    #[test]
    fn aes256_fips197_vectors() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let pt = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let expected = "8ea2b7ca516745bfeafc49904b496089";

        let mut b1 = pt;
        AesKey::new_portable(&key).unwrap().encrypt_block(&mut b1);
        assert_eq!(encode_hex(&b1), expected);

        let mut b2 = pt;
        AesKey::new(&key).unwrap().encrypt_block(&mut b2);
        assert_eq!(encode_hex(&b2), expected);
    }
}
