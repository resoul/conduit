use super::Zeroize;
use super::bigint::BigUint;
use super::p384::parse_der_ecdsa_sig;

const P_U32: [u32; 8] = [
    0xffff_ffff, 0xffff_ffff, 0xffff_ffff, 0x0000_0000, 0x0000_0000, 0x0000_0000, 0x0000_0001,
    0xffff_ffff,
];

const B_BE: [u8; 32] = [
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86, 0xbc,
    0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6, 0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
];

const N_BE: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];

const GX_BE: [u8; 32] = [
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2,
    0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
];

const GY_BE: [u8; 32] = [
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16,
    0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];

#[derive(Clone, Copy, PartialEq, Eq)]
struct Fe([u32; 8]);

impl Zeroize for Fe {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Fe {
    const ZERO: Self = Self([0; 8]);
    const ONE: Self = Self([1, 0, 0, 0, 0, 0, 0, 0]);

    fn from_be_bytes(b: &[u8; 32]) -> Option<Self> {
        let mut limbs = [0u32; 8];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let off = (7 - i) * 4;
            *limb = u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
        }
        let fe = Self(limbs);
        let (_, borrow) = sub_raw(&fe.0, &P_U32);
        if borrow == 0 {
            return None;
        }
        Some(fe)
    }

    fn to_be_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, &limb) in self.0.iter().enumerate() {
            let off = (7 - i) * 4;
            out[off..off + 4].copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    #[inline]
    fn is_zero_ct(self) -> u32 {
        let mut acc = 0u32;
        for &w in &self.0 {
            acc |= w;
        }

        ((acc | acc.wrapping_neg()) >> 31) ^ 1
    }

    #[inline]
    fn cmov(&mut self, other: &Self, cond: u32) {
        let mask = 0u32.wrapping_sub(cond & 1);
        for i in 0..8 {
            self.0[i] ^= mask & (self.0[i] ^ other.0[i]);
        }
    }

    #[inline]
    fn add(self, rhs: Self) -> Self {
        let (sum, carry) = add_raw(&self.0, &rhs.0);
        let (reduced, borrow) = sub_raw(&sum, &P_U32);
        // If carry == 1 or borrow == 0, use `reduced`
        let use_reduced = carry | (borrow ^ 1);
        let mut out = Self(sum);
        out.cmov(&Self(reduced), use_reduced);
        out
    }

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        let (diff, borrow) = sub_raw(&self.0, &rhs.0);
        let (added, _) = add_raw(&diff, &P_U32);
        let mut out = Self(diff);
        out.cmov(&Self(added), borrow);
        out
    }

    #[inline]
    fn mul(self, rhs: Self) -> Self {
        let mut prod = [0u64; 16];
        for i in 0..8 {
            let mut carry = 0u64;
            for j in 0..8 {
                let v = (self.0[i] as u64) * (rhs.0[j] as u64) + prod[i + j] + carry;
                prod[i + j] = v & 0xffff_ffff;
                carry = v >> 32;
            }
            prod[i + 8] = carry;
        }
        let mut c = [0i64; 16];
        for i in 0..16 {
            c[i] = prod[i] as i64;
        }

        // NIST FIPS 186-4 §D.2.3 fast reduction mod P-256
        let mut acc = [0i64; 8];
        acc[0] = c[0] + c[8] + c[9] - c[11] - c[12] - c[13] - c[14];
        acc[1] = c[1] + c[9] + c[10] - c[12] - c[13] - c[14] - c[15];
        acc[2] = c[2] + c[10] + c[11] - c[13] - c[14] - c[15];
        acc[3] = c[3] + 2 * (c[11] + c[12]) + c[13] - c[15] - c[8] - c[9];
        acc[4] = c[4] + 2 * (c[12] + c[13]) + c[14] - c[9] - c[10];
        acc[5] = c[5] + 2 * (c[13] + c[14]) + c[15] - c[10] - c[11];
        acc[6] = c[6] + 2 * (c[14] + c[15]) + c[13] + c[14] - c[8] - c[9];
        acc[7] = c[7] + 2 * c[15] + c[15] + c[8] - c[10] - c[11] - c[12] - c[13];

        // Propagate signed carries (branchless, constant-time)
        let mut carry = 0i64;
        for w in &mut acc {
            let v = *w + carry;
            *w = v & 0xffff_ffff;
            carry = v >> 32;
        }
        // Fold top carry `k` using 2^256 ≡ 2^224 - 2^192 - 2^96 + 1 (mod p), branchless
        for _ in 0..2 {
            let k = carry;
            acc[0] += k;
            acc[3] -= k;
            acc[6] -= k;
            acc[7] += k;
            carry = 0;
            for w in &mut acc {
                let v = *w + carry;
                *w = v & 0xffff_ffff;
                carry = v >> 32;
            }
        }
        let mut res = Self([
            acc[0] as u32,
            acc[1] as u32,
            acc[2] as u32,
            acc[3] as u32,
            acc[4] as u32,
            acc[5] as u32,
            acc[6] as u32,
            acc[7] as u32,
        ]);
        // Final constant-time reduction: if carry < 0 add p; if carry > 0 or res >= p sub p
        let neg_mask = ((carry >> 63) & 1) as u32;
        let (added, _) = add_raw(&res.0, &P_U32);
        res.cmov(&Self(added), neg_mask);
        carry += neg_mask as i64;

        for _ in 0..3 {
            let (subbed, borrow) = sub_raw(&res.0, &P_U32);
            let pos_mask = (((-carry) >> 63) & 1) as u32;
            let need_sub = pos_mask | (borrow ^ 1);
            res.cmov(&Self(subbed), need_sub);
            carry -= pos_mask as i64;
        }
        res
    }

    #[inline]
    fn sq(self) -> Self {
        self.mul(self)
    }

    /// Fermat inversion x^(p-2) mod p (constant-time).
    fn inv(self) -> Self {
        let mut res = Self::ONE;
        let mut base = self;
        // p - 2 in little-endian u32
        let exp: [u32; 8] = [
            0xffff_fffd,
            0xffff_ffff,
            0xffff_ffff,
            0x0000_0000,
            0x0000_0000,
            0x0000_0000,
            0x0000_0001,
            0xffff_ffff,
        ];
        for (wi, &word) in exp.iter().enumerate() {
            for bit in 0..32 {
                let prod = res.mul(base);
                res.cmov(&prod, (word >> bit) & 1);
                if !(wi == 7 && bit == 31) {
                    base = base.sq();
                }
            }
        }
        res
    }
}

