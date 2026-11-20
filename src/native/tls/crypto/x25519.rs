//! X25519 key exchange (RFC 7748), constant-time 5x51-bit field arithmetic.

use crate::native::crypto::constant_time_eq;

use super::Zeroize;

/// Field element mod 2^255 - 19 in 5 radix-2^51 limbs.
#[derive(Clone, Copy)]
struct Fe([u64; 5]);

impl Zeroize for Fe {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

const MASK51: u64 = (1u64 << 51) - 1;

impl Fe {
    const ZERO: Self = Self([0, 0, 0, 0, 0]);
    const ONE: Self = Self([1, 0, 0, 0, 0]);

    fn from_bytes(b: &[u8; 32]) -> Self {
        let load64 = |off: usize| -> u64 {
            u64::from_le_bytes([
                b[off],
                b[off + 1],
                b[off + 2],
                b[off + 3],
                b[off + 4],
                b[off + 5],
                b[off + 6],
                b[off + 7],
            ])
        };
        let t0 = load64(0);
        let t1 = load64(6);
        let t2 = load64(12);
        let t3 = load64(19);
        let t4 = load64(24);
        Self([
            t0 & MASK51,
            (t1 >> 3) & MASK51,
            (t2 >> 6) & MASK51,
            (t3 >> 1) & MASK51,
            (t4 >> 12) & MASK51,
        ])
    }

    fn to_bytes(mut self) -> [u8; 32] {
        self.reduce();
        // Canonicalize mod 2^255 - 19
        let mut q = (self.0[0] + 19) >> 51;
        q = (self.0[1] + q) >> 51;
        q = (self.0[2] + q) >> 51;
        q = (self.0[3] + q) >> 51;
        q = (self.0[4] + q) >> 51;

        self.0[0] += 19 * q;
        let c = self.0[0] >> 51;
        self.0[0] &= MASK51;
        self.0[1] += c;
        let c = self.0[1] >> 51;
        self.0[1] &= MASK51;
        self.0[2] += c;
        let c = self.0[2] >> 51;
        self.0[2] &= MASK51;
        self.0[3] += c;
        let c = self.0[3] >> 51;
        self.0[3] &= MASK51;
        self.0[4] = (self.0[4] + c) & MASK51;

        let mut out = [0u8; 32];
        let l0 = self.0[0] | (self.0[1] << 51);
        let l1 = (self.0[1] >> 13) | (self.0[2] << 38);
        let l2 = (self.0[2] >> 26) | (self.0[3] << 25);
        let l3 = (self.0[3] >> 39) | (self.0[4] << 12);
        out[0..8].copy_from_slice(&l0.to_le_bytes());
        out[8..16].copy_from_slice(&l1.to_le_bytes());
        out[16..24].copy_from_slice(&l2.to_le_bytes());
        out[24..32].copy_from_slice(&l3.to_le_bytes());
        out
    }

