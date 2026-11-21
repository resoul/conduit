//! TLS record layer: framing, AEAD encryption/decryption, sequence numbers, and non-blocking buffering.

use std::io::{self, Read, Write};

use crate::native::tls::crypto::{Zeroize, zeroize_vec_full};
use crate::native::tls::crypto::chacha_poly::ChaCha20Poly1305;
use crate::native::tls::crypto::gcm::AesGcm;
use crate::native::tls::error::{AlertDescription, TlsError};

pub const CT_CHANGE_CIPHER_SPEC: u8 = 20;
pub const CT_ALERT: u8 = 21;
pub const CT_HANDSHAKE: u8 = 22;
pub const CT_APPLICATION_DATA: u8 = 23;

pub const MAX_PLAINTEXT: usize = 16_384;
pub const MAX_CIPHERTEXT_TLS13: usize = MAX_PLAINTEXT + 256;
pub const MAX_CIPHERTEXT_TLS12: usize = MAX_PLAINTEXT + 2048;
/// Fixed pre-allocated receive buffer larger than any single TLS 1.2/1.3 record (18,437 B).
const RX_BUF_CAP: usize = 24 * 1024;
/// Rekey before reaching AES-GCM 2^24.5 safety limit.
pub const TLS13_REKEY_SEQ_LIMIT: u64 = 1 << 23;

#[derive(Clone)]
pub enum AeadCipher {
    AesGcm(Box<AesGcm>),
    ChaCha20Poly1305(ChaCha20Poly1305),
}

impl AeadCipher {
    pub fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
        match self {
            Self::AesGcm(c) => c.seal_in_place(nonce, aad, buf),
            Self::ChaCha20Poly1305(c) => c.seal_in_place(nonce, aad, buf),
        }
    }

    pub fn open_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8], tag: &[u8; 16]) -> bool {
        match self {
            Self::AesGcm(c) => c.open_in_place(nonce, aad, buf, tag),
            Self::ChaCha20Poly1305(c) => c.open_in_place(nonce, aad, buf, tag),
        }
    }
}

#[derive(Clone)]
pub enum CipherState {
    Plain,
    Tls13 {
        cipher: AeadCipher,
        iv: [u8; 12],
        seq: u64,
    },
    Tls12 {
        cipher: AeadCipher,
        implicit_iv: [u8; 4],
        seq: u64,
        is_chacha: bool,
        chacha_iv: [u8; 12],
    },
}

impl Drop for CipherState {
    fn drop(&mut self) {
        match self {
            Self::Plain => {}
            Self::Tls13 { iv, .. } => iv.zeroize(),
            Self::Tls12 { implicit_iv, chacha_iv, .. } => {
                implicit_iv.zeroize();
                chacha_iv.zeroize();
            }
        }
    }
}

impl CipherState {
    pub fn seq(&self) -> u64 {
        match self {
            Self::Plain => 0,
            Self::Tls13 { seq, .. } | Self::Tls12 { seq, .. } => *seq,
        }
    }

    pub fn needs_tls13_rekey(&self) -> bool {
        matches!(self, Self::Tls13 { seq, .. } if *seq >= TLS13_REKEY_SEQ_LIMIT)
    }
}

pub struct RecordLayer {
    pub read_state: CipherState,
    pub write_state: CipherState,
    /// Number of unencrypted ChangeCipherSpec records still allowed during TLS 1.3 handshake.
    tls13_ccs_remaining: u8,
    /// Fixed-length incoming socket buffer for zero-fill-free reads and in-place decryption.
    rx_buf: Vec<u8>,
    rx_pos: usize,
    rx_end: usize,
    /// Reusable outgoing record buffer.
    tx_buf: Vec<u8>,
}

impl Drop for RecordLayer {
    fn drop(&mut self) {
        zeroize_vec_full(&mut self.rx_buf);
        zeroize_vec_full(&mut self.tx_buf);
    }
}

impl Default for RecordLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordLayer {
    pub fn new() -> Self {
        Self {
            read_state: CipherState::Plain,
            write_state: CipherState::Plain,
            tls13_ccs_remaining: 0,
            rx_buf: vec![0u8; RX_BUF_CAP],
            rx_pos: 0,
            rx_end: 0,
            tx_buf: Vec::with_capacity(MAX_CIPHERTEXT_TLS13 + 5),
        }
    }

    pub fn set_tls13_ccs_budget(&mut self, count: u8) {
        self.tls13_ccs_remaining = count;
    }

    pub fn consume_plain_ccs(&mut self, payload: &[u8]) -> bool {
        if self.tls13_ccs_remaining > 0 && payload == [1] {
            self.tls13_ccs_remaining -= 1;
            true
        } else {
            false
        }
    }

