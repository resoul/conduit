//! Arbitrary-precision unsigned integers for RSA (2048-4096 bit) and P-384 verification.

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigUint {
    /// Little-endian 64-bit limbs. Normalized: no trailing zero limbs.
    limbs: Vec<u64>,
}

impl BigUint {
    pub fn zero() -> Self {
        Self { limbs: Vec::new() }
    }

    pub fn one() -> Self {
        Self { limbs: vec![1] }
    }

    pub fn from_u64(v: u64) -> Self {
        if v == 0 {
            Self::zero()
        } else {
            Self { limbs: vec![v] }
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn is_even(&self) -> bool {
        self.limbs.first().is_none_or(|&w| w & 1 == 0)
    }

    pub fn bit_len(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(&top) => (self.limbs.len() - 1) * 64 + (64 - top.leading_zeros() as usize),
        }
    }

    pub fn bit(&self, idx: usize) -> bool {
        let limb = idx / 64;
        let bit = idx % 64;
        self.limbs.get(limb).is_some_and(|&w| ((w >> bit) & 1) != 0)
    }

    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        let first_nz = bytes.iter().position(|&b| b != 0);
        let Some(start) = first_nz else {
            return Self::zero();
        };
        let bytes = &bytes[start..];
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(8));
        let mut i = bytes.len();
        while i > 0 {
            let take = i.min(8);
            let mut buf = [0u8; 8];
            buf[8 - take..].copy_from_slice(&bytes[i - take..i]);
            limbs.push(u64::from_be_bytes(buf));
            i -= take;
        }
        let mut out = Self { limbs };
        out.trim();
        out
    }

    pub fn to_be_bytes_padded(&self, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for (i, &limb) in self.limbs.iter().enumerate() {
            let bytes = limb.to_be_bytes();
            let end = len.saturating_sub(i * 8);
            if end == 0 {
                break;
            }
            let start = end.saturating_sub(8);
            let take = end - start;
            out[start..end].copy_from_slice(&bytes[8 - take..]);
        }
        out
    }

    fn trim(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }

    pub fn cmp_val(&self, other: &Self) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for (&a, &b) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
            match a.cmp(&b) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    pub fn add(&self, other: &Self) -> Self {
        let max_len = self.limbs.len().max(other.limbs.len());
        let mut out = Vec::with_capacity(max_len + 1);
        let mut carry = 0u128;
        for i in 0..max_len {
            let a = self.limbs.get(i).copied().unwrap_or(0) as u128;
            let b = other.limbs.get(i).copied().unwrap_or(0) as u128;
            let sum = a + b + carry;
            out.push(sum as u64);
            carry = sum >> 64;
        }
        if carry != 0 {
            out.push(carry as u64);
        }
        let mut res = Self { limbs: out };
        res.trim();
        res
    }

    /// Returns `self - other` assuming `self >= other`.
    pub fn sub_checked(&self, other: &Self) -> Option<Self> {
        if self.cmp_val(other) == Ordering::Less {
            return None;
        }
        let mut out = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0u128;
        for (i, &limb) in self.limbs.iter().enumerate() {
            let a = limb as u128;
            let b = other.limbs.get(i).copied().unwrap_or(0) as u128;
            let sub = b + borrow;
            if a >= sub {
                out.push((a - sub) as u64);
                borrow = 0;
            } else {
                out.push(((1u128 << 64) + a - sub) as u64);
                borrow = 1;
            }
        }
        let mut res = Self { limbs: out };
        res.trim();
        Some(res)
    }

    pub fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut out = vec![0u64; self.limbs.len() + other.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u128;
            for (j, &b) in other.limbs.iter().enumerate() {
                let prod = (a as u128) * (b as u128) + (out[i + j] as u128) + carry;
                out[i + j] = prod as u64;
                carry = prod >> 64;
            }
            out[i + other.limbs.len()] = carry as u64;
        }
        let mut res = Self { limbs: out };
        res.trim();
        res
    }

    pub fn shl1(&mut self) {
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let next_carry = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next_carry;
        }
        if carry != 0 {
            self.limbs.push(carry);
        }
    }

    pub fn rem(&self, modulus: &Self) -> Self {
        if modulus.is_zero() || self.cmp_val(modulus) == Ordering::Less {
            return self.clone();
        }
        let bits = self.bit_len();
        let mut rem = Self::zero();
        for i in (0..bits).rev() {
            rem.shl1();
            if self.bit(i) {
                if rem.limbs.is_empty() {
                    rem.limbs.push(1);
                } else {
                    rem.limbs[0] |= 1;
                }
            }
            if rem.cmp_val(modulus) != Ordering::Less
                && let Some(diff) = rem.sub_checked(modulus)
            {
                rem = diff;
            }
        }
        rem
    }

    pub fn mod_add(&self, other: &Self, m: &Self) -> Self {
        let s = self.add(other);
        if s.cmp_val(m) != Ordering::Less {
            s.sub_checked(m).unwrap_or_else(|| s.rem(m))
        } else {
            s
        }
    }

    pub fn mod_sub(&self, other: &Self, m: &Self) -> Self {
        if self.cmp_val(other) != Ordering::Less {
            self.sub_checked(other).unwrap()
        } else {
            let diff = other.sub_checked(self).unwrap();
            m.sub_checked(&diff.rem(m)).unwrap()
        }
    }

    pub fn mod_mul(&self, other: &Self, m: &Self) -> Self {
        self.mul(other).rem(m)
    }

    /// Public-exponent modular exponentiation (uses Montgomery multiplication for odd moduli).
    pub fn mod_pow(&self, exp: &Self, m: &Self) -> Self {
        if m.is_zero() {
            return Self::zero();
        }
        if !m.is_even() {
            return self.mod_pow_montgomery(exp, m);
        }
        let mut base = self.rem(m);
        let mut res = Self::one().rem(m);
        let bits = exp.bit_len();
        for i in 0..bits {
            if exp.bit(i) {
                res = res.mod_mul(&base, m);
            }
            if i + 1 < bits {
                base = base.mod_mul(&base, m);
            }
        }
        res
    }

    fn mod_pow_montgomery(&self, exp: &Self, m: &Self) -> Self {
        let k = m.limbs.len();
        let n = &m.limbs;
        let n0_inv = mont_n0_inv(n[0]);

        // Compute R^2 mod m where R = 2^(64*k)
        let mut r2 = Self::zero();
        r2.limbs = vec![0u64; 2 * k + 1];
        r2.limbs[2 * k] = 1;
        let r2_mod = r2.rem(m);

        let base_mod = self.rem(m);
        let mut base_limbs = vec![0u64; k];
        base_limbs[..base_mod.limbs.len()].copy_from_slice(&base_mod.limbs);
        let mut r2_limbs = vec![0u64; k];
        r2_limbs[..r2_mod.limbs.len()].copy_from_slice(&r2_mod.limbs);

        let base_mont = mont_mul(&base_limbs, &r2_limbs, n, n0_inv);
        let mut one_limbs = vec![0u64; k];
        one_limbs[0] = 1;
        let mut acc_mont = mont_mul(&one_limbs, &r2_limbs, n, n0_inv);

        let bits = exp.bit_len();
        for i in (0..bits).rev() {
            acc_mont = mont_mul(&acc_mont, &acc_mont, n, n0_inv);
            if exp.bit(i) {
                acc_mont = mont_mul(&acc_mont, &base_mont, n, n0_inv);
            }
        }

        let res_limbs = mont_mul(&acc_mont, &one_limbs, n, n0_inv);
        let mut res = Self { limbs: res_limbs };
        res.trim();
        res
    }

    /// Modular inverse for prime modulus `p > 2` via Fermat's Little Theorem: `a^(p-2) mod p`.
    pub fn mod_inv_prime(&self, p: &Self) -> Option<Self> {
        if self.is_zero() || p.cmp_val(&Self::from_u64(2)) != Ordering::Greater {
            return None;
        }
        let p_minus_2 = p.sub_checked(&Self::from_u64(2))?;
        let inv = self.mod_pow(&p_minus_2, p);
        if inv.is_zero() { None } else { Some(inv) }
    }
}