    #[inline]
    fn reduce(&mut self) {
        for _ in 0..2 {
            let c0 = self.0[0] >> 51;
            self.0[0] &= MASK51;
            self.0[1] += c0;
            let c1 = self.0[1] >> 51;
            self.0[1] &= MASK51;
            self.0[2] += c1;
            let c2 = self.0[2] >> 51;
            self.0[2] &= MASK51;
            self.0[3] += c2;
            let c3 = self.0[3] >> 51;
            self.0[3] &= MASK51;
            self.0[4] += c3;
            let c4 = self.0[4] >> 51;
            self.0[4] &= MASK51;
            self.0[0] += c4 * 19;
        }
    }

    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self([
            self.0[0] + rhs.0[0],
            self.0[1] + rhs.0[1],
            self.0[2] + rhs.0[2],
            self.0[3] + rhs.0[3],
            self.0[4] + rhs.0[4],
        ])
    }

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        // Add 2*p to avoid underflow
        const P0: u64 = 0x000f_ffff_ffff_ffda; // 2 * (2^51 - 19)
        const P1234: u64 = 0x000f_ffff_ffff_fffe; // 2 * (2^51 - 1)
        let mut r = Self([
            self.0[0] + P0 - rhs.0[0],
            self.0[1] + P1234 - rhs.0[1],
            self.0[2] + P1234 - rhs.0[2],
            self.0[3] + P1234 - rhs.0[3],
            self.0[4] + P1234 - rhs.0[4],
        ]);
        r.reduce();
        r
    }

    #[inline]
    fn mul(self, rhs: Self) -> Self {
        let a0 = self.0[0] as u128;
        let a1 = self.0[1] as u128;
        let a2 = self.0[2] as u128;
        let a3 = self.0[3] as u128;
        let a4 = self.0[4] as u128;

        let b0 = rhs.0[0] as u128;
        let b1 = rhs.0[1] as u128;
        let b2 = rhs.0[2] as u128;
        let b3 = rhs.0[3] as u128;
        let b4 = rhs.0[4] as u128;

        let b1_19 = b1 * 19;
        let b2_19 = b2 * 19;
        let b3_19 = b3 * 19;
        let b4_19 = b4 * 19;

        let mut t0 = a0 * b0 + a1 * b4_19 + a2 * b3_19 + a3 * b2_19 + a4 * b1_19;
        let mut t1 = a0 * b1 + a1 * b0 + a2 * b4_19 + a3 * b3_19 + a4 * b2_19;
        let mut t2 = a0 * b2 + a1 * b1 + a2 * b0 + a3 * b4_19 + a4 * b3_19;
        let mut t3 = a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * b4_19;
        let mut t4 = a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;

        let m51 = MASK51 as u128;
        t1 += t0 >> 51;
        let r0 = (t0 & m51) as u64;
        t2 += t1 >> 51;
        let r1 = (t1 & m51) as u64;
        t3 += t2 >> 51;
        let r2 = (t2 & m51) as u64;
        t4 += t3 >> 51;
        let r3 = (t3 & m51) as u64;
        t0 = (r0 as u128) + (t4 >> 51) * 19;
        let r4 = (t4 & m51) as u64;

        let mut res = Self([
            (t0 & m51) as u64,
            r1 + (t0 >> 51) as u64,
            r2,
            r3,
            r4,
        ]);
        res.reduce();
        res
    }

    #[inline]
    fn sq(self) -> Self {
        self.mul(self)
    }

    #[inline]
    fn mul_a24(self) -> Self {
        const A24: u128 = 121_665;
        let mut t0 = (self.0[0] as u128) * A24;
        let mut t1 = (self.0[1] as u128) * A24;
        let mut t2 = (self.0[2] as u128) * A24;
        let mut t3 = (self.0[3] as u128) * A24;
        let mut t4 = (self.0[4] as u128) * A24;

        let m51 = MASK51 as u128;
        t1 += t0 >> 51;
        let r0 = (t0 & m51) as u64;
        t2 += t1 >> 51;
        let r1 = (t1 & m51) as u64;
        t3 += t2 >> 51;
        let r2 = (t2 & m51) as u64;
        t4 += t3 >> 51;
        let r3 = (t3 & m51) as u64;
        t0 = (r0 as u128) + (t4 >> 51) * 19;
        let r4 = (t4 & m51) as u64;

        let mut res = Self([(t0 & m51) as u64, r1 + (t0 >> 51) as u64, r2, r3, r4]);
        res.reduce();
        res
    }

    /// Modular inverse mod 2^255 - 19 via Fermat: x^(p-2).
    fn inv(self) -> Self {
        let z2 = self.sq();
        let z9 = z2.sq().sq().mul(self);
        let z11 = z9.mul(z2);
        let z2_5_0 = z11.sq().mul(z9);
        let mut t = z2_5_0;
        for _ in 0..5 {
            t = t.sq();
        }
        let z2_10_0 = t.mul(z2_5_0);
        t = z2_10_0;
        for _ in 0..10 {
            t = t.sq();
        }
        let z2_20_0 = t.mul(z2_10_0);
        t = z2_20_0;
        for _ in 0..20 {
            t = t.sq();
        }
        let z2_40_0 = t.mul(z2_20_0);
        t = z2_40_0;
        for _ in 0..10 {
            t = t.sq();
        }
        let z2_50_0 = t.mul(z2_10_0);
        t = z2_50_0;
        for _ in 0..50 {
            t = t.sq();
        }
        let z2_100_0 = t.mul(z2_50_0);
        t = z2_100_0;
        for _ in 0..100 {
            t = t.sq();
        }
        let z2_200_0 = t.mul(z2_100_0);
        t = z2_200_0;
        for _ in 0..50 {
            t = t.sq();
        }
        let z2_250_0 = t.mul(z2_50_0);
        t = z2_250_0;
        for _ in 0..5 {
            t = t.sq();
        }
        t.mul(z11)
    }

    #[inline]
    fn cswap(swap: u64, a: &mut Self, b: &mut Self) {
        let mask = 0u64.wrapping_sub(swap & 1);
        for i in 0..5 {
            let x = mask & (a.0[i] ^ b.0[i]);
            a.0[i] ^= x;
            b.0[i] ^= x;
        }
    }
}

