//! NIST P-384 (secp384r1) ECDSA signature verification.

use super::bigint::BigUint;

const P384_B: [u8; 48] = [
    0xb3, 0x31, 0x2f, 0xa7, 0xe2, 0x3e, 0xe7, 0xe4, 0x98, 0x8e, 0x05, 0x6b, 0xe3, 0xf8, 0x2d, 0x19,
    0x18, 0x1d, 0x9c, 0x6e, 0xfe, 0x81, 0x41, 0x12, 0x03, 0x14, 0x08, 0x8f, 0x50, 0x13, 0x87, 0x5a,
    0xc6, 0x56, 0x39, 0x8d, 0x8a, 0x2e, 0xd1, 0x9d, 0x2a, 0x85, 0xc8, 0xed, 0xd3, 0xec, 0x2a, 0xef,
];

const P384_N: [u8; 48] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc7, 0x63, 0x4d, 0x81, 0xf4, 0x37, 0x2d, 0xdf,
    0x58, 0x1a, 0x0d, 0xb2, 0x48, 0xb0, 0xa7, 0x7a, 0xec, 0xec, 0x19, 0x6a, 0xcc, 0xc5, 0x29, 0x73,
];

const P384_GX: [u8; 48] = [
    0xaa, 0x87, 0xca, 0x22, 0xbe, 0x8b, 0x05, 0x37, 0x8e, 0xb1, 0xc7, 0x1e, 0xf3, 0x20, 0xad, 0x74,
    0x6e, 0x1d, 0x3b, 0x62, 0x8b, 0xa7, 0x9b, 0x98, 0x59, 0xf7, 0x41, 0xe0, 0x82, 0x54, 0x2a, 0x38,
    0x55, 0x02, 0xf2, 0x5d, 0xbf, 0x55, 0x29, 0x6c, 0x3a, 0x54, 0x5e, 0x38, 0x72, 0x76, 0x0a, 0xb7,
];

const P384_GY: [u8; 48] = [
    0x36, 0x17, 0xde, 0x4a, 0x96, 0x26, 0x2c, 0x6f, 0x5d, 0x9e, 0x98, 0xbf, 0x92, 0x92, 0xdc, 0x29,
    0xf8, 0xf4, 0x1d, 0xbd, 0x28, 0x9a, 0x14, 0x7c, 0xe9, 0xda, 0x31, 0x13, 0xb5, 0xf0, 0xb8, 0xc0,
    0x0a, 0x60, 0xb1, 0xce, 0x1d, 0x7e, 0x81, 0x9d, 0x7a, 0x43, 0x1d, 0x7c, 0x90, 0xea, 0x0e, 0x5f,
];

const P_LIMBS: [u64; 6] = [
    0x0000_0000_ffff_ffff,
    0xffff_ffff_0000_0000,
    0xffff_ffff_ffff_fffe,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
];

/// `2^384 mod p` in little-endian 64-bit limbs.
const R_MOD_P: [u64; 6] = [
    0xffff_ffff_0000_0001,
    0x0000_0000_ffff_ffff,
    0x0000_0000_0000_0001,
    0,
    0,
    0,
];

/// `2^768 mod p` in little-endian 64-bit limbs.
const R2_MOD_P: [u64; 6] = [
    0xffff_fffe_0000_0001,
    0x0000_0002_0000_0000,
    0xffff_fffe_0000_0000,
    0x0000_0002_0000_0000,
    0x0000_0000_0000_0001,
    0,
];

/// `-p[0]^-1 mod 2^64`.
const P0_INV: u64 = 0x0000_0001_0000_0001;

const P_MINUS_2: [u64; 6] = [
    0x0000_0000_ffff_fffd,
    0xffff_ffff_0000_0000,
    0xffff_ffff_ffff_fffe,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
];

#[inline]
fn add_raw(a: &[u64; 6], b: &[u64; 6]) -> ([u64; 6], u64) {
    let mut out = [0u64; 6];
    let mut carry = 0u128;
    for i in 0..6 {
        let s = (a[i] as u128) + (b[i] as u128) + carry;
        out[i] = s as u64;
        carry = s >> 64;
    }
    (out, carry as u64)
}

