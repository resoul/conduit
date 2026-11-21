//! GHASH over GF(2^128) for AES-GCM (NIST SP 800-38D).
//! Includes PCLMULQDQ (x86_64), PMULL (aarch64), and a portable constant-time fallback.

use super::Zeroize;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Portable,
    #[cfg(target_arch = "x86_64")]
    Pclmul,
    #[cfg(target_arch = "aarch64")]
    Pmull,
}

#[derive(Clone)]
pub struct Ghash {
    h: u128,
    acc: u128,
    backend: Backend,
}

impl Drop for Ghash {
    fn drop(&mut self) {
        let mut z = self.h.to_be_bytes();
        z.zeroize();
        self.h = 0;
        self.acc = 0;
    }
}

impl Ghash {
    pub fn new(h_bytes: &[u8; 16]) -> Self {
        let backend = Self::detect_backend();
        Self::with_backend(h_bytes, backend)
    }

    pub fn new_portable(h_bytes: &[u8; 16]) -> Self {
        Self::with_backend(h_bytes, Backend::Portable)
    }

    fn detect_backend() -> Backend {
        if cfg!(miri) {
            return Backend::Portable;
        }
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("pclmulqdq")
            && std::arch::is_x86_feature_detected!("sse2")
        {
            return Backend::Pclmul;
        }
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("aes") {
            return Backend::Pmull;
        }
        Backend::Portable
    }

    fn with_backend(h_bytes: &[u8; 16], backend: Backend) -> Self {
        Self {
            h: u128::from_be_bytes(*h_bytes),
            acc: 0,
            backend,
        }
    }

    pub fn update_padded(&mut self, data: &[u8]) {
        let (chunks, rem) = data.as_chunks::<16>();
        for chunk in chunks {
            self.update_block(chunk);
        }
        if !rem.is_empty() {
            let mut padded = [0u8; 16];
            padded[..rem.len()].copy_from_slice(rem);
            self.update_block(&padded);
        }
    }

    pub fn update_block(&mut self, block: &[u8; 16]) {
        let x = u128::from_be_bytes(*block);
        self.acc ^= x;
        self.acc = match self.backend {
            Backend::Portable => gf128_mul_ct(self.acc, self.h),
            #[cfg(target_arch = "x86_64")]
            Backend::Pclmul => {
                // SAFETY: pclmulqdq + sse2 detected at construction.
                unsafe { gf128_mul_pclmul(self.acc, self.h) }
            }
            #[cfg(target_arch = "aarch64")]
            Backend::Pmull => {
                // SAFETY: aes (includes pmull) detected at construction.
                unsafe { gf128_mul_pmull(self.acc, self.h) }
            }
        };
    }

    pub fn finalize(self) -> [u8; 16] {
        self.acc.to_be_bytes()
    }
}

