use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::native::crypto::{constant_time_eq, hmac_sha256_bytes};
use crate::native::json_decode::decode_json_with_plan;
use crate::native::json_encode::encode_json_obj;
use crate::native::{JsonDecodePlan, StructSchema};
use crate::vm::value::{Obj, SCHEMA_JWT_ERROR, StructData};

pub const JWT_SIGN: u8 = 0;
pub const JWT_VERIFY: u8 = 1;
pub const JWT_DECODE_UNVERIFIED: u8 = 2;

#[inline]
pub fn make_jwt_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_JWT_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
fn now_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

// ── SHA-512 & HMAC-SHA512 (FIPS 180-4 / RFC 2104) ───────────────────────────

const H512_INIT: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

fn sha512_compress(h: &mut [u64; 8], blocks: &[[u8; 128]]) {
    for block in blocks {
        let mut w = [0u64; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 8;
            *word = u64::from_be_bytes([
                block[j],
                block[j + 1],
                block[j + 2],
                block[j + 3],
                block[j + 4],
                block[j + 5],
                block[j + 6],
                block[j + 7],
            ]);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

pub fn sha512_bytes(data: &[u8]) -> [u8; 64] {
    let mut h = H512_INIT;
    let (full_blocks, rem) = data.as_chunks::<128>();
    sha512_compress(&mut h, full_blocks);

    let mut tail = [0u8; 256];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let total_tail = if rem.len() < 112 { 128 } else { 256 };
    let bit_len = (data.len() as u128).wrapping_mul(8);
    tail[total_tail - 16..total_tail].copy_from_slice(&bit_len.to_be_bytes());
    let tail_blocks = tail[..total_tail].as_chunks::<128>().0;
    sha512_compress(&mut h, tail_blocks);

    let mut out = [0u8; 64];
    for (i, word) in h.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn hmac_sha512_bytes(key: &[u8], data: &[u8]) -> [u8; 64] {
    let mut k_block = [0u8; 128];
    if key.len() > 128 {
        let hashed = sha512_bytes(key);
        k_block[..64].copy_from_slice(&hashed);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 128];
    let mut opad = [0x5cu8; 128];
    for i in 0..128 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }
    let mut inner = Vec::with_capacity(128 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = sha512_bytes(&inner);

    let mut outer = [0u8; 192];
    outer[..128].copy_from_slice(&opad);
    outer[128..].copy_from_slice(&inner_hash);
    sha512_bytes(&outer)
}

// ── Strict Unpadded Base64URL (RFC 4648 §5) ─────────────────────────────────

const B64URL_ALPHA: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

const B64URL_LUT: [i8; 256] = {
    let mut lut = [-1i8; 256];
    let mut i = 0usize;
    while i < 64 {
        lut[B64URL_ALPHA[i] as usize] = i as i8;
        i += 1;
    }
    lut
};

pub fn b64url_encode(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity((bytes.len() * 4).div_ceil(3));
    let (chunks, rem) = bytes.as_chunks::<3>();
    for &[b0, b1, b2] in chunks {
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.extend_from_slice(&[
            B64URL_ALPHA[((n >> 18) & 63) as usize],
            B64URL_ALPHA[((n >> 12) & 63) as usize],
            B64URL_ALPHA[((n >> 6) & 63) as usize],
            B64URL_ALPHA[(n & 63) as usize],
        ]);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.extend_from_slice(&[
                B64URL_ALPHA[((n >> 18) & 63) as usize],
                B64URL_ALPHA[((n >> 12) & 63) as usize],
            ]);
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.extend_from_slice(&[
                B64URL_ALPHA[((n >> 18) & 63) as usize],
                B64URL_ALPHA[((n >> 12) & 63) as usize],
                B64URL_ALPHA[((n >> 6) & 63) as usize],
            ]);
        }
        _ => {}
    }
    // SAFETY: `B64URL_ALPHA` contains ASCII only.
    unsafe { String::from_utf8_unchecked(out) }
}

pub fn b64url_decode(s: &str) -> Result<Vec<u8>, String> {
    let bytes = s.as_bytes();
    if bytes.len() % 4 == 1 {
        return Err("Invalid Base64URL length".to_string());
    }
    let mut out = Vec::with_capacity((bytes.len() * 3) / 4);
    let (chunks, rem) = bytes.as_chunks::<4>();
    for &[c0, c1, c2, c3] in chunks {
        let v0 = B64URL_LUT[c0 as usize];
        let v1 = B64URL_LUT[c1 as usize];
        let v2 = B64URL_LUT[c2 as usize];
        let v3 = B64URL_LUT[c3 as usize];
        if (v0 | v1 | v2 | v3) < 0 {
            return Err("Invalid Base64URL character".to_string());
        }
        let n = ((v0 as u32) << 18) | ((v1 as u32) << 12) | ((v2 as u32) << 6) | (v3 as u32);
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    match rem.len() {
        2 => {
            let v0 = B64URL_LUT[rem[0] as usize];
            let v1 = B64URL_LUT[rem[1] as usize];
            if (v0 | v1) < 0 {
                return Err("Invalid Base64URL character".to_string());
            }
            if (v1 & 0x0f) != 0 {
                return Err("Non-canonical Base64URL trailing bits".to_string());
            }
            out.push(((v0 as u8) << 2) | ((v1 as u8) >> 4));
        }
        3 => {
            let v0 = B64URL_LUT[rem[0] as usize];
            let v1 = B64URL_LUT[rem[1] as usize];
            let v2 = B64URL_LUT[rem[2] as usize];
            if (v0 | v1 | v2) < 0 {
                return Err("Invalid Base64URL character".to_string());
            }
            if (v2 & 0x03) != 0 {
                return Err("Non-canonical Base64URL trailing bits".to_string());
            }
            out.push(((v0 as u8) << 2) | ((v1 as u8) >> 4));
            out.push(((v1 as u8) << 4) | ((v2 as u8) >> 2));
        }
        _ => {}
    }
    Ok(out)
}

// ── Minimal String/Depth-Aware JSON Object Helpers ──────────────────────────

#[inline]
fn skip_ws_bytes(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

fn parse_json_str_at(src: &str, mut pos: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(pos) != Some(&b'"') {
        return None;
    }
    pos += 1;
    let start = pos;
    let mut i = pos;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            return Some((src[start..i].to_string(), i + 1));
        }
        if b == b'\\' {
            break;
        }
        if b < 0x20 {
            return None;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let mut out = String::from(&src[start..i]);
    pos = i;
    while pos < bytes.len() {
        match bytes[pos] {
            b'"' => return Some((out, pos + 1)),
            b'\\' => {
                pos += 1;
                let esc = *bytes.get(pos)?;
                pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0C'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = src.get(pos..pos + 4)?;
                        let code = u16::from_str_radix(hex, 16).ok()?;
                        pos += 4;
                        if (0xD800..=0xDBFF).contains(&code) {
                            if bytes.get(pos..pos + 2) != Some(b"\\u") {
                                return None;
                            }
                            pos += 2;
                            let hex2 = src.get(pos..pos + 4)?;
                            let low = u16::from_str_radix(hex2, 16).ok()?;
                            pos += 4;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return None;
                            }
                            let cp = 0x10000 + ((((code - 0xD800) as u32) << 10) | ((low - 0xDC00) as u32));
                            out.push(char::from_u32(cp)?);
                        } else if (0xDC00..=0xDFFF).contains(&code) {
                            return None;
                        } else {
                            out.push(char::from_u32(u32::from(code))?);
                        }
                    }
                    _ => return None,
                }
            }
            b if b < 0x20 => return None,
            _ => {
                let ch = src[pos..].chars().next()?;
                out.push(ch);
                pos += ch.len_utf8();
            }
        }
    }
    None
}

fn skip_json_value_at(src: &str, mut pos: usize, depth: usize) -> Option<usize> {
    if depth > 64 {
        return None;
    }
    let bytes = src.as_bytes();
    pos = skip_ws_bytes(bytes, pos);
    let b = *bytes.get(pos)?;
    match b {
        b'"' => {
            let (_, end) = parse_json_str_at(src, pos)?;
            Some(end)
        }
        b'{' => {
            pos = skip_ws_bytes(bytes, pos + 1);
            if bytes.get(pos) == Some(&b'}') {
                return Some(pos + 1);
            }
            loop {
                pos = skip_ws_bytes(bytes, pos);
                let (_, next) = parse_json_str_at(src, pos)?;
                pos = skip_ws_bytes(bytes, next);
                if bytes.get(pos) != Some(&b':') {
                    return None;
                }
                pos = skip_json_value_at(src, pos + 1, depth + 1)?;
                pos = skip_ws_bytes(bytes, pos);
                match bytes.get(pos)? {
                    b',' => pos += 1,
                    b'}' => return Some(pos + 1),
                    _ => return None,
                }
            }
        }
        b'[' => {
            pos = skip_ws_bytes(bytes, pos + 1);
            if bytes.get(pos) == Some(&b']') {
                return Some(pos + 1);
            }
            loop {
                pos = skip_json_value_at(src, pos, depth + 1)?;
                pos = skip_ws_bytes(bytes, pos);
                match bytes.get(pos)? {
                    b',' => pos += 1,
                    b']' => return Some(pos + 1),
                    _ => return None,
                }
            }
        }
        b't' if bytes.get(pos..pos + 4) == Some(b"true") => Some(pos + 4),
        b'f' if bytes.get(pos..pos + 5) == Some(b"false") => Some(pos + 5),
        b'n' if bytes.get(pos..pos + 4) == Some(b"null") => Some(pos + 4),
        b'-' | b'0'..=b'9' => {
            if bytes.get(pos) == Some(&b'-') {
                pos += 1;
            }
            let first = *bytes.get(pos)?;
            if !first.is_ascii_digit() {
                return None;
            }
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'.') {
                pos += 1;
                if !bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            if matches!(bytes.get(pos), Some(b'e' | b'E')) {
                pos += 1;
                if matches!(bytes.get(pos), Some(b'+' | b'-')) {
                    pos += 1;
                }
                if !bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            Some(pos)
        }
        _ => None,
    }
}

pub fn json_obj_get_raw_span(json: &str, key: &str) -> Result<Option<(usize, usize)>, String> {
    let bytes = json.as_bytes();
    let mut pos = skip_ws_bytes(bytes, 0);
    if bytes.get(pos) != Some(&b'{') {
        return Err("Expected JSON object".to_string());
    }
    pos = skip_ws_bytes(bytes, pos + 1);
    if bytes.get(pos) == Some(&b'}') {
        pos = skip_ws_bytes(bytes, pos + 1);
        if pos != bytes.len() {
            return Err("Trailing characters after JSON object".to_string());
        }
        return Ok(None);
    }
    let mut found: Option<(usize, usize)> = None;
    loop {
        pos = skip_ws_bytes(bytes, pos);
        let (k, after_key) = parse_json_str_at(json, pos)
            .ok_or_else(|| "Malformed JSON object key".to_string())?;
        pos = skip_ws_bytes(bytes, after_key);
        if bytes.get(pos) != Some(&b':') {
            return Err("Expected ':' in JSON object".to_string());
        }
        let val_start = skip_ws_bytes(bytes, pos + 1);
        let val_end = skip_json_value_at(json, val_start, 0)
            .ok_or_else(|| "Malformed JSON value".to_string())?;
        if k == key {
            found = Some((val_start, val_end));
        }
        pos = skip_ws_bytes(bytes, val_end);
        match bytes.get(pos) {
            Some(b',') => pos += 1,
            Some(b'}') => {
                let end = skip_ws_bytes(bytes, pos + 1);
                if end != bytes.len() {
                    return Err("Trailing characters after JSON object".to_string());
                }
                return Ok(found);
            }
            _ => return Err("Malformed JSON object delimiter".to_string()),
        }
    }
}

pub fn json_obj_get_raw_field<'a>(json: &'a str, key: &str) -> Result<Option<&'a str>, String> {
    Ok(json_obj_get_raw_span(json, key)?.map(|(s, e)| &json[s..e]))
}

pub fn json_obj_get_string(json: &str, key: &str) -> Option<String> {
    let raw = json_obj_get_raw_field(json, key).ok()??;
    let (s, end) = parse_json_str_at(raw, 0)?;
    if end == raw.len() {
        Some(s)
    } else {
        None
    }
}

pub fn json_obj_get_i64(json: &str, key: &str) -> Result<Option<i64>, String> {
    let Some(raw) = json_obj_get_raw_field(json, key)? else {
        return Ok(None);
    };
    if raw == "null" {
        return Ok(None);
    }
    let val = raw
        .parse::<i64>()
        .map_err(|_| format!("JWT numeric claim '{}' must be an integer", key))?;
    Ok(Some(val))
}

fn inject_iat_exp(payload_json: &str, now: i64, ttl_secs: Option<i64>) -> Result<String, String> {
    let trimmed = payload_json.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err("JWT claims must serialize to a JSON object".to_string());
    }
    let mut s = trimmed.to_string();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut additions: Vec<String> = Vec::new();

    match json_obj_get_raw_span(&s, "iat")? {
        None => additions.push(format!("\"iat\":{}", now)),
        Some((st, en)) if &s[st..en] == "null" => replacements.push((st, en, now.to_string())),
        Some(_) => {}
    }
    if let Some(ttl) = ttl_secs
        && ttl > 0
    {
        let exp_val = now.saturating_add(ttl);
        match json_obj_get_raw_span(&s, "exp")? {
            None => additions.push(format!("\"exp\":{}", exp_val)),
            Some((st, en)) if &s[st..en] == "null" => {
                replacements.push((st, en, exp_val.to_string()))
            }
            Some(_) => {}
        }
    }
    replacements.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (st, en, rep) in replacements {
        s.replace_range(st..en, &rep);
    }
    if additions.is_empty() {
        return Ok(s);
    }
    let inner = s[1..s.len() - 1].trim();
    let extra = additions.join(",");
    if inner.is_empty() {
        Ok(format!("{{{}}}", extra))
    } else {
        Ok(format!("{{{},{}}}", inner, extra))
    }
}

