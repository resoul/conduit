//! Cryptographic primitives for Conduit's native TLS client.

pub mod aes;
pub mod bigint;
pub mod chacha20;
pub mod chacha_poly;
pub mod gcm;
pub mod ghash;
pub mod hmac_hkdf;
pub mod p256;
pub mod p384;
pub mod poly1305;
pub mod rsa;
pub mod sha2;
pub mod x25519;

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{Ordering, compiler_fence};

/// Trait for securely zeroing sensitive memory after use.
pub trait Zeroize {
    fn zeroize(&mut self);
}

impl Zeroize for u8 {
    #[inline]
    fn zeroize(&mut self) {
        // SAFETY: self is a valid, aligned mutable reference.
        unsafe { std::ptr::write_volatile(self, 0) };
        compiler_fence(Ordering::SeqCst);
    }
}

impl Zeroize for u32 {
    #[inline]
    fn zeroize(&mut self) {
        // SAFETY: self is a valid, aligned mutable reference.
        unsafe { std::ptr::write_volatile(self, 0) };
        compiler_fence(Ordering::SeqCst);
    }
}

impl Zeroize for u64 {
    #[inline]
    fn zeroize(&mut self) {
        // SAFETY: self is a valid, aligned mutable reference.
        unsafe { std::ptr::write_volatile(self, 0) };
        compiler_fence(Ordering::SeqCst);
    }
}

impl<T: Zeroize, const N: usize> Zeroize for [T; N] {
    #[inline]
    fn zeroize(&mut self) {
        for item in self.iter_mut() {
            item.zeroize();
        }
    }
}

impl<T: Zeroize> Zeroize for [T] {
    #[inline]
    fn zeroize(&mut self) {
        for item in self.iter_mut() {
            item.zeroize();
        }
    }
}

impl<T: Zeroize> Zeroize for Vec<T> {
    #[inline]
    fn zeroize(&mut self) {
        self.as_mut_slice().zeroize();
        self.clear();
    }
}

/// Zeroizes all allocated bytes of a `Vec<u8>` including spare capacity.
#[inline]
pub fn zeroize_vec_full(v: &mut Vec<u8>) {
    let cap = v.capacity();
    if cap > 0 {
        v.resize(cap, 0);
        v.as_mut_slice().zeroize();
    }
    v.clear();
}

/// Wrapper that automatically zeroizes its inner value on Drop.
#[derive(Clone)]
pub struct Zeroizing<T: Zeroize>(T);

impl<T: Zeroize> Zeroizing<T> {
    #[inline]
    pub fn new(val: T) -> Self {
        Self(val)
    }
}

impl<T: Zeroize> Drop for Zeroizing<T> {
    #[inline]
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> Deref for Zeroizing<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Zeroize> DerefMut for Zeroizing<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
