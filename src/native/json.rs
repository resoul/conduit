use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use super::{NativeFuncDef, NativeMethodDef, NativeModule, NativeTypeDef, NativeValue};
use crate::types::{Type, TypeId};
use crate::value::{MapKey, OrderedMap, Value};

const MAX_JSON_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct JsonNumber {
    pub raw: String,
    pub int_val: Option<i64>,
    pub float_val: f64,
    pub is_int_syntax: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonNode {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonNode>),
    Object(Vec<(String, JsonNode)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonValue(pub JsonNode);

impl NativeValue for JsonValue {
    fn type_id(&self) -> TypeId {
        TypeId::native("cdt.json", "Value")
    }

    fn type_name(&self) -> &'static str {
        "json.Value"
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", stringify_json_node(&self.0))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn dyn_eq(&self, other: &dyn NativeValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JsonValue>()
            .map_or(false, |o| self.0 == o.0)
    }
}

pub fn make_decode_error(path: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.json", "DecodeError"),
        fields,
    }
}

pub fn make_encode_error(path: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.json", "EncodeError"),
        fields,
    }
}

pub fn join_field_path(base: &str, field: &str) -> String {
    if base.is_empty() {
        field.to_string()
    } else {
        format!("{}.{}", base, field)
    }
}

pub fn join_index_path(base: &str, idx: usize) -> String {
    format!("{}[{}]", base, idx)
}