    fn compact_if_needed(&mut self, needed_record_total: Option<usize>) {
        if self.rx_pos == self.rx_end {
            self.rx_pos = 0;
            self.rx_end = 0;
            return;
        }
        if self.rx_pos > 0
            && (self.rx_buf.len() - self.rx_end < 4096
                || needed_record_total.is_some_and(|total| self.rx_pos + total > self.rx_buf.len()))
        {
            self.rx_buf.copy_within(self.rx_pos..self.rx_end, 0);
            self.rx_end -= self.rx_pos;
            self.rx_pos = 0;
        }
    }

    /// Blocking read of one complete TLS record. Returns `(content_type, plaintext_slice)`.
    pub fn read_record(&mut self, stream: &mut impl Read) -> Result<(u8, &[u8]), TlsError> {
        loop {
            let avail = self.rx_end - self.rx_pos;
            let mut needed_total = None;
            if avail >= 5 {
                let hdr = &self.rx_buf[self.rx_pos..self.rx_pos + 5];
                let ct = hdr[0];
                let rec_len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
                let max_len = match self.read_state {
                    CipherState::Plain | CipherState::Tls13 { .. } => MAX_CIPHERTEXT_TLS13,
                    CipherState::Tls12 { .. } => MAX_CIPHERTEXT_TLS12,
                };
                if rec_len > max_len {
                    return Err(TlsError::RemoteAlert {
                        level: 2,
                        description: AlertDescription::RecordOverflow,
                    });
                }
                let total = 5 + rec_len;
                if avail >= total {
                    return self.decrypt_buffered_record(ct, rec_len);
                }
                needed_total = Some(total);
            }
            self.compact_if_needed(needed_total);
            match stream.read(&mut self.rx_buf[self.rx_end..]) {
                Ok(0) => return Err(TlsError::UnexpectedEof),
                Ok(n) => self.rx_end += n,
                Err(e) => return Err(TlsError::from(e)),
            }
        }
    }

    /// Non-blocking attempt to read one complete TLS record.
    /// Returns `Ok(None)` if socket would block before a full record is available.
    pub fn try_read_record_nonblocking(
        &mut self,
        stream: &mut impl Read,
    ) -> Result<Option<(u8, &[u8])>, TlsError> {
        loop {
            let avail = self.rx_end - self.rx_pos;
            let mut needed_total = None;
            if avail >= 5 {
                let hdr = &self.rx_buf[self.rx_pos..self.rx_pos + 5];
                let ct = hdr[0];
                let rec_len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
                let max_len = match self.read_state {
                    CipherState::Plain | CipherState::Tls13 { .. } => MAX_CIPHERTEXT_TLS13,
                    CipherState::Tls12 { .. } => MAX_CIPHERTEXT_TLS12,
                };
                if rec_len > max_len {
                    return Err(TlsError::ProtocolError("record overflow".to_string()));
                }
                let total = 5 + rec_len;
                if avail >= total {
                    return self.decrypt_buffered_record(ct, rec_len).map(Some);
                }
                needed_total = Some(total);
            }
            self.compact_if_needed(needed_total);
            match stream.read(&mut self.rx_buf[self.rx_end..]) {
                Ok(0) => return Err(TlsError::UnexpectedEof),
                Ok(n) => self.rx_end += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(TlsError::from(e)),
            }
        }
    }

