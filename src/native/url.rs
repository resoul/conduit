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
pub fn make_url_obj(
    scheme: &str,
    host: &str,
    port: i64,
    path: &str,
    query: &str,
    fragment: &str,
) -> Obj {
    let mut s = StructData::new(SCHEMA_URL, 1, 5);
    s.set_scalar(0, port as u64);
    s.set_obj(0, Obj::str(scheme));
    s.set_obj(1, Obj::str(host));
    s.set_obj(2, Obj::str(path));
    s.set_obj(3, Obj::str(query));
    s.set_obj(4, Obj::str(fragment));
    Obj::Record(Arc::new(s))
}

pub fn url_parse(raw: &str) -> Obj {
    let s = raw.trim();
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

    make_url_obj(
        &scheme.to_ascii_lowercase(),
        host,
        port,
        path,
        query,
        fragment,
    )
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

pub fn decode_query_str(q: &str) -> Obj {
    let trimmed = q.strip_prefix('?').unwrap_or(q);
    let mut map = OrderedMap::default();
    if trimmed.is_empty() {
        return Obj::Map(Arc::new(map));
    }
    for part in trimmed.split('&') {
        if part.is_empty() {
            continue;
        }
        let (raw_k, raw_v) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let k = match percent_decode(raw_k, true) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let v = match percent_decode(raw_v, true) {
            Ok(s) => s,
            Err(e) => return e,
        };
        map.set(MapKey::Str(Arc::new(VmString::from(k))), MapVal::Obj(Obj::str(v)));
    }
    Obj::Map(Arc::new(map))
}