#[inline]
fn sub_raw(a: &[u64; 6], b: &[u64; 6]) -> ([u64; 6], u64) {
    let mut out = [0u64; 6];
    let mut borrow = 0u128;
    for i in 0..6 {
        let sub = (b[i] as u128) + borrow;
        let ai = a[i] as u128;
        if ai >= sub {
            out[i] = (ai - sub) as u64;
            borrow = 0;
        } else {
            out[i] = ((1u128 << 64) + ai - sub) as u64;
            borrow = 1;
        }
    }
    (out, borrow as u64)
}

#[inline]
fn mont_mul(a: &[u64; 6], b: &[u64; 6]) -> [u64; 6] {
    let mut t = [0u64; 8];
    for &ai in a {
        let mut carry = 0u128;
        for j in 0..6 {
            let p = (ai as u128) * (b[j] as u128) + (t[j] as u128) + carry;
            t[j] = p as u64;
            carry = p >> 64;
        }
        let s = (t[6] as u128) + carry;
        t[6] = s as u64;
        t[7] = (s >> 64) as u64;

        let m = t[0].wrapping_mul(P0_INV);
        let mut carry2 = ((m as u128) * (P_LIMBS[0] as u128) + (t[0] as u128)) >> 64;
        for j in 1..6 {
            let p = (m as u128) * (P_LIMBS[j] as u128) + (t[j] as u128) + carry2;
            t[j - 1] = p as u64;
            carry2 = p >> 64;
        }
        let s2 = (t[6] as u128) + carry2;
        t[5] = s2 as u64;
        t[6] = t[7] + ((s2 >> 64) as u64);
        t[7] = 0;
    }

    let raw = [t[0], t[1], t[2], t[3], t[4], t[5]];
    let (reduced, borrow) = sub_raw(&raw, &P_LIMBS);
    if t[6] > 0 || borrow == 0 {
        reduced
    } else {
        raw
    }
}

/// P-384 prime field element stored in Montgomery representation (`x * 2^384 mod p`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fe384([u64; 6]);

impl Fe384 {
    const ZERO: Self = Self([0; 6]);
    const ONE: Self = Self(R_MOD_P);

    fn from_be_bytes(b: &[u8; 48]) -> Option<Self> {
        let mut limbs = [0u64; 6];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let off = (5 - i) * 8;
            *limb = u64::from_be_bytes(b[off..off + 8].try_into().ok()?);
        }
        let (_, borrow) = sub_raw(&limbs, &P_LIMBS);
        if borrow == 0 {
            return None;
        }
        Some(Self(mont_mul(&limbs, &R2_MOD_P)))
    }

    fn to_be_bytes(self) -> [u8; 48] {
        let raw = mont_mul(&self.0, &[1, 0, 0, 0, 0, 0]);
        let mut out = [0u8; 48];
        for (i, &limb) in raw.iter().enumerate() {
            let off = (5 - i) * 8;
            out[off..off + 8].copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    #[inline]
    fn is_zero(self) -> bool {
        self.0 == [0; 6]
    }

    #[inline]
    fn add(self, rhs: Self) -> Self {
        let (sum, carry) = add_raw(&self.0, &rhs.0);
        let (reduced, borrow) = sub_raw(&sum, &P_LIMBS);
        if carry != 0 || borrow == 0 {
            Self(reduced)
        } else {
            Self(sum)
        }
    }

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        let (diff, borrow) = sub_raw(&self.0, &rhs.0);
        if borrow != 0 {
            let (added, _) = add_raw(&diff, &P_LIMBS);
            Self(added)
        } else {
            Self(diff)
        }
    }

    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self(mont_mul(&self.0, &rhs.0))
    }

    #[inline]
    fn sq(self) -> Self {
        self.mul(self)
    }

    /// Fermat modular inversion `self^(p - 2) mod p`.
    fn inv(self) -> Self {
        let mut acc = Self::ONE;
        for i in (0..384).rev() {
            acc = acc.sq();
            let bit = (P_MINUS_2[i / 64] >> (i % 64)) & 1;
            if bit != 0 {
                acc = acc.mul(self);
            }
        }
        acc
    }
}