// ── Public JWT Sign / Verify / Decode API ───────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum JwtAlg {
    Hs256,
    Hs512,
}

impl JwtAlg {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "HS256" => Some(Self::Hs256),
            "HS512" => Some(Self::Hs512),
            _ => None,
        }
    }

    fn header_json(self) -> &'static [u8] {
        match self {
            Self::Hs256 => br#"{"alg":"HS256","typ":"JWT"}"#,
            Self::Hs512 => br#"{"alg":"HS512","typ":"JWT"}"#,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hs256 => "HS256",
            Self::Hs512 => "HS512",
        }
    }

    fn verify_mac(self, key: &[u8], data: &[u8], sig: &[u8]) -> bool {
        match self {
            Self::Hs256 => {
                let expected = hmac_sha256_bytes(key, data);
                constant_time_eq(&expected, sig)
            }
            Self::Hs512 => {
                let expected = hmac_sha512_bytes(key, data);
                constant_time_eq(&expected, sig)
            }
        }
    }

    fn sign_b64url(self, key: &[u8], data: &[u8]) -> String {
        match self {
            Self::Hs256 => b64url_encode(&hmac_sha256_bytes(key, data)),
            Self::Hs512 => b64url_encode(&hmac_sha512_bytes(key, data)),
        }
    }
}

