use std::io::Write as _;

use crate::native::{
    FieldSlot, JsonError, ScalarKind, StructSchema, err_at_field, err_at_index, find_schema,
};
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT,
    StructData,
};

#[inline]
pub(crate) fn write_escaped_bytes(s: &str, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'"');
    let bytes = s.as_bytes();
    let mut chunk_start = 0usize;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'"' || byte == b'\\' || byte < 0x20 {
            out.extend_from_slice(&bytes[chunk_start..index]);
            let esc: &[u8] = match byte {
                b'"' => b"\\\"",
                b'\\' => b"\\\\",
                b'\x08' => b"\\b",
                b'\x0C' => b"\\f",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                b'\t' => b"\\t",
                other => {
                    out.extend_from_slice(&[
                        b'\\',
                        b'u',
                        b'0',
                        b'0',
                        HEX[(other >> 4) as usize],
                        HEX[(other & 0x0F) as usize],
                    ]);
                    chunk_start = index + 1;
                    continue;
                }
            };
            out.extend_from_slice(esc);
            chunk_start = index + 1;
        }
    }
    out.extend_from_slice(&bytes[chunk_start..]);
    out.push(b'"');
}

#[inline]
pub(crate) fn write_i64(out: &mut Vec<u8>, val: i64) {
    let mut buf = [0; 20];
    out.extend_from_slice(crate::vm::string::format_i64(val, &mut buf).as_bytes());
}

#[inline]
pub(crate) fn write_f64(out: &mut Vec<u8>, val: f64) -> Result<(), JsonError> {
    if !val.is_finite() {
        return Err(JsonError::root(
            "Cannot encode non-finite Float (NaN or Infinity) to JSON",
        ));
    }
    if val == 0.0 {
        if val.is_sign_negative() {
            out.extend_from_slice(b"-0.0");
        } else {
            out.extend_from_slice(b"0.0");
        }
        return Ok(());
    }
    if val > -1e15 && val < 1e15 {
        let trunc = val.trunc();
        if trunc == val {
            write_i64(out, trunc as i64);
            out.extend_from_slice(b".0");
            return Ok(());
        }
        let doubled = val * 2.0;
        if doubled.trunc() == doubled {
            if val < 0.0 {
                out.push(b'-');
            }
            write_i64(out, trunc.abs() as i64);
            out.extend_from_slice(b".5");
            return Ok(());
        }
    }
    let start = out.len();
    let _ = write!(out, "{}", val);
    if !out[start..]
        .iter()
        .any(|b| matches!(b, b'.' | b'e' | b'E'))
    {
        out.extend_from_slice(b".0");
    }
    Ok(())
}

pub fn encode_json_obj(val: &Obj, schemas: &[StructSchema]) -> Result<String, JsonError> {
    let cap = match val {
        Obj::List(ListStorage::Obj(items)) => items.len().saturating_mul(224).max(64),
        Obj::List(ListStorage::Int(items)) => items.len().saturating_mul(8).max(32),
        Obj::List(ListStorage::Float(items)) => items.len().saturating_mul(12).max(32),
        Obj::Record(_) => 256,
        _ => 64,
    };
    let mut out = Vec::with_capacity(cap);
    encode_obj(val, schemas, &mut out)?;
    // SAFETY: `write_escaped_bytes` only injects ASCII escapes around valid `&str` UTF-8 bytes,
    // `wire_prefix` comes from `&str` keys, and numbers/literals are pure ASCII.
    Ok(unsafe { String::from_utf8_unchecked(out) })
}