/// Computes `-n0^-1 mod 2^64` for odd `n0` using Newton's method.
#[inline]
fn mont_n0_inv(n0: u64) -> u64 {
    let mut x = 1u64;
    for _ in 0..6 {
        x = x.wrapping_mul(2u64.wrapping_sub(n0.wrapping_mul(x)));
    }
    x.wrapping_neg()
}

/// Montgomery multiplication `a * b * R^-1 mod n` (SOS/CIOS), `a.len() == b.len() == n.len() == k`.
fn mont_mul(a: &[u64], b: &[u64], n: &[u64], n0_inv: u64) -> Vec<u64> {
    let k = n.len();
    let mut t = vec![0u64; k + 2];
    for &ai in a {
        let mut carry = 0u128;
        for j in 0..k {
            let p = (ai as u128) * (b[j] as u128) + (t[j] as u128) + carry;
            t[j] = p as u64;
            carry = p >> 64;
        }
        let s = (t[k] as u128) + carry;
        t[k] = s as u64;
        t[k + 1] = (s >> 64) as u64;

        let m = t[0].wrapping_mul(n0_inv);
        let mut carry2 = ((m as u128) * (n[0] as u128) + (t[0] as u128)) >> 64;
        for j in 1..k {
            let p = (m as u128) * (n[j] as u128) + (t[j] as u128) + carry2;
            t[j - 1] = p as u64;
            carry2 = p >> 64;
        }
        let s2 = (t[k] as u128) + carry2;
        t[k - 1] = s2 as u64;
        t[k] = t[k + 1] + ((s2 >> 64) as u64);
        t[k + 1] = 0;
    }

    // Final conditional subtraction if t >= n
    let mut ge = t[k] > 0;
    if !ge {
        ge = true;
        for j in (0..k).rev() {
            if t[j] != n[j] {
                ge = t[j] > n[j];
                break;
            }
        }
    }
    if ge {
        let mut borrow = 0u128;
        for j in 0..k {
            let sub = (n[j] as u128) + borrow;
            let tj = t[j] as u128;
            if tj >= sub {
                t[j] = (tj - sub) as u64;
                borrow = 0;
            } else {
                t[j] = ((1u128 << 64) + tj - sub) as u64;
                borrow = 1;
            }
        }
    }
    t.truncate(k);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigint_basic_arithmetic() {
        let a = BigUint::from_be_bytes(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11]);
        let b = BigUint::from_be_bytes(&[0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10]);
        let sum = a.add(&b);
        let diff = sum.sub_checked(&b).unwrap();
        assert_eq!(diff, a);

        let m = BigUint::from_u64(1_000_000_007);
        let g = BigUint::from_u64(5);
        let e = BigUint::from_u64(123_456_789);
        let p = g.mod_pow(&e, &m);
        // Verified against Python pow(5, 123456789, 1000000007) == 372224738
        assert_eq!(p, BigUint::from_u64(372_224_738));
    }
}