pub fn join_key_path(base: &str, key: &MapKey) -> String {
    format!("{}{}", base, key.format_path_segment())
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            src,
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn err(&self, path: &str, msg: impl Into<String>) -> (String, String) {
        (path.to_string(), msg.into())
    }

    fn parse_root(&mut self) -> Result<JsonNode, (String, String)> {
        self.skip_ws();
        let node = self.parse_value("", 0)?;
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(self.err("", "Unexpected trailing characters after JSON value"));
        }
        Ok(node)
    }

    fn parse_value(&mut self, path: &str, depth: usize) -> Result<JsonNode, (String, String)> {
        self.skip_ws();
        let b = match self.peek() {
            Some(c) => c,
            None => return Err(self.err(path, "Unexpected end of JSON input")),
        };
        if matches!(b, b'[' | b'{') && depth >= MAX_JSON_DEPTH {
            return Err(self.err(path, "Maximum JSON nesting depth (128) exceeded"));
        }
        match b {
            b'n' => self.parse_literal(b"null", JsonNode::Null, path),
            b't' => self.parse_literal(b"true", JsonNode::Bool(true), path),
            b'f' => self.parse_literal(b"false", JsonNode::Bool(false), path),
            b'"' => self.parse_string(path).map(JsonNode::String),
            b'[' => self.parse_array(path, depth),
            b'{' => self.parse_object(path, depth),
            b'-' | b'0'..=b'9' => self.parse_number(path).map(JsonNode::Number),
            other => Err(self.err(
                path,
                format!("Unexpected character '{}' in JSON input", other as char),
            )),
        }
    }

    fn parse_literal(
        &mut self,
        lit: &[u8],
        node: JsonNode,
        path: &str,
    ) -> Result<JsonNode, (String, String)> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(node)
        } else {
            Err(self.err(
                path,
                format!(
                    "Invalid JSON literal (expected '{}')",
                    std::str::from_utf8(lit).unwrap_or("")
                ),
            ))
        }
    }

    fn parse_string(&mut self, path: &str) -> Result<String, (String, String)> {
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let esc = self
                    .peek()
                    .ok_or_else(|| self.err(path, "Unterminated escape sequence in JSON string"))?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4(path)?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            if self.bytes[self.pos..].starts_with(b"\\u") {
                                self.pos += 2;
                                let low = self.parse_hex4(path)?;
                                if (0xDC00..=0xDFFF).contains(&low) {
                                    let combined =
                                        0x10000 + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                                    let ch = char::from_u32(combined).ok_or_else(|| {
                                        self.err(path, "Invalid Unicode surrogate pair")
                                    })?;
                                    out.push(ch);
                                } else {
                                    return Err(self.err(path, "Invalid low Unicode surrogate"));
                                }
                            } else {
                                return Err(self.err(path, "Unpaired high Unicode surrogate"));
                            }
                        } else if (0xDC00..=0xDFFF).contains(&code) {
                            return Err(self.err(path, "Unexpected low Unicode surrogate"));
                        } else {
                            let ch = char::from_u32(code as u32)
                                .ok_or_else(|| self.err(path, "Invalid Unicode scalar value"))?;
                            out.push(ch);
                        }
                    }
                    other => {
                        return Err(self.err(
                            path,
                            format!("Invalid JSON escape sequence '\\{}'", other as char),
                        ));
                    }
                }
            } else if b < 0x20 {
                return Err(self.err(path, "Unescaped control character in JSON string"));
            } else {
                let rest = &self.src[self.pos..];
                let ch = rest
                    .chars()
                    .next()
                    .ok_or_else(|| self.err(path, "Invalid UTF-8 in JSON string"))?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Err(self.err(path, "Unterminated JSON string"))
    }

    fn parse_hex4(&mut self, path: &str) -> Result<u16, (String, String)> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err(path, "Incomplete \\uXXXX escape in JSON string"));
        }
        let bytes = &self.bytes[self.pos..self.pos + 4];
        let mut value = 0u16;
        for byte in bytes {
            let digit = match *byte {
                b'0'..=b'9' => *byte - b'0',
                b'a'..=b'f' => *byte - b'a' + 10,
                b'A'..=b'F' => *byte - b'A' + 10,
                _ => return Err(self.err(path, "Invalid \\uXXXX hex sequence")),
            };
            value = (value << 4) | u16::from(digit);
        }
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self, path: &str) -> Result<JsonNumber, (String, String)> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.err(path, "Leading zeros are not allowed in JSON numbers"));
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err(path, "Invalid JSON number")),
        }

        let mut is_int_syntax = true;
        if self.peek() == Some(b'.') {
            is_int_syntax = false;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err(path, "Expected digit after decimal point in JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_int_syntax = false;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err(path, "Expected exponent digits in JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let raw = self.src[start..self.pos].to_string();
        let int_val = if is_int_syntax {
            raw.parse::<i64>().ok()
        } else {
            None
        };
        let float_val = raw
            .parse::<f64>()
            .map_err(|_| self.err(path, format!("Invalid JSON float number '{}'", raw)))?;
        if !float_val.is_finite() {
            return Err(self.err(
                path,
                format!("JSON number '{}' overflows finite IEEE 754 range", raw),
            ));
        }

        Ok(JsonNumber {
            raw,
            int_val,
            float_val,
            is_int_syntax,
        })
    }

    fn parse_array(&mut self, path: &str, depth: usize) -> Result<JsonNode, (String, String)> {
        self.pos += 1;
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonNode::Array(items));
        }
        loop {
            let item_path = join_index_path(path, items.len());
            items.push(self.parse_value(&item_path, depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonNode::Array(items));
                }
                _ => return Err(self.err(&item_path, "Expected ',' or ']' in JSON array")),
            }
        }
    }

    fn parse_object(&mut self, path: &str, depth: usize) -> Result<JsonNode, (String, String)> {
        self.pos += 1;
        self.skip_ws();
        let mut entries = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonNode::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err(path, "Expected string key in JSON object"));
            }
            let key = self.parse_string(path)?;
            let field_path = join_field_path(path, &key);
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err(&field_path, "Expected ':' after JSON object key"));
            }
            self.pos += 1;
            let val = self.parse_value(&field_path, depth + 1)?;
            entries.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonNode::Object(entries));
                }
                _ => return Err(self.err(&field_path, "Expected ',' or '}' in JSON object")),
            }
        }
    }
}

pub fn parse_json(raw: &str) -> Result<JsonNode, (String, String)> {
    JsonParser::new(raw).parse_root()
}

pub fn write_json_escaped_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn stringify_json_node(node: &JsonNode) -> String {
    let mut out = String::new();
    write_json_node(node, &mut out);
    out
}

fn write_json_node(node: &JsonNode, out: &mut String) {
    match node {
        JsonNode::Null => out.push_str("null"),
        JsonNode::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonNode::Number(n) => {
            out.push_str(&n.raw);
            if !n.is_int_syntax
                && !n.raw.contains('.')
                && !n.raw.contains('e')
                && !n.raw.contains('E')
            {
                out.push_str(".0");
            }
        }
        JsonNode::String(s) => write_json_escaped_string(s, out),
        JsonNode::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_node(item, out);
            }
            out.push(']');
        }
        JsonNode::Object(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_escaped_string(k, out);
                out.push(':');
                write_json_node(v, out);
            }
            out.push('}');
        }
    }
}

