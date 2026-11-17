use std::sync::Arc;

use crate::vm::string::VmString;
use crate::vm::value::{MapKey, MapVal, Obj, OrderedMap, SCHEMA_URL, SCHEMA_URL_ERROR, StructData};

pub const URL_PARSE: u8 = 0;
pub const URL_ENCODE: u8 = 1;
pub const URL_DECODE: u8 = 2;
pub const URL_ENCODE_QUERY: u8 = 3;
pub const URL_DECODE_QUERY: u8 = 4;

#[inline]
pub fn make_url_error(kind: &str, message: impl Into<String>) -> Obj {
    let mut s = StructData::new(SCHEMA_URL_ERROR, 0, 2);
    s.inline_objs[0] = Obj::str(kind);
    s.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(s))
}

#[inline]
fn sub_range(full: &str, part: &str) -> std::ops::Range<usize> {
    let start = (part.as_ptr() as usize) - (full.as_ptr() as usize);
    start..(start + part.len())
}

#[inline]
fn slice_obj(base: &Arc<VmString>, full: &str, part: &str) -> Obj {
    Obj::Str(Arc::new(VmString::slice_arc(base, sub_range(full, part))))
}

pub fn url_parse(raw: &Arc<VmString>) -> Obj {
    let full = raw.as_str();
    let s = full.trim();
    let Some(scheme_end) = s.find("://") else {
        return make_url_error("invalid_url", "URL must contain '://'");
    };
    let scheme = &s[..scheme_end];
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return make_url_error("invalid_url", format!("Invalid URL scheme '{}'", scheme));
    }
    let rest = &s[scheme_end + 3..];
    let (before_frag, fragment) = match rest.split_once('#') {
        Some((a, f)) => (a, f),
        None => (rest, ""),
    };
    let (before_query, query) = match before_frag.split_once('?') {
        Some((a, q)) => (a, q),
        None => (before_frag, ""),
    };
    let (authority, path) = match before_query.find('/') {
        Some(idx) => (&before_query[..idx], &before_query[idx..]),
        None => (before_query, ""),
    };
    let host_port = match authority.rsplit_once('@') {
        Some((_, hp)) => hp,
        None => authority,
    };

    let (host, port): (&str, i64) = if host_port.starts_with('[') {
        if let Some(end_br) = host_port.find(']') {
            let h = &host_port[1..end_br];
            let tail = &host_port[end_br + 1..];
            if let Some(port_str) = tail.strip_prefix(':') {
                match port_str.parse::<i64>() {
                    Ok(p) if (1..=65535).contains(&p) => (h, p),
                    _ => return make_url_error("invalid_port", format!("Invalid port '{}'", port_str)),
                }
            } else if tail.is_empty() {
                (h, -1)
            } else {
                return make_url_error("invalid_url", "Malformed IPv6 authority");
            }
        } else {
            return make_url_error("invalid_url", "Unclosed '[' in IPv6 host");
        }
    } else if let Some((h, port_str)) = host_port.rsplit_once(':') {
        if port_str.is_empty() {
            (h, -1)
        } else {
            match port_str.parse::<i64>() {
                Ok(p) if (1..=65535).contains(&p) => (h, p),
                _ => return make_url_error("invalid_port", format!("Invalid port '{}'", port_str)),
            }
        }
    } else {
        (host_port, -1)
    };

    if host.is_empty() {
        return make_url_error("invalid_url", "URL host cannot be empty");
    }

    let scheme_obj = if scheme.bytes().any(|b| b.is_ascii_uppercase()) {
        Obj::str(scheme.to_ascii_lowercase())
    } else {
        slice_obj(raw, full, scheme)
    };
    let mut rec = StructData::new(SCHEMA_URL, 1, 5);
    rec.set_scalar(0, port as u64);
    rec.set_obj(0, scheme_obj);
    rec.set_obj(1, slice_obj(raw, full, host));
    rec.set_obj(2, if path.is_empty() { Obj::Str(Arc::new(VmString::slice_arc(raw, 0..0))) } else { slice_obj(raw, full, path) });
    rec.set_obj(3, if query.is_empty() { Obj::Str(Arc::new(VmString::slice_arc(raw, 0..0))) } else { slice_obj(raw, full, query) });
    rec.set_obj(4, if fragment.is_empty() { Obj::Str(Arc::new(VmString::slice_arc(raw, 0..0))) } else { slice_obj(raw, full, fragment) });
    Obj::Record(Arc::new(rec))
}

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX_UPPER[(b >> 4) as usize] as char);
            out.push(HEX_UPPER[(b & 0x0f) as usize] as char);
        }
    }
    out
}

#[inline]
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn percent_decode(input: &str, plus_as_space: bool) -> Result<String, Obj> {
    let bytes = input.as_bytes();
    if !bytes.contains(&b'%') && (!plus_as_space || !bytes.contains(&b'+')) {
        return Ok(input.to_string());
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(make_url_error("invalid_encoding", "Incomplete '%' escape"));
                }
                let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) else {
                    return Err(make_url_error("invalid_encoding", "Invalid hex in '%' escape"));
                };
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| make_url_error("invalid_utf8", e.to_string()))
}

pub fn encode_query_map(map: &OrderedMap) -> Obj {
    let mut pairs: Vec<(&str, &str)> = Vec::with_capacity(map.entries.len());
    for (k, v) in map.entries.iter() {
        if let (MapKey::Str(ks), MapVal::Obj(Obj::Str(vs))) = (k, v) {
            pairs.push((ks.as_str(), vs.as_str()));
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = String::new();
    for (idx, (k, v)) in pairs.iter().enumerate() {
        if idx > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(k));
        out.push('=');
        out.push_str(&percent_encode(v));
    }
    Obj::str(out)
}

pub fn decode_query_str(q_arc: &Arc<VmString>) -> Obj {
    let full = q_arc.as_str();
    let trimmed = full.strip_prefix('?').unwrap_or(full);
    let mut map = OrderedMap::default();
    if trimmed.is_empty() {
        return Obj::Map(Arc::new(map));
    }
    let slice_vm = |part: &str| -> Result<VmString, Obj> {
        let bytes = part.as_bytes();
        if !bytes.contains(&b'%') && !bytes.contains(&b'+') {
            Ok(VmString::slice_arc(q_arc, sub_range(full, part)))
        } else {
            percent_decode(part, true).map(VmString::from)
        }
    };
    for part in trimmed.split('&') {
        if part.is_empty() {
            continue;
        }
        let (raw_k, raw_v) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let k = match slice_vm(raw_k) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let v = match slice_vm(raw_v) {
            Ok(s) => s,
            Err(e) => return e,
        };
        map.set(MapKey::Str(Arc::new(k)), MapVal::Obj(Obj::Str(Arc::new(v))));
    }
    Obj::Map(Arc::new(map))
}
