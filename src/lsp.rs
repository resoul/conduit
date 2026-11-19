use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::frontend::ast::{ItemKind, Program};
use crate::frontend::check::CheckedProgram;
use crate::frontend::check_file_with_overrides;
use crate::lsp_docs::{GLOBAL_KEYWORDS_COMPLETION, lookup_builtin_hover, lookup_native_hover, module_completions};

pub fn run_lsp_server() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = BufReader::new(stdin.lock());
    let writer = stdout.lock();
    run_server_loop(reader, writer)
}

pub fn run_server_loop(mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
    let mut documents: HashMap<String, String> = HashMap::new();
    let mut checked_cache: HashMap<String, (Program, CheckedProgram)> = HashMap::new();

    while let Some(msg) = read_message(&mut reader)? {
        let method = extract_json_string(&msg, "method").unwrap_or_default();
        let id = extract_json_id(&msg);

        match method.as_str() {
            "initialize" => {
                let resp = format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"capabilities":{{"textDocumentSync":1,"hoverProvider":true,"definitionProvider":true,"documentSymbolProvider":true,"documentFormattingProvider":true,"renameProvider":true,"completionProvider":{{"triggerCharacters":[".",":"]}}}}}}}}"#,
                    id
                );
                send_message(&mut writer, &resp)?;
            }
            "initialized" => {}
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (extract_uri(&msg), extract_open_text(&msg)) {
                    documents.insert(uri.clone(), text);
                    let diag = validate_and_cache(&uri, &documents, &mut checked_cache);
                    send_message(&mut writer, &diag)?;
                }
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(text)) = (extract_uri(&msg), extract_change_text(&msg)) {
                    documents.insert(uri.clone(), text);
                    let diag = validate_and_cache(&uri, &documents, &mut checked_cache);
                    send_message(&mut writer, &diag)?;
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = extract_uri(&msg) {
                    documents.remove(&uri);
                    checked_cache.remove(&uri);
                }
            }
            "textDocument/hover" => {
                let resp = handle_hover(&msg, &id, &documents, &checked_cache);
                send_message(&mut writer, &resp)?;
            }
            "textDocument/definition" => {
                let resp = handle_definition(&msg, &id, &documents);
                send_message(&mut writer, &resp)?;
            }
            "textDocument/documentSymbol" => {
                let resp = handle_document_symbol(&msg, &id, &documents);
                send_message(&mut writer, &resp)?;
            }
            "textDocument/completion" => {
                let resp = handle_completion(&msg, &id, &documents, &checked_cache);
                send_message(&mut writer, &resp)?;
            }
            "textDocument/rename" => {
                let resp = handle_rename(&msg, &id, &documents);
                send_message(&mut writer, &resp)?;
            }
            "textDocument/formatting" => {
                let resp = handle_formatting(&msg, &id, &documents);
                send_message(&mut writer, &resp)?;
            }
            "shutdown" => {
                let resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id);
                send_message(&mut writer, &resp)?;
            }
            "exit" => break,
            _ => {
                if !id.is_empty() && id != "null" {
                    let resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id);
                    send_message(&mut writer, &resp)?;
                }
            }
        }
    }
    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:")
            && let Ok(len) = rest.trim().parse::<usize>()
        {
            content_length = Some(len);
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).to_string()))
}

fn send_message(writer: &mut impl Write, payload: &str) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{}", payload.len(), payload)?;
    writer.flush()
}

fn uri_to_path(uri: &str) -> PathBuf {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(raw.replace("%20", " "))
}

fn build_overrides(documents: &HashMap<String, String>) -> HashMap<PathBuf, String> {
    let mut overrides = HashMap::new();
    for (u, text) in documents {
        let p = uri_to_path(u);
        if let Ok(canon) = p.canonicalize() {
            overrides.insert(canon, text.clone());
        }
        overrides.insert(p, text.clone());
    }
    overrides
}