pub const BASEPOINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Raw X25519 scalar multiplication (RFC 7748 §5).
pub fn x25519_raw(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k_clamped = *k;
    k_clamped[0] &= 248;
    k_clamped[31] &= 127;
    k_clamped[31] |= 64;

    let mut u_masked = *u;
    u_masked[31] &= 127;

    let x_1 = Fe::from_bytes(&u_masked);
    let mut x_2 = Fe::ONE;
    let mut z_2 = Fe::ZERO;
    let mut x_3 = x_1;
    let mut z_3 = Fe::ONE;
    let mut swap = 0u64;

    for t in (0..255).rev() {
        let k_t = ((k_clamped[t / 8] >> (t & 7)) & 1) as u64;
        swap ^= k_t;
        Fe::cswap(swap, &mut x_2, &mut x_3);
        Fe::cswap(swap, &mut z_2, &mut z_3);
        swap = k_t;

        let a = x_2.add(z_2);
        let aa = a.sq();
        let b = x_2.sub(z_2);
        let bb = b.sq();
        let e = aa.sub(bb);
        let c = x_3.add(z_3);
        let d = x_3.sub(z_3);
        let da = d.mul(a);
        let cb = c.mul(b);
        x_3 = da.add(cb).sq();
        z_3 = x_1.mul(da.sub(cb).sq());
        x_2 = aa.mul(bb);
        z_2 = e.mul(aa.add(e.mul_a24()));
    }
    Fe::cswap(swap, &mut x_2, &mut x_3);
    Fe::cswap(swap, &mut z_2, &mut z_3);

    k_clamped.zeroize();
    let res = x_2.mul(z_2.inv()).to_bytes();
    x_2.zeroize();
    z_2.zeroize();
    x_3.zeroize();
    z_3.zeroize();
    res
}

pub fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    x25519_raw(secret, &BASEPOINT)
}

/// X25519 ECDH shared secret with RFC 7748 all-zero output check.
pub fn x25519_shared(secret: &[u8; 32], peer_public: &[u8; 32]) -> Option<[u8; 32]> {
    let ss = x25519_raw(secret, peer_public);
    if constant_time_eq(&ss, &[0u8; 32]) {
        None
    } else {
        Some(ss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    fn decode_hex_32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn x25519_rfc7748_section_5_2() {
        let scalar = decode_hex_32("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let u_in = decode_hex_32("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
        let out = x25519_raw(&scalar, &u_in);
        assert_eq!(
            encode_hex(&out),
            "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
        );
    }

    #[test]
    fn x25519_rfc7748_iterative_1_and_1000() {
        let mut k = BASEPOINT;
        let mut u = BASEPOINT;
        for iter in 1..=1000 {
            let res = x25519_raw(&k, &u);
            u = k;
            k = res;
            if iter == 1 {
                assert_eq!(
                    encode_hex(&k),
                    "422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079"
                );
            }
        }
        assert_eq!(
            encode_hex(&k),
            "684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51"
        );
    }

    #[test]
    fn x25519_rfc7748_section_6_1_diffie_hellman() {
        let a_priv = decode_hex_32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let a_pub_expected = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";
        let b_priv = decode_hex_32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let b_pub_expected = "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f";

        let a_pub = x25519_public(&a_priv);
        let b_pub = x25519_public(&b_priv);
        assert_eq!(encode_hex(&a_pub), a_pub_expected);
        assert_eq!(encode_hex(&b_pub), b_pub_expected);

        let ss_a = x25519_shared(&a_priv, &b_pub).unwrap();
        let ss_b = x25519_shared(&b_priv, &a_pub).unwrap();
        assert_eq!(ss_a, ss_b);
        assert_eq!(
            encode_hex(&ss_a),
            "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
        );

        // Low-order point yields all-zero -> None
        assert!(x25519_shared(&a_priv, &[0u8; 32]).is_none());
    }
}