pub fn jwt_sign(
    alg_str: &str,
    claims_obj: &Obj,
    secret: &str,
    ttl_secs: Option<i64>,
    schemas: &[StructSchema],
) -> Obj {
    let Some(alg) = JwtAlg::parse(alg_str) else {
        return make_jwt_error(
            "unsupported_alg",
            format!("Unsupported JWT algorithm '{}' (expected HS256 or HS512)", alg_str),
        );
    };
    if let Some(ttl) = ttl_secs
        && ttl < 0
    {
        return make_jwt_error("invalid_ttl", "JWT TTL must be non-negative");
    }
    let payload_raw = match encode_json_obj(claims_obj, schemas) {
        Ok(s) => s,
        Err(e) => return make_jwt_error("encode", e.to_string()),
    };
    let trimmed = payload_raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return make_jwt_error("encode", "JWT claims must be a struct or Map<String, ...>");
    }
    let payload_json = if ttl_secs.is_some() {
        match inject_iat_exp(trimmed, now_secs(), ttl_secs) {
            Ok(s) => s,
            Err(e) => return make_jwt_error("encode", e),
        }
    } else {
        trimmed.to_string()
    };

    let hdr_b64 = b64url_encode(alg.header_json());
    let pay_b64 = b64url_encode(payload_json.as_bytes());
    let signing_input = format!("{}.{}", hdr_b64, pay_b64);
    let sig_b64 = alg.sign_b64url(secret.as_bytes(), signing_input.as_bytes());
    Obj::str(format!("{}.{}", signing_input, sig_b64))
}

