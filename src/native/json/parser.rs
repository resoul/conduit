use std::borrow::Cow;

use super::{
    join_field_path, join_index_path, prepend_error_path, JsonNode, JsonNumber, MAX_JSON_DEPTH,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BorrowedJsonNumber<'a> {
    pub raw: &'a str,
    pub int_val: Option<i64>,
    pub float_val: f64,
    pub is_int_syntax: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum BorrowedJsonNode<'a> {
    Null,
    Bool(bool),
    Number(BorrowedJsonNumber<'a>),
    String(Cow<'a, str>),
    Array(Vec<BorrowedJsonNode<'a>>),
    Object(Vec<(Cow<'a, str>, BorrowedJsonNode<'a>)>),
}

impl<'a> BorrowedJsonNode<'a> {
    pub(super) fn into_owned(self) -> JsonNode {
        match self {
            Self::Null => JsonNode::Null,
            Self::Bool(value) => JsonNode::Bool(value),
            Self::Number(number) => JsonNode::Number(JsonNumber {
                raw: number.raw.to_owned(),
                int_val: number.int_val,
                float_val: number.float_val,
                is_int_syntax: number.is_int_syntax,
            }),
            Self::String(value) => JsonNode::String(value.into_owned()),
            Self::Array(items) => {
                JsonNode::Array(items.into_iter().map(Self::into_owned).collect())
            }
            Self::Object(entries) => JsonNode::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect(),
            ),
        }
    }

    #[cfg(test)]
    pub(super) fn from_owned_ref(node: &'a JsonNode) -> Self {
        match node {
            JsonNode::Null => Self::Null,
            JsonNode::Bool(value) => Self::Bool(*value),
            JsonNode::Number(number) => Self::Number(BorrowedJsonNumber {
                raw: &number.raw,
                int_val: number.int_val,
                float_val: number.float_val,
                is_int_syntax: number.is_int_syntax,
            }),
            JsonNode::String(value) => Self::String(Cow::Borrowed(value)),
            JsonNode::Array(items) => Self::Array(items.iter().map(Self::from_owned_ref).collect()),
            JsonNode::Object(entries) => Self::Object(
                entries
                    .iter()
                    .map(|(key, value)| (Cow::Borrowed(key.as_str()), Self::from_owned_ref(value)))
                    .collect(),
            ),
        }
    }
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
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn err(message: impl Into<String>) -> (String, String) {
        (String::new(), message.into())
    }

    fn parse_root(&mut self) -> Result<BorrowedJsonNode<'a>, (String, String)> {
        self.skip_ws();
        let node = self.parse_value(0)?;
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(Self::err("Unexpected trailing characters after JSON value"));
        }
        Ok(node)
    }

    fn parse_value(&mut self, depth: usize) -> Result<BorrowedJsonNode<'a>, (String, String)> {
        self.skip_ws();
        let byte = self
            .peek()
            .ok_or_else(|| Self::err("Unexpected end of JSON input"))?;
        if matches!(byte, b'[' | b'{') && depth >= MAX_JSON_DEPTH {
            return Err(Self::err("Maximum JSON nesting depth (128) exceeded"));
        }
        match byte {
            b'n' => self.parse_literal(b"null", BorrowedJsonNode::Null),
            b't' => self.parse_literal(b"true", BorrowedJsonNode::Bool(true)),
            b'f' => self.parse_literal(b"false", BorrowedJsonNode::Bool(false)),
            b'"' => self.parse_string().map(BorrowedJsonNode::String),
            b'[' => self.parse_array(depth),
            b'{' => self.parse_object(depth),
            b'-' | b'0'..=b'9' => self.parse_number().map(BorrowedJsonNode::Number),
            other => Err(Self::err(format!(
                "Unexpected character '{}' in JSON input",
                other as char
            ))),
        }
    }

    fn parse_literal(
        &mut self,
        literal: &[u8],
        node: BorrowedJsonNode<'a>,
    ) -> Result<BorrowedJsonNode<'a>, (String, String)> {
        if self.bytes[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            Ok(node)
        } else {
            Err(Self::err(format!(
                "Invalid JSON literal (expected '{}')",
                std::str::from_utf8(literal).unwrap_or("")
            )))
        }
    }

    fn parse_string(&mut self) -> Result<Cow<'a, str>, (String, String)> {
        self.pos += 1;
        let start = self.pos;
        let mut chunk_start = start;
        let mut output: Option<String> = None;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    let end = self.pos;
                    self.pos += 1;
                    return Ok(match output {
                        Some(mut output) => {
                            output.push_str(&self.src[chunk_start..end]);
                            Cow::Owned(output)
                        }
                        None => Cow::Borrowed(&self.src[start..end]),
                    });
                }
                b'\\' => {
                    let output =
                        output.get_or_insert_with(|| String::with_capacity(self.pos - start + 16));
                    output.push_str(&self.src[chunk_start..self.pos]);
                    self.pos += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| Self::err("Unterminated escape sequence in JSON string"))?;
                    self.pos += 1;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000C}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if self.bytes[self.pos..].starts_with(b"\\u") {
                                    self.pos += 2;
                                    let low = self.parse_hex4()?;
                                    if (0xDC00..=0xDFFF).contains(&low) {
                                        let combined = 0x10000
                                            + (((code as u32 - 0xD800) << 10)
                                                | (low as u32 - 0xDC00));
                                        let ch = char::from_u32(combined).ok_or_else(|| {
                                            Self::err("Invalid Unicode surrogate pair")
                                        })?;
                                        output.push(ch);
                                    } else {
                                        return Err(Self::err("Invalid low Unicode surrogate"));
                                    }
                                } else {
                                    return Err(Self::err("Unpaired high Unicode surrogate"));
                                }
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                return Err(Self::err("Unexpected low Unicode surrogate"));
                            } else {
                                let ch = char::from_u32(code as u32)
                                    .ok_or_else(|| Self::err("Invalid Unicode scalar value"))?;
                                output.push(ch);
                            }
                        }
                        other => {
                            return Err(Self::err(format!(
                                "Invalid JSON escape sequence '\\{}'",
                                other as char
                            )));
                        }
                    }
                    chunk_start = self.pos;
                }
                byte if byte < 0x20 => {
                    return Err(Self::err("Unescaped control character in JSON string"));
                }
                _ => self.pos += 1,
            }
        }
        Err(Self::err("Unterminated JSON string"))
    }

    fn parse_hex4(&mut self) -> Result<u16, (String, String)> {
        if self.pos + 4 > self.bytes.len() {
            return Err(Self::err("Incomplete \\uXXXX escape in JSON string"));
        }
        let mut value = 0u16;
        for byte in &self.bytes[self.pos..self.pos + 4] {
            let digit = match *byte {
                b'0'..=b'9' => *byte - b'0',
                b'a'..=b'f' => *byte - b'a' + 10,
                b'A'..=b'F' => *byte - b'A' + 10,
                _ => return Err(Self::err("Invalid \\uXXXX hex sequence")),
            };
            value = (value << 4) | u16::from(digit);
        }
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<BorrowedJsonNumber<'a>, (String, String)> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(Self::err("Leading zeros are not allowed in JSON numbers"));
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(Self::err("Invalid JSON number")),
        }

        let mut is_int_syntax = true;
        if self.peek() == Some(b'.') {
            is_int_syntax = false;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(Self::err(
                    "Expected digit after decimal point in JSON number",
                ));
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
                return Err(Self::err("Expected exponent digits in JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let raw = &self.src[start..self.pos];
        let int_val = if is_int_syntax {
            raw.parse::<i64>().ok()
        } else {
            None
        };
        let float_val = match int_val {
            Some(0) if raw.starts_with('-') => -0.0,
            Some(value) => value as f64,
            None => raw
                .parse::<f64>()
                .map_err(|_| Self::err(format!("Invalid JSON float number '{}'", raw)))?,
        };
        if !float_val.is_finite() {
            return Err(Self::err(format!(
                "JSON number '{}' overflows finite IEEE 754 range",
                raw
            )));
        }
        Ok(BorrowedJsonNumber {
            raw,
            int_val,
            float_val,
            is_int_syntax,
        })
    }

    fn parse_array(&mut self, depth: usize) -> Result<BorrowedJsonNode<'a>, (String, String)> {
        self.pos += 1;
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(BorrowedJsonNode::Array(items));
        }
        loop {
            let index = items.len();
            let item = self.parse_value(depth + 1).map_err(|(path, message)| {
                (
                    prepend_error_path(join_index_path("", index), &path),
                    message,
                )
            })?;
            items.push(item);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(BorrowedJsonNode::Array(items));
                }
                _ => {
                    return Err((
                        join_index_path("", index),
                        "Expected ',' or ']' in JSON array".to_owned(),
                    ));
                }
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<BorrowedJsonNode<'a>, (String, String)> {
        self.pos += 1;
        self.skip_ws();
        let mut entries = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(BorrowedJsonNode::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(Self::err("Expected string key in JSON object"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err((
                    join_field_path("", &key),
                    "Expected ':' after JSON object key".to_owned(),
                ));
            }
            self.pos += 1;
            let value = self.parse_value(depth + 1).map_err(|(path, message)| {
                (
                    prepend_error_path(join_field_path("", &key), &path),
                    message,
                )
            })?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(BorrowedJsonNode::Object(entries));
                }
                _ => {
                    return Err((
                        join_field_path("", &entries.last().unwrap().0),
                        "Expected ',' or '}' in JSON object".to_owned(),
                    ));
                }
            }
        }
    }
}

