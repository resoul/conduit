//! Strict DER/ASN.1 reader with depth and bounds checking (no panics on untrusted input).

use crate::native::tls::error::TlsError;

pub const MAX_ASN1_DEPTH: usize = 16;

pub const TAG_BOOLEAN: u8 = 0x01;
pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0c;
pub const TAG_PRINTABLE_STRING: u8 = 0x13;
pub const TAG_IA5_STRING: u8 = 0x16;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;

#[derive(Clone, Copy)]
pub struct DerReader<'a> {
    data: &'a [u8],
    pos: usize,
    depth: usize,
}

pub struct Tlv<'a> {
    pub tag: u8,
    /// Full raw TLV bytes including tag and length.
    pub raw: &'a [u8],
    /// Value bytes inside the TLV.
    pub value: &'a [u8],
    depth: usize,
}

impl<'a> Tlv<'a> {
    pub fn reader(&self) -> Result<DerReader<'a>, TlsError> {
        if self.depth >= MAX_ASN1_DEPTH {
            return Err(TlsError::CertificateError("ASN.1 nesting depth exceeded".to_string()));
        }
        Ok(DerReader {
            data: self.value,
            pos: 0,
            depth: self.depth + 1,
        })
    }
}

impl<'a> DerReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            depth: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    pub fn peek_tag(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn read_tlv(&mut self) -> Result<Tlv<'a>, TlsError> {
        let start = self.pos;
        if self.pos >= self.data.len() {
            return Err(TlsError::DecodeError("unexpected end of ASN.1 data"));
        }
        let tag = self.data[self.pos];
        // Reject high-tag-number form (tag & 0x1f == 0x1f) for simplicity and strictness
        if (tag & 0x1f) == 0x1f {
            return Err(TlsError::DecodeError("unsupported ASN.1 high-tag-number form"));
        }
        self.pos += 1;

        let len = self.read_der_length()?;
        let end = self
            .pos
            .checked_add(len)
            .ok_or(TlsError::DecodeError("ASN.1 length overflow"))?;
        if end > self.data.len() {
            return Err(TlsError::DecodeError("ASN.1 element truncated"));
        }
        let value = &self.data[self.pos..end];
        let raw = &self.data[start..end];
        self.pos = end;