fn validate_and_cache(
    uri: &str,
    documents: &HashMap<String, String>,
    cache: &mut HashMap<String, (Program, CheckedProgram)>,
) -> String {
    let path = uri_to_path(uri);
    let overrides = build_overrides(documents);
    match check_file_with_overrides(&path, &overrides) {
        Ok((prog, checked)) => {
            cache.insert(uri.to_string(), (prog, checked));
            format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":[]}}}}"#,
                escape_json(uri)
            )
        }
        Err(err) => {
            let (line, col, msg) = parse_error_span(&err);
            format_diagnostic(uri, line, col, msg)
        }
    }
}

fn parse_error_span(err: &str) -> (usize, usize, &str) {
    if let Some(rest) = err.strip_prefix('[')
        && let Some((coords, msg)) = rest.split_once(']')
        && let Some((l_str, c_str)) = coords.split_once(':')
        && let (Ok(l), Ok(c)) = (l_str.trim().parse::<usize>(), c_str.trim().parse::<usize>())
    {
        return (l, c, msg.trim());
    }
    (1, 1, err)
}

fn format_diagnostic(uri: &str, line: usize, col: usize, msg: &str) -> String {
    let l = line.saturating_sub(1);
    let c = col.saturating_sub(1);
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":[{{"range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}},"severity":1,"source":"conduit","message":"{}"}}]}}}}"#,
        escape_json(uri),
        l,
        c,
        l,
        c + 1,
        escape_json(msg)
    )
}

fn find_decl_span_in_source(source: &str, sym: &str) -> Option<(usize, usize)> {
    let tokens = crate::frontend::lexer::Lexer::new(source).tokenize_all().ok()?;
    let prog = crate::frontend::parser::Parser::new(tokens).parse_program().ok()?;
    for item in &prog.items {
        let matched = match &item.kind {
            ItemKind::Func { name, .. }
            | ItemKind::Struct { name, .. }
            | ItemKind::TypeAlias { name, .. }
            | ItemKind::Const { name, .. }
            | ItemKind::Enum { name, .. } => name == sym,
            _ => false,
        };
        if matched {
            return Some((item.span.line.saturating_sub(1), item.span.col.saturating_sub(1)));
        }
    }
    None
}

fn resolve_import_path(base_uri: &str, rel: &str) -> Option<std::path::PathBuf> {
    let base_path = uri_to_path(base_uri);
    let dir = base_path.parent()?;
    let cand = dir.join(rel);
    if cand.is_file() {
        return Some(cand);
    }
    let with_ext = dir.join(format!("{}.cdt", rel));
    if with_ext.is_file() {
        return Some(with_ext);
    }
    let mod_cdt = cand.join("mod.cdt");
    if mod_cdt.is_file() {
        return Some(mod_cdt);
    }
    None
}

fn path_to_uri(path: &std::path::Path) -> String {
    let norm = path.to_string_lossy().replace('\\', "/");
    if norm.starts_with('/') {
        format!("file://{}", norm)
    } else {
        format!("file:///{}", norm)
    }
}

fn extract_quoted_at(line_str: &str, ch: usize) -> Option<String> {
    let bytes = line_str.as_bytes();
    let pos = ch.min(bytes.len());
    let left = line_str[..pos].rfind('"')?;
    let right_rel = line_str[pos..].find('"')?;
    Some(line_str[left + 1..pos + right_rel].to_string())
}