pub fn decode_node_to_conduit(
    node: &JsonNode,
    target_ty: &Type,
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, Type)>>,
) -> Result<Value, (String, String)> {
    match target_ty {
        Type::Optional(inner) => {
            if matches!(node, JsonNode::Null) {
                Ok(Value::Nil)
            } else {
                decode_node_to_conduit(node, inner, path, resolve_struct)
            }
        }
        Type::Custom(tid) if tid == &TypeId::native("cdt.json", "Value") => {
            Ok(Value::Native(Arc::new(JsonValue(node.clone()))))
        }
        Type::Int => match node {
            JsonNode::Number(num) => {
                if !num.is_int_syntax {
                    Err((
                        path.to_string(),
                        format!(
                            "Expected Int, got fractional or exponential number '{}'",
                            num.raw
                        ),
                    ))
                } else if let Some(n) = num.int_val {
                    Ok(Value::Int(n))
                } else {
                    Err((
                        path.to_string(),
                        format!("Integer value '{}' overflows 64-bit Int", num.raw),
                    ))
                }
            }
            other => Err((
                path.to_string(),
                format!("Expected Int, got {}", json_kind_name(other)),
            )),
        },
        Type::Float => match node {
            JsonNode::Number(num) => Ok(Value::Float(num.float_val)),
            other => Err((
                path.to_string(),
                format!("Expected Float, got {}", json_kind_name(other)),
            )),
        },
        Type::Bool => match node {
            JsonNode::Bool(b) => Ok(Value::Bool(*b)),
            other => Err((
                path.to_string(),
                format!("Expected Bool, got {}", json_kind_name(other)),
            )),
        },
        Type::String => match node {
            JsonNode::String(s) => Ok(Value::String(s.clone())),
            other => Err((
                path.to_string(),
                format!("Expected String, got {}", json_kind_name(other)),
            )),
        },
        Type::List(inner) => match node {
            JsonNode::Array(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    let item_path = join_index_path(path, i);
                    vals.push(decode_node_to_conduit(item, inner, &item_path, resolve_struct)?);
                }
                Ok(Value::List(vals))
            }
            other => Err((
                path.to_string(),
                format!("Expected List, got {}", json_kind_name(other)),
            )),
        },
        Type::Map(key_ty, val_ty) => {
            if **key_ty != Type::String {
                return Err((
                    path.to_string(),
                    format!("JSON objects can only decode to Map<String, V>, got Map<{}, {}>", key_ty, val_ty),
                ));
            }
            match node {
                JsonNode::Object(entries) => {
                    let mut map = OrderedMap::new();
                    for (k, v) in entries {
                        let map_key = MapKey::String(k.clone());
                        let entry_path = join_key_path(path, &map_key);
                        let decoded_v =
                            decode_node_to_conduit(v, val_ty, &entry_path, resolve_struct)?;
                        map.set(map_key, decoded_v);
                    }
                    Ok(Value::Map(map))
                }
                other => Err((
                    path.to_string(),
                    format!("Expected JSON object for Map<String, {}>, got {}", val_ty, json_kind_name(other)),
                )),
            }
        }
        Type::Tuple(elem_types) => match node {
            JsonNode::Array(items) => {
                if items.len() != elem_types.len() {
                    return Err((
                        path.to_string(),
                        format!(
                            "Expected tuple JSON array of length {}, got length {}",
                            elem_types.len(),
                            items.len()
                        ),
                    ));
                }
                let mut vals = Vec::with_capacity(elem_types.len());
                for (i, (item, elem_ty)) in items.iter().zip(elem_types.iter()).enumerate() {
                    let elem_path = join_index_path(path, i);
                    vals.push(decode_node_to_conduit(
                        item,
                        elem_ty,
                        &elem_path,
                        resolve_struct,
                    )?);
                }
                Ok(Value::Tuple(vals))
            }
            other => Err((
                path.to_string(),
                format!("Expected tuple JSON array, got {}", json_kind_name(other)),
            )),
        },
        Type::Custom(type_id) => {
            decode_struct_node(node, type_id, &[], path, resolve_struct)
        }
        Type::Generic { type_id, type_args } => {
            decode_struct_node(node, type_id, type_args, path, resolve_struct)
        }
        Type::Union(variants) => decode_union_node(node, variants, path, resolve_struct),
        other => Err((
            path.to_string(),
            format!("Unsupported target type '{}' for json.decode", other),
        )),
    }
}