/// Constant-time GF(2^128) multiplication (NIST SP 800-38D Algorithm 1).
#[inline]
fn gf128_mul_ct(x: u128, y: u128) -> u128 {
    const R: u128 = 0xe100_0000_0000_0000_0000_0000_0000_0000;
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        let bit = (x >> (127 - i)) & 1;
        let mask = 0u128.wrapping_sub(bit);
        z ^= v & mask;

        let lsb = v & 1;
        let r_mask = 0u128.wrapping_sub(lsb);
        v = (v >> 1) ^ (R & r_mask);
    }
    z
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "pclmulqdq,sse2")]
unsafe fn gf128_mul_pclmul(x: u128, y: u128) -> u128 {
    use std::arch::x86_64::*;
    unsafe {
        let a = _mm_set_epi64x((x >> 64) as i64, x as i64);
        let b = _mm_set_epi64x((y >> 64) as i64, y as i64);

        let p00 = _mm_clmulepi64_si128::<0x00>(a, b);
        let p11 = _mm_clmulepi64_si128::<0x11>(a, b);
        let p01 = _mm_clmulepi64_si128::<0x01>(a, b);
        let p10 = _mm_clmulepi64_si128::<0x10>(a, b);

        let mid = _mm_xor_si128(p01, p10);
        let mut lo = _mm_xor_si128(p00, _mm_slli_si128::<8>(mid));
        let mut hi = _mm_xor_si128(p11, _mm_srli_si128::<8>(mid));

        // 256-bit left shift by 1 bit for reflected polynomial representation
        let lo_carry = _mm_srli_epi64::<63>(_mm_srli_si128::<8>(lo));
        let lo_cross = _mm_slli_si128::<8>(_mm_srli_epi64::<63>(lo));
        lo = _mm_or_si128(_mm_slli_epi64::<1>(lo), lo_cross);

        let hi_cross = _mm_slli_si128::<8>(_mm_srli_epi64::<63>(hi));
        hi = _mm_or_si128(_mm_slli_epi64::<1>(hi), _mm_or_si128(hi_cross, lo_carry));

        // Phase 1 reduction: a = lo ^ (lo << 127) ^ (lo << 126) ^ (lo << 121)
        let lo_hi = _mm_slli_si128::<8>(lo);
        let red_a = _mm_xor_si128(
            lo,
            _mm_xor_si128(
                _mm_slli_epi64::<63>(lo_hi),
                _mm_xor_si128(_mm_slli_epi64::<62>(lo_hi), _mm_slli_epi64::<57>(lo_hi)),
            ),
        );

        // Phase 2 reduction: b = red_a ^ (red_a >> 1) ^ (red_a >> 2) ^ (red_a >> 7)
        let a_hi = _mm_srli_si128::<8>(red_a);
        let shr1 = _mm_or_si128(_mm_srli_epi64::<1>(red_a), _mm_slli_epi64::<63>(a_hi));
        let shr2 = _mm_or_si128(_mm_srli_epi64::<2>(red_a), _mm_slli_epi64::<62>(a_hi));
        let shr7 = _mm_or_si128(_mm_srli_epi64::<7>(red_a), _mm_slli_epi64::<57>(a_hi));
        let red_b = _mm_xor_si128(red_a, _mm_xor_si128(shr1, _mm_xor_si128(shr2, shr7)));

        let res = _mm_xor_si128(hi, red_b);
        let mut out = [0u8; 16];
        _mm_storeu_si128(out.as_mut_ptr() as *mut __m128i, res);
        u128::from_le_bytes(out)
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "aes")]
unsafe fn gf128_mul_pmull(x: u128, y: u128) -> u128 {
    // Use vmull_p64 for 64x64 carry-less multiplication (available with aes/pmull on ARMv8)
    use std::arch::aarch64::*;
    let x0 = x as u64;
    let x1 = (x >> 64) as u64;
    let y0 = y as u64;
    let y1 = (y >> 64) as u64;

    let p00 = vmull_p64(x0, y0);
    let p11 = vmull_p64(x1, y1);
    let p01 = vmull_p64(x0, y1);
    let p10 = vmull_p64(x1, y0);

    let mid = p01 ^ p10;
    let mut lo = p00 ^ (mid << 64);
    let mut hi = p11 ^ (mid >> 64);

    // Shift (hi:lo) left by 1 for reflected representation
    let carry = lo >> 127;
    lo <<= 1;
    hi = (hi << 1) | carry;

    // First phase reduction
    let a = lo ^ (lo << 127) ^ (lo << 126) ^ (lo << 121);
    // Second phase reduction
    let b = a ^ (a >> 1) ^ (a >> 2) ^ (a >> 7);
    hi ^ b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn ghash_sp800_38d_vector() {
        let h: [u8; 16] = [
            0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b, 0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34,
            0x2b, 0x2e,
        ];
        let c: [u8; 16] = [
            0x03, 0x88, 0xda, 0xce, 0x60, 0xb6, 0xa3, 0x92, 0xf3, 0x28, 0xc2, 0xb9, 0x71, 0xb2,
            0xfe, 0x78,
        ];
        let mut len_blk = [0u8; 16];
        len_blk[8..16].copy_from_slice(&(128u64).to_be_bytes());

        let mut g_ct = Ghash::new_portable(&h);
        g_ct.update_block(&c);
        g_ct.update_block(&len_blk);
        let tag_ct = g_ct.finalize();

        let mut g_hw = Ghash::new(&h);
        g_hw.update_block(&c);
        g_hw.update_block(&len_blk);
        let tag_hw = g_hw.finalize();

        assert_eq!(tag_ct, tag_hw);
        assert_eq!(encode_hex(&tag_ct), "f38cbb1ad69223dcc3457ae5b6b0f885");
    }
}