fn handle_definition(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let null_resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id);
    let Some(uri) = extract_uri(msg) else {
        return null_resp;
    };
    let Some(doc) = documents.get(&uri) else {
        return null_resp;
    };
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let ch = extract_json_num(msg, "character").unwrap_or(0);

    // 1. If cursor is inside a relative import string `"./..."`, jump to that file.
    if let Some(line_str) = doc.lines().nth(line)
        && line_str.trim_start().starts_with("import")
        && let Some(rel) = extract_quoted_at(line_str, ch)
        && rel.starts_with('.')
        && let Some(target_path) = resolve_import_path(&uri, &rel)
    {
        let target_uri = path_to_uri(&target_path);
        return format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"uri":"{}","range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":0}}}}}}}}"#,
            id,
            escape_json(&target_uri)
        );
    }

    let (word, qualified) = get_words_at_position(doc, line, ch);
    if word.is_empty() {
        return null_resp;
    }

    // 2. Declaration in the current file
    if let Some((dl, dc)) = find_decl_span_in_source(doc, &word) {
        return format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"uri":"{}","range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}}}}}}"#,
            id,
            escape_json(&uri),
            dl,
            dc,
            dl,
            dc + word.len()
        );
    }

    // 3. Imported symbol from a relative module (`import { foo } from "./bar"` or `import * as bar from "./bar"`)
    if let Ok(tokens) = crate::frontend::lexer::Lexer::new(doc).tokenize_all()
        && let Ok(prog) = crate::frontend::parser::Parser::new(tokens).parse_program()
    {
        let prefix = qualified.strip_suffix(&format!(".{}", word));
        for item in &prog.items {
            if let ItemKind::Import { path, alias, names } = &item.kind
                && path.starts_with('.')
                && let Some(target_path) = resolve_import_path(&uri, path)
            {
                let target_sym = if let Some(spec) = names.iter().find(|s| s.alias == word) {
                    Some(spec.name.as_str())
                } else if prefix == alias.as_deref() && alias.is_some() {
                    Some(word.as_str())
                } else {
                    None
                };
                if let Some(sym_name) = target_sym {
                    let target_uri = path_to_uri(&target_path);
                    let target_src = documents
                        .get(&target_uri)
                        .cloned()
                        .or_else(|| std::fs::read_to_string(&target_path).ok());
                    if let Some(tsrc) = target_src {
                        let (dl, dc) = find_decl_span_in_source(&tsrc, sym_name).unwrap_or((0, 0));
                        return format!(
                            r#"{{"jsonrpc":"2.0","id":{},"result":{{"uri":"{}","range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}}}}}}"#,
                            id,
                            escape_json(&target_uri),
                            dl,
                            dc,
                            dl,
                            dc + sym_name.len()
                        );
                    }
                }
            }
        }
    }

    null_resp
}

fn handle_document_symbol(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let empty_resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":[]}}"#, id);
    let Some(uri) = extract_uri(msg) else {
        return empty_resp;
    };
    let Some(doc) = documents.get(&uri) else {
        return empty_resp;
    };
    let Ok(tokens) = crate::frontend::lexer::Lexer::new(doc).tokenize_all() else {
        return empty_resp;
    };
    let Ok(prog) = crate::frontend::parser::Parser::new(tokens).parse_program() else {
        return empty_resp;
    };
    let mut syms = Vec::new();
    for item in &prog.items {
        let (name, kind) = match &item.kind {
            ItemKind::Func { name, .. } => (name.as_str(), 12u8),
            ItemKind::Struct { name, .. } => (name.as_str(), 23u8),
            ItemKind::TypeAlias { name, .. } => (name.as_str(), 5u8),
            ItemKind::Const { name, .. } => (name.as_str(), 14u8),
            ItemKind::Enum { name, .. } => (name.as_str(), 10u8),
            ItemKind::Test { name, .. } => (name.as_str(), 12u8),
            _ => continue,
        };
        let line = item.span.line.saturating_sub(1);
        let col = item.span.col.saturating_sub(1);
        let end_col = col + name.len();
        syms.push(format!(
            r#"{{"name":"{}","kind":{},"range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}},"selectionRange":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}}}}"#,
            escape_json(name),
            kind,
            line,
            col,
            line,
            end_col,
            line,
            col,
            line,
            end_col
        ));
    }
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":[{}]}}"#, id, syms.join(","))
}