fn decode_struct_node(
    node: &JsonNode,
    type_id: &TypeId,
    type_args: &[Type],
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, Type)>>,
) -> Result<Value, (String, String)> {
    let declared_fields = resolve_struct(type_id, type_args).ok_or_else(|| {
        (
            path.to_string(),
            format!("Type '{}' cannot be decoded from JSON", type_id),
        )
    })?;
    let entries = match node {
        JsonNode::Object(e) => e,
        other => {
            return Err((
                path.to_string(),
                format!("Expected JSON object for '{}', got {}", type_id, json_kind_name(other)),
            ));
        }
    };

    let mut json_fields: HashMap<&str, &JsonNode> = HashMap::with_capacity(entries.len());
    for (k, v) in entries {
        let field_path = join_field_path(path, k);
        if !declared_fields.iter().any(|(fname, _)| fname == k) {
            return Err((
                field_path,
                format!("Unknown field '{}' for type '{}'", k, type_id),
            ));
        }
        json_fields.insert(k.as_str(), v);
    }

    let mut out_fields = HashMap::with_capacity(declared_fields.len());
    for (fname, fty) in declared_fields {
        let field_path = join_field_path(path, &fname);
        if let Some(jval) = json_fields.get(fname.as_str()) {
            let decoded = decode_node_to_conduit(jval, &fty, &field_path, resolve_struct)?;
            out_fields.insert(fname, decoded);
        } else if matches!(fty, Type::Optional(_)) || union_contains_nil(&fty) {
            out_fields.insert(fname, Value::Nil);
        } else {
            return Err((
                field_path,
                format!("Missing required field '{}'", fname),
            ));
        }
    }

    Ok(Value::Struct {
        type_id: type_id.clone(),
        fields: out_fields,
    })
}

fn union_contains_nil(ty: &Type) -> bool {
    match ty {
        Type::Nil | Type::Optional(_) => true,
        Type::Union(vs) => vs.iter().any(union_contains_nil),
        _ => false,
    }
}

fn decode_union_node(
    node: &JsonNode,
    variants: &[Type],
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, Type)>>,
) -> Result<Value, (String, String)> {
    let candidates: Vec<&Type> = match node {
        JsonNode::Null => variants
            .iter()
            .filter(|v| matches!(v, Type::Nil | Type::Optional(_)))
            .collect(),
        JsonNode::Bool(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::Bool) || matches!(v, Type::Optional(inner) if **inner == Type::Bool))
            .collect(),
        JsonNode::String(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::String) || matches!(v, Type::Optional(inner) if **inner == Type::String))
            .collect(),
        JsonNode::Number(number) => variants
            .iter()
            .filter(|v| {
                (number.is_int_syntax && matches!(v, Type::Int))
                    || (!number.is_int_syntax && matches!(v, Type::Float))
                    || matches!(v, Type::Optional(inner) if (number.is_int_syntax && **inner == Type::Int) || (!number.is_int_syntax && **inner == Type::Float))
            })
            .collect(),
        JsonNode::Array(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::List(_) | Type::Tuple(_)))
            .collect(),
        JsonNode::Object(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::Map(_, _) | Type::Custom(_) | Type::Generic { .. }))
            .collect(),
    };

    if candidates.len() > 1 {
        return Err((
            path.to_string(),
            format!("Ambiguous union target for JSON {}", json_kind_name(node)),
        ));
    }

    if let Some(target) = candidates.into_iter().next() {
        if matches!(node, JsonNode::Null) {
            return Ok(Value::Nil);
        }
        return decode_node_to_conduit(node, target, path, resolve_struct);
    }

    if let Some(jv) = variants
        .iter()
        .find(|v| matches!(v, Type::Custom(id) if id == &TypeId::native("cdt.json", "Value")))
    {
        return decode_node_to_conduit(node, jv, path, resolve_struct);
    }

    Err((
        path.to_string(),
        format!(
            "JSON {} does not match any variant of target union",
            json_kind_name(node)
        ),
    ))
}