    fn decrypt_buffered_record(&mut self, ct: u8, rec_len: usize) -> Result<(u8, &[u8]), TlsError> {
        let start = self.rx_pos;
        let hdr: [u8; 5] = self.rx_buf[start..start + 5].try_into().unwrap();
        let payload_start = start + 5;
        let payload_end = payload_start + rec_len;
        self.rx_pos = payload_end;

        match &mut self.read_state {
            CipherState::Plain => Ok((ct, &self.rx_buf[payload_start..payload_end])),
            CipherState::Tls13 { cipher, iv, seq } => {
                // Allow at most one unencrypted ChangeCipherSpec during TLS 1.3 handshake (RFC 8446 §5)
                if ct == CT_CHANGE_CIPHER_SPEC
                    && self.tls13_ccs_remaining > 0
                    && rec_len == 1
                    && self.rx_buf[payload_start] == 1
                {
                    self.tls13_ccs_remaining -= 1;
                    return Ok((CT_CHANGE_CIPHER_SPEC, &self.rx_buf[payload_start..payload_end]));
                }
                if ct != CT_APPLICATION_DATA {
                    return Err(TlsError::ProtocolError(format!(
                        "unexpected unencrypted record type {ct} in TLS 1.3 encrypted state"
                    )));
                }
                if rec_len < 17 {
                    return Err(TlsError::DecodeError("TLS 1.3 ciphertext too short"));
                }
                let next_seq = seq
                    .checked_add(1)
                    .ok_or(TlsError::CryptoError("TLS 1.3 read sequence overflow"))?;
                let nonce = make_tls13_nonce(iv, *seq);
                *seq = next_seq;

                let ct_len = rec_len - 16;
                let tag: [u8; 16] = self.rx_buf[payload_start + ct_len..payload_end]
                    .try_into()
                    .unwrap();
                let pt_slice = &mut self.rx_buf[payload_start..payload_start + ct_len];

                if !cipher.open_in_place(&nonce, &hdr, pt_slice, &tag) {
                    pt_slice.zeroize();
                    return Err(TlsError::RemoteAlert {
                        level: 2,
                        description: AlertDescription::BadRecordMac,
                    });
                }

                // Strip trailing zero padding and read inner content type
                let mut end = payload_start + ct_len;
                while end > payload_start && self.rx_buf[end - 1] == 0 {
                    end -= 1;
                }
                if end == payload_start {
                    return Err(TlsError::ProtocolError("empty TLSInnerPlaintext".to_string()));
                }
                end -= 1;
                let inner_ct = self.rx_buf[end];
                if end - payload_start > MAX_PLAINTEXT {
                    return Err(TlsError::ProtocolError("decrypted TLS 1.3 plaintext > 16 KiB".to_string()));
                }
                Ok((inner_ct, &self.rx_buf[payload_start..end]))
            }
            CipherState::Tls12 {
                cipher,
                implicit_iv,
                seq,
                is_chacha,
                chacha_iv,
            } => {
                let next_seq = seq
                    .checked_add(1)
                    .ok_or(TlsError::CryptoError("TLS 1.2 read sequence overflow"))?;
                let cur_seq = *seq;
                *seq = next_seq;

                if *is_chacha {
                    if rec_len < 16 {
                        return Err(TlsError::DecodeError("TLS 1.2 ChaCha ciphertext too short"));
                    }
                    let pt_len = rec_len - 16;
                    if pt_len > MAX_PLAINTEXT {
                        return Err(TlsError::ProtocolError("TLS 1.2 plaintext > 16 KiB".to_string()));
                    }
                    let nonce = make_tls13_nonce(chacha_iv, cur_seq);
                    let aad = make_tls12_aad(cur_seq, ct, pt_len);
                    let tag: [u8; 16] = self.rx_buf[payload_start + pt_len..payload_end]
                        .try_into()
                        .unwrap();
                    let pt_slice = &mut self.rx_buf[payload_start..payload_start + pt_len];
                    if !cipher.open_in_place(&nonce, &aad, pt_slice, &tag) {
                        pt_slice.zeroize();
                        return Err(TlsError::RemoteAlert {
                            level: 2,
                            description: AlertDescription::BadRecordMac,
                        });
                    }
                    Ok((ct, &self.rx_buf[payload_start..payload_start + pt_len]))
                } else {
                    if rec_len < 24 {
                        return Err(TlsError::DecodeError("TLS 1.2 AES-GCM ciphertext too short"));
                    }
                    let pt_len = rec_len - 24;
                    if pt_len > MAX_PLAINTEXT {
                        return Err(TlsError::ProtocolError("TLS 1.2 plaintext > 16 KiB".to_string()));
                    }
                    let mut nonce = [0u8; 12];
                    nonce[..4].copy_from_slice(implicit_iv);
                    nonce[4..12].copy_from_slice(&self.rx_buf[payload_start..payload_start + 8]);
                    let aad = make_tls12_aad(cur_seq, ct, pt_len);
                    let pt_start = payload_start + 8;
                    let tag: [u8; 16] = self.rx_buf[pt_start + pt_len..payload_end]
                        .try_into()
                        .unwrap();
                    let pt_slice = &mut self.rx_buf[pt_start..pt_start + pt_len];
                    if !cipher.open_in_place(&nonce, &aad, pt_slice, &tag) {
                        pt_slice.zeroize();
                        return Err(TlsError::RemoteAlert {
                            level: 2,
                            description: AlertDescription::BadRecordMac,
                        });
                    }
                    Ok((ct, &self.rx_buf[pt_start..pt_start + pt_len]))
                }
            }
        }
    }

    pub fn write_record(
        &mut self,
        stream: &mut impl Write,
        ct: u8,
        payload: &[u8],
    ) -> Result<(), TlsError> {
        if payload.is_empty() {
            return self.write_single_chunk(stream, ct, &[], 0x0303);
        }
        for chunk in payload.chunks(MAX_PLAINTEXT) {
            self.write_single_chunk(stream, ct, chunk, 0x0303)?;
        }
        Ok(())
    }