fn find_func_hover(word: &str, prog: &Program, checked: &CheckedProgram, doc: &str) -> Option<String> {
    let fid = checked.func_by_name.get(word).copied().or_else(|| {
        let suffix = format!("__{}", word);
        checked
            .func_by_name
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, &id)| id)
    })?;
    let sig = checked.funcs.get(fid as usize)?;
    let mut param_names: Vec<String> = Vec::new();
    let mut decl_line: Option<usize> = None;
    for item in &prog.items {
        if let ItemKind::Func { name, params, .. } = &item.kind
            && name == &sig.name
        {
            param_names = params.iter().map(|p| p.name.clone()).collect();
            decl_line = Some(item.span.line);
            break;
        }
    }
    let params_str = sig
        .params
        .iter()
        .enumerate()
        .map(|(i, ty)| {
            let pname = param_names.get(i).map(|s| s.as_str()).unwrap_or("_");
            format!("{}: {}", pname, ty)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!("```conduit\nfunc {}({}) -> {}\n```", word, params_str, sig.return_type);
    if let Some(line_1based) = decl_line
        && let Some(comment) = extract_leading_doc(doc, line_1based)
    {
        out.push_str("\n\n");
        out.push_str(&comment);
    }
    Some(out)
}

fn find_struct_hover(word: &str, checked: &CheckedProgram) -> Option<String> {
    let sid = checked.struct_by_name.get(word).copied().or_else(|| {
        let suffix = format!("__{}", word);
        checked
            .struct_by_name
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, &id)| id)
    })?;
    let st = checked.structs.get(sid)?;
    let fields = st
        .fields
        .iter()
        .map(|f| format!("    {}: {}", f.name, f.ty))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = if fields.is_empty() {
        format!("```conduit\nstruct {} {{}}\n```", word)
    } else {
        format!("```conduit\nstruct {} {{\n{}\n}}\n```", word, fields)
    };
    if let Some(about) = checked.struct_abouts.get(&st.name) {
        out.push_str("\n\n");
        out.push_str(about);
    }
    Some(out)
}

fn extract_leading_doc(doc: &str, line_1based: usize) -> Option<String> {
    if line_1based <= 1 {
        return None;
    }
    let lines: Vec<&str> = doc.lines().collect();
    let mut idx = line_1based.saturating_sub(2);
    let mut comments = Vec::new();
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if let Some(c) = trimmed.strip_prefix("///").or_else(|| trimmed.strip_prefix("//")) {
            comments.push(c.trim());
        } else {
            break;
        }
        if idx == 0 {
            break;
        }
        idx -= 1;
    }
    if comments.is_empty() {
        return None;
    }
    comments.reverse();
    Some(comments.join("\n"))
}

fn handle_hover(
    msg: &str,
    id: &str,
    documents: &HashMap<String, String>,
    cache: &HashMap<String, (Program, CheckedProgram)>,
) -> String {
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let character = extract_json_num(msg, "character").unwrap_or(0);
    let uri = extract_uri(msg).unwrap_or_default();
    let doc = documents.get(&uri).map(|s| s.as_str()).unwrap_or("");
    let (word, qualified) = get_words_at_position(doc, line, character);

    let hover = if word.is_empty() {
        None
    } else if let Some(native) = lookup_native_hover(&word, &qualified) {
        Some(native.to_string())
    } else if let Some((prog, checked)) = cache.get(&uri) {
        find_func_hover(&word, prog, checked, doc)
            .or_else(|| find_struct_hover(&word, checked))
            .or_else(|| find_const_hover(&word, prog, checked, doc))
            .or_else(|| find_enum_hover(&word, prog, checked, doc))
            .or_else(|| lookup_builtin_hover(&word).map(str::to_string))
    } else {
        lookup_builtin_hover(&word).map(str::to_string)
    };

    match hover {
        Some(md) => format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"contents":{{"kind":"markdown","value":"{}"}}}}}}"#,
            id,
            escape_json(&md)
        ),
        None => format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id),
    }
}