        Ok(Tlv {
            tag,
            raw,
            value,
            depth: self.depth,
        })
    }

    fn read_der_length(&mut self) -> Result<usize, TlsError> {
        if self.pos >= self.data.len() {
            return Err(TlsError::DecodeError("truncated ASN.1 length"));
        }
        let first = self.data[self.pos];
        self.pos += 1;
        if first & 0x80 == 0 {
            return Ok(first as usize);
        }
        let num_octets = (first & 0x7f) as usize;
        // Reject indefinite length (0x80) and > 4 byte lengths
        if num_octets == 0 || num_octets > 4 {
            return Err(TlsError::DecodeError("invalid DER length encoding"));
        }
        if self.pos + num_octets > self.data.len() {
            return Err(TlsError::DecodeError("truncated DER long length"));
        }
        // DER requires minimal length encoding: leading byte cannot be 0
        if self.data[self.pos] == 0 {
            return Err(TlsError::DecodeError("non-minimal DER length"));
        }
        let mut len = 0usize;
        for _ in 0..num_octets {
            len = (len << 8) | (self.data[self.pos] as usize);
            self.pos += 1;
        }
        // DER requires lengths < 128 to use short form
        if len < 128 {
            return Err(TlsError::DecodeError("non-minimal DER length (< 128 in long form)"));
        }
        Ok(len)
    }

    pub fn read_tag(&mut self, expected: u8) -> Result<Tlv<'a>, TlsError> {
        let tlv = self.read_tlv()?;
        if tlv.tag != expected {
            return Err(TlsError::DecodeError("unexpected ASN.1 tag"));
        }
        Ok(tlv)
    }

    pub fn read_sequence(&mut self) -> Result<DerReader<'a>, TlsError> {
        self.read_tag(TAG_SEQUENCE)?.reader()
    }

    pub fn read_set(&mut self) -> Result<DerReader<'a>, TlsError> {
        self.read_tag(TAG_SET)?.reader()
    }

    pub fn read_oid(&mut self) -> Result<&'a [u8], TlsError> {
        let tlv = self.read_tag(TAG_OID)?;
        if tlv.value.is_empty() {
            return Err(TlsError::DecodeError("empty ASN.1 OID"));
        }
        Ok(tlv.value)
    }

    pub fn read_integer_bytes(&mut self) -> Result<&'a [u8], TlsError> {
        let tlv = self.read_tag(TAG_INTEGER)?;
        if tlv.value.is_empty() {
            return Err(TlsError::DecodeError("empty ASN.1 INTEGER"));
        }
        Ok(tlv.value)
    }

    pub fn read_positive_integer_be(&mut self) -> Result<&'a [u8], TlsError> {
        let raw = self.read_integer_bytes()?;
        if (raw[0] & 0x80) != 0 {
            return Err(TlsError::DecodeError("negative ASN.1 INTEGER where positive expected"));
        }
        // Strip optional single leading 0x00 sign-padding byte
        if raw.len() > 1 && raw[0] == 0x00 {
            if (raw[1] & 0x80) == 0 {
                return Err(TlsError::DecodeError("non-minimal ASN.1 INTEGER"));
            }
            Ok(&raw[1..])
        } else {
            Ok(raw)
        }
    }

    pub fn read_boolean(&mut self) -> Result<bool, TlsError> {
        let tlv = self.read_tag(TAG_BOOLEAN)?;
        if tlv.value.len() != 1 {
            return Err(TlsError::DecodeError("invalid ASN.1 BOOLEAN length"));
        }
        match tlv.value[0] {
            0x00 => Ok(false),
            0xff => Ok(true),
            _ => Err(TlsError::DecodeError("non-DER ASN.1 BOOLEAN value")),
        }
    }

    pub fn read_bit_string(&mut self) -> Result<(u8, &'a [u8]), TlsError> {
        let tlv = self.read_tag(TAG_BIT_STRING)?;
        if tlv.value.is_empty() {
            return Err(TlsError::DecodeError("empty ASN.1 BIT STRING"));
        }
        let unused_bits = tlv.value[0];
        if unused_bits > 7 || (tlv.value.len() == 1 && unused_bits != 0) {
            return Err(TlsError::DecodeError("invalid ASN.1 BIT STRING unused bits"));
        }
        if unused_bits > 0 && (tlv.value[tlv.value.len() - 1] & ((1u8 << unused_bits) - 1)) != 0 {
            return Err(TlsError::DecodeError("non-zero ASN.1 BIT STRING unused bits"));
        }
        Ok((unused_bits, &tlv.value[1..]))
    }

    pub fn read_octet_string(&mut self) -> Result<&'a [u8], TlsError> {
        Ok(self.read_tag(TAG_OCTET_STRING)?.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_every_truncated_prefix_of_a_nested_sequence() {
        let der = [TAG_SEQUENCE, 5, TAG_INTEGER, 1, 42, TAG_NULL, 0];
        for len in 0..der.len() {
            assert!(DerReader::new(&der[..len]).read_sequence().is_err());
            assert!(crate::native::tls::x509::parse_x509(&der[..len]).is_err());
        }
        let mut sequence = DerReader::new(&der).read_sequence().unwrap();
        assert_eq!(sequence.read_positive_integer_be().unwrap(), &[42]);
        assert!(sequence.read_tag(TAG_NULL).is_ok());
        assert!(sequence.is_empty());
    }

    #[test]
    fn asn1_rejects_non_minimal_and_indefinite_lengths() {
        // Indefinite length 0x30 0x80 0x00 0x00
        assert!(DerReader::new(&[0x30, 0x80, 0x00, 0x00]).read_tlv().is_err());
        // Non-minimal length: length 5 encoded as 0x81 0x05
        assert!(DerReader::new(&[0x04, 0x81, 0x05, 1, 2, 3, 4, 5]).read_tlv().is_err());
        // Leading zero in long length: 0x82 0x00 0x80
        let mut buf = vec![0x04, 0x82, 0x00, 0x80];
        buf.resize(132, 0);
        assert!(DerReader::new(&buf).read_tlv().is_err());
        // Truncated
        assert!(DerReader::new(&[0x04, 0x05, 0x01, 0x02]).read_tlv().is_err());
    }

    #[test]
    fn wycheproof_strict_der_primitives_and_depth_limit() {
        // BIT STRING unused bits must be 0 when unused_bits > 0 (X.690 §11.2.2).
        assert!(DerReader::new(&[0x03, 0x02, 0x01, 0x01]).read_bit_string().is_err());
        assert!(DerReader::new(&[0x03, 0x02, 0x07, 0x40]).read_bit_string().is_err());
        assert!(DerReader::new(&[0x03, 0x02, 0x08, 0x00]).read_bit_string().is_err());
        assert!(DerReader::new(&[0x03, 0x01, 0x01]).read_bit_string().is_err());
        assert!(DerReader::new(&[0x03, 0x00]).read_bit_string().is_err());
        assert_eq!(
            DerReader::new(&[0x03, 0x02, 0x01, 0xfe]).read_bit_string().unwrap(),
            (1, &[0xfe][..])
        );
        assert_eq!(
            DerReader::new(&[0x03, 0x01, 0x00]).read_bit_string().unwrap(),
            (0, &[][..])
        );

        // Non-DER BOOLEAN values rejected (only 0x00 and 0xff allowed).
        assert!(!DerReader::new(&[0x01, 0x01, 0x00]).read_boolean().unwrap());
        assert!(DerReader::new(&[0x01, 0x01, 0xff]).read_boolean().unwrap());
        assert!(DerReader::new(&[0x01, 0x01, 0x01]).read_boolean().is_err());
        assert!(DerReader::new(&[0x01, 0x01, 0x7f]).read_boolean().is_err());
        assert!(DerReader::new(&[0x01, 0x02, 0xff, 0x00]).read_boolean().is_err());

        // Non-minimal, negative, and empty positive INTEGERs rejected.
        assert!(DerReader::new(&[0x02, 0x00]).read_positive_integer_be().is_err());
        assert!(DerReader::new(&[0x02, 0x01, 0x80]).read_positive_integer_be().is_err());
        assert!(DerReader::new(&[0x02, 0x02, 0x00, 0x7f]).read_positive_integer_be().is_err());
        assert_eq!(
            DerReader::new(&[0x02, 0x02, 0x00, 0x80]).read_positive_integer_be().unwrap(),
            &[0x80]
        );

        // High-tag-number form and >4 byte length octets rejected.
        assert!(DerReader::new(&[0x1f, 0x01, 0x00]).read_tlv().is_err());
        assert!(DerReader::new(&[0x04, 0x85, 1, 2, 3, 4, 5]).read_tlv().is_err());

        // Excessive nesting beyond MAX_ASN1_DEPTH (16) rejected without deep recursion.
        let mut nested = vec![0x04u8, 0x01, 0xaa];
        for _ in 0..=MAX_ASN1_DEPTH {
            let mut wrap = vec![TAG_SEQUENCE, nested.len() as u8];
            wrap.extend_from_slice(&nested);
            nested = wrap;
        }
        let mut cur = DerReader::new(&nested);
        for _ in 0..MAX_ASN1_DEPTH {
            cur = cur.read_sequence().unwrap();
        }
        assert!(cur.read_sequence().is_err());
    }
}
