use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: Option<String>,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

fn parse_single_diagnostic(line_str: &str) -> Diagnostic {
    let trimmed = line_str.trim_end();
    if let Some(after_open) = trimmed.strip_prefix('[')
        && let Some((header, msg)) = after_open.split_once(']')
        && let Some((rest, col_str)) = header.rsplit_once(':')
        && let Some((file_part, line_str_num)) = rest.rsplit_once(':')
        && let (Ok(l), Ok(c)) = (
            line_str_num.trim().parse::<usize>(),
            col_str.trim().parse::<usize>(),
        )
    {
        return Diagnostic {
            file: if file_part.is_empty() {
                None
            } else {
                Some(file_part.to_string())
            },
            line: l,
            col: c,
            message: msg.trim().to_string(),
        };
    }
    if let Some(after_open) = trimmed.strip_prefix('[')
        && let Some((header, msg)) = after_open.split_once(']')
        && let Some((line_str_num, col_str)) = header.split_once(':')
        && let (Ok(l), Ok(c)) = (
            line_str_num.trim().parse::<usize>(),
            col_str.trim().parse::<usize>(),
        )
    {
        return Diagnostic {
            file: None,
            line: l,
            col: c,
            message: msg.trim().to_string(),
        };
    }
    Diagnostic {
        file: None,
        line: 0,
        col: 0,
        message: trimmed.to_string(),
    }
}

pub fn tag_file_error(path: &Path, raw: String) -> String {
    let path_str = path.display().to_string();
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let diag = parse_single_diagnostic(trimmed);
        if diag.file.is_some() {
            out.push(trimmed.to_string());
        } else if diag.line > 0 {
            out.push(format!("[{}:{}:{}] {}", path_str, diag.line, diag.col, diag.message));
        } else {
            out.push(format!("[{}:0:0] {}", path_str, diag.message));
        }
    }
    if out.is_empty() {
        format!("[{}:0:0] {}", path_str, raw.trim())
    } else {
        out.join("\n")
    }
}

pub fn parse_diagnostics(raw: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(parse_single_diagnostic(line));
    }
    if out.is_empty() && !raw.trim().is_empty() {
        out.push(Diagnostic {
            file: None,
            line: 0,
            col: 0,
            message: raw.trim().to_string(),
        });
    }
    out
}