fn handle_completion(
    msg: &str,
    id: &str,
    documents: &HashMap<String, String>,
    cache: &HashMap<String, (Program, CheckedProgram)>,
) -> String {
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let character = extract_json_num(msg, "character").unwrap_or(0);
    let uri = extract_uri(msg).unwrap_or_default();
    let doc = documents.get(&uri).map(|s| s.as_str()).unwrap_or("");

    if let Some(alias) = get_dot_prefix_alias(doc, line, character)
        && let Some(items) = module_completions(&alias)
    {
        return format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, items);
    }

    let mut items: Vec<String> = GLOBAL_KEYWORDS_COMPLETION
        .iter()
        .map(|(label, kind, detail)| {
            format!(
                r#"{{"label":"{}","kind":{},"detail":"{}"}}"#,
                escape_json(label),
                kind,
                escape_json(detail)
            )
        })
        .collect();

    if let Some((_, checked)) = cache.get(&uri) {
        if let Some(prefix) = get_dot_prefix_alias(doc, line, character)
            && let Some(variants) = checked.enums.get(&prefix)
        {
            let vitems: Vec<String> = variants
                .iter()
                .map(|v| {
                    format!(
                        r#"{{"label":"{}","kind":20,"detail":"{}.{}"}}"#,
                        escape_json(v),
                        escape_json(&prefix),
                        escape_json(v)
                    )
                })
                .collect();
            return format!(r#"{{"jsonrpc":"2.0","id":{},"result":[{}]}}"#, id, vitems.join(","));
        }
        for f in &checked.funcs {
            if !f.name.contains("__") {
                let params = f.params.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
                let detail = format!("func {}({}) -> {}", f.name, params, f.return_type);
                items.push(format!(
                    r#"{{"label":"{}","kind":3,"detail":"{}"}}"#,
                    escape_json(&f.name),
                    escape_json(&detail)
                ));
            }
        }
        for s in &checked.structs {
            if !s.name.contains("__") {
                items.push(format!(
                    r#"{{"label":"{}","kind":7,"detail":"struct {}"}}"#,
                    escape_json(&s.name),
                    escape_json(&s.name)
                ));
            }
        }
        for (cname, (cty, _)) in &checked.consts {
            if !cname.contains("__") {
                items.push(format!(
                    r#"{{"label":"{}","kind":21,"detail":"const {}: {}"}}"#,
                    escape_json(cname),
                    escape_json(cname),
                    escape_json(&cty.to_string())
                ));
            }
        }
        for ename in checked.enums.keys() {
            if !ename.contains("__") {
                items.push(format!(
                    r#"{{"label":"{}","kind":10,"detail":"enum {}"}}"#,
                    escape_json(ename),
                    escape_json(ename)
                ));
            }
        }
    }

    format!(r#"{{"jsonrpc":"2.0","id":{},"result":[{}]}}"#, id, items.join(","))
}

fn find_const_hover(
    word: &str,
    prog: &Program,
    checked: &CheckedProgram,
    doc: &str,
) -> Option<String> {
    let (ty, val) = checked.consts.get(word).or_else(|| {
        let suffix = format!("__{}", word);
        checked
            .consts
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, v)| v)
    })?;
    let val_str = match val {
        crate::frontend::check::ConstVal::Int(n) => n.to_string(),
        crate::frontend::check::ConstVal::Float(f) => f.to_string(),
        crate::frontend::check::ConstVal::Bool(b) => b.to_string(),
        crate::frontend::check::ConstVal::String(s) => format!("{:?}", s),
    };
    let mut out = format!("```conduit\nconst {}: {} = {}\n```", word, ty, val_str);
    if let Some(item) = prog
        .items
        .iter()
        .find(|i| matches!(&i.kind, ItemKind::Const { name, .. } if name == word))
        && let Some(comment) = extract_leading_doc(doc, item.span.line)
    {
        out.push_str("\n\n");
        out.push_str(&comment);
    }
    Some(out)
}

fn find_enum_hover(
    word: &str,
    prog: &Program,
    checked: &CheckedProgram,
    doc: &str,
) -> Option<String> {
    let variants = checked.enums.get(word).or_else(|| {
        let suffix = format!("__{}", word);
        checked
            .enums
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, v)| v)
    })?;
    let mut out = format!(
        "```conduit\nenum {} {{ {} }}\n```",
        word,
        variants.join(", ")
    );
    if let Some(item) = prog
        .items
        .iter()
        .find(|i| matches!(&i.kind, ItemKind::Enum { name, .. } if name == word))
        && let Some(comment) = extract_leading_doc(doc, item.span.line)
    {
        out.push_str("\n\n");
        out.push_str(&comment);
    }
    Some(out)
}