pub fn encode_obj(val: &Obj, schemas: &[StructSchema], out: &mut Vec<u8>) -> Result<(), JsonError> {
    match val {
        Obj::DeferredField(_) => return encode_obj(val.resolved(), schemas, out),
        Obj::Bytes(bytes) => {
            out.push(b'[');
            for (i, byte) in bytes.iter().enumerate() {
                if i > 0 { out.push(b','); }
                write_i64(out, i64::from(*byte));
            }
            out.push(b']');
        }
        Obj::Empty => out.extend_from_slice(b"null"),
        Obj::Str(s) => write_escaped_bytes(s, out),
        Obj::List(storage) => encode_list(storage, schemas, out)?,
        Obj::Record(rec) => match rec.schema_id {
            SCHEMA_BOXED_INT => write_i64(out, rec.inline_scalars[0] as i64),
            SCHEMA_BOXED_FLOAT => write_f64(out, f64::from_bits(rec.inline_scalars[0]))?,
            SCHEMA_BOXED_BOOL => out.extend_from_slice(if rec.inline_scalars[0] != 0 {
                b"true"
            } else {
                b"false"
            }),
            _ => encode_struct(rec, schemas, out)?,
        },
        Obj::Map(map) => {
            out.push(b'{');
            for (index, (key, entry_val)) in map.entries().iter().enumerate() {
                let key_str = match key {
                    MapKey::Str(s) => s,
                    MapKey::Int(k) => {
                        return Err(JsonError::new(
                            format!("[{k}]"),
                            "JSON objects require String map keys",
                        ));
                    }
                    MapKey::Bool(k) => {
                        return Err(JsonError::new(
                            format!("[{k}]"),
                            "JSON objects require String map keys",
                        ));
                    }
                };
                if index > 0 {
                    out.push(b',');
                }
                write_escaped_bytes(key_str, out);
                out.push(b':');
                let res = match entry_val {
                    MapVal::Scalar(v) => {
                        write_i64(out, *v as i64);
                        Ok(())
                    }
                    MapVal::Float(v) => write_f64(out, *v),
                    MapVal::Bool(b) => {
                        out.extend_from_slice(if *b { b"true" } else { b"false" });
                        Ok(())
                    }
                    MapVal::Obj(child) => encode_obj(child, schemas, out),
                };
                if let Err(err) = res {
                    return Err(err_at_field(key_str, err));
                }
            }
            out.push(b'}');
        }
        Obj::Closure(_) | Obj::Func(_) | Obj::DbQuery(_) => {
            return Err(JsonError::root(
                "Value of type 'Function' is not JSON-serializable",
            ));
        }
    }
    Ok(())
}

#[inline]
fn encode_list(
    storage: &ListStorage,
    schemas: &[StructSchema],
    out: &mut Vec<u8>,
) -> Result<(), JsonError> {
    out.push(b'[');
    match storage {
        ListStorage::Int(items) => {
            for (index, &v) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_i64(out, v);
            }
        }
        ListStorage::Float(items) => {
            for (index, &v) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                if let Err(err) = write_f64(out, v) {
                    return Err(err_at_index(index, err));
                }
            }
        }
        ListStorage::Obj(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                if let Err(err) = encode_obj(item, schemas, out) {
                    return Err(err_at_index(index, err));
                }
            }
        }
    }
    out.push(b']');
    Ok(())
}

#[inline]
fn encode_struct_field(
    rec: &StructData,
    field: &crate::native::CompiledStructField,
    schemas: &[StructSchema],
    out: &mut Vec<u8>,
) -> Result<(), JsonError> {
    match field.slot {
        FieldSlot::Scalar {
            kind: ScalarKind::Int,
            slot,
        } => write_i64(out, rec.get_scalar(slot as usize) as i64),
        FieldSlot::Scalar {
            kind: ScalarKind::Float,
            slot,
        } => {
            if let Err(err) = write_f64(out, f64::from_bits(rec.get_scalar(slot as usize))) {
                return Err(err_at_field(&field.json_name, err));
            }
        }
        FieldSlot::Scalar {
            kind: ScalarKind::Bool,
            slot,
        } => {
            out.extend_from_slice(if rec.get_scalar(slot as usize) != 0 {
                b"true"
            } else {
                b"false"
            });
        }
        FieldSlot::Obj { slot, .. } => {
            if let Err(err) = encode_obj(rec.get_obj(slot as usize), schemas, out) {
                return Err(err_at_field(&field.json_name, err));
            }
        }
    }
    Ok(())
}

#[inline]
fn encode_struct(
    rec: &StructData,
    schemas: &[StructSchema],
    out: &mut Vec<u8>,
) -> Result<(), JsonError> {
    let Some(schema) = find_schema(schemas, rec.schema_id) else {
        return Err(JsonError::root(format!(
            "Missing schema metadata for schema_id {}",
            rec.schema_id
        )));
    };
    if let Some(tag) = schema.enum_tag.as_deref() {
        out.extend_from_slice(b"{\"type\":");
        write_escaped_bytes(tag, out);
        for (idx, field) in schema.fields.iter().enumerate() {
            if idx == 0 {
                out.push(b',');
                out.extend_from_slice(field.wire_prefix.strip_prefix(b"{").unwrap_or(&field.wire_prefix));
            } else {
                out.extend_from_slice(&field.wire_prefix);
            }
            encode_struct_field(rec, field, schemas, out)?;
        }
        out.push(b'}');
        return Ok(());
    }
    if schema.fields.is_empty() {
        out.extend_from_slice(b"{}");
        return Ok(());
    }
    for field in schema.fields.iter() {
        out.extend_from_slice(&field.wire_prefix);
        encode_struct_field(rec, field, schemas, out)?;
    }
    out.push(b'}');
    Ok(())
}