fn split_jwt(token: &str) -> Result<(&str, &str, &str), Obj> {
    let mut parts = token.split('.');
    let (Some(p0), Some(p1), Some(p2), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(make_jwt_error("malformed", "JWT must have exactly 3 dot-separated segments"));
    };
    if p0.is_empty() || p1.is_empty() || p2.is_empty() {
        return Err(make_jwt_error("malformed", "JWT segments cannot be empty"));
    }
    Ok((p0, p1, p2))
}

pub fn jwt_verify(
    expected_alg_str: &str,
    token: &str,
    secret: &str,
    leeway_secs: i64,
    plan: &JsonDecodePlan,
) -> Obj {
    let Some(alg) = JwtAlg::parse(expected_alg_str) else {
        return make_jwt_error(
            "unsupported_alg",
            format!(
                "Unsupported JWT algorithm '{}' (expected HS256 or HS512)",
                expected_alg_str
            ),
        );
    };
    if leeway_secs < 0 {
        return make_jwt_error("invalid_input", "JWT leeway_secs must be non-negative");
    }
    let (p0, p1, p2) = match split_jwt(token) {
        Ok(parts) => parts,
        Err(err) => return err,
    };

    let hdr_bytes = match b64url_decode(p0) {
        Ok(b) => b,
        Err(e) => return make_jwt_error("malformed", format!("Invalid JWT header: {}", e)),
    };
    let hdr_str = match String::from_utf8(hdr_bytes) {
        Ok(s) => s,
        Err(e) => return make_jwt_error("malformed", format!("JWT header is not valid UTF-8: {}", e)),
    };
    let Some(hdr_alg) = json_obj_get_string(&hdr_str, "alg") else {
        return make_jwt_error("malformed", "JWT header missing string 'alg' field");
    };
    if hdr_alg != alg.as_str() {
        return make_jwt_error(
            "unsupported_alg",
            format!("JWT header alg '{}' does not match expected '{}'", hdr_alg, alg.as_str()),
        );
    }

    let sig_bytes = match b64url_decode(p2) {
        Ok(b) => b,
        Err(e) => return make_jwt_error("malformed", format!("Invalid JWT signature encoding: {}", e)),
    };
    let signing_len = p0.len() + 1 + p1.len();
    let signing_input = &token.as_bytes()[..signing_len];
    if !alg.verify_mac(secret.as_bytes(), signing_input, &sig_bytes) {
        return make_jwt_error("invalid_signature", "JWT signature verification failed");
    }

    let pay_bytes = match b64url_decode(p1) {
        Ok(b) => b,
        Err(e) => return make_jwt_error("malformed", format!("Invalid JWT payload encoding: {}", e)),
    };
    let pay_str = match String::from_utf8(pay_bytes) {
        Ok(s) => s,
        Err(e) => return make_jwt_error("malformed", format!("JWT payload is not valid UTF-8: {}", e)),
    };

    let now = now_secs();
    match json_obj_get_i64(&pay_str, "exp") {
        Ok(Some(exp)) => {
            if now > exp.saturating_add(leeway_secs) {
                return make_jwt_error("expired", "JWT token has expired");
            }
        }
        Ok(None) => {}
        Err(e) => return make_jwt_error("malformed", e),
    }
    match json_obj_get_i64(&pay_str, "nbf") {
        Ok(Some(nbf)) => {
            if now.saturating_add(leeway_secs) < nbf {
                return make_jwt_error("not_before", "JWT token is not active yet (nbf)");
            }
        }
        Ok(None) => {}
        Err(e) => return make_jwt_error("malformed", e),
    }

    match decode_json_with_plan(&pay_str, plan) {
        Ok(val) => val,
        Err(e) => make_jwt_error("decode", e.to_string()),
    }
}