#[inline]
fn add_raw(a: &[u32; 8], b: &[u32; 8]) -> ([u32; 8], u32) {
    let mut out = [0u32; 8];
    let mut carry = 0u64;
    for i in 0..8 {
        let s = (a[i] as u64) + (b[i] as u64) + carry;
        out[i] = s as u32;
        carry = s >> 32;
    }
    (out, carry as u32)
}

#[inline]
fn sub_raw(a: &[u32; 8], b: &[u32; 8]) -> ([u32; 8], u32) {
    let mut out = [0u32; 8];
    let mut borrow = 0u64;
    for i in 0..8 {
        let diff = (a[i] as u64)
            .wrapping_sub(b[i] as u64)
            .wrapping_sub(borrow);
        out[i] = diff as u32;
        borrow = (diff >> 63) & 1;
    }
    (out, borrow as u32)
}

#[derive(Clone, Copy)]
struct JacobianPoint {
    x: Fe,
    y: Fe,
    z: Fe,
}

impl Zeroize for JacobianPoint {
    fn zeroize(&mut self) {
        self.x.zeroize();
        self.y.zeroize();
        self.z.zeroize();
    }
}

impl JacobianPoint {
    const INFINITY: Self = Self {
        x: Fe::ONE,
        y: Fe::ONE,
        z: Fe::ZERO,
    };

