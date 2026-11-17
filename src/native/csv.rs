use std::ops::Range;
use std::sync::Arc;

use crate::vm::string::VmString;
use crate::vm::value::{ListStorage, MapKey, MapVal, Obj, OrderedMap};

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

enum Cell {
    Slice(Range<usize>),
    Owned(String),
}

impl Cell {
    #[inline]
    fn into_vm_string(self, base: &Arc<VmString>) -> VmString {
        match self {
            Cell::Slice(r) => VmString::slice_arc(base, r),
            Cell::Owned(s) => VmString::from(s),
        }
    }

    #[inline]
    fn into_obj(self, base: &Arc<VmString>) -> Obj {
        Obj::Str(Arc::new(self.into_vm_string(base)))
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

fn parse_csv_cells_ascii(input: &str, delim: u8) -> Result<Vec<Vec<Cell>>, CsvError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;

    loop {
        if i >= len {
            break;
        }
        // Parse one row
        loop {
            if i < len && bytes[i] == b'"' {
                // Quoted field
                i += 1;
                let q_start = i;
                let mut buf: Option<String> = None;
                let mut chunk_start = i;
                loop {
                    while i < len && bytes[i] != b'"' && bytes[i] != b'\n' {
                        i += 1;
                    }
                    if i >= len {
                        return Err(CsvError::new(
                            "InvalidCsv",
                            line,
                            "Unterminated quoted field in CSV",
                        ));
                    }
                    if bytes[i] == b'\n' {
                        line += 1;
                        i += 1;
                        continue;
                    }
                    // bytes[i] == b'"'
                    if i + 1 < len && bytes[i + 1] == b'"' {
                        let s = buf.get_or_insert_with(String::new);
                        s.push_str(&input[chunk_start..i]);
                        s.push('"');
                        i += 2;
                        chunk_start = i;
                    } else {
                        let cell = match buf {
                            Some(mut s) => {
                                s.push_str(&input[chunk_start..i]);
                                Cell::Owned(s)
                            }
                            None => Cell::Slice(q_start..i),
                        };
                        i += 1; // skip closing quote
                        // Skip trailing unquoted chars up to delimiter/newline (if any)
                        if i < len && bytes[i] != delim && bytes[i] != b'\r' && bytes[i] != b'\n' {
                            let mut s = match cell {
                                Cell::Slice(r) => input[r].to_owned(),
                                Cell::Owned(s) => s,
                            };
                            let extra_start = i;
                            while i < len && bytes[i] != delim && bytes[i] != b'\r' && bytes[i] != b'\n' {
                                if bytes[i] == b'"' {
                                    return Err(CsvError::new(
                                        "InvalidCsv",
                                        line,
                                        "Unexpected quote in unquoted CSV field",
                                    ));
                                }
                                i += 1;
                            }
                            s.push_str(&input[extra_start..i]);
                            row.push(Cell::Owned(s));
                        } else {
                            row.push(cell);
                        }
                        break;
                    }
                }
            } else {
                // Unquoted field
                let start = i;
                while i < len {
                    let b = bytes[i];
                    if b == delim || b == b'\r' || b == b'\n' {
                        break;
                    }
                    if b == b'"' {
                        return Err(CsvError::new(
                            "InvalidCsv",
                            line,
                            "Unexpected quote in unquoted CSV field",
                        ));
                    }
                    i += 1;
                }
                row.push(Cell::Slice(start..i));
            }

            if i >= len {
                rows.push(std::mem::take(&mut row));
                break;
            }
            if bytes[i] == delim {
                i += 1;
                if i == len {
                    row.push(Cell::Slice(i..i));
                    rows.push(std::mem::take(&mut row));
                    break;
                }
                continue;
            }
            if bytes[i] == b'\r' {
                i += 1;
                if i < len && bytes[i] == b'\n' {
                    i += 1;
                }
                rows.push(std::mem::take(&mut row));
                line += 1;
                break;
            }
            if bytes[i] == b'\n' {
                i += 1;
                rows.push(std::mem::take(&mut row));
                line += 1;
                break;
            }
        }
    }

    Ok(rows)
}

fn parse_csv_cells_unicode(input: &str, delim: char) -> Result<Vec<Vec<Cell>>, CsvError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
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
            row.push(Cell::Owned(std::mem::take(&mut field)));
            at_field_start = true;
        } else if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(Cell::Owned(std::mem::take(&mut field)));
            rows.push(std::mem::take(&mut row));
            line += 1;
            at_field_start = true;
        } else if ch == '\n' {
            row.push(Cell::Owned(std::mem::take(&mut field)));
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
        row.push(Cell::Owned(field));
        rows.push(row);
    }
    Ok(rows)
}

pub fn parse_csv_objs(base: &Arc<VmString>, delimiter: &str) -> Result<Obj, CsvError> {
    let delim = validate_delimiter(delimiter)?;
    let input = base.as_str();
    let cells = if delim.is_ascii() {
        parse_csv_cells_ascii(input, delim as u8)?
    } else {
        parse_csv_cells_unicode(input, delim)?
    };
    let row_objs: Vec<Obj> = cells
        .into_iter()
        .map(|r| {
            let objs: Vec<Obj> = r.into_iter().map(|c| c.into_obj(base)).collect();
            Obj::List(ListStorage::Obj(Arc::new(objs)))
        })
        .collect();
    Ok(Obj::List(ListStorage::Obj(Arc::new(row_objs))))
}

pub fn parse_csv_records_objs(base: &Arc<VmString>) -> Result<Obj, CsvError> {
    let rows = parse_csv_cells_ascii(base.as_str(), b',')?;
    let mut it = rows.into_iter();
    let Some(header_cells) = it.next() else {
        return Ok(Obj::List(ListStorage::Obj(Arc::new(Vec::new()))));
    };
    let headers: Vec<Arc<VmString>> = header_cells
        .into_iter()
        .map(|c| Arc::new(c.into_vm_string(base)))
        .collect();
    let mut records = Vec::new();
    for (idx, row) in it.enumerate() {
        let line = idx + 2;
        if row.len() > headers.len() {
            return Err(CsvError::new(
                "ColumnMismatch",
                line,
                format!("Row has {} columns, expected at most {}", row.len(), headers.len()),
            ));
        }
        let mut om = OrderedMap::default();
        let mut row_it = row.into_iter();
        for key in &headers {
            let val = match row_it.next() {
                Some(cell) => Obj::Str(Arc::new(cell.into_vm_string(base))),
                None => Obj::str(""),
            };
            om.set(MapKey::Str(Arc::clone(key)), MapVal::Obj(val));
        }
        records.push(Obj::Map(Arc::new(om)));
    }
    Ok(Obj::List(ListStorage::Obj(Arc::new(records))))
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