#[derive(Clone, Copy)]
struct JacPoint384 {
    x: Fe384,
    y: Fe384,
    z: Fe384,
}

impl JacPoint384 {
    const INFINITY: Self = Self {
        x: Fe384::ONE,
        y: Fe384::ONE,
        z: Fe384::ZERO,
    };

    #[inline]
    fn is_infinity(&self) -> bool {
        self.z.is_zero()
    }

    fn double(&self) -> Self {
        if self.z.is_zero() || self.y.is_zero() {
            return Self::INFINITY;
        }
        // Jacobian point doubling for a = -3
        let delta = self.z.sq();
        let gamma = self.y.sq();
        let beta = self.x.mul(gamma);
        let p = self.x.sub(delta).mul(self.x.add(delta));
        let alpha = p.add(p).add(p);

        let b2 = beta.add(beta);
        let b4 = b2.add(b2);
        let b8 = b4.add(b4);
        let x3 = alpha.sq().sub(b8);
        let z3 = self.y.add(self.z).sq().sub(gamma).sub(delta);

        let g2 = gamma.sq();
        let two_g2 = g2.add(g2);
        let four_g2 = two_g2.add(two_g2);
        let eight_g2 = four_g2.add(four_g2);
        let y3 = alpha.mul(b4.sub(x3)).sub(eight_g2);

        Self {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    fn add(&self, other: &Self) -> Self {
        if self.is_infinity() {
            return *other;
        }
        if other.is_infinity() {
            return *self;
        }
        let z1z1 = self.z.sq();
        let z2z2 = other.z.sq();
        let u1 = self.x.mul(z2z2);
        let u2 = other.x.mul(z1z1);
        let s1 = self.y.mul(other.z).mul(z2z2);
        let s2 = other.y.mul(self.z).mul(z1z1);

        let h = u2.sub(u1);
        let r = s2.sub(s1);
        if h.is_zero() {
            if r.is_zero() {
                return self.double();
            }
            return Self::INFINITY;
        }
        let hh = h.sq();
        let hhh = hh.mul(h);
        let v = u1.mul(hh);
        let x3 = r.sq().sub(hhh).sub(v.add(v));
        let y3 = r.mul(v.sub(x3)).sub(s1.mul(hhh));
        let z3 = self.z.mul(other.z).mul(h);
        Self {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    fn add_affine(&self, ax: Fe384, ay: Fe384) -> Self {
        if self.is_infinity() {
            return Self {
                x: ax,
                y: ay,
                z: Fe384::ONE,
            };
        }
        let z1z1 = self.z.sq();
        let u2 = ax.mul(z1z1);
        let s2 = ay.mul(self.z).mul(z1z1);

        let h = u2.sub(self.x);
        let r = s2.sub(self.y);
        if h.is_zero() {
            if r.is_zero() {
                return self.double();
            }
            return Self::INFINITY;
        }
        let hh = h.sq();
        let hhh = hh.mul(h);
        let v = self.x.mul(hh);
        let x3 = r.sq().sub(hhh).sub(v.add(v));
        let y3 = r.mul(v.sub(x3)).sub(self.y.mul(hhh));
        let z3 = self.z.mul(h);
        Self {
            x: x3,
            y: y3,
            z: z3,
        }
    }
}

fn is_on_curve(x: Fe384, y: Fe384) -> bool {
    let Some(b) = Fe384::from_be_bytes(&P384_B) else {
        return false;
    };
    let lhs = y.sq();
    let x3 = x.sq().mul(x);
    let three_x = x.add(x).add(x);
    let rhs = x3.sub(three_x).add(b);
    lhs == rhs
}

fn scalar_mul_shamir(
    u1: &[u8; 48],
    gx: Fe384,
    gy: Fe384,
    u2: &[u8; 48],
    qx: Fe384,
    qy: Fe384,
) -> JacPoint384 {
    let g = JacPoint384 {
        x: gx,
        y: gy,
        z: Fe384::ONE,
    };
    let g_plus_q = g.add_affine(qx, qy);
    let mut acc = JacPoint384::INFINITY;
    for byte_idx in 0..48 {
        let b1 = u1[byte_idx];
        let b2 = u2[byte_idx];
        for bit in (0..8).rev() {
            if !acc.is_infinity() {
                acc = acc.double();
            }
            match (((b1 >> bit) & 1) != 0, ((b2 >> bit) & 1) != 0) {
                (true, true) => acc = acc.add(&g_plus_q),
                (true, false) => acc = acc.add_affine(gx, gy),
                (false, true) => acc = acc.add_affine(qx, qy),
                (false, false) => {}
            }
        }
    }
    acc
}

pub fn parse_der_ecdsa_sig(sig: &[u8]) -> Option<(&[u8], &[u8])> {
    if sig.len() < 8 || sig[0] != 0x30 {
        return None;
    }
    let (seq_len, mut pos) = if sig[1] & 0x80 == 0 {
        (sig[1] as usize, 2)
    } else if sig[1] == 0x81 && sig.len() > 2 && sig[2] >= 128 {
        (sig[2] as usize, 3)
    } else {
        return None;
    };
    if pos + seq_len != sig.len() {
        return None;
    }
    if pos + 2 > sig.len() || sig[pos] != 0x02 || (sig[pos + 1] & 0x80) != 0 {
        return None;
    }
    let r_len = sig[pos + 1] as usize;
    pos += 2;
    if r_len == 0 || pos + r_len > sig.len() {
        return None;
    }
    if (r_len > 1 && sig[pos] == 0 && (sig[pos + 1] & 0x80) == 0) || (sig[pos] & 0x80) != 0 {
        return None;
    }
    let r = &sig[pos..pos + r_len];
    pos += r_len;

    if pos + 2 > sig.len() || sig[pos] != 0x02 || (sig[pos + 1] & 0x80) != 0 {
        return None;
    }
    let s_len = sig[pos + 1] as usize;
    pos += 2;
    if s_len == 0 || pos + s_len != sig.len() {
        return None;
    }
    if (s_len > 1 && sig[pos] == 0 && (sig[pos + 1] & 0x80) == 0) || (sig[pos] & 0x80) != 0 {
        return None;
    }
    let s = &sig[pos..pos + s_len];
    Some((r, s))
}

/// Verify an ECDSA P-384 signature (DER-encoded `r, s`) over a digest.
pub fn ecdsa_p384_verify_der(pubkey_uncompressed: &[u8], digest: &[u8], der_sig: &[u8]) -> bool {
    let Some((r_bytes, s_bytes)) = parse_der_ecdsa_sig(der_sig) else {
        return false;
    };
    ecdsa_p384_verify_rs(pubkey_uncompressed, digest, r_bytes, s_bytes)
}

pub fn ecdsa_p384_verify_rs(
    pubkey_uncompressed: &[u8],
    digest: &[u8],
    r_bytes: &[u8],
    s_bytes: &[u8],
) -> bool {
    if pubkey_uncompressed.len() != 97 || pubkey_uncompressed[0] != 0x04 {
        return false;
    }
    let Ok(qx_raw): Result<[u8; 48], _> = pubkey_uncompressed[1..49].try_into() else {
        return false;
    };
    let Ok(qy_raw): Result<[u8; 48], _> = pubkey_uncompressed[49..97].try_into() else {
        return false;
    };
    let Some(qx) = Fe384::from_be_bytes(&qx_raw) else {
        return false;
    };
    let Some(qy) = Fe384::from_be_bytes(&qy_raw) else {
        return false;
    };
    if !is_on_curve(qx, qy) {
        return false;
    }

    let n = BigUint::from_be_bytes(&P384_N);
    let r = BigUint::from_be_bytes(r_bytes);
    let s = BigUint::from_be_bytes(s_bytes);
    if r.is_zero()
        || s.is_zero()
        || r.cmp_val(&n) != std::cmp::Ordering::Less
        || s.cmp_val(&n) != std::cmp::Ordering::Less
    {
        return false;
    }

    let z_slice = if digest.len() > 48 {
        &digest[..48]
    } else {
        digest
    };
    let z = BigUint::from_be_bytes(z_slice).rem(&n);

    let Some(s_inv) = s.mod_inv_prime(&n) else {
        return false;
    };
    let u1_bytes: [u8; 48] = z
        .mod_mul(&s_inv, &n)
        .to_be_bytes_padded(48)
        .try_into()
        .unwrap();
    let u2_bytes: [u8; 48] = r
        .mod_mul(&s_inv, &n)
        .to_be_bytes_padded(48)
        .try_into()
        .unwrap();

    let gx = Fe384::from_be_bytes(&P384_GX).unwrap();
    let gy = Fe384::from_be_bytes(&P384_GY).unwrap();

    let res = scalar_mul_shamir(&u1_bytes, gx, gy, &u2_bytes, qx, qy);
    if res.is_infinity() {
        return false;
    }
    let z_inv = res.z.inv();
    let rx_bytes = res.x.mul(z_inv.sq()).to_be_bytes();
    BigUint::from_be_bytes(&rx_bytes).rem(&n) == r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn push_der_uint(out: &mut Vec<u8>, be: &[u8]) {
        let mut s = be;
        while s.len() > 1 && s[0] == 0 {
            s = &s[1..];
        }
        out.push(0x02);
        if s[0] & 0x80 != 0 {
            out.push((s.len() + 1) as u8);
            out.push(0x00);
        } else {
            out.push(s.len() as u8);
        }
        out.extend_from_slice(s);
    }

    #[test]
    fn p384_cavp_verify_vector() {
        // Valid P-384 SHA-384 ECDSA vector over b"sample"
        let qx = hex_to_bytes("ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64def8f0ea9055866064a254515480bc13");
        let qy = hex_to_bytes("8015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1288b231c3ae0d4fe7344fd2533264720");
        let r = hex_to_bytes("a419306e90216224bc2a397d9072243fc2fbfc6b9d373438f2d6aa2eca1fd07f5ab9db472df1721edd15401fb348d946");
        let s = hex_to_bytes("95924bb328035cad8082f436ddd195dcb276692d0c9de05455b69470a641fad3e61e1fc8fb9ee04c728931b5a8616e01");
        let digest = crate::native::tls::crypto::sha2::sha384_bytes(b"sample");
        let mut pubkey = vec![0x04];
        pubkey.extend_from_slice(&qx);
        pubkey.extend_from_slice(&qy);

        assert!(ecdsa_p384_verify_rs(&pubkey, &digest, &r, &s));

        let mut body = Vec::new();
        push_der_uint(&mut body, &r);
        push_der_uint(&mut body, &s);
        let mut der_sig = vec![0x30, body.len() as u8];
        der_sig.extend_from_slice(&body);
        assert!(ecdsa_p384_verify_der(&pubkey, &digest, &der_sig));

        // Mutated digest, r, s, zero r/s, r >= n, and bad pubkey all rejected
        let mut bad_digest = digest;
        bad_digest[0] ^= 1;
        assert!(!ecdsa_p384_verify_rs(&pubkey, &bad_digest, &r, &s));

        let mut bad_r = r.clone();
        bad_r[47] ^= 1;
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &bad_r, &s));

        let mut bad_s = s.clone();
        bad_s[47] ^= 1;
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &r, &bad_s));

        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &[0u8; 48], &s));
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &r, &[0u8; 48]));
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &P384_N, &s));
        assert!(!ecdsa_p384_verify_rs(&[0x04; 97], &digest, &r, &s));
    }

    #[test]
    fn p384_generator_self_check() {
        let gx = Fe384::from_be_bytes(&P384_GX).unwrap();
        let gy = Fe384::from_be_bytes(&P384_GY).unwrap();
        assert!(is_on_curve(gx, gy));
        assert_eq!(gx.mul(gx.inv()), Fe384::ONE);
        assert_eq!(gy.mul(gy.inv()), Fe384::ONE);

        // Curve order check: n * G == O
        let order_g = scalar_mul_shamir(&P384_N, gx, gy, &[0u8; 48], gx, gy);
        assert!(order_g.is_infinity());
    }

    #[test]
    fn wycheproof_p384_ecdsa_der_and_boundary_rejections() {
        let qx = hex_to_bytes("ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64def8f0ea9055866064a254515480bc13");
        let qy = hex_to_bytes("8015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1288b231c3ae0d4fe7344fd2533264720");
        let r = hex_to_bytes("a419306e90216224bc2a397d9072243fc2fbfc6b9d373438f2d6aa2eca1fd07f5ab9db472df1721edd15401fb348d946");
        let s = hex_to_bytes("95924bb328035cad8082f436ddd195dcb276692d0c9de05455b69470a641fad3e61e1fc8fb9ee04c728931b5a8616e01");
        let digest = crate::native::tls::crypto::sha2::sha384_bytes(b"sample");
        let mut pubkey = vec![0x04];
        pubkey.extend_from_slice(&qx);
        pubkey.extend_from_slice(&qy);

        let mut body = Vec::new();
        push_der_uint(&mut body, &r);
        push_der_uint(&mut body, &s);
        let mut der = vec![0x30, body.len() as u8];
        der.extend_from_slice(&body);
        assert!(ecdsa_p384_verify_der(&pubkey, &digest, &der));

        // Non-minimal SEQUENCE length 0x30 0x81 <len < 128>.
        let mut nonmin_seq = vec![0x30, 0x81, body.len() as u8];
        nonmin_seq.extend_from_slice(&body);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &nonmin_seq));

        // Indefinite SEQUENCE length 0x30 0x80.
        let mut indef = vec![0x30, 0x80];
        indef.extend_from_slice(&body);
        indef.extend_from_slice(&[0x00, 0x00]);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &indef));

        // Long-form INTEGER length 0x02 0x81 0x31 ...
        let mut lf_body = vec![0x02, 0x81, 0x31, 0x00];
        lf_body.extend_from_slice(&r);
        push_der_uint(&mut lf_body, &s);
        let mut lf_der = vec![0x30, lf_body.len() as u8];
        lf_der.extend_from_slice(&lf_body);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &lf_der));

        // Negative integer (missing leading 0x00 when MSB=1) and non-minimal leading 0x00.
        let mut neg_body = vec![0x02, r.len() as u8];
        neg_body.extend_from_slice(&r);
        push_der_uint(&mut neg_body, &s);
        let mut neg_der = vec![0x30, neg_body.len() as u8];
        neg_der.extend_from_slice(&neg_body);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &neg_der));

        let mut pad_body = vec![0x02, 0x02, 0x00, 0x01];
        push_der_uint(&mut pad_body, &s);
        let mut pad_der = vec![0x30, pad_body.len() as u8];
        pad_der.extend_from_slice(&pad_body);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &pad_der));

        // Trailing garbage, truncated prefixes, and wrong tags.
        let mut extra_out = der.clone();
        extra_out.push(0x00);
        assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &extra_out));
        for len in 0..der.len() {
            assert!(!ecdsa_p384_verify_der(&pubkey, &digest, &der[..len]));
        }

        // Scalar boundary checks (s == n, s == n + 1) and invalid pubkey coordinates/prefix/length.
        let mut n_plus_1 = P384_N;
        n_plus_1[47] += 1;
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &r, &P384_N));
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &r, &n_plus_1));

        let p384_p = hex_to_bytes("fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff");
        let mut bad_x = pubkey.clone();
        bad_x[1..49].copy_from_slice(&p384_p);
        assert!(!ecdsa_p384_verify_rs(&bad_x, &digest, &r, &s));

        let mut bad_prefix = pubkey.clone();
        bad_prefix[0] = 0x02;
        assert!(!ecdsa_p384_verify_rs(&bad_prefix, &digest, &r, &s));
        assert!(!ecdsa_p384_verify_rs(&pubkey[..96], &digest, &r, &s));
    }
}