    fn cmov(&mut self, other: &Self, cond: u32) {
        self.x.cmov(&other.x, cond);
        self.y.cmov(&other.y, cond);
        self.z.cmov(&other.z, cond);
    }

    fn double(&self) -> Self {
        // Point doubling for a = -3 in Jacobian coordinates (hyperelliptic.org)
        let delta = self.z.sq();
        let gamma = self.y.sq();
        let beta = self.x.mul(gamma);
        let alpha = {
            let t1 = self.x.sub(delta);
            let t2 = self.x.add(delta);
            let p = t1.mul(t2);
            p.add(p).add(p) // 3 * (X - Z^2) * (X + Z^2)
        };
        let x3 = alpha.sq().sub(beta.add(beta).add(beta).add(beta).add(beta).add(beta).add(beta).add(beta)); // - 8*beta
        let z3 = self.y.add(self.z).sq().sub(gamma).sub(delta);
        let four_beta = beta.add(beta).add(beta).add(beta);
        let eight_gamma2 = {
            let g2 = gamma.sq();
            let g4 = g2.add(g2);
            let g8 = g4.add(g4);
            g8.add(g8) // wait: 8*gamma^2! g4 is 2*g2, g8 is 4*g2, so g8.add(g8) = 8*g2!
        };
        let y3 = alpha.mul(four_beta.sub(x3)).sub(eight_gamma2);

        let mut out = Self {
            x: x3,
            y: y3,
            z: z3,
        };
        let is_inf = self.z.is_zero_ct();
        out.cmov(&Self::INFINITY, is_inf);
        out
    }

    /// Constant-time Jacobian addition handling infinity and P == Q.
    fn add_ct(&self, other: &Self) -> Self {
        let z1z1 = self.z.sq();
        let z2z2 = other.z.sq();
        let u1 = self.x.mul(z2z2);
        let u2 = other.x.mul(z1z1);
        let s1 = self.y.mul(other.z).mul(z2z2);
        let s2 = other.y.mul(self.z).mul(z1z1);

        let h = u2.sub(u1);
        let r = s2.sub(s1);

        let hh = h.sq();
        let hhh = hh.mul(h);
        let v = u1.mul(hh);

        let x3 = r.sq().sub(hhh).sub(v.add(v));
        let y3 = r.mul(v.sub(x3)).sub(s1.mul(hhh));
        let z3 = self.z.mul(other.z).mul(h);

        let mut out = Self {
            x: x3,
            y: y3,
            z: z3,
        };
        let self_inf = self.z.is_zero_ct();
        let other_inf = other.z.is_zero_ct();
        let h_zero = h.is_zero_ct();
        let r_zero = r.is_zero_ct();

        let dbl = self.double();
        // If P == Q (h == 0 && r == 0 && neither inf), use doubled point
        let same = h_zero & r_zero & (self_inf ^ 1) & (other_inf ^ 1);
        out.cmov(&dbl, same);
        // If P == -Q (h == 0 && r != 0 && neither inf), infinity
        let neg = h_zero & (r_zero ^ 1) & (self_inf ^ 1) & (other_inf ^ 1);
        out.cmov(&Self::INFINITY, neg);
        out.cmov(other, self_inf);
        out.cmov(self, other_inf);
        out
    }
}

fn parse_uncompressed_point(bytes: &[u8]) -> Option<(Fe, Fe)> {
    if bytes.len() != 65 || bytes[0] != 0x04 {
        return None;
    }
    let x = Fe::from_be_bytes(bytes[1..33].try_into().ok()?)?;
    let y = Fe::from_be_bytes(bytes[33..65].try_into().ok()?)?;
    // Verify y^2 == x^3 - 3x + b (mod p)
    let b = Fe::from_be_bytes(&B_BE)?;
    let lhs = y.sq();
    let x3 = x.sq().mul(x);
    let three_x = x.add(x).add(x);
    let rhs = x3.sub(three_x).add(b);
    if lhs != rhs {
        return None;
    }
    Some((x, y))
}