pub(super) fn parse_borrowed_json(raw: &str) -> Result<BorrowedJsonNode<'_>, (String, String)> {
    JsonParser::new(raw).parse_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strings_keys_and_numbers_borrow_the_input() {
        let raw = r#"{"plain":"héllo 世界","number":42,"empty":""}"#;
        let BorrowedJsonNode::Object(entries) = parse_borrowed_json(raw).unwrap() else {
            panic!("expected object");
        };
        assert!(entries
            .iter()
            .all(|(key, _)| matches!(key, Cow::Borrowed(_))));
        let BorrowedJsonNode::String(Cow::Borrowed(value)) = &entries[0].1 else {
            panic!("plain strings must borrow");
        };
        assert_eq!(*value, "héllo 世界");
        assert_eq!(value.as_ptr(), raw[10..].as_ptr());
        let BorrowedJsonNode::Number(number) = &entries[1].1 else {
            panic!("expected number");
        };
        assert_eq!(number.raw, "42");
        assert!(matches!(
            entries[2].1,
            BorrowedJsonNode::String(Cow::Borrowed(""))
        ));
    }

    #[test]
    fn escaped_strings_copy_chunks_and_decode_surrogate_pairs() {
        let raw = r#"{"na\u006de":"é\n世界\t\"\\\/\b\f\r\uD83D\uDE00終"}"#;
        let node = parse_borrowed_json(raw).unwrap();
        let BorrowedJsonNode::Object(entries) = &node else {
            panic!("expected object");
        };
        assert!(matches!(&entries[0].0, Cow::Owned(key) if key == "name"));
        assert!(matches!(
            &entries[0].1,
            BorrowedJsonNode::String(Cow::Owned(value))
                if value == "é\n世界\t\"\\/\u{0008}\u{000C}\r😀終"
        ));
        let owned = node.into_owned();
        assert_eq!(BorrowedJsonNode::from_owned_ref(&owned).into_owned(), owned);
    }

    #[test]
    fn malformed_strings_keep_diagnostics_at_utf8_boundaries() {
        for (raw, message) in [
            ("\"é\\", "Unterminated escape sequence in JSON string"),
            ("\"é", "Unterminated JSON string"),
            ("\"é\n\"", "Unescaped control character in JSON string"),
            (r#""é\u12""#, "Incomplete \\uXXXX escape in JSON string"),
            (r#""é\u12界""#, "Invalid \\uXXXX hex sequence"),
            (r#""é\uD83D終""#, "Unpaired high Unicode surrogate"),
            (r#""é\uD83D\u0041""#, "Invalid low Unicode surrogate"),
            (r#""é\uDC00""#, "Unexpected low Unicode surrogate"),
            (r#""é\界""#, "Invalid JSON escape sequence '\\ç'"),
        ] {
            assert_eq!(
                parse_borrowed_json(raw).unwrap_err(),
                (String::new(), message.to_owned()),
                "{raw}"
            );
        }
    }

    #[test]
    fn numbers_preserve_negative_zero_rounding_and_overflow_checks() {
        for raw in [
            "0",
            "-0",
            "1",
            "-1",
            "9007199254740993",
            "-9007199254740995",
            "9223372036854775807",
            "-9223372036854775808",
            "9223372036854775808",
            "-9223372036854775809",
            "1.25",
            "1e2",
            "-0.0",
            "-0e4",
        ] {
            let BorrowedJsonNode::Number(number) = parse_borrowed_json(raw).unwrap() else {
                panic!("expected number");
            };
            assert_eq!(number.raw.as_ptr(), raw.as_ptr());
            assert_eq!(
                number.float_val.to_bits(),
                raw.parse::<f64>().unwrap().to_bits(),
                "{raw}"
            );
            let is_int = !raw.contains(['.', 'e', 'E']);
            assert_eq!(number.is_int_syntax, is_int, "{raw}");
            assert_eq!(
                number.int_val,
                if is_int {
                    raw.parse::<i64>().ok()
                } else {
                    None
                },
                "{raw}"
            );
        }
        for raw in ["1e400".to_owned(), "9".repeat(400)] {
            assert_eq!(
                parse_borrowed_json(&raw).unwrap_err().1,
                format!("JSON number '{}' overflows finite IEEE 754 range", raw)
            );
        }
    }

    #[test]
    fn lazy_paths_preserve_syntax_error_locations_and_depth_limit() {
        for (raw, path, message) in [
            (
                r#"{"outer":{"a.b":!}}"#,
                "outer[\"a.b\"]",
                "Unexpected character '!' in JSON input",
            ),
            (
                r#"{"outer":[0,]}"#,
                "outer[1]",
                "Unexpected character ']' in JSON input",
            ),
            (
                r#"{"outer":[0 1]}"#,
                "outer[0]",
                "Expected ',' or ']' in JSON array",
            ),
            (
                r#"{"outer":{"a.b" 1}}"#,
                "outer[\"a.b\"]",
                "Expected ':' after JSON object key",
            ),
            (
                r#"{"outer":{"a.b":1 "next":2}}"#,
                "outer[\"a.b\"]",
                "Expected ',' or '}' in JSON object",
            ),
            (
                r#"{"outer":{"a.b":1,}}"#,
                "outer",
                "Expected string key in JSON object",
            ),
            (
                r#"{"outer":{"bad\q":1}}"#,
                "outer",
                "Invalid JSON escape sequence '\\q'",
            ),
            (
                r#"{"outer":[]} false"#,
                "",
                "Unexpected trailing characters after JSON value",
            ),
        ] {
            assert_eq!(
                parse_borrowed_json(raw).unwrap_err(),
                (path.to_owned(), message.to_owned()),
                "{raw}"
            );
        }
        let valid = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_DEPTH),
            "]".repeat(MAX_JSON_DEPTH)
        );
        assert!(parse_borrowed_json(&valid).is_ok());
        let invalid = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert_eq!(
            parse_borrowed_json(&invalid).unwrap_err(),
            (
                "[0]".repeat(MAX_JSON_DEPTH),
                "Maximum JSON nesting depth (128) exceeded".to_owned()
            )
        );
    }
}