fn handle_rename(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let null_resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id);
    let Some(uri) = extract_uri(msg) else {
        return null_resp;
    };
    let Some(doc) = documents.get(&uri) else {
        return null_resp;
    };
    let Some(new_name) = extract_json_string(msg, "newName") else {
        return null_resp;
    };
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let ch = extract_json_num(msg, "character").unwrap_or(0);
    let (word, _) = get_words_at_position(doc, line, ch);
    if word.is_empty() {
        return null_resp;
    }
    let mut edits = Vec::new();
    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for (line_idx, line_str) in doc.lines().enumerate() {
        let bytes = line_str.as_bytes();
        let mut search_from = 0usize;
        while let Some(rel) = line_str[search_from..].find(&word) {
            let start = search_from + rel;
            let end = start + word.len();
            let left_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
            let right_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
            if left_ok && right_ok {
                let char_start = line_str[..start].chars().count();
                let char_end = char_start + word.chars().count();
                edits.push(format!(
                    r#"{{"range":{{"start":{{"line":{},"character":{}}},"end":{{"line":{},"character":{}}}}},"newText":"{}"}}"#,
                    line_idx,
                    char_start,
                    line_idx,
                    char_end,
                    escape_json(&new_name)
                ));
            }
            search_from = end;
        }
    }
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"result":{{"changes":{{"{}":[{}]}}}}}}"#,
        id,
        escape_json(&uri),
        edits.join(",")
    )
}

fn handle_formatting(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let empty_resp = format!(r#"{{"jsonrpc":"2.0","id":{},"result":[]}}"#, id);
    let Some(uri) = extract_uri(msg) else {
        return empty_resp;
    };
    let Some(doc) = documents.get(&uri) else {
        return empty_resp;
    };
    let formatted = crate::fmt::format_source(doc);
    if &formatted == doc {
        return empty_resp;
    }
    let line_count = doc.lines().count() + usize::from(doc.ends_with('\n'));
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"result":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":{},"character":0}}}},"newText":"{}"}}]}}"#,
        id,
        line_count,
        escape_json(&formatted)
    )
}

fn get_dot_prefix_alias(text: &str, line_idx: usize, char_idx: usize) -> Option<String> {
    let line = text.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    if char_idx == 0 || char_idx > chars.len() {
        return None;
    }
    let before: String = chars[..char_idx].iter().collect();
    let trimmed = before.trim_end();
    let without_dot = trimmed.strip_suffix('.')?;
    let mut start = without_dot.len();
    let bytes = without_dot.as_bytes();
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    let alias = &without_dot[start..];
    (!alias.is_empty()).then(|| alias.to_string())
}

