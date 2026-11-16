#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    pub kind: String,
    pub line: i64,
    pub message: String,
}

impl CsvError {
    fn new(kind: &str, line: usize, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            line: line as i64,
            message: message.into(),
        }
    }
}

fn validate_delimiter(delim: &str) -> Result<char, CsvError> {
    let mut chars = delim.chars();
    let Some(ch) = chars.next() else {
        return Err(CsvError::new("InvalidDelimiter", 0, "CSV delimiter must not be empty"));
    };
    if chars.next().is_some() || ch == '"' || ch == '\r' || ch == '\n' {
        return Err(CsvError::new(
            "InvalidDelimiter",
            0,
            "CSV delimiter must be a single character (not quote or newline)",
        ));
    }
    Ok(ch)
}

pub fn parse_csv(input: &str, delimiter: &str) -> Result<Vec<Vec<String>>, CsvError> {
    let delim = validate_delimiter(delimiter)?;
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut at_field_start = true;
    let mut line: usize = 1;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                if ch == '\n' {
                    line += 1;
                }
                field.push(ch);
            }
        } else if ch == '"' && at_field_start {
            in_quotes = true;
            at_field_start = false;
        } else if ch == '"' {
            return Err(CsvError::new(
                "InvalidCsv",
                line,
                "Unexpected quote in unquoted CSV field",
            ));
        } else if ch == delim {
            row.push(std::mem::take(&mut field));
            at_field_start = true;
        } else if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
            line += 1;
            at_field_start = true;
        } else if ch == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
            line += 1;
            at_field_start = true;
        } else {
            field.push(ch);
            at_field_start = false;
        }
    }

    if in_quotes {
        return Err(CsvError::new(
            "InvalidCsv",
            line,
            "Unterminated quoted field in CSV",
        ));
    }
    if !at_field_start || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

pub fn parse_csv_records(input: &str) -> Result<Vec<Vec<(String, String)>>, CsvError> {
    let rows = parse_csv(input, ",")?;
    let Some((headers, data_rows)) = rows.split_first() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::with_capacity(data_rows.len());
    for (idx, row) in data_rows.iter().enumerate() {
        let line = idx + 2;
        if row.len() > headers.len() {
            return Err(CsvError::new(
                "ColumnMismatch",
                line,
                format!("Row has {} columns, expected at most {}", row.len(), headers.len()),
            ));
        }
        let mut pairs = Vec::with_capacity(headers.len());
        for (col_idx, key) in headers.iter().enumerate() {
            let val = row.get(col_idx).cloned().unwrap_or_default();
            pairs.push((key.clone(), val));
        }
        records.push(pairs);
    }
    Ok(records)
}

pub fn stringify_csv(rows: &[Vec<String>], delimiter: &str) -> Result<String, CsvError> {
    let delim = validate_delimiter(delimiter)?;
    let mut out = String::new();
    for (r_idx, row) in rows.iter().enumerate() {
        if r_idx > 0 {
            out.push('\n');
        }
        for (c_idx, cell) in row.iter().enumerate() {
            if c_idx > 0 {
                out.push(delim);
            }
            let needs_quote =
                cell.contains(delim) || cell.contains('"') || cell.contains('\n') || cell.contains('\r');
            if needs_quote {
                out.push('"');
                for ch in cell.chars() {
                    if ch == '"' {
                        out.push_str("\"\"");
                    } else {
                        out.push(ch);
                    }
                }
                out.push('"');
            } else {
                out.push_str(cell);
            }
        }
    }
    Ok(out)
}
