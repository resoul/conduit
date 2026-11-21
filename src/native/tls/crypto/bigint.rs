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

    /// Subtracts `other` from `self` in place assuming `self >= other`.
    fn sub_assign(&mut self, other: &Self) {
        let mut borrow = 0u128;
        for (i, limb) in self.limbs.iter_mut().enumerate() {
            let a = *limb as u128;
            let b = other.limbs.get(i).copied().unwrap_or(0) as u128;
            let sub = b + borrow;
            if a >= sub {
                *limb = (a - sub) as u64;
                borrow = 0;
            } else {
                *limb = ((1u128 << 64) + a - sub) as u64;
                borrow = 1;
            }
        }
        debug_assert_eq!(borrow, 0);
        self.trim();
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
        let mut rem = Self {
            limbs: Vec::with_capacity(modulus.limbs.len() + 1),
        };
        for i in (0..bits).rev() {
            rem.shl1();
            if self.bit(i) {
                if rem.limbs.is_empty() {
                    rem.limbs.push(1);
                } else {
                    rem.limbs[0] |= 1;
                }
            }
            if rem.cmp_val(modulus) != Ordering::Less {
                rem.sub_assign(modulus);
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
        if m.is_zero() || (m.limbs.len() == 1 && m.limbs[0] == 1) {
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
        const STACK_MONT_LIMBS: usize = 64; // Up to 4096-bit RSA without heap allocations
        let k = m.limbs.len();
        let base_mod = self.rem(m);
        if k <= STACK_MONT_LIMBS {
            let mut r1 = [0u64; STACK_MONT_LIMBS];
            let mut r2 = [0u64; STACK_MONT_LIMBS];
            let mut base_mont = [0u64; STACK_MONT_LIMBS];
            let mut acc = [0u64; STACK_MONT_LIMBS];
            let mut tmp = [0u64; STACK_MONT_LIMBS];
            let mut scratch = [0u64; STACK_MONT_LIMBS + 2];
            mod_pow_montgomery_bufs(
                &base_mod,
                exp,
                m,
                &mut r1[..k],
                &mut r2[..k],
                &mut base_mont[..k],
                &mut acc[..k],
                &mut tmp[..k],
                &mut scratch[..k + 2],
            )
        } else {
            let mut r1 = vec![0u64; k];
            let mut r2 = vec![0u64; k];
            let mut base_mont = vec![0u64; k];
            let mut acc = vec![0u64; k];
            let mut tmp = vec![0u64; k];
            let mut scratch = vec![0u64; k + 2];
            mod_pow_montgomery_bufs(
                &base_mod,
                exp,
                m,
                &mut r1,
                &mut r2,
                &mut base_mont,
                &mut acc,
                &mut tmp,
                &mut scratch,
            )
        }
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

#[allow(clippy::too_many_arguments)]
fn mod_pow_montgomery_bufs(
    base_mod: &BigUint,
    exp: &BigUint,
    m: &BigUint,
    r1: &mut [u64],
    r2: &mut [u64],
    base_mont: &mut [u64],
    acc: &mut [u64],
    tmp: &mut [u64],
    scratch: &mut [u64],
) -> BigUint {
    if base_mod.is_zero() {
        return if exp.is_zero() {
            BigUint::one()
        } else {
            BigUint::zero()
        };
    }

    let k = m.limbs.len();
    let n = &m.limbs;
    let n0_inv = mont_n0_inv(n[0]);

    // Compute R mod m (R = 2^(64*k)) via allocation-free doublings from 2^(m_bits - 1) < m.
    r1.fill(0);
    let m_bits = m.bit_len();
    r1[(m_bits - 1) / 64] = 1u64 << ((m_bits - 1) % 64);
    for _ in (m_bits - 1)..(64 * k) {
        double_mod(r1, n);
    }

    // Compute R^2 mod m: tmp = 2^64 * R mod m, then raise tmp to k in Montgomery domain.
    tmp.copy_from_slice(r1);
    for _ in 0..64 {
        double_mod(tmp, n);
    }
    r2.copy_from_slice(tmp);
    if k > 1 {
        let top = (usize::BITS - 1 - k.leading_zeros()) as usize;
        for bit in (0..top).rev() {
            mont_mul_with_scratch(r2, r2, n, n0_inv, scratch, acc);
            r2.copy_from_slice(acc);
            if (k >> bit) & 1 != 0 {
                mont_mul_with_scratch(r2, tmp, n, n0_inv, scratch, acc);
                r2.copy_from_slice(acc);
            }
        }
    }

    tmp.fill(0);
    tmp[..base_mod.limbs.len()].copy_from_slice(&base_mod.limbs);
    mont_mul_with_scratch(tmp, r2, n, n0_inv, scratch, base_mont);

    // Initialize acc = 1 * R mod m = R mod m.
    acc.copy_from_slice(r1);
    let mut acc_ref: &mut [u64] = acc;
    let mut tmp_ref: &mut [u64] = tmp;

    let bits = exp.bit_len();
    for i in (0..bits).rev() {
        mont_mul_with_scratch(acc_ref, acc_ref, n, n0_inv, scratch, tmp_ref);
        std::mem::swap(&mut acc_ref, &mut tmp_ref);
        if exp.bit(i) {
            mont_mul_with_scratch(acc_ref, base_mont, n, n0_inv, scratch, tmp_ref);
            std::mem::swap(&mut acc_ref, &mut tmp_ref);
        }
    }

    mont_reduce_with_scratch(acc_ref, n, n0_inv, scratch, tmp_ref);
    let mut res = BigUint {
        limbs: tmp_ref.to_vec(),
    };
    res.trim();
    res
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

/// Doubles `buf` modulo `n` in place assuming `buf < n`.
#[inline]
fn double_mod(buf: &mut [u64], n: &[u64]) {
    let mut carry = 0u64;
    for w in buf.iter_mut() {
        let next = *w >> 63;
        *w = (*w << 1) | carry;
        carry = next;
    }
    let mut ge = carry != 0;
    if !ge {
        ge = true;
        for j in (0..n.len()).rev() {
            if buf[j] != n[j] {
                ge = buf[j] > n[j];
                break;
            }
        }
    }
    if ge {
        let mut borrow = 0u128;
        for j in 0..n.len() {
            let sub = (n[j] as u128) + borrow;
            let bj = buf[j] as u128;
            if bj >= sub {
                buf[j] = (bj - sub) as u64;
                borrow = 0;
            } else {
                buf[j] = ((1u128 << 64) + bj - sub) as u64;
                borrow = 1;
            }
        }
    }
}

#[inline]
fn mont_cond_sub(t: &mut [u64], n: &[u64], out: &mut [u64]) {
    let k = n.len();
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
                out[j] = (tj - sub) as u64;
                borrow = 0;
            } else {
                out[j] = ((1u128 << 64) + tj - sub) as u64;
                borrow = 1;
            }
        }
    } else {
        out.copy_from_slice(&t[..k]);
    }
}

/// Montgomery multiplication `a * b * R^-1 mod n` (CIOS) writing the `k`-limb result into `out`.
fn mont_mul_with_scratch(
    a: &[u64],
    b: &[u64],
    n: &[u64],
    n0_inv: u64,
    t: &mut [u64],
    out: &mut [u64],
) {
    let k = n.len();
    t.fill(0);
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
    mont_cond_sub(t, n, out);
}

/// Montgomery reduction `a * R^-1 mod n` writing the `k`-limb result into `out`.
fn mont_reduce_with_scratch(
    a: &[u64],
    n: &[u64],
    n0_inv: u64,
    t: &mut [u64],
    out: &mut [u64],
) {
    let k = n.len();
    t[..k].copy_from_slice(a);
    t[k] = 0;
    t[k + 1] = 0;
    for _ in 0..k {
        let m = t[0].wrapping_mul(n0_inv);
        let mut carry = ((m as u128) * (n[0] as u128) + (t[0] as u128)) >> 64;
        for j in 1..k {
            let p = (m as u128) * (n[j] as u128) + (t[j] as u128) + carry;
            t[j - 1] = p as u64;
            carry = p >> 64;
        }
        let s = (t[k] as u128) + carry;
        t[k - 1] = s as u64;
        t[k] = (s >> 64) as u64;
    }
    mont_cond_sub(t, n, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_pow_naive(base: &BigUint, exp: &BigUint, m: &BigUint) -> BigUint {
        let mut b = base.rem(m);
        let mut res = BigUint::one().rem(m);
        let bits = exp.bit_len();
        for i in 0..bits {
            if exp.bit(i) {
                res = res.mod_mul(&b, m);
            }
            if i + 1 < bits {
                b = b.mod_mul(&b, m);
            }
        }
        res
    }

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

    #[test]
    fn bigint_montgomery_mod_pow_edge_and_large_moduli() {
        // Edge cases: m = 1, exp = 0, base = 0
        let one = BigUint::one();
        let zero = BigUint::zero();
        let seven = BigUint::from_u64(7);
        assert_eq!(seven.mod_pow(&BigUint::from_u64(5), &one), zero);
        assert_eq!(zero.mod_pow(&zero, &seven), one);
        assert_eq!(zero.mod_pow(&BigUint::from_u64(5), &seven), zero);
        assert_eq!(seven.mod_pow(&zero, &BigUint::from_u64(13)), one);

        // k = 6 (P-384 prime p): Fermat's Little Theorem and modular inverse
        let p384_p_bytes = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
        ];
        let p384_p = BigUint::from_be_bytes(&p384_p_bytes);
        let p384_p_minus_1 = p384_p.sub_checked(&one).unwrap();
        let val = BigUint::from_be_bytes(&[0xab; 47]);
        assert_eq!(val.mod_pow(&p384_p_minus_1, &p384_p), one);
        let inv = val.mod_inv_prime(&p384_p).unwrap();
        assert_eq!(val.mod_mul(&inv, &p384_p), one);

        // k = 32 (2048-bit odd modulus, stack Montgomery path) with e = 65537 vs naive mod_pow
        let mut m2048_bytes = vec![0x81u8; 256];
        m2048_bytes[255] = 0x5d;
        let m2048 = BigUint::from_be_bytes(&m2048_bytes);
        let e65537 = BigUint::from_u64(65537);
        let base2048 = BigUint::from_be_bytes(&[0x3c; 255]);
        assert_eq!(
            base2048.mod_pow(&e65537, &m2048),
            mod_pow_naive(&base2048, &e65537, &m2048)
        );

        // k = 65 (> STACK_MONT_LIMBS heap fallback path)
        let mut m_heap_bytes = vec![0x93u8; 520];
        m_heap_bytes[519] = 0x7b;
        let m_heap = BigUint::from_be_bytes(&m_heap_bytes);
        let e17 = BigUint::from_u64(17);
        let base_heap = BigUint::from_be_bytes(&[0x55; 519]);
        assert_eq!(
            base_heap.mod_pow(&e17, &m_heap),
            mod_pow_naive(&base_heap, &e17, &m_heap)
        );
    }
}