fn json_kind_name(node: &JsonNode) -> &'static str {
    match node {
        JsonNode::Null => "null",
        JsonNode::Bool(_) => "bool",
        JsonNode::Number(_) => "number",
        JsonNode::String(_) => "string",
        JsonNode::Array(_) => "array",
        JsonNode::Object(_) => "object",
    }
}

pub fn encode_conduit_value(
    val: &Value,
    path: &str,
    out: &mut String,
    struct_field_order: &dyn Fn(&TypeId) -> Option<Vec<String>>,
) -> Result<(), (String, String)> {
    match val {
        Value::Nil => {
            out.push_str("null");
            Ok(())
        }
        Value::Bool(b) => {
            out.push_str(if *b { "true" } else { "false" });
            Ok(())
        }
        Value::Int(n) => {
            let _ = write!(out, "{}", n);
            Ok(())
        }
        Value::Float(f) => {
            if !f.is_finite() {
                return Err((
                    path.to_string(),
                    "Cannot encode non-finite Float (NaN or Infinity) to JSON".to_string(),
                ));
            }
            let formatted = f.to_string();
            out.push_str(&formatted);
            if !formatted.contains('.')
                && !formatted.contains('e')
                && !formatted.contains('E')
            {
                out.push_str(".0");
            }
            Ok(())
        }
        Value::String(s) => {
            write_json_escaped_string(s, out);
            Ok(())
        }
        Value::List(items) | Value::Tuple(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let item_path = join_index_path(path, i);
                encode_conduit_value(item, &item_path, out, struct_field_order)?;
            }
            out.push(']');
            Ok(())
        }
        Value::Map(map) => {
            out.push('{');
            for (i, (k, v)) in map.entries().iter().enumerate() {
                let key_str = match k {
                    MapKey::String(s) => s,
                    _ => {
                        return Err((
                            join_key_path(path, k),
                            "JSON objects require String map keys".to_string(),
                        ));
                    }
                };
                if i > 0 {
                    out.push(',');
                }
                write_json_escaped_string(key_str, out);
                out.push(':');
                let entry_path = join_key_path(path, k);
                encode_conduit_value(v, &entry_path, out, struct_field_order)?;
            }
            out.push('}');
            Ok(())
        }
        Value::Struct { type_id, fields } => {
            out.push('{');
            let ordered_names: Vec<String> = if let Some(decl_order) = struct_field_order(type_id) {
                decl_order
            } else {
                let mut names: Vec<String> = fields.keys().cloned().collect();
                names.sort();
                names
            };
            let mut first = true;
            for name in ordered_names {
                if let Some(fval) = fields.get(&name) {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_json_escaped_string(&name, out);
                    out.push(':');
                    let field_path = join_field_path(path, &name);
                    encode_conduit_value(fval, &field_path, out, struct_field_order)?;
                }
            }
            out.push('}');
            Ok(())
        }
        Value::Native(nat) => {
            if let Some(jv) = nat.as_any().downcast_ref::<JsonValue>() {
                write_json_node(&jv.0, out);
                Ok(())
            } else {
                Err((
                    path.to_string(),
                    format!("Value of native type '{}' is not JSON-serializable", nat.type_name()),
                ))
            }
        }
        other => Err((
            path.to_string(),
            format!("Value of type '{}' is not JSON-serializable", other.type_name()),
        )),
    }
}

fn wrap_json_value(node: JsonNode) -> Value {
    Value::Native(Arc::new(JsonValue(node)))
}

fn with_json_value<F>(receiver: &Arc<dyn NativeValue>, f: F) -> Result<Value, Value>
where
    F: FnOnce(&JsonNode) -> Value,
{
    if let Some(jv) = receiver.as_any().downcast_ref::<JsonValue>() {
        Ok(f(&jv.0))
    } else {
        Ok(Value::Nil)
    }
}