    pub fn write_plain_with_legacy_ver(
        &mut self,
        stream: &mut impl Write,
        ct: u8,
        legacy_ver: u16,
        payload: &[u8],
    ) -> Result<(), TlsError> {
        for chunk in payload.chunks(MAX_PLAINTEXT) {
            self.write_single_chunk(stream, ct, chunk, legacy_ver)?;
        }
        Ok(())
    }

    fn write_single_chunk(
        &mut self,
        stream: &mut impl Write,
        ct: u8,
        chunk: &[u8],
        legacy_ver: u16,
    ) -> Result<(), TlsError> {
        debug_assert!(chunk.len() <= MAX_PLAINTEXT);
        self.tx_buf.clear();

        match &mut self.write_state {
            CipherState::Plain => {
                let len = chunk.len() as u16;
                self.tx_buf.push(ct);
                self.tx_buf.extend_from_slice(&legacy_ver.to_be_bytes());
                self.tx_buf.extend_from_slice(&len.to_be_bytes());
                self.tx_buf.extend_from_slice(chunk);
            }
            CipherState::Tls13 { cipher, iv, seq } => {
                let next_seq = seq
                    .checked_add(1)
                    .ok_or(TlsError::CryptoError("TLS 1.3 write sequence overflow"))?;
                let nonce = make_tls13_nonce(iv, *seq);
                *seq = next_seq;

                let enc_len = chunk.len() + 1 + 16;
                let hdr = [
                    CT_APPLICATION_DATA,
                    0x03,
                    0x03,
                    (enc_len >> 8) as u8,
                    (enc_len & 0xff) as u8,
                ];
                self.tx_buf.extend_from_slice(&hdr);
                self.tx_buf.extend_from_slice(chunk);
                self.tx_buf.push(ct);
                let tag = cipher.seal_in_place(&nonce, &hdr, &mut self.tx_buf[5..]);
                self.tx_buf.extend_from_slice(&tag);
            }
            CipherState::Tls12 {
                cipher,
                implicit_iv,
                seq,
                is_chacha,
                chacha_iv,
            } => {
                let next_seq = seq
                    .checked_add(1)
                    .ok_or(TlsError::CryptoError("TLS 1.2 write sequence overflow"))?;
                let cur_seq = *seq;
                *seq = next_seq;
                let aad = make_tls12_aad(cur_seq, ct, chunk.len());

                if *is_chacha {
                    let enc_len = chunk.len() + 16;
                    let hdr = [ct, 0x03, 0x03, (enc_len >> 8) as u8, (enc_len & 0xff) as u8];
                    let nonce = make_tls13_nonce(chacha_iv, cur_seq);
                    self.tx_buf.extend_from_slice(&hdr);
                    self.tx_buf.extend_from_slice(chunk);
                    let tag = cipher.seal_in_place(&nonce, &aad, &mut self.tx_buf[5..]);
                    self.tx_buf.extend_from_slice(&tag);
                } else {
                    let explicit = cur_seq.to_be_bytes();
                    let mut nonce = [0u8; 12];
                    nonce[..4].copy_from_slice(implicit_iv);
                    nonce[4..12].copy_from_slice(&explicit);
                    let enc_len = 8 + chunk.len() + 16;
                    let hdr = [ct, 0x03, 0x03, (enc_len >> 8) as u8, (enc_len & 0xff) as u8];
                    self.tx_buf.extend_from_slice(&hdr);
                    self.tx_buf.extend_from_slice(&explicit);
                    self.tx_buf.extend_from_slice(chunk);
                    let tag = cipher.seal_in_place(&nonce, &aad, &mut self.tx_buf[13..]);
                    self.tx_buf.extend_from_slice(&tag);
                }
            }
        }

        stream.write_all(&self.tx_buf)?;
        stream.flush()?;
        Ok(())
    }
}

#[inline]
fn make_tls13_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *iv;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 {
        nonce[4 + i] ^= seq_bytes[i];
    }
    nonce
}

#[inline]
fn make_tls12_aad(seq: u64, ct: u8, pt_len: usize) -> [u8; 13] {
    let mut aad = [0u8; 13];
    aad[..8].copy_from_slice(&seq.to_be_bytes());
    aad[8] = ct;
    aad[9] = 0x03;
    aad[10] = 0x03;
    aad[11] = (pt_len >> 8) as u8;
    aad[12] = (pt_len & 0xff) as u8;
    aad
}
