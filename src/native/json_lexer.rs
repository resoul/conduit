use std::borrow::Cow;
use std::sync::Arc;

use crate::vm::value::Obj;

use crate::native::json_decode::JsonDecoder;
use crate::native::{
    JsonError, MAX_JSON_DEPTH, err_at_field, err_at_index, join_field_path, join_index_path,
};

static POW10: [f64; 11] = [
    1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10,
];

impl<'a> JsonDecoder<'a> {
    #[inline]
    pub(crate) fn parse_string_obj(&mut self, depth: usize) -> Result<Obj, JsonError> {
        self.skip_ws();
        if self.peek() != Some(b'"') {
            return Err(self.wrong_kind("String", depth));
        }
        Ok(match self.parse_string_cow()? {
            Cow::Borrowed(s) => Obj::Str(Arc::new(s.into())),
            Cow::Owned(s) => Obj::str(s),
        })
    }

    #[inline]
    pub(crate) fn parse_string_cow(&mut self) -> Result<Cow<'a, str>, JsonError> {
        self.pos += 1;
        let start = self.pos;
        let len = self.bytes.len();
        let mut i = start;
        while i < len {
            let b = self.bytes[i];
            if b == b'"' {
                self.pos = i + 1;
                return Ok(Cow::Borrowed(&self.src[start..i]));
            }
            if b == b'\\' || b < 0x20 {
                return self.parse_string_escaped(start, i).map(Cow::Owned);
            }
            i += 1;
        }
        self.pos = len;
        Err(JsonError::root("Unterminated JSON string"))
    }

    #[cold]
    #[inline(never)]
    fn parse_string_escaped(
        &mut self,
        start: usize,
        first_special: usize,
    ) -> Result<String, JsonError> {
        self.pos = first_special;
        if self.bytes[self.pos] < 0x20 {
            return Err(JsonError::root("Unescaped control character in JSON string"));
        }
        let mut output = String::with_capacity(self.pos - start + 16);
        output.push_str(&self.src[start..self.pos]);
        let mut chunk_start = self.pos;

        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    let end = self.pos;
                    self.pos += 1;
                    output.push_str(&self.src[chunk_start..end]);
                    return Ok(output);
                }
                b'\\' => {
                    output.push_str(&self.src[chunk_start..self.pos]);
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| {
                        JsonError::root("Unterminated escape sequence in JSON string")
                    })?;
                    self.pos += 1;
                    match esc {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000C}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => self.push_unicode_escape(&mut output)?,
                        _ => {
                            let ch = self.src[self.pos - 1..].chars().next().unwrap_or('?');
                            return Err(JsonError::root(format!(
                                "Invalid JSON escape sequence '\\{}'",
                                ch
                            )));
                        }
                    }
                    chunk_start = self.pos;
                }
                byte if byte < 0x20 => {
                    return Err(JsonError::root("Unescaped control character in JSON string"));
                }
                _ => self.pos += 1,
            }
        }
        Err(JsonError::root("Unterminated JSON string"))
    }

    fn push_unicode_escape(&mut self, output: &mut String) -> Result<(), JsonError> {
        let code = self.parse_hex4()?;
        if (0xD800..=0xDBFF).contains(&code) {
            if self.bytes[self.pos..].starts_with(b"\\u") {
                self.pos += 2;
                let low = self.parse_hex4()?;
                if (0xDC00..=0xDFFF).contains(&low) {
                    let combined =
                        0x10000 + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                    let ch = char::from_u32(combined)
                        .ok_or_else(|| JsonError::root("Invalid Unicode surrogate pair"))?;
                    output.push(ch);
                    Ok(())
                } else {
                    Err(JsonError::root("Invalid low Unicode surrogate"))
                }
            } else {
                Err(JsonError::root("Unpaired high Unicode surrogate"))
            }
        } else if (0xDC00..=0xDFFF).contains(&code) {
            Err(JsonError::root("Unexpected low Unicode surrogate"))
        } else {
            let ch = char::from_u32(code as u32)
                .ok_or_else(|| JsonError::root("Invalid Unicode scalar value"))?;
            output.push(ch);
            Ok(())
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(JsonError::root("Incomplete \\uXXXX escape in JSON string"));
        }
        let mut value = 0u16;
        for &byte in &self.bytes[self.pos..self.pos + 4] {
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(JsonError::root("Invalid \\uXXXX hex sequence")),
            };
            value = (value << 4) | u16::from(digit);
        }
        self.pos += 4;
        Ok(value)
    }

    #[inline]
    pub(crate) fn parse_bool(&mut self, depth: usize) -> Result<bool, JsonError> {
        self.skip_ws();
        let rest = &self.bytes[self.pos..];
        if rest.starts_with(b"true") {
            self.pos += 4;
            Ok(true)
        } else if rest.starts_with(b"false") {
            self.pos += 5;
            Ok(false)
        } else {
            Err(self.wrong_kind("Bool", depth))
        }
    }

    #[inline]
    pub(crate) fn parse_i64(&mut self, depth: usize) -> Result<i64, JsonError> {
        self.skip_ws();
        let Some(first) = self.peek() else {
            return Err(JsonError::root("Unexpected end of JSON input"));
        };
        if !matches!(first, b'-' | b'0'..=b'9') {
            return Err(self.wrong_kind("Int", depth));
        }
        let start = self.pos;
        let neg = first == b'-';
        if neg {
            self.pos += 1;
        }
        let mut val: u64 = 0;
        let mut digits = 0usize;
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(JsonError::root(
                        "Leading zeros are not allowed in JSON numbers",
                    ));
                }
            }
            Some(b'1'..=b'9') => {
                while let Some(b) = self.peek() {
                    if !b.is_ascii_digit() {
                        break;
                    }
                    if digits < 18 {
                        val = val * 10 + u64::from(b - b'0');
                    }
                    digits += 1;
                    self.pos += 1;
                }
            }
            _ => return Err(JsonError::root("Invalid JSON number")),
        }

        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return self.parse_fractional_for_int(start);
        }
        if digits <= 18 {
            return Ok(if neg { -(val as i64) } else { val as i64 });
        }
        let raw = &self.src[start..self.pos];
        if let Ok(parsed) = raw.parse::<i64>() {
            Ok(parsed)
        } else if !raw.parse::<f64>().is_ok_and(f64::is_finite) {
            Err(JsonError::root(format!(
                "JSON number '{}' overflows finite IEEE 754 range",
                raw
            )))
        } else {
            Err(JsonError::root(format!(
                "Integer value '{}' overflows 64-bit Int",
                raw
            )))
        }
    }

    #[cold]
    #[inline(never)]
    fn parse_fractional_for_int(&mut self, start: usize) -> Result<i64, JsonError> {
        self.scan_fraction_and_exp()?;
        let raw = &self.src[start..self.pos];
        if !raw.parse::<f64>().is_ok_and(f64::is_finite) {
            return Err(JsonError::root(format!(
                "JSON number '{}' overflows finite IEEE 754 range",
                raw
            )));
        }
        Err(JsonError::root(format!(
            "Expected Int, got fractional or exponential number '{}'",
            raw
        )))
    }

    #[inline]
    pub(crate) fn parse_f64(&mut self, depth: usize) -> Result<f64, JsonError> {
        self.skip_ws();
        let Some(first) = self.peek() else {
            return Err(JsonError::root("Unexpected end of JSON input"));
        };
        if !matches!(first, b'-' | b'0'..=b'9') {
            return Err(self.wrong_kind("Float", depth));
        }
        let start = self.pos;
        let neg = first == b'-';
        if neg {
            self.pos += 1;
        }

        let mut mantissa: u64 = 0;
        let mut total_digits = 0usize;
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(JsonError::root(
                        "Leading zeros are not allowed in JSON numbers",
                    ));
                }
            }
            Some(b'1'..=b'9') => {
                while let Some(b) = self.peek() {
                    if !b.is_ascii_digit() {
                        break;
                    }
                    total_digits += 1;
                    if total_digits <= 15 {
                        mantissa = mantissa * 10 + u64::from(b - b'0');
                    }
                    self.pos += 1;
                }
            }
            _ => return Err(JsonError::root("Invalid JSON number")),
        }

        let mut frac_digits = 0usize;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::root(
                    "Expected digit after decimal point in JSON number",
                ));
            }
            while let Some(b) = self.peek() {
                if !b.is_ascii_digit() {
                    break;
                }
                frac_digits += 1;
                total_digits += 1;
                if total_digits <= 15 {
                    mantissa = mantissa * 10 + u64::from(b - b'0');
                }
                self.pos += 1;
            }
        }

        let mut has_exp = false;
        if matches!(self.peek(), Some(b'e' | b'E')) {
            has_exp = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::root("Expected exponent digits in JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        if !has_exp && total_digits <= 15 && frac_digits <= 10 {
            let abs_val = if frac_digits == 0 {
                mantissa as f64
            } else {
                (mantissa as f64) / POW10[frac_digits]
            };
            return Ok(if neg { -abs_val } else { abs_val });
        }

        let raw = &self.src[start..self.pos];
        let val = raw
            .parse::<f64>()
            .map_err(|_| JsonError::root(format!("Invalid JSON float number '{}'", raw)))?;
        if !val.is_finite() {
            return Err(JsonError::root(format!(
                "JSON number '{}' overflows finite IEEE 754 range",
                raw
            )));
        }
        Ok(val)
    }

    #[cold]
    #[inline(never)]
    fn scan_fraction_and_exp(&mut self) -> Result<(), JsonError> {
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::root(
                    "Expected digit after decimal point in JSON number",
                ));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::root("Expected exponent digits in JSON number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn wrong_kind(&mut self, expected: &str, depth: usize) -> JsonError {
        match self.skip_value(depth) {
            Ok(kind) => JsonError::root(format!("Expected {}, got {}", expected, kind)),
            Err(syntax_err) => syntax_err,
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn skip_value(&mut self, depth: usize) -> Result<&'static str, JsonError> {
        self.skip_ws();
        let Some(byte) = self.peek() else {
            return Err(JsonError::root("Unexpected end of JSON input"));
        };
        if matches!(byte, b'[' | b'{') && depth >= MAX_JSON_DEPTH {
            return Err(JsonError::root("Maximum JSON nesting depth (128) exceeded"));
        }
        match byte {
            b'n' => self.consume_literal(b"null", "null"),
            b't' => self.consume_literal(b"true", "bool"),
            b'f' => self.consume_literal(b"false", "bool"),
            b'"' => {
                let _ = self.parse_string_cow()?;
                Ok("string")
            }
            b'[' => self.skip_array(depth),
            b'{' => self.skip_object(depth),
            b'-' | b'0'..=b'9' => {
                let _ = self.parse_f64(depth)?;
                Ok("number")
            }
            other => Err(JsonError::root(format!(
                "Unexpected character '{}' in JSON input",
                other as char
            ))),
        }
    }

    fn consume_literal(
        &mut self,
        literal: &[u8],
        kind: &'static str,
    ) -> Result<&'static str, JsonError> {
        if self.bytes[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            Ok(kind)
        } else {
            Err(JsonError::root(format!(
                "Invalid JSON literal (expected '{}')",
                std::str::from_utf8(literal).unwrap_or("")
            )))
        }
    }

    fn skip_array(&mut self, depth: usize) -> Result<&'static str, JsonError> {
        self.pos += 1;
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok("array");
        }
        let mut idx = 0usize;
        loop {
            self.skip_value(depth + 1)
                .map_err(|e| err_at_index(idx, e))?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    idx += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok("array");
                }
                _ => {
                    return Err(JsonError::new(
                        join_index_path("", idx),
                        "Expected ',' or ']' in JSON array",
                    ));
                }
            }
        }
    }

    fn skip_object(&mut self, depth: usize) -> Result<&'static str, JsonError> {
        self.pos += 1;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok("object");
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(JsonError::root("Expected string key in JSON object"));
            }
            let key = self.parse_string_cow()?.into_owned();
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(JsonError::new(
                    join_field_path("", &key),
                    "Expected ':' after JSON object key",
                ));
            }
            self.pos += 1;
            self.skip_value(depth + 1)
                .map_err(|e| err_at_field(&key, e))?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok("object");
                }
                _ => {
                    return Err(JsonError::new(
                        join_field_path("", &key),
                        "Expected ',' or '}' in JSON object",
                    ));
                }
            }
        }
    }
}
