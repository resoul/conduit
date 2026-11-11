use std::fmt::Write as _;

use super::{
    JsonValue, join_field_path, join_index_path, join_key_path, prepend_error_path, write_json_node,
};
use crate::value::{MapKey, Value};

pub(super) fn write_escaped_string(s: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('"');
    let mut chunk_start = 0;
    for (index, &byte) in s.as_bytes().iter().enumerate() {
        if byte == b'"' || byte == b'\\' || byte < 0x20 {
            out.push_str(&s[chunk_start..index]);
            match byte {
                b'"' => out.push_str("\\\""),
                b'\\' => out.push_str("\\\\"),
                b'\x08' => out.push_str("\\b"),
                b'\x0C' => out.push_str("\\f"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                _ => {
                    out.push_str("\\u00");
                    out.push(HEX[(byte >> 4) as usize] as char);
                    out.push(HEX[(byte & 0x0F) as usize] as char);
                }
            }
            chunk_start = index + 1;
        }
    }
    out.push_str(&s[chunk_start..]);
    out.push('"');
}

pub fn encode_with_struct_schemas(
    val: &Value,
    path: &str,
    out: &mut String,
) -> Result<(), (String, String)> {
    match val {
        Value::Nil => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Int(value) => {
            let _ = write!(out, "{}", value);
        }
        Value::Float(value) => {
            if !value.is_finite() {
                return Err((
                    path.to_string(),
                    "Cannot encode non-finite Float (NaN or Infinity) to JSON".to_string(),
                ));
            }
            let start = out.len();
            let _ = write!(out, "{}", value);
            if !out.as_bytes()[start..]
                .iter()
                .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
            {
                out.push_str(".0");
            }
        }
        Value::String(value) => write_escaped_string(value, out),
        Value::List(items) | Value::Tuple(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_with_struct_schemas(item, "", out).map_err(|(child, message)| {
                    (
                        prepend_error_path(join_index_path(path, index), &child),
                        message,
                    )
                })?;
            }
            out.push(']');
        }
        Value::Map(map) => {
            out.push('{');
            for (index, (key, value)) in map.entries().iter().enumerate() {
                let key_str = match key {
                    MapKey::String(value) => value,
                    _ => {
                        return Err((
                            join_key_path(path, key),
                            "JSON objects require String map keys".to_string(),
                        ));
                    }
                };
                if index > 0 {
                    out.push(',');
                }
                write_escaped_string(key_str, out);
                out.push(':');
                encode_with_struct_schemas(value, "", out).map_err(|(child, message)| {
                    (
                        prepend_error_path(join_key_path(path, key), &child),
                        message,
                    )
                })?;
            }
            out.push('}');
        }
        Value::Struct { schema, values } => {
            out.push('{');
            let prefixes = schema.json_prefixes();
            let json_names = schema.json_names();
            for (index, ((prefix, json_name), value)) in
                prefixes.iter().zip(json_names.iter()).zip(values.iter()).enumerate()
            {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(prefix);
                encode_with_struct_schemas(value, "", out).map_err(|(child, message)| {
                    (
                        prepend_error_path(join_field_path(path, json_name), &child),
                        message,
                    )
                })?;
            }
            out.push('}');
        }
        Value::Native(value) => {
            if let Some(json) = value.as_any().downcast_ref::<JsonValue>() {
                write_json_node(&json.0, out);
            } else {
                return Err((
                    path.to_string(),
                    format!(
                        "Value of native type '{}' is not JSON-serializable",
                        value.type_name()
                    ),
                ));
            }
        }
        other => {
            return Err((
                path.to_string(),
                format!(
                    "Value of type '{}' is not JSON-serializable",
                    other.type_name()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeId;
    use crate::value::StructSchema;

    #[test]
    fn sliced_escaping_preserves_unicode_and_appends_to_existing_output() {
        let mut out = "prefix:".to_string();
        write_escaped_string("Ok😀/\"\\\u{8}\u{C}\n\r\t\0\u{B}\u{1F}終", &mut out);
        assert_eq!(
            out,
            "prefix:\"Ok😀/\\\"\\\\\\b\\f\\n\\r\\t\\u0000\\u000B\\u001F終\""
        );
        write_escaped_string("", &mut out);
        assert!(out.ends_with("終\"\"\""));
    }

    #[test]
    fn sliced_escaping_matches_legacy_for_all_control_bytes() {
        let input: String = (0_u8..=0x1F).map(char::from).collect();
        let mut expected = String::from("\"");
        for ch in input.chars() {
            match ch {
                '\u{8}' => expected.push_str("\\b"),
                '\u{C}' => expected.push_str("\\f"),
                '\n' => expected.push_str("\\n"),
                '\r' => expected.push_str("\\r"),
                '\t' => expected.push_str("\\t"),
                _ => {
                    let _ = write!(expected, "\\u{:04X}", ch as u32);
                }
            }
        }
        expected.push('"');
        let mut actual = String::new();
        write_escaped_string(&input, &mut actual);
        assert_eq!(actual, expected);
    }

    #[test]
    fn floats_inspect_only_appended_bytes_and_preserve_signed_zero() {
        for value in [0.0, -0.0, 1.0, -2.0, 1.5, 1e30, 1e-30, f64::MIN, f64::MAX] {
            let mut out = "previous.eE:".to_string();
            encode_with_struct_schemas(&Value::Float(value), "", &mut out).unwrap();
            let mut formatted = value.to_string();
            if !formatted.contains(['.', 'e', 'E']) {
                formatted.push_str(".0");
            }
            assert_eq!(out, format!("previous.eE:{}", formatted));
        }
        let mut out = String::new();
        encode_with_struct_schemas(&Value::Float(-0.0), "", &mut out).unwrap();
        assert_eq!(out, "-0.0");
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut out = "prefix".to_string();
            let error = encode_with_struct_schemas(&Value::Float(value), "root", &mut out)
                .unwrap_err();
            assert_eq!(
                error,
                (
                    "root".to_string(),
                    "Cannot encode non-finite Float (NaN or Infinity) to JSON".to_string()
                )
            );
            assert_eq!(out, "prefix");
        }
    }

    #[test]
    fn encoded_key_prefixes_preserve_names_quoting_order_and_error_paths() {
        let type_id = TypeId::program("Tagged");
        let schema = StructSchema::with_json_names(
            type_id.clone(),
            vec![
                ("second".into(), "a\"b\\\n\u{1F}😀".into()),
                ("first".into(), "".into()),
            ],
        );
        let value = Value::Struct {
            schema,
            values: vec![Value::Int(2), Value::Int(1)],
        };
        let mut out = "prefix:".to_string();
        encode_with_struct_schemas(&value, "", &mut out).unwrap();
        assert_eq!(out, "prefix:{\"a\\\"b\\\\\\n\\u001F😀\":2,\"\":1}");

        let value = Value::Struct {
            schema: StructSchema::with_json_names(
                type_id,
                vec![("second".into(), "a\"b\\\n\u{1F}😀".into())],
            ),
            values: vec![Value::Function("f".into())],
        };
        let error = encode_with_struct_schemas(&value, "items[0]", &mut String::new()).unwrap_err();
        assert_eq!(error.0, join_field_path("items[0]", "a\"b\\\n\u{1F}😀"));
    }

    #[test]
    fn struct_fields_preserve_schema_order_and_maps_keep_insertion_order() {
        let value = Value::Struct {
            schema: StructSchema::new(TypeId::program("Ordered"), ["z".into(), "a".into()]),
            values: vec![Value::Int(1), Value::Int(2)],
        };
        let mut out = String::new();
        encode_with_struct_schemas(&value, "", &mut out).unwrap();
        assert_eq!(out, "{\"z\":1,\"a\":2}");

        let mut map = crate::value::OrderedMap::new();
        map.set(MapKey::String("z".into()), Value::Int(1));
        map.set(MapKey::String("a".into()), Value::Int(2));
        out.clear();
        encode_with_struct_schemas(&Value::Map(map), "", &mut out).unwrap();
        assert_eq!(out, "{\"z\":1,\"a\":2}");
    }
}