fn scalar_mul_ct(scalar: &[u8; 32], x: Fe, y: Fe) -> JacobianPoint {
    let base = JacobianPoint { x, y, z: Fe::ONE };
    let mut r0 = JacobianPoint::INFINITY;
    let mut r1 = base;
    for byte in scalar {
        for bit in (0..8).rev() {
            let b = ((*byte >> bit) & 1) as u32;
            // Constant-time Montgomery ladder in Jacobian coordinates:
            // if b == 0: r1 = r0 + r1; r0 = 2*r0
            // if b == 1: r0 = r0 + r1; r1 = 2*r1
            let sum = r0.add_ct(&r1);
            let dbl0 = r0.double();
            let dbl1 = r1.double();
            r0.cmov(&dbl0, b ^ 1);
            r0.cmov(&sum, b);
            r1.cmov(&sum, b ^ 1);
            r1.cmov(&dbl1, b);
        }
    }
    r0
}

/// Compute P-256 public key (uncompressed 65 bytes: `0x04 || x || y`) from 32-byte scalar.
pub fn p256_public_key(scalar: &[u8; 32]) -> Option<[u8; 65]> {
    let n = BigUint::from_be_bytes(&N_BE);
    let s = BigUint::from_be_bytes(scalar);
    if s.is_zero() || s.cmp_val(&n) != std::cmp::Ordering::Less {
        return None;
    }
    let gx = Fe::from_be_bytes(&GX_BE)?;
    let gy = Fe::from_be_bytes(&GY_BE)?;
    let res = scalar_mul_ct(scalar, gx, gy);
    if res.z.is_zero_ct() == 1 {
        return None;
    }
    let z_inv = res.z.inv();
    let z_inv2 = z_inv.sq();
    let z_inv3 = z_inv2.mul(z_inv);
    let ax = res.x.mul(z_inv2).to_be_bytes();
    let ay = res.y.mul(z_inv3).to_be_bytes();
    let mut out = [0u8; 65];
    out[0] = 0x04;
    out[1..33].copy_from_slice(&ax);
    out[33..65].copy_from_slice(&ay);
    Some(out)
}

/// Constant-time P-256 ECDH: returns 32-byte x-coordinate shared secret.
pub fn p256_ecdh(scalar: &[u8; 32], peer_uncompressed: &[u8]) -> Option<[u8; 32]> {
    let n = BigUint::from_be_bytes(&N_BE);
    let s = BigUint::from_be_bytes(scalar);
    if s.is_zero() || s.cmp_val(&n) != std::cmp::Ordering::Less {
        return None;
    }
    let (px, py) = parse_uncompressed_point(peer_uncompressed)?;
    let mut res = scalar_mul_ct(scalar, px, py);
    if res.z.is_zero_ct() == 1 {
        res.zeroize();
        return None;
    }
    let z_inv = res.z.inv();
    let z_inv2 = z_inv.sq();
    let ss = res.x.mul(z_inv2).to_be_bytes();
    res.zeroize();
    Some(ss)
}

/// Verify an ECDSA P-256 signature (DER-encoded `r, s`) over a digest.
pub fn ecdsa_p256_verify_der(pubkey_uncompressed: &[u8], digest: &[u8], der_sig: &[u8]) -> bool {
    let Some((r_bytes, s_bytes)) = parse_der_ecdsa_sig(der_sig) else {
        return false;
    };
    ecdsa_p256_verify_rs(pubkey_uncompressed, digest, r_bytes, s_bytes)
}

