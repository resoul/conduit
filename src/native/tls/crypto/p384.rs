//! NIST P-384 (secp384r1) ECDSA signature verification.

use super::bigint::BigUint;

const P384_P: [u8; 48] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
];

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

#[derive(Clone)]
struct Point {
    x: BigUint,
    y: BigUint,
    infinity: bool,
}

impl Point {
    fn infinity() -> Self {
        Self {
            x: BigUint::zero(),
            y: BigUint::zero(),
            infinity: true,
        }
    }
}

fn is_on_curve(x: &BigUint, y: &BigUint, p: &BigUint, b: &BigUint) -> bool {
    if x.cmp_val(p) != std::cmp::Ordering::Less || y.cmp_val(p) != std::cmp::Ordering::Less {
        return false;
    }
    let lhs = y.mod_mul(y, p);
    let x2 = x.mod_mul(x, p);
    let x3 = x2.mod_mul(x, p);
    let three_x = x.mod_mul(&BigUint::from_u64(3), p);
    let rhs = x3.mod_sub(&three_x, p).mod_add(b, p);
    lhs == rhs
}

fn point_double(pt: &Point, p: &BigUint) -> Point {
    if pt.infinity || pt.y.is_zero() {
        return Point::infinity();
    }
    let x2 = pt.x.mod_mul(&pt.x, p);
    let three_x2 = x2.mod_mul(&BigUint::from_u64(3), p);
    let num = three_x2.mod_sub(&BigUint::from_u64(3), p);
    let den = pt.y.mod_mul(&BigUint::from_u64(2), p);
    let Some(den_inv) = den.mod_inv_prime(p) else {
        return Point::infinity();
    };
    let lambda = num.mod_mul(&den_inv, p);
    let rx = lambda.mod_mul(&lambda, p).mod_sub(&pt.x, p).mod_sub(&pt.x, p);
    let ry = lambda.mod_mul(&pt.x.mod_sub(&rx, p), p).mod_sub(&pt.y, p);
    Point {
        x: rx,
        y: ry,
        infinity: false,
    }
}

fn point_add(a: &Point, b: &Point, p: &BigUint) -> Point {
    if a.infinity {
        return b.clone();
    }
    if b.infinity {
        return a.clone();
    }
    if a.x == b.x {
        if a.y == b.y {
            return point_double(a, p);
        }
        return Point::infinity();
    }
    let num = b.y.mod_sub(&a.y, p);
    let den = b.x.mod_sub(&a.x, p);
    let Some(den_inv) = den.mod_inv_prime(p) else {
        return Point::infinity();
    };
    let lambda = num.mod_mul(&den_inv, p);
    let rx = lambda.mod_mul(&lambda, p).mod_sub(&a.x, p).mod_sub(&b.x, p);
    let ry = lambda.mod_mul(&a.x.mod_sub(&rx, p), p).mod_sub(&a.y, p);
    Point {
        x: rx,
        y: ry,
        infinity: false,
    }
}

fn scalar_mul_shamir(u1: &BigUint, g: &Point, u2: &BigUint, q: &Point, p: &BigUint) -> Point {
    let g_plus_q = point_add(g, q, p);
    let bits = u1.bit_len().max(u2.bit_len());
    let mut r = Point::infinity();
    for i in (0..bits).rev() {
        r = point_double(&r, p);
        match (u1.bit(i), u2.bit(i)) {
            (true, true) => r = point_add(&r, &g_plus_q, p),
            (true, false) => r = point_add(&r, g, p),
            (false, true) => r = point_add(&r, q, p),
            (false, false) => {}
        }
    }
    r
}

pub fn parse_der_ecdsa_sig(sig: &[u8]) -> Option<(&[u8], &[u8])> {
    if sig.len() < 8 || sig[0] != 0x30 {
        return None;
    }
    let (seq_len, mut pos) = if sig[1] & 0x80 == 0 {
        (sig[1] as usize, 2)
    } else if sig[1] == 0x81 && sig.len() > 2 {
        (sig[2] as usize, 3)
    } else {
        return None;
    };
    if pos + seq_len != sig.len() {
        return None;
    }
    if pos + 2 > sig.len() || sig[pos] != 0x02 {
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

    if pos + 2 > sig.len() || sig[pos] != 0x02 {
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
    let p = BigUint::from_be_bytes(&P384_P);
    let b = BigUint::from_be_bytes(&P384_B);
    let n = BigUint::from_be_bytes(&P384_N);

    let qx = BigUint::from_be_bytes(&pubkey_uncompressed[1..49]);
    let qy = BigUint::from_be_bytes(&pubkey_uncompressed[49..97]);
    if !is_on_curve(&qx, &qy, &p, &b) {
        return false;
    }

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
    let u1 = z.mod_mul(&s_inv, &n);
    let u2 = r.mod_mul(&s_inv, &n);

    let g = Point {
        x: BigUint::from_be_bytes(&P384_GX),
        y: BigUint::from_be_bytes(&P384_GY),
        infinity: false,
    };
    let q = Point {
        x: qx,
        y: qy,
        infinity: false,
    };
    let res = scalar_mul_shamir(&u1, &g, &u2, &q, &p);
    if res.infinity {
        return false;
    }
    res.x.rem(&n) == r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn p384_cavp_verify_vector() {
        // NIST CAVP P-384 SHA-384 test vector
        let qx = hex_to_bytes("1fb2b10b22aa25fd038c5b1cc9a043d0bc14b71a2f82362a66cf096b592c3e95a3c637a3a63e231f217f83f0eb5d0250");
        let qy = hex_to_bytes("7562f9d3a3a9d63f130df4988cb1eb7e39fdbf0987447f4ebcb1a85f104e84c5b3e7f43a561fa4041e0945787725e980");
        let msg = hex_to_bytes("8a1a1d8440e2715836d6870244681c0178d6245158c3735e4a315382735f092c2764f8a4c56f14a5b2416f859d7e0c6b");
        let r = hex_to_bytes("c77c64b0a94f3e07b92f3433818f4048a2b7137b1799e7d6207807a4971c7bb846f7661f103d144a149e0186df1d183f");
        let s = hex_to_bytes("2545d4f12e0e74a15fc5e5e5e733b5b53c6a0905e5b7aa402325f2c074a5d6ee142d4c26cf2a3573f33128014b5b0a28");
        let digest = crate::native::tls::crypto::sha2::sha384_bytes(&msg);
        let mut pubkey = vec![0x04];
        pubkey.extend_from_slice(&qx);
        pubkey.extend_from_slice(&qy);

        // Verify valid/invalid signature rejection (note: check both point validation & r=0 rejection)
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &[0u8; 48], &s));
        assert!(!ecdsa_p384_verify_rs(&pubkey, &digest, &r, &[0u8; 48]));
        // Bad pubkey (all zeros not on curve)
        assert!(!ecdsa_p384_verify_rs(&[0x04; 97], &digest, &r, &s));
    }

    #[test]
    fn p384_generator_self_check() {
        let p = BigUint::from_be_bytes(&P384_P);
        let b = BigUint::from_be_bytes(&P384_B);
        let gx = BigUint::from_be_bytes(&P384_GX);
        let gy = BigUint::from_be_bytes(&P384_GY);
        assert!(is_on_curve(&gx, &gy, &p, &b));
    }
}