pub fn render_diagnostics(raw: &str, entry_path: Option<&Path>, use_color: bool) -> String {
    let color = use_color && std::env::var_os("NO_COLOR").is_none();
    let (red_bold, cyan_bold, reset) = if color {
        ("\x1b[1;31m", "\x1b[1;36m", "\x1b[0m")
    } else {
        ("", "", "")
    };

    let diags = parse_diagnostics(raw);
    if diags.is_empty() {
        return String::new();
    }

    let mut file_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut out = String::new();

    for (idx, d) in diags.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if d.line > 0 && d.col > 0 {
            let _ = writeln!(
                out,
                "{}error[{}:{}]{}: {}",
                red_bold, d.line, d.col, reset, d.message
            );
        } else {
            let _ = writeln!(out, "{}error{}: {}", red_bold, reset, d.message);
        }

        let display_file = d
            .file
            .clone()
            .or_else(|| entry_path.map(|p| p.display().to_string()));
        if let Some(file_path) = display_file {
            if d.line > 0 && d.col > 0 {
                let _ = writeln!(out, " {}-->{} {}:{}:{}", cyan_bold, reset, file_path, d.line, d.col);
            } else {
                let _ = writeln!(out, " {}-->{} {}", cyan_bold, reset, file_path);
            }

            if d.line > 0 {
                let contents = file_cache
                    .entry(file_path.clone())
                    .or_insert_with(|| std::fs::read_to_string(&file_path).ok());
                if let Some(src) = contents.as_deref()
                    && let Some(src_line) = src.lines().nth(d.line - 1)
                {
                    let src_line = src_line.strip_suffix('\r').unwrap_or(src_line);
                    let num_str = d.line.to_string();
                    let gutter = " ".repeat(num_str.len());
                    let pad: String = src_line
                        .chars()
                        .take(d.col.saturating_sub(1))
                        .map(|c| if c == '\t' { '\t' } else { ' ' })
                        .collect();
                    let _ = writeln!(out, " {} {}|{}", gutter, cyan_bold, reset);
                    let _ = writeln!(out, " {}{} |{} {}", cyan_bold, num_str, reset, src_line);
                    let _ = writeln!(
                        out,
                        " {} {}|{} {}{}^{}",
                        gutter, cyan_bold, reset, pad, red_bold, reset
                    );
                }
            }
        }
    }

    if diags.len() > 1 {
        let _ = write!(
            out,
            "\n{}error{}: aborting due to {} previous errors",
            red_bold,
            reset,
            diags.len()
        );
    } else if out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_file_spans_including_windows_paths() {
        let raw = "[2:10] Type mismatch\n[C:\\foo\\bar.cdt:3:7] Undefined variable 'x'\n[10:0] Deprecated\nPlain error";
        let diags = parse_diagnostics(raw);
        assert_eq!(diags.len(), 4);
        assert_eq!(
            diags[0],
            Diagnostic {
                file: None,
                line: 2,
                col: 10,
                message: "Type mismatch".to_string(),
            }
        );
        assert_eq!(
            diags[1],
            Diagnostic {
                file: Some("C:\\foo\\bar.cdt".to_string()),
                line: 3,
                col: 7,
                message: "Undefined variable 'x'".to_string(),
            }
        );
        assert_eq!(
            diags[2],
            Diagnostic {
                file: None,
                line: 10,
                col: 0,
                message: "Deprecated".to_string(),
            }
        );
        assert_eq!(
            diags[3],
            Diagnostic {
                file: None,
                line: 0,
                col: 0,
                message: "Plain error".to_string(),
            }
        );
    }

    #[test]
    fn renders_snippet_with_tabs_and_multi_error_summary() {
        let dir = std::env::temp_dir().join(format!("cdt_diag_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.cdt");
        std::fs::write(&file, "func main() -> Void {\n\tx := \"hi\" + 1\n\ty := unknown_var\n}\n")
            .unwrap();

        let raw = "[2:6] Operator '+' cannot be applied to String and Int\n[3:7] Undefined variable 'unknown_var'";
        let rendered = render_diagnostics(raw, Some(&file), false);
        assert!(rendered.contains("error[2:6]: Operator '+' cannot be applied to String and Int"));
        assert!(rendered.contains("2 | \tx := \"hi\" + 1"));
        assert!(rendered.contains("   | \t    ^"));
        assert!(rendered.contains("error[3:7]: Undefined variable 'unknown_var'"));
        assert!(rendered.contains("error: aborting due to 2 previous errors"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handles_missing_or_out_of_range_file_cleanly() {
        let missing = Path::new("/tmp/does_not_exist_conduit_diag_12345.cdt");
        let rendered = render_diagnostics("[99:4] Boom", Some(missing), false);
        assert!(rendered.contains("error[99:4]: Boom"));
        assert!(rendered.contains("--> /tmp/does_not_exist_conduit_diag_12345.cdt:99:4"));
        assert!(!rendered.contains("99 |"));
    }

    #[test]
    fn tag_file_error_preserves_existing_file_tag_and_tags_untagged() {
        let p = Path::new("sub/mod.cdt");
        let res = tag_file_error(
            p,
            "[4:2] Syntax error: bad token\n[other.cdt:1:1] Already tagged\nGeneric failure".to_string(),
        );
        let lines: Vec<&str> = res.lines().collect();
        assert_eq!(lines[0], "[sub/mod.cdt:4:2] Syntax error: bad token");
        assert_eq!(lines[1], "[other.cdt:1:1] Already tagged");
        assert_eq!(lines[2], "[sub/mod.cdt:0:0] Generic failure");
    }
}