pub fn jwt_decode_unverified(token: &str, plan: &JsonDecodePlan) -> Obj {
    let (_p0, p1, _p2) = match split_jwt(token) {
        Ok(parts) => parts,
        Err(err) => return err,
    };
    let pay_bytes = match b64url_decode(p1) {
        Ok(b) => b,
        Err(e) => return make_jwt_error("malformed", format!("Invalid JWT payload encoding: {}", e)),
    };
    let pay_str = match String::from_utf8(pay_bytes) {
        Ok(s) => s,
        Err(e) => return make_jwt_error("malformed", format!("JWT payload is not valid UTF-8: {}", e)),
    };
    match decode_json_with_plan(&pay_str, plan) {
        Ok(val) => val,
        Err(e) => make_jwt_error("decode", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn sha512_nist_abc_and_long_vector() {
        let digest = sha512_bytes(b"abc");
        assert_eq!(
            encode_hex(&digest),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        let two_block = sha512_bytes(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu");
        assert_eq!(
            encode_hex(&two_block),
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        );
    }

    #[test]
    fn hmac_sha512_rfc4231_case_1_and_2() {
        let key1 = [0x0bu8; 20];
        let mac1 = hmac_sha512_bytes(&key1, b"Hi There");
        assert_eq!(
            encode_hex(&mac1),
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
        let mac2 = hmac_sha512_bytes(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            encode_hex(&mac2),
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        );
    }

    #[test]
    fn b64url_roundtrip_and_strict_rejections() {
        for input in [
            b"".as_slice(),
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            b"\xff\xfe\xfd\xfc\xfb",
        ] {
            let enc = b64url_encode(input);
            assert!(!enc.contains('='));
            assert!(!enc.contains('+'));
            assert!(!enc.contains('/'));
            assert_eq!(b64url_decode(&enc).unwrap(), input);
        }
        assert!(b64url_decode("Zm9v=").is_err());
        assert!(b64url_decode("a").is_err());
        assert!(b64url_decode("ab+c").is_err());
        assert!(b64url_decode("ab c").is_err());
        // Non-canonical trailing bits: 'f' is 0x66 -> "Zg" (110011 0000), changing last char to "Zh" sets trailing bit
        assert!(b64url_decode("Zh").is_err());
    }

    #[test]
    fn json_obj_helpers_handle_nesting_and_escapes() {
        let j = r#"{"nested":{"alg":"none","exp":1},"desc":"a\"exp\":0\\}","alg":"HS256","exp":1900000000,"opt":null}"#;
        assert_eq!(json_obj_get_string(j, "alg").as_deref(), Some("HS256"));
        assert_eq!(json_obj_get_i64(j, "exp").unwrap(), Some(1900000000));
        assert_eq!(json_obj_get_i64(j, "opt").unwrap(), None);
        assert_eq!(json_obj_get_i64(j, "missing").unwrap(), None);
    }
}