pub fn create_json_module() -> NativeModule {
    let mut module = NativeModule::new(
        "cdt.json",
        "json",
        "Standard library for typed JSON decoding, encoding, and dynamic JSON trees",
    );

    let json_val_ty = Type::Custom(TypeId::native("cdt.json", "Value"));
    let decode_err_ty = Type::Custom(TypeId::native("cdt.json", "DecodeError"));
    let encode_err_ty = Type::Custom(TypeId::native("cdt.json", "EncodeError"));

    module.add_type(NativeTypeDef {
        name: "Value",
        qualified_name: "json.Value",
        doc: "Dynamic JSON node representing null, bool, number, string, array, or object",
        is_error: false,
        fields: vec![],
    });

    module.add_type(NativeTypeDef {
        name: "DecodeError",
        qualified_name: "json.DecodeError",
        doc: "Error returned when parsing or typed decoding of JSON fails",
        is_error: true,
        fields: vec![("path", Type::String), ("message", Type::String)],
    });

    module.add_type(NativeTypeDef {
        name: "EncodeError",
        qualified_name: "json.EncodeError",
        doc: "Error returned when encoding a value to JSON fails",
        is_error: true,
        fields: vec![("path", Type::String), ("message", Type::String)],
    });

    module.add_function(NativeFuncDef {
        name: "parse",
        doc: "Parses a JSON string into a dynamic json.Value tree",
        params: vec![("raw", Type::String)],
        return_type: Type::union(vec![json_val_ty.clone(), decode_err_ty.clone()]),
        is_validator: false,
        implementation: |args| {
            let raw = match args.into_iter().next() {
                Some(Value::String(s)) => s,
                _ => return Ok(make_decode_error("", "Expected String argument for json.parse")),
            };
            match parse_json(&raw) {
                Ok(node) => Ok(wrap_json_value(node)),
                Err((path, msg)) => Ok(make_decode_error(path, msg)),
            }
        },
    });

    module.add_function(NativeFuncDef {
        name: "stringify",
        doc: "Serializes a dynamic json.Value into a compact JSON string",
        params: vec![("val", json_val_ty.clone())],
        return_type: Type::String,
        is_validator: false,
        implementation: |args| {
            if let Some(Value::Native(nat)) = args.into_iter().next() {
                if let Some(jv) = nat.as_any().downcast_ref::<JsonValue>() {
                    return Ok(Value::String(stringify_json_node(&jv.0)));
                }
            }
            Ok(Value::String("null".to_string()))
        },
    });

    module.add_function(NativeFuncDef {
        name: "decode",
        doc: "Decodes a JSON string into target schema type T",
        params: vec![("raw", Type::String)],
        return_type: Type::union(vec![Type::Any, decode_err_ty]),
        is_validator: false,
        implementation: |_args| Ok(make_decode_error("", "json.decode requires a target type argument")),
    });

    module.add_function(NativeFuncDef {
        name: "encode",
        doc: "Serializes a Conduit value into a deterministic JSON string",
        params: vec![("value", Type::Any)],
        return_type: Type::union(vec![Type::String, encode_err_ty]),
        is_validator: false,
        implementation: |args| {
            let val = args.into_iter().next().unwrap_or(Value::Nil);
            let mut out = String::new();
            match encode_conduit_value(&val, "", &mut out, &|_| None) {
                Ok(()) => Ok(Value::String(out)),
                Err((path, msg)) => Ok(make_encode_error(path, msg)),
            }
        },
    });

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "is_null",
            doc: "Returns true if this JSON value is null",
            params: vec![],
            return_type: Type::Bool,
            implementation: |recv, _| {
                with_json_value(recv, |node| Value::Bool(matches!(node, JsonNode::Null)))
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_string",
            doc: "Extracts a String if this node is a JSON string, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::String)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::String(s) => Value::String(s.clone()),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_int",
            doc: "Extracts an Int if this node is an exact JSON integer fitting i64, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Int)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Number(n) if n.is_int_syntax => {
                        n.int_val.map(Value::Int).unwrap_or(Value::Nil)
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_float",
            doc: "Extracts a Float if this node is a JSON number, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Float)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Number(n) => Value::Float(n.float_val),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_bool",
            doc: "Extracts a Bool if this node is a JSON boolean, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Bool)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Bool(b) => Value::Bool(*b),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_list",
            doc: "Extracts a list of json.Value elements if this node is a JSON array, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::List(Box::new(json_val_ty.clone())))),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Array(items) => {
                        Value::List(items.iter().cloned().map(wrap_json_value).collect())
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_map",
            doc: "Extracts an ordered Map<String, json.Value> if this node is a JSON object, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Map(
                Box::new(Type::String),
                Box::new(json_val_ty),
            ))),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Object(entries) => {
                        let mut map = OrderedMap::new();
                        for (k, v) in entries {
                            map.set(MapKey::String(k.clone()), wrap_json_value(v.clone()));
                        }
                        Value::Map(map)
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module
}

