use std::fs;
use std::path::Path;

pub fn format_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut depth: usize = 0;
    let mut switch_stack: Vec<(usize, bool)> = Vec::new();
    let mut prev_blank = false;
    let mut in_block_comment = false;

    for raw_line in src.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            if !out.is_empty() && !prev_blank {
                out.push('\n');
                prev_blank = true;
            }
            continue;
        }
        prev_blank = false;

        let (leading_closes, opens, closes, ends_with_open_switch) =
            scan_line_braces(trimmed, &mut in_block_comment);

        let is_case_line = trimmed.starts_with("case ") && trimmed.ends_with(':');

        if let Some(&(sw_depth, in_case)) = switch_stack.last()
            && in_case
        {
            let closing_switch =
                leading_closes > 0 && depth.saturating_sub(1).saturating_sub(leading_closes) < sw_depth;
            if is_case_line || closing_switch {
                depth = depth.saturating_sub(1);
                if let Some(top) = switch_stack.last_mut() {
                    top.1 = false;
                }
                if closing_switch {
                    switch_stack.pop();
                }
            }
        }

        let indent = depth.saturating_sub(leading_closes);

        for _ in 0..indent {
            out.push_str("    ");
        }
        out.push_str(trimmed);
        out.push('\n');

        depth = depth.saturating_add(opens).saturating_sub(closes);

        if ends_with_open_switch {
            switch_stack.push((depth, false));
        } else if is_case_line
            && let Some(top) = switch_stack.last_mut()
            && !top.1
        {
            depth += 1;
            top.1 = true;
        }

        while let Some(&(sw_depth, in_case)) = switch_stack.last() {
            let effective_sw_depth = if in_case { sw_depth + 1 } else { sw_depth };
            if depth < effective_sw_depth && !is_case_line {
                if in_case {
                    depth = depth.saturating_sub(1);
                }
                switch_stack.pop();
            } else {
                break;
            }
        }
    }

    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

fn scan_line_braces(line: &str, in_block_comment: &mut bool) -> (usize, usize, usize, bool) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut leading_closes = 0usize;
    let mut seen_non_close = false;
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut in_string = false;

    while i < bytes.len() {
        if *in_block_comment {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                *in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            break;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            *in_block_comment = true;
            seen_non_close = true;
            i += 2;
            continue;
        }
        match bytes[i] {
            b'"' => {
                in_string = true;
                seen_non_close = true;
            }
            b'{' => {
                opens += 1;
                seen_non_close = true;
            }
            b'}' => {
                closes += 1;
                if !seen_non_close {
                    leading_closes += 1;
                }
            }
            b' ' | b'\t' => {}
            _ => {
                seen_non_close = true;
            }
        }
        i += 1;
    }

    let is_switch = line.starts_with("switch ") && line.ends_with('{');
    (leading_closes, opens, closes, is_switch)
}

pub fn format_path(path: &Path, check_only: bool) -> Result<usize, String> {
    let mut changed = 0usize;
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("cdt") {
            changed += format_one_file(path, check_only)?;
        }
        return Ok(changed);
    }
    if path.is_dir() {
        let entries = fs::read_dir(path)
            .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name.starts_with('.') {
                continue;
            }
            changed += format_path(&p, check_only)?;
        }
        return Ok(changed);
    }
    Err(format!("Path not found: '{}'", path.display()))
}

fn format_one_file(path: &Path, check_only: bool) -> Result<usize, String> {
    let orig = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
    let formatted = format_source(&orig);
    if orig == formatted {
        return Ok(0);
    }
    if !check_only {
        fs::write(path, &formatted)
            .map_err(|e| format!("Failed to write '{}': {}", path.display(), e))?;
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn formats_functions_enums_and_switch() {
        let raw = "enum Role {\nAdmin,\nUser,\n}\n\n\nfunc f(r: Role)->Int{\nswitch r {\ncase Role.Admin:\nreturn 1\ncase _:\nreturn 0\n}\n}\n";
        let expected = "enum Role {\n    Admin,\n    User,\n}\n\nfunc f(r: Role)->Int{\n    switch r {\n        case Role.Admin:\n            return 1\n        case _:\n            return 0\n    }\n}\n";
        assert_eq!(format_source(raw), expected);
    }
}
