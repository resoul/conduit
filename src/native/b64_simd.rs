//! SIMD fast paths for standard-alphabet Base64 (RFC 4648).
//!
//! Both routines process only whole blocks and report how many input bytes they consumed, so the
//! caller finishes the tail (and classifies any error) with its scalar code.

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline]
pub(crate) fn available() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        !cfg!(miri)
    }
    #[cfg(target_arch = "x86_64")]
    {
        !cfg!(miri) && std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        false
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) unsafe fn encode_prefix(src: &[u8], dst: &mut Vec<u8>) -> usize {
    let blocks = src.len() / 48;
    if blocks == 0 {
        return 0;
    }
    dst.reserve(blocks * 64);

    unsafe {
        let alpha = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let tbl = uint8x16x4_t(
            vld1q_u8(alpha.as_ptr()),
            vld1q_u8(alpha.as_ptr().add(16)),
            vld1q_u8(alpha.as_ptr().add(32)),
            vld1q_u8(alpha.as_ptr().add(48)),
        );
        let m03 = vdupq_n_u8(0x03);
        let m0f = vdupq_n_u8(0x0f);
        let m3f = vdupq_n_u8(0x3f);

        let in_ptr = src.as_ptr();
        let mut out_ptr = dst.as_mut_ptr().add(dst.len());
        let mut off = 0usize;

        for _ in 0..blocks {
            let inp = vld3q_u8(in_ptr.add(off));
            let i0 = vshrq_n_u8::<2>(inp.0);
            let i1 = vorrq_u8(vshlq_n_u8::<4>(vandq_u8(inp.0, m03)), vshrq_n_u8::<4>(inp.1));
            let i2 = vorrq_u8(vshlq_n_u8::<2>(vandq_u8(inp.1, m0f)), vshrq_n_u8::<6>(inp.2));
            let i3 = vandq_u8(inp.2, m3f);

            let out = uint8x16x4_t(
                vqtbl4q_u8(tbl, i0),
                vqtbl4q_u8(tbl, i1),
                vqtbl4q_u8(tbl, i2),
                vqtbl4q_u8(tbl, i3),
            );
            vst4q_u8(out_ptr, out);
            off += 48;
            out_ptr = out_ptr.add(64);
        }
        dst.set_len(dst.len() + blocks * 64);
        off
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) unsafe fn decode_prefix(src: &[u8], dst: &mut Vec<u8>) -> usize {
    let blocks = src.len() / 64;
    if blocks == 0 {
        return 0;
    }
    dst.reserve(blocks * 48);

    unsafe {
        let lo_lut_arr: [u8; 16] = [
            0x15, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, // 0..7
            0x11, 0x11, 0x13, 0x1A, 0x1B, 0x1B, 0x1B, 0x1A, // 8..15
        ];
        let hi_lut_arr: [u8; 16] = [
            0x10, 0x10, 0x01, 0x02, 0x04, 0x08, 0x04, 0x08, // 0..7
            0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, // 8..15
        ];
        let roll_lut_arr: [u8; 16] = [
            0, 0, 19, 4, 191, 191, 185, 185, // 0..7 (+, 0-9, A-O, P-Z, a-o, p-z)
            16, 0, 0, 0, 0, 0, 0, 0,         // 8='/' -> +16
        ];

        let lo_lut = vld1q_u8(lo_lut_arr.as_ptr());
        let hi_lut = vld1q_u8(hi_lut_arr.as_ptr());
        let roll_lut = vld1q_u8(roll_lut_arr.as_ptr());
        let mask_0f = vdupq_n_u8(0x0f);
        let slash = vdupq_n_u8(b'/');
        let six = vdupq_n_u8(6);

        let in_ptr = src.as_ptr();
        let mut out_ptr = dst.as_mut_ptr().add(dst.len());
        let mut off = 0usize;
        let mut done_blocks = 0usize;

        for _ in 0..blocks {
            let inp = vld4q_u8(in_ptr.add(off));

            macro_rules! map_and_check {
                ($c:expr) => {{
                    let hi = vshrq_n_u8::<4>($c);
                    let lo = vandq_u8($c, mask_0f);
                    let err = vandq_u8(vqtbl1q_u8(lo_lut, lo), vqtbl1q_u8(hi_lut, hi));
                    let is_sl = vandq_u8(vceqq_u8($c, slash), six);
                    let idx = vaddq_u8(hi, is_sl);
                    let val = vaddq_u8($c, vqtbl1q_u8(roll_lut, idx));
                    (val, err)
                }};
            }

            let (v0, e0) = map_and_check!(inp.0);
            let (v1, e1) = map_and_check!(inp.1);
            let (v2, e2) = map_and_check!(inp.2);
            let (v3, e3) = map_and_check!(inp.3);

            let any_err = vorrq_u8(vorrq_u8(e0, e1), vorrq_u8(e2, e3));
            if vmaxvq_u8(any_err) != 0 {
                break;
            }

            let o0 = vorrq_u8(vshlq_n_u8::<2>(v0), vshrq_n_u8::<4>(v1));
            let o1 = vorrq_u8(vshlq_n_u8::<4>(v1), vshrq_n_u8::<2>(v2));
            let o2 = vorrq_u8(vshlq_n_u8::<6>(v2), v3);
            vst3q_u8(out_ptr, uint8x16x3_t(o0, o1, o2));

            off += 64;
            out_ptr = out_ptr.add(48);
            done_blocks += 1;
        }
        dst.set_len(dst.len() + done_blocks * 48);
        off
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn encode_prefix(src: &[u8], dst: &mut Vec<u8>) -> usize {
    if src.len() < 28 {
        return 0;
    }
    let iters = (src.len() - 28) / 24 + 1;
    dst.reserve(iters * 32);

    unsafe {
        let shuf = _mm256_setr_epi8(
            1, 0, 2, 1, 4, 3, 5, 4, 7, 6, 8, 7, 10, 9, 11, 10, //
            1, 0, 2, 1, 4, 3, 5, 4, 7, 6, 8, 7, 10, 9, 11, 10,
        );
        let mask_a = _mm256_set1_epi32(0x0fc0_fc00);
        let mul_a = _mm256_set1_epi32(0x0400_0040);
        let mask_b = _mm256_set1_epi32(0x003f_03f0);
        let mul_b = _mm256_set1_epi32(0x0100_0010);
        let shift_lut = _mm256_setr_epi8(
            b'a' as i8 - 26,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'+' as i8 - 62,
            b'/' as i8 - 63,
            b'A' as i8,
            0,
            0,
            b'a' as i8 - 26,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'0' as i8 - 52,
            b'+' as i8 - 62,
            b'/' as i8 - 63,
            b'A' as i8,
            0,
            0,
        );
        let c51 = _mm256_set1_epi8(51);
        let c26 = _mm256_set1_epi8(26);
        let c13 = _mm256_set1_epi8(13);

        let mut out_ptr = dst.as_mut_ptr().add(dst.len());
        let in_ptr = src.as_ptr();
        let mut off = 0usize;
        for _ in 0..iters {
            let lo = _mm_loadu_si128(in_ptr.add(off) as *const __m128i);
            let hi = _mm_loadu_si128(in_ptr.add(off + 12) as *const __m128i);
            let v = _mm256_set_m128i(hi, lo);
            let grouped = _mm256_shuffle_epi8(v, shuf);
            let a = _mm256_mulhi_epu16(_mm256_and_si256(grouped, mask_a), mul_a);
            let b = _mm256_mullo_epi16(_mm256_and_si256(grouped, mask_b), mul_b);
            let indices = _mm256_or_si256(a, b);

            let mut lut_idx = _mm256_subs_epu8(indices, c51);
            let less26 = _mm256_cmpgt_epi8(c26, indices);
            lut_idx = _mm256_add_epi8(lut_idx, _mm256_and_si256(less26, c13));
            let shifts = _mm256_shuffle_epi8(shift_lut, lut_idx);
            let encoded = _mm256_add_epi8(indices, shifts);
            _mm256_storeu_si256(out_ptr as *mut __m256i, encoded);

            off += 24;
            out_ptr = out_ptr.add(32);
        }
        dst.set_len(dst.len() + iters * 32);
        off
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn decode_prefix(src: &[u8], dst: &mut Vec<u8>) -> usize {
    let blocks = src.len() / 32;
    if blocks == 0 {
        return 0;
    }
    dst.reserve(blocks * 24 + 8);

    unsafe {
        let lut_lo = _mm256_setr_epi8(
            0x15, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, //
            0x11, 0x11, 0x13, 0x1a, 0x1b, 0x1b, 0x1b, 0x1a, //
            0x15, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, //
            0x11, 0x11, 0x13, 0x1a, 0x1b, 0x1b, 0x1b, 0x1a,
        );
        let lut_hi = _mm256_setr_epi8(
            0x10, 0x10, 0x01, 0x02, 0x04, 0x08, 0x04, 0x08, //
            0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, //
            0x10, 0x10, 0x01, 0x02, 0x04, 0x08, 0x04, 0x08, //
            0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
        );
        let lut_roll = _mm256_setr_epi8(
            0, 16, 19, 4, -65, -65, -71, -71, 0, 0, 0, 0, 0, 0, 0, 0, //
            0, 16, 19, 4, -65, -65, -71, -71, 0, 0, 0, 0, 0, 0, 0, 0,
        );
        let mask_2f = _mm256_set1_epi8(0x2f);
        let mask_0f = _mm256_set1_epi8(0x0f);
        let mul_1 = _mm256_set1_epi32(0x0140_0140);
        let mul_2 = _mm256_set1_epi32(0x0001_1000);
        let pack_shuf = _mm256_setr_epi8(
            2, 1, 0, 6, 5, 4, 10, 9, 8, 14, 13, 12, -1, -1, -1, -1, //
            2, 1, 0, 6, 5, 4, 10, 9, 8, 14, 13, 12, -1, -1, -1, -1,
        );
        let lane_perm = _mm256_setr_epi32(0, 1, 2, 4, 5, 6, 3, 7);

        let in_ptr = src.as_ptr();
        let mut out_ptr = dst.as_mut_ptr().add(dst.len());
        let mut off = 0usize;
        let mut done = 0usize;

        for _ in 0..blocks {
            let v = _mm256_loadu_si256(in_ptr.add(off) as *const __m256i);
            let hi_nib = _mm256_and_si256(_mm256_srli_epi32(v, 4), mask_0f);
            let lo_nib = _mm256_and_si256(v, mask_0f);
            let err = _mm256_and_si256(
                _mm256_shuffle_epi8(lut_lo, lo_nib),
                _mm256_shuffle_epi8(lut_hi, hi_nib),
            );
            if _mm256_testz_si256(err, err) == 0 {
                break;
            }
            let eq_2f = _mm256_cmpeq_epi8(v, mask_2f);
            let roll_idx = _mm256_add_epi8(hi_nib, eq_2f);
            let rolled = _mm256_add_epi8(v, _mm256_shuffle_epi8(lut_roll, roll_idx));

            let merged = _mm256_maddubs_epi16(rolled, mul_1);
            let packed32 = _mm256_madd_epi16(merged, mul_2);
            let bytes = _mm256_shuffle_epi8(packed32, pack_shuf);
            let final_v = _mm256_permutevar8x32_epi32(bytes, lane_perm);
            _mm256_storeu_si256(out_ptr as *mut __m256i, final_v);

            off += 32;
            out_ptr = out_ptr.add(24);
            done += 1;
        }
        dst.set_len(dst.len() + done * 24);
        off
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) unsafe fn encode_prefix(_src: &[u8], _dst: &mut Vec<u8>) -> usize {
    0
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) unsafe fn decode_prefix(_src: &[u8], _dst: &mut Vec<u8>) -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn ref_encode(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for ch in bytes.chunks(3) {
            let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
            out.push(ALPHA[((n >> 18) & 63) as usize]);
            out.push(ALPHA[((n >> 12) & 63) as usize]);
            out.push(if ch.len() > 1 { ALPHA[((n >> 6) & 63) as usize] } else { b'=' });
            out.push(if ch.len() > 2 { ALPHA[(n & 63) as usize] } else { b'=' });
        }
        out
    }

    #[test]
    fn encode_prefix_matches_reference() {
        if !available() {
            return;
        }
        let data: Vec<u8> = (0..257).map(|i| (i * 37) as u8).collect();
        let mut out = Vec::new();
        let consumed = unsafe { encode_prefix(&data, &mut out) };
        assert!(consumed > 0);
        assert_eq!(consumed % 3, 0);
        assert_eq!(out, ref_encode(&data[..consumed]));
    }

    #[test]
    fn decode_prefix_matches_reference_and_rejects_every_bad_byte() {
        if !available() {
            return;
        }
        let raw: Vec<u8> = (0..192).map(|i| (i * 53) as u8).collect();
        let mut enc = ref_encode(&raw);
        let mut dec = Vec::new();
        let consumed = unsafe { decode_prefix(&enc, &mut dec) };
        assert_eq!(consumed, enc.len());
        assert_eq!(dec, raw);

        // Every non-alphabet byte at position 37 must stop before that block.
        for b in 0u8..=255 {
            if ALPHA.contains(&b) {
                continue;
            }
            let saved = enc[37];
            enc[37] = b;
            dec.clear();
            let c = unsafe { decode_prefix(&enc, &mut dec) };
            assert!(c <= 37, "bad byte 0x{:02x} was not rejected", b);
            enc[37] = saved;
        }
    }
}