pub fn ecdsa_p256_verify_rs(
    pubkey_uncompressed: &[u8],
    digest: &[u8],
    r_bytes: &[u8],
    s_bytes: &[u8],
) -> bool {
    let Some((qx, qy)) = parse_uncompressed_point(pubkey_uncompressed) else {
        return false;
    };
    let n = BigUint::from_be_bytes(&N_BE);
    let r = BigUint::from_be_bytes(r_bytes);
    let s = BigUint::from_be_bytes(s_bytes);
    if r.is_zero()
        || s.is_zero()
        || r.cmp_val(&n) != std::cmp::Ordering::Less
        || s.cmp_val(&n) != std::cmp::Ordering::Less
    {
        return false;
    }
    let z_slice = if digest.len() > 32 {
        &digest[..32]
    } else {
        digest
    };
    let z = BigUint::from_be_bytes(z_slice).rem(&n);
    let Some(s_inv) = s.mod_inv_prime(&n) else {
        return false;
    };
    let u1 = z.mod_mul(&s_inv, &n).to_be_bytes_padded(32);
    let u2 = r.mod_mul(&s_inv, &n).to_be_bytes_padded(32);

    let gx = Fe::from_be_bytes(&GX_BE).unwrap();
    let gy = Fe::from_be_bytes(&GY_BE).unwrap();

    let p1 = scalar_mul_ct(&u1.try_into().unwrap(), gx, gy);
    let p2 = scalar_mul_ct(&u2.try_into().unwrap(), qx, qy);
    let sum = p1.add_ct(&p2);
    if sum.z.is_zero_ct() == 1 {
        return false;
    }
    let z_inv = sum.z.inv();
    let rx_bytes = sum.x.mul(z_inv.sq()).to_be_bytes();
    let rx = BigUint::from_be_bytes(&rx_bytes).rem(&n);
    rx == r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::{encode_hex, sha256_bytes};

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn p256_ecdh_and_validation() {
        let d_a: [u8; 32] = hex_to_bytes("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433")
            .try_into()
            .unwrap();
        let d_b: [u8; 32] = hex_to_bytes("c6ef9c5d78ae012a011164acb377ce20c64f2c9043c16c94dc9f1e9c28791846")
            .try_into()
            .unwrap();
        let pub_a = p256_public_key(&d_a).unwrap();
        let pub_b = p256_public_key(&d_b).unwrap();

        let ss_a = p256_ecdh(&d_a, &pub_b).unwrap();
        let ss_b = p256_ecdh(&d_b, &pub_a).unwrap();
        assert_eq!(ss_a, ss_b);

        // RFC 5903 / NIST CAVP check for pub_a:
        assert_eq!(
            encode_hex(&pub_a[1..33]),
            "dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c3772581180"
        );
        assert_eq!(
            encode_hex(&pub_a[33..65]),
            "5271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3"
        );

        // Invalid point (not on curve) must be rejected
        let mut bad_pub = pub_b;
        bad_pub[1] ^= 1;
        assert!(p256_ecdh(&d_a, &bad_pub).is_none());

        // Point at infinity / wrong prefix
        assert!(p256_ecdh(&d_a, &[0u8; 65]).is_none());
    }

    #[test]
    fn p256_ecdsa_cavp_and_wycheproof_rejections() {
        // RFC 6979 / NIST P-256 test vector
        let qx = hex_to_bytes("60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6");
        let qy = hex_to_bytes("7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
        let mut pubkey = vec![0x04];
        pubkey.extend_from_slice(&qx);
        pubkey.extend_from_slice(&qy);

        let msg = b"sample";
        let digest = sha256_bytes(msg);
        let r = hex_to_bytes("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716");
        let s = hex_to_bytes("f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8");
        assert!(ecdsa_p256_verify_rs(&pubkey, &digest, &r, &s));

        // Mutated digest fails
        let mut bad_digest = digest;
        bad_digest[0] ^= 1;
        assert!(!ecdsa_p256_verify_rs(&pubkey, &bad_digest, &r, &s));

        // r = 0 or s = 0 fails
        assert!(!ecdsa_p256_verify_rs(&pubkey, &digest, &[0u8; 32], &s));
        assert!(!ecdsa_p256_verify_rs(&pubkey, &digest, &r, &[0u8; 32]));

        // r >= n fails
        assert!(!ecdsa_p256_verify_rs(&pubkey, &digest, &N_BE, &s));
    }
}
