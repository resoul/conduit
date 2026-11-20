//! Poly1305 one-time authenticator (RFC 8439 §2.5), constant-time 26-bit limbs.

use super::Zeroize;

pub struct Poly1305 {
    r: [u32; 5],
    s: [u32; 4],
    h: [u32; 5],
    buf: [u8; 16],
    buf_len: usize,
}

impl Drop for Poly1305 {
    fn drop(&mut self) {
        self.r.zeroize();
        self.s.zeroize();
        self.h.zeroize();
        self.buf.zeroize();
    }
}

impl Poly1305 {
    pub fn new(key: &[u8; 32]) -> Self {
        let mut r_bytes = [0u8; 16];
        r_bytes.copy_from_slice(&key[..16]);
        // Clamp r per RFC 8439 §2.5
        r_bytes[3] &= 15;
        r_bytes[7] &= 15;
        r_bytes[11] &= 15;
        r_bytes[15] &= 15;
        r_bytes[4] &= 252;
        r_bytes[8] &= 252;
        r_bytes[12] &= 252;

        let t0 = u32::from_le_bytes([r_bytes[0], r_bytes[1], r_bytes[2], r_bytes[3]]);
        let t1 = u32::from_le_bytes([r_bytes[4], r_bytes[5], r_bytes[6], r_bytes[7]]);
        let t2 = u32::from_le_bytes([r_bytes[8], r_bytes[9], r_bytes[10], r_bytes[11]]);
        let t3 = u32::from_le_bytes([r_bytes[12], r_bytes[13], r_bytes[14], r_bytes[15]]);
        r_bytes.zeroize();

        let r = [
            t0 & 0x03ff_ffff,
            ((t0 >> 26) | (t1 << 6)) & 0x03ff_ffff,
            ((t1 >> 20) | (t2 << 12)) & 0x03ff_ffff,
            ((t2 >> 14) | (t3 << 18)) & 0x03ff_ffff,
            (t3 >> 8) & 0x03ff_ffff,
        ];
        let s = [
            u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
            u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
            u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
            u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        ];
        Self {
            r,
            s,
            h: [0; 5],
            buf: [0; 16],
            buf_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        if self.buf_len > 0 {
            let take = (16 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 16 {
                let blk = self.buf;
                self.process_block(&blk, false);
                self.buf_len = 0;
            }
        }
        let (chunks, rem) = data.as_chunks::<16>();
        for chunk in chunks {
            self.process_block(chunk, false);
        }
        if !rem.is_empty() {
            self.buf[..rem.len()].copy_from_slice(rem);
            self.buf_len = rem.len();
        }
    }

    fn process_block(&mut self, block: &[u8; 16], is_final_partial: bool) {
        let hibit = if is_final_partial { 0 } else { 1u32 << 24 };
        let t0 = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let t1 = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let t2 = u32::from_le_bytes([block[8], block[9], block[10], block[11]]);
        let t3 = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

        let h0 = u64::from(self.h[0]) + u64::from(t0 & 0x03ff_ffff);
        let h1 = u64::from(self.h[1]) + u64::from(((t0 >> 26) | (t1 << 6)) & 0x03ff_ffff);
        let h2 = u64::from(self.h[2]) + u64::from(((t1 >> 20) | (t2 << 12)) & 0x03ff_ffff);
        let h3 = u64::from(self.h[3]) + u64::from(((t2 >> 14) | (t3 << 18)) & 0x03ff_ffff);
        let h4 = u64::from(self.h[4]) + u64::from((t3 >> 8) | hibit);

        let r0 = u64::from(self.r[0]);
        let r1 = u64::from(self.r[1]);
        let r2 = u64::from(self.r[2]);
        let r3 = u64::from(self.r[3]);
        let r4 = u64::from(self.r[4]);

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        let mut c = (d0 >> 26) as u32;
        self.h[0] = (d0 as u32) & 0x03ff_ffff;
        d1 += u64::from(c);
        c = (d1 >> 26) as u32;
        self.h[1] = (d1 as u32) & 0x03ff_ffff;
        d2 += u64::from(c);
        c = (d2 >> 26) as u32;
        self.h[2] = (d2 as u32) & 0x03ff_ffff;
        d3 += u64::from(c);
        c = (d3 >> 26) as u32;
        self.h[3] = (d3 as u32) & 0x03ff_ffff;
        d4 += u64::from(c);
        c = (d4 >> 26) as u32;
        self.h[4] = (d4 as u32) & 0x03ff_ffff;
        self.h[0] += c * 5;
        c = self.h[0] >> 26;
        self.h[0] &= 0x03ff_ffff;
        self.h[1] += c;
    }

    pub fn finalize(mut self) -> [u8; 16] {
        if self.buf_len > 0 {
            let mut blk = [0u8; 16];
            blk[..self.buf_len].copy_from_slice(&self.buf[..self.buf_len]);
            blk[self.buf_len] = 1;
            self.process_block(&blk, true);
        }

        let mut c = self.h[1] >> 26;
        self.h[1] &= 0x03ff_ffff;
        self.h[2] += c;
        c = self.h[2] >> 26;
        self.h[2] &= 0x03ff_ffff;
        self.h[3] += c;
        c = self.h[3] >> 26;
        self.h[3] &= 0x03ff_ffff;
        self.h[4] += c;
        c = self.h[4] >> 26;
        self.h[4] &= 0x03ff_ffff;
        self.h[0] += c * 5;
        c = self.h[0] >> 26;
        self.h[0] &= 0x03ff_ffff;
        self.h[1] += c;

        // Constant-time freeze mod 2^130 - 5
        let mut g0 = self.h[0].wrapping_add(5);
        c = g0 >> 26;
        g0 &= 0x03ff_ffff;
        let mut g1 = self.h[1].wrapping_add(c);
        c = g1 >> 26;
        g1 &= 0x03ff_ffff;
        let mut g2 = self.h[2].wrapping_add(c);
        c = g2 >> 26;
        g2 &= 0x03ff_ffff;
        let mut g3 = self.h[3].wrapping_add(c);
        c = g3 >> 26;
        g3 &= 0x03ff_ffff;
        let g4 = self.h[4].wrapping_add(c).wrapping_sub(1 << 26);

        let mask = (g4 >> 31).wrapping_sub(1);
        let nmask = !mask;
        self.h[0] = (self.h[0] & nmask) | (g0 & mask);
        self.h[1] = (self.h[1] & nmask) | (g1 & mask);
        self.h[2] = (self.h[2] & nmask) | (g2 & mask);
        self.h[3] = (self.h[3] & nmask) | (g3 & mask);
        self.h[4] = (self.h[4] & nmask) | (g4 & mask);

        let f0 = self.h[0] | (self.h[1] << 26);
        let f1 = (self.h[1] >> 6) | (self.h[2] << 20);
        let f2 = (self.h[2] >> 12) | (self.h[3] << 14);
        let f3 = (self.h[3] >> 18) | (self.h[4] << 8);

        let mut acc = u64::from(f0) + u64::from(self.s[0]);
        let w0 = acc as u32;
        acc = u64::from(f1) + u64::from(self.s[1]) + (acc >> 32);
        let w1 = acc as u32;
        acc = u64::from(f2) + u64::from(self.s[2]) + (acc >> 32);
        let w2 = acc as u32;
        acc = u64::from(f3) + u64::from(self.s[3]) + (acc >> 32);
        let w3 = acc as u32;

        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&w0.to_le_bytes());
        out[4..8].copy_from_slice(&w1.to_le_bytes());
        out[8..12].copy_from_slice(&w2.to_le_bytes());
        out[12..16].copy_from_slice(&w3.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn poly1305_rfc8439_vector() {
        let key: [u8; 32] = [
            0x85, 0xd6, 0xbe, 0x78, 0x57, 0x55, 0x6d, 0x33, 0x7f, 0x44, 0x52, 0xfe, 0x42, 0xd5,
            0x06, 0xa8, 0x01, 0x03, 0x80, 0x8a, 0xfb, 0x0d, 0xb2, 0xfd, 0x4a, 0xbf, 0xf6, 0xaf,
            0x41, 0x49, 0xf5, 0x1b,
        ];
        let msg = b"Cryptographic Forum Research Group";
        let mut p = Poly1305::new(&key);
        p.update(msg);
        assert_eq!(encode_hex(&p.finalize()), "a8061dc1305136c6c22b8baf0c0127a9");
    }
}