fn get_words_at_position(text: &str, line_idx: usize, char_idx: usize) -> (String, String) {
    let Some(line) = text.lines().nth(line_idx) else {
        return (String::new(), String::new());
    };
    let chars: Vec<char> = line.chars().collect();
    if char_idx >= chars.len() {
        return (String::new(), String::new());
    }
    let mut start = char_idx;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    let mut end = char_idx;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    if start >= end {
        return (String::new(), String::new());
    }
    let word: String = chars[start..end].iter().collect();
    let mut qstart = start;
    if qstart >= 2 && chars[qstart - 1] == '.' {
        let mut p = qstart - 1;
        while p > 0 && (chars[p - 1].is_alphanumeric() || chars[p - 1] == '_') {
            p -= 1;
        }
        if p < qstart - 1 {
            qstart = p;
        }
    }
    let qualified: String = chars[qstart..end].iter().collect();
    (word, qualified)
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn extract_json_id(msg: &str) -> String {
    if let Some(pos) = msg.find(r#""id":"#) {
        let rest = msg[pos + 5..].trim_start();
        if let Some(after_quote) = rest.strip_prefix('"') {
            if let Some(end) = after_quote.find('"') {
                return format!(r#""{}""#, &after_quote[..end]);
            }
        } else {
            let end = rest.find([',', '}', ' ']).unwrap_or(rest.len());
            return rest[..end].to_string();
        }
    }
    "null".to_string()
}

fn extract_json_string(msg: &str, key: &str) -> Option<String> {
    let pat = format!(r#""{}":"#, key);
    let pos = msg.find(&pat)?;
    let rest = msg[pos + pat.len()..].trim_start();
    let after = rest.strip_prefix('"')?;
    unescape_json_until_quote(after)
}

fn extract_json_num(msg: &str, key: &str) -> Option<usize> {
    let pat = format!(r#""{}":"#, key);
    let pos = msg.find(&pat)?;
    let rest = msg[pos + pat.len()..].trim_start();
    let end = rest.find([',', '}', ' ']).unwrap_or(rest.len());
    rest[..end].parse::<usize>().ok()
}

fn extract_uri(msg: &str) -> Option<String> {
    extract_json_string(msg, "uri")
}

fn extract_open_text(msg: &str) -> Option<String> {
    extract_json_string(msg, "text")
}

fn extract_change_text(msg: &str) -> Option<String> {
    let pos = msg.rfind(r#""text":"#)?;
    let rest = msg[pos + 7..].trim_start();
    let after = rest.strip_prefix('"')?;
    unescape_json_until_quote(after)
}

fn unescape_json_until_quote(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000C}'),
                'u' => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        hex.push(chars.next()?);
                    }
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                    }
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload)
    }

    #[test]
    fn lsp_diagnostics_hover_and_completion() {
        let src = "// Adds two numbers\nfunc add(a: Int, b: Int) -> Int {\n    return a + b\n}\nfunc main() -> Int { return add(1, 2) }";
        // Simulate Gson-style \u003e / \u003d escaping used by JetBrains LSP4IJ
        let escaped_src = escape_json(src).replace('>', "\\u003e").replace('=', "\\u003d");
        let init = frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        let open = frame(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///tmp/test_lsp.cdt","text":"{}"}}}}}}"#,
            escaped_src
        ));
        let hover_fn = frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///tmp/test_lsp.cdt"},"position":{"line":4,"character":29}}}"#,
        );
        let def_fn = frame(
            r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///tmp/test_lsp.cdt"},"position":{"line":4,"character":29}}}"#,
        );
        let doc_sym = frame(
            r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///tmp/test_lsp.cdt"}}}"#,
        );
        let rename_fn = frame(
            r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///tmp/test_lsp.cdt"},"position":{"line":4,"character":29},"newName":"sum2"}}"#,
        );
        let exit = frame(r#"{"jsonrpc":"2.0","method":"exit"}"#);

        let input = format!("{}{}{}{}{}{}{}", init, open, hover_fn, def_fn, doc_sym, rename_fn, exit);
        let mut out = Vec::new();
        run_server_loop(io::Cursor::new(input.as_bytes()), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(r#""hoverProvider":true"#));
        assert!(text.contains(r#""definitionProvider":true"#));
        assert!(text.contains(r#""documentSymbolProvider":true"#));
        assert!(text.contains(r#""renameProvider":true"#));
        assert!(text.contains(r#""diagnostics":[]"#));
        assert!(text.contains("func add(a: Int, b: Int) -> Int"));
        assert!(text.contains("Adds two numbers"));
        assert!(text.contains(r#""id":3,"result":{"uri":"file:///tmp/test_lsp.cdt","range":{"start":{"line":1,"character":0}"#));
        assert!(text.contains(r#""id":4,"result":[{"name":"add","kind":12"#));
        assert!(text.contains(r#""id":5,"result":{"changes":{"file:///tmp/test_lsp.cdt":[{"range":"#));
        assert!(text.contains(r#""newText":"sum2""#));
    }
}
