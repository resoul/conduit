use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::check::TypeChecker;
use crate::loader::ModuleLoader;
use crate::native::NativeRegistry;

pub fn run_lsp_server() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = BufReader::new(stdin.lock());
    let writer = stdout.lock();
    run_server_loop(reader, writer)
}

pub fn run_server_loop(mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
    let mut documents: HashMap<String, String> = HashMap::new();

    loop {
        let msg = match read_message(&mut reader)? {
            Some(m) => m,
            None => break,
        };

        let method = extract_json_string(&msg, "method").unwrap_or_default();
        let id = extract_json_id(&msg);

        match method.as_str() {
            "initialize" => {
                let resp = r#"{"jsonrpc":"2.0","id":"#
                    .to_string()
                    + &id
                    + r#","result":{"capabilities":{"textDocumentSync":1,"hoverProvider":true,"completionProvider":{"triggerCharacters":[".",":"]}}}}"#;
                send_message(&mut writer, &resp)?;
            }
            "initialized" => {}
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (extract_uri(&msg), extract_open_text(&msg)) {
                    documents.insert(uri.clone(), text.clone());
                    let diag = validate_document_with_env(&uri, &text, &documents);
                    send_message(&mut writer, &diag)?;
                }
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(text)) = (extract_uri(&msg), extract_change_text(&msg)) {
                    documents.insert(uri.clone(), text.clone());
                    let diag = validate_document_with_env(&uri, &text, &documents);
                    send_message(&mut writer, &diag)?;
                }
            }
            "textDocument/hover" => {
                let hover_resp = handle_hover(&msg, &id, &documents);
                send_message(&mut writer, &hover_resp)?;
            }
            "textDocument/completion" => {
                let comp_resp = handle_completion(&msg, &id, &documents);
                send_message(&mut writer, &comp_resp)?;
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
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            if let Ok(len) = rest.trim().parse::<usize>() {
                content_length = Some(len);
            }
        }
    }

    let len = match content_length {
        Some(l) => l,
        None => return Ok(None),
    };

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let msg = String::from_utf8_lossy(&buf).to_string();
    Ok(Some(msg))
}

fn send_message(writer: &mut impl Write, payload: &str) -> io::Result<()> {
    let len = payload.as_bytes().len();
    write!(writer, "Content-Length: {}\r\n\r\n{}", len, payload)?;
    writer.flush()
}

fn uri_to_path(uri: &str) -> PathBuf {
    if let Some(stripped) = uri.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(uri)
    }
}

pub fn validate_document(uri: &str, text: &str) -> String {
    validate_document_with_env(uri, text, &HashMap::new())
}

pub fn validate_document_with_env(uri: &str, text: &str, documents: &HashMap<String, String>) -> String {
    let file_path = uri_to_path(uri);
    let mut loader = ModuleLoader::new();

    // Register unsaved open documents
    for (doc_uri, doc_text) in documents {
        loader.with_virtual_file(uri_to_path(doc_uri), doc_text.clone());
    }
    loader.with_virtual_file(&file_path, text.to_string());

    let root_canon = match loader.load_project(&file_path) {
        Ok(c) => c,
        Err(err) => {
            return format_diagnostic(uri, err.span.line, err.span.col, &err.message);
        }
    };

    let mut checker = TypeChecker::new();
    if let Err(err) = checker.check_project(&root_canon, &loader) {
        return format_diagnostic(uri, err.span.line, err.span.col, &err.message);
    }

    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":[]}}}}"#,
        escape_json(uri)
    )
}

fn format_diagnostic(uri: &str, line: usize, col: usize, msg: &str) -> String {
    let l = if line > 0 { line - 1 } else { 0 };
    let c = if col > 0 { col - 1 } else { 0 };

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

fn handle_hover(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let character = extract_json_num(msg, "character").unwrap_or(0);

    let (word, qualified_word) = if let Some(uri) = extract_uri(msg) {
        if let Some(doc) = documents.get(&uri) {
            get_words_at_position(doc, line, character)
        } else {
            (String::new(), String::new())
        }
    } else {
        (String::new(), String::new())
    };

    let registry = NativeRegistry::new();
    let mut hover_text = String::new();

    // 1. Check native registry for time module
    if let Some(time_mod) = registry.get_module("time") {
        for ty in time_mod.types.values() {
            if ty.name == word || ty.qualified_name == qualified_word || ty.qualified_name == word {
                hover_text = format!("**{}** ({})\n\n{}", ty.qualified_name, if ty.is_error { "error" } else { "type" }, ty.doc);
                break;
            }
        }
        if hover_text.is_empty() {
            for f in time_mod.functions.values() {
                let qname = format!("time.{}", f.name);
                if f.name == word || qname == qualified_word || qname == word {
                    let params_str = f.params.iter().map(|(p, t)| format!("{}: {}", p, t)).collect::<Vec<_>>().join(", ");
                    hover_text = format!("**func {}({}) -> {}**\n\n{}", qname, params_str, f.return_type, f.doc);
                    break;
                }
            }
        }
        if hover_text.is_empty() {
            for ((target_type, _), m) in &time_mod.methods {
                if m.name == word || format!("{}.{}", target_type, m.name) == qualified_word {
                    let params_str = m.params.iter().map(|(p, t)| format!("{}: {}", p, t)).collect::<Vec<_>>().join(", ");
                    hover_text = format!("**method {}.{}({}) -> {}**\n\n{}", target_type, m.name, params_str, m.return_type, m.doc);
                    break;
                }
            }
        }
    }

    if hover_text.is_empty() {
        hover_text = match word.as_str() {
            "export" => "**export** — Export a top-level function, type, or error to make it accessible to other modules\n```conduit\nexport func find(id: Int) -> User\nexport type User { name: String }\nexport error NotFound {}\n```".to_string(),
            "import" => "**import** — Import a module from standard library (`cdt.*`) or local file path\n```conduit\nimport cdt.time\nimport \"./users\" as users\n```".to_string(),
            "as" => "**as** — Alias an imported module\n```conduit\nimport cdt.time as clock\nimport \"./users\" as users\n```".to_string(),
            "func" => "**func** — Function declaration\n```conduit\nfunc name(param: Type) -> ReturnType\n```".to_string(),
            "type" => "**type** — Custom structure definition\n```conduit\ntype User {\n    name: String\n    age: Int\n}\n```".to_string(),
            "error" => "**error** — Typed error definition\n```conduit\nerror InvalidUserName {\n    message: String\n}\n```".to_string(),
            "switch" => "**switch** — Exhaustive pattern matching statement\n```conduit\nswitch result {\n    case User(u):\n        ...\n    case InvalidUserName(e):\n        ...\n}\n```".to_string(),
            "case" => "**case** — Pattern branch inside switch statement".to_string(),
            "var" => "**var** — Mutable variable declaration\n```conduit\nvar counter: Int = 0\n```".to_string(),
            "guard" => "**guard** — Early exit condition check\n```conduit\nguard condition else {\n    return error_or_default\n}\n```".to_string(),
            "return" => "**return** — Return a value from the current function".to_string(),
            "if" => "**if** — Conditional branching statement".to_string(),
            "while" => "**while** — Precondition loop\n```conduit\nwhile condition { ... }\n```".to_string(),
            "for" => "**for** — Iteration loop over lists, maps, or ranges\n```conduit\nfor item in list { ... }\nfor (k, v) in map { ... }\n```".to_string(),
            "break" => "**break** — Immediately terminate the innermost loop".to_string(),
            "continue" => "**continue** — Skip the rest of the current iteration and jump to the next item".to_string(),
            "print" => "**print(msg)** — Built-in console output function".to_string(),
            "Int" => "**Int** — 64-bit signed integer type".to_string(),
            "Float" => "**Float** — IEEE 754 64-bit floating-point type (never mixed implicitly with `Int`)".to_string(),
            "String" => "**String** — UTF-8 encoded text string type".to_string(),
            "Bool" => "**Bool** — Boolean type (`true` or `false`)".to_string(),
            "IndexError" => "**IndexError** — Built-in prelude error returned on out-of-bounds list indexing (`{ index: Int, length: Int, message: String }`)".to_string(),
            "KeyError" => "**KeyError** — Built-in prelude error returned when indexing a missing key in a `Map` (`{ key: String, message: String }`)".to_string(),
            "ParseError" => "**ParseError** — Built-in prelude error returned when parsing fails (`{ value: String, message: String }`)".to_string(),
            "Map" => "**Map<K, V>** — Ordered key-value dictionary (`K` must be `String`, `Int`, or `Bool`). Construct with `Map<K, V>()` or `Map.from([(k, v)])`.".to_string(),
            "nil" => "**nil** — Represents the absence of a value in optional types (`T?`)".to_string(),
            "trim" => "`.trim() -> String` — Returns a string slice with leading and trailing whitespace removed".to_string(),
            "lower" => "`.lower() -> String` — Returns a lowercase copy of the string".to_string(),
            "upper" => "`.upper() -> String` — Returns an uppercase copy of the string".to_string(),
            "len" => "`.len() -> Int` — Returns UTF-8 byte length for `String`, element count for `[T]`, or entry count for `Map<K, V>`".to_string(),
            "char_count" => "`.char_count() -> Int` — Returns the number of Unicode scalar values in the string".to_string(),
            "split" => "`.split(sep: String) -> [String]` — Splits the string by `sep` into a list of strings".to_string(),
            "index_of" => "`String.index_of(sub: String) -> Int?` — Returns the Unicode scalar (`char`) index of the first occurrence of `sub`, or `nil`".to_string(),
            "slice" => "`.slice(start: Int, end: Int)` — Returns a sub-list or Unicode scalar substring in `[start, end)` with clamped bounds".to_string(),
            "starts_with" => "`.starts_with(prefix: String) -> Bool` — Checks whether the string starts with `prefix`".to_string(),
            "ends_with" => "`.ends_with(suffix: String) -> Bool` — Checks whether the string ends with `suffix`".to_string(),
            "replace" => "`.replace(from: String, to: String) -> String` — Replaces all occurrences of `from` with `to`".to_string(),
            "to_string" => "`.to_string() -> String` — Converts `Int`, `Float`, `Bool`, or `String` explicitly to `String`".to_string(),
            "to_int" => "`String.to_int() -> Int | ParseError` — Parses a strict decimal integer without surrounding whitespace".to_string(),
            "to_float" => "`.to_float() -> Float | ParseError` (on `String`) or `Int.to_float() -> Float` (on `Int`)".to_string(),
            "floor" => "`Float.floor() -> Int` — Rounds down toward negative infinity".to_string(),
            "ceil" => "`Float.ceil() -> Int` — Rounds up toward positive infinity".to_string(),
            "round" => "`Float.round() -> Int` — Rounds to nearest integer".to_string(),
            "trunc" => "`Float.trunc() -> Int` — Truncates fractional digits toward zero".to_string(),
            "is_nan" => "`Float.is_nan() -> Bool` — Returns `true` if `Float` is `NaN`".to_string(),
            "is_infinite" => "`Float.is_infinite() -> Bool` — Returns `true` if `Float` is +/-infinity".to_string(),
            "is_finite" => "`Float.is_finite() -> Bool` — Returns `true` if `Float` is neither `NaN` nor infinite".to_string(),
            "is_null" => "`json.Value.is_null() -> Bool` — Returns `true` if JSON node is `null`".to_string(),
            "as_string" => "`json.Value.as_string() -> String?` — Extracts JSON string or `nil`".to_string(),
            "as_int" => "`json.Value.as_int() -> Int?` — Extracts exact `i64` JSON integer or `nil`".to_string(),
            "as_float" => "`json.Value.as_float() -> Float?` — Extracts JSON number as `Float` or `nil`".to_string(),
            "as_bool" => "`json.Value.as_bool() -> Bool?` — Extracts JSON boolean or `nil`".to_string(),
            "as_list" => "`json.Value.as_list() -> [json.Value]?` — Extracts JSON array or `nil`".to_string(),
            "as_map" => "`json.Value.as_map() -> Map<String, json.Value>?` — Extracts JSON object or `nil`".to_string(),
            "append" => "`.append(item)` — Appends an element to the end of the list".to_string(),
            "reverse" => "`List.reverse() -> [T]` — Returns a new list with elements in reverse order".to_string(),
            "enumerate" => "`List.enumerate() -> [(Int, T)]` — Returns `(index, item)` pairs starting at `0`".to_string(),
            "join" => "`[String].join(sep: String) -> String` — Joins string elements with separator `sep`".to_string(),
            "contains" => "`.contains(item) -> Bool` — Checks whether the element or substring exists".to_string(),
            "get" => "`.get(index_or_key) -> V?` — Safely gets a value by index (`List`) or key (`Map`), returning `nil` if absent".to_string(),
            "set" => "`.set(key: K, value: V) -> Void` — Inserts or updates a `Map` entry (updates preserve key position)".to_string(),
            "remove" => "`.remove(key: K) -> V?` — Removes `key` from a `Map` and returns its previous value, or `nil`".to_string(),
            "contains_key" => "`.contains_key(key: K) -> Bool` — Returns `true` if `key` is present in the `Map`".to_string(),
            "keys" => "`.keys() -> [K]` — Returns `Map` keys in insertion order".to_string(),
            "values" => "`.values() -> [V]` — Returns `Map` values in insertion order".to_string(),
            "entries" => "`.entries() -> [(K, V)]` — Returns `Map` `(key, value)` pairs in insertion order".to_string(),
            "from" => "`Map.from(pairs: [(K, V)]) -> Map<K, V>` — Builds a `Map` from a list of `(key, value)` tuples".to_string(),
            _ => String::new(),
        };
    }

    if hover_text.is_empty() {
        format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id)
    } else {
        format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"contents":{{"kind":"markdown","value":"{}"}}}}}}"#,
            id,
            escape_json(&hover_text)
        )
    }
}

fn handle_completion(msg: &str, id: &str, documents: &HashMap<String, String>) -> String {
    let line = extract_json_num(msg, "line").unwrap_or(0);
    let character = extract_json_num(msg, "character").unwrap_or(0);

    let prefix_alias = if let Some(uri) = extract_uri(msg) {
        if let Some(doc) = documents.get(&uri) {
            get_dot_prefix_alias(doc, line, character)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(alias) = prefix_alias {
        if alias == "time" {
            let items = r#"[
                {"label":"now","kind":3,"detail":"time.now() -> time.Timestamp | time.TimeError","insertText":"now()"},
                {"label":"monotonic","kind":3,"detail":"time.monotonic() -> time.Instant","insertText":"monotonic()"},
                {"label":"elapsed","kind":3,"detail":"time.elapsed(start: time.Instant) -> time.Duration | time.TimeError","insertText":"elapsed(${1:start})","insertTextFormat":2},
                {"label":"seconds","kind":3,"detail":"time.seconds(value: Int) -> time.Duration | time.TimeError","insertText":"seconds(${1:val})","insertTextFormat":2},
                {"label":"milliseconds","kind":3,"detail":"time.milliseconds(value: Int) -> time.Duration | time.TimeError","insertText":"milliseconds(${1:val})","insertTextFormat":2},
                {"label":"Timestamp","kind":7,"detail":"time.Timestamp type"},
                {"label":"Instant","kind":7,"detail":"time.Instant type"},
                {"label":"Duration","kind":7,"detail":"time.Duration type"},
                {"label":"TimeError","kind":7,"detail":"time.TimeError error"}
            ]"#;
            return format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, items);
        } else if let Some(uri) = extract_uri(msg) {
            // Check if alias is imported from a local module in current document
            if let Some(doc) = documents.get(&uri) {
                let file_path = uri_to_path(&uri);
                let mut loader = ModuleLoader::new();
                for (doc_uri, doc_text) in documents {
                    loader.with_virtual_file(uri_to_path(doc_uri), doc_text.clone());
                }
                loader.with_virtual_file(&file_path, doc.clone());
                if let Ok(root_canon) = loader.load_project(&file_path) {
                    if let Some(loaded_mod) = loader.modules.get(&root_canon) {
                        if let Some((target, _)) = loaded_mod.imports.get(&alias) {
                            if let crate::loader::ModuleTarget::Local(path) = target {
                                if let Some(target_mod) = loader.modules.get(path) {
                                    let mut items = Vec::new();
                                    for item in &target_mod.program.items {
                                        if item.is_exported {
                                            match &item.kind {
                                                crate::ast::ItemKind::Func { name, .. } => {
                                                    items.push(format!(r#"{{"label":"{}","kind":3,"detail":"Exported function"}}"#, name));
                                                }
                                                crate::ast::ItemKind::Type { name, .. } => {
                                                    items.push(format!(r#"{{"label":"{}","kind":7,"detail":"Exported type"}}"#, name));
                                                }
                                                crate::ast::ItemKind::Error { name, .. } => {
                                                    items.push(format!(r#"{{"label":"{}","kind":7,"detail":"Exported error"}}"#, name));
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    let json_arr = format!("[{}]", items.join(","));
                                    return format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, json_arr);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let items = r#"[
        {"label":"export","kind":14,"detail":"Export declaration","insertText":"export "},
        {"label":"import","kind":14,"detail":"Import module","insertText":"import "},
        {"label":"import cdt.time","kind":14,"detail":"Import time library","insertText":"import cdt.time\n"},
        {"label":"as","kind":14,"detail":"Import alias keyword"},
        {"label":"func","kind":14,"detail":"Function declaration","insertText":"func ${1:name}(${2:params}) -> ${3:Type} {\n\t${0}\n}","insertTextFormat":2},
        {"label":"type","kind":7,"detail":"Custom structure","insertText":"type ${1:Name} {\n\t${2:field}: ${3:Type}\n}","insertTextFormat":2},
        {"label":"error","kind":7,"detail":"Typed error definition","insertText":"error ${1:Name} {\n\t${2:message}: String\n}","insertTextFormat":2},
        {"label":"switch","kind":14,"detail":"Switch pattern matching","insertText":"switch ${1:expr} {\n    case ${2:Pattern}:\n        ${0}\n}","insertTextFormat":2},
        {"label":"case","kind":14,"detail":"Case branch","insertText":"case ${1:Pattern}:\n    ${0}","insertTextFormat":2},
        {"label":"return","kind":14,"detail":"Return value"},
        {"label":"if","kind":14,"detail":"If condition"},
        {"label":"guard","kind":14,"detail":"Guard early exit","insertText":"guard ${1:condition} else {\n\treturn ${2:value}\n}","insertTextFormat":2},
        {"label":"while","kind":14,"detail":"While loop","insertText":"while ${1:condition} {\n\t$0\n}","insertTextFormat":2},
        {"label":"for","kind":14,"detail":"For-in loop"},
        {"label":"break","kind":14,"detail":"Break out of loop"},
        {"label":"continue","kind":14,"detail":"Continue to next loop iteration"},
        {"label":"var","kind":6,"detail":"Variable declaration"},
        {"label":"print","kind":3,"detail":"print(...) built-in function"},
        {"label":"Int","kind":7,"detail":"64-bit integer type"},
        {"label":"Float","kind":7,"detail":"64-bit IEEE 754 float type"},
        {"label":"String","kind":7,"detail":"String type"},
        {"label":"Bool","kind":7,"detail":"Boolean type"},
        {"label":"IndexError","kind":7,"detail":"Built-in list index out-of-bounds error"},
        {"label":"KeyError","kind":7,"detail":"Built-in missing Map key error"},
        {"label":"ParseError","kind":7,"detail":"Built-in parsing error"},
        {"label":"Map","kind":7,"detail":"Map<K, V> ordered key-value dictionary"},
        {"label":"trim","kind":2,"detail":"String.trim() -> String"},
        {"label":"lower","kind":2,"detail":"String.lower() -> String"},
        {"label":"upper","kind":2,"detail":"String.upper() -> String"},
        {"label":"len","kind":2,"detail":"String / List / Map.len() -> Int"},
        {"label":"char_count","kind":2,"detail":"String.char_count() -> Int"},
        {"label":"split","kind":2,"detail":"String.split(sep: String) -> [String]","insertText":"split(${1:sep})","insertTextFormat":2},
        {"label":"starts_with","kind":2,"detail":"String.starts_with(prefix: String) -> Bool","insertText":"starts_with(${1:prefix})","insertTextFormat":2},
        {"label":"ends_with","kind":2,"detail":"String.ends_with(suffix: String) -> Bool","insertText":"ends_with(${1:suffix})","insertTextFormat":2},
        {"label":"replace","kind":2,"detail":"String.replace(from: String, to: String) -> String","insertText":"replace(${1:from}, ${2:to})","insertTextFormat":2},
        {"label":"to_string","kind":2,"detail":"Int/Float/Bool/String.to_string() -> String"},
        {"label":"index_of","kind":2,"detail":"String.index_of(sub) -> Int?"},
        {"label":"slice","kind":2,"detail":"slice(start, end)"},
        {"label":"reverse","kind":2,"detail":"List.reverse() -> [T]"},
        {"label":"enumerate","kind":2,"detail":"List.enumerate() -> [(Int, T)]"},
        {"label":"join","kind":2,"detail":"[String].join(sep) -> String"},
        {"label":"to_int","kind":2,"detail":"String.to_int() -> Int | ParseError"},
        {"label":"to_float","kind":2,"detail":"to_float() -> Float / Float | ParseError"},
        {"label":"floor","kind":2,"detail":"Float.floor() -> Int"},
        {"label":"ceil","kind":2,"detail":"Float.ceil() -> Int"},
        {"label":"round","kind":2,"detail":"Float.round() -> Int"},
        {"label":"trunc","kind":2,"detail":"Float.trunc() -> Int"},
        {"label":"is_nan","kind":2,"detail":"Float.is_nan() -> Bool"},
        {"label":"is_infinite","kind":2,"detail":"Float.is_infinite() -> Bool"},
        {"label":"is_finite","kind":2,"detail":"Float.is_finite() -> Bool"},
        {"label":"is_null","kind":2,"detail":"json.Value.is_null() -> Bool"},
        {"label":"as_string","kind":2,"detail":"json.Value.as_string() -> String?"},
        {"label":"as_int","kind":2,"detail":"json.Value.as_int() -> Int?"},
        {"label":"as_float","kind":2,"detail":"json.Value.as_float() -> Float?"},
        {"label":"as_bool","kind":2,"detail":"json.Value.as_bool() -> Bool?"},
        {"label":"as_list","kind":2,"detail":"json.Value.as_list() -> [json.Value]?"},
        {"label":"as_map","kind":2,"detail":"json.Value.as_map() -> Map<String, json.Value>?"},
        {"label":"append","kind":2,"detail":"List.append(item)"},
        {"label":"contains","kind":2,"detail":"List/String.contains(item) -> Bool"},
        {"label":"get","kind":2,"detail":"List/Map.get(key_or_index) -> V?","insertText":"get(${1:key})","insertTextFormat":2},
        {"label":"set","kind":2,"detail":"Map.set(key: K, value: V) -> Void","insertText":"set(${1:key}, ${2:value})","insertTextFormat":2},
        {"label":"remove","kind":2,"detail":"Map.remove(key: K) -> V?","insertText":"remove(${1:key})","insertTextFormat":2},
        {"label":"contains_key","kind":2,"detail":"Map.contains_key(key: K) -> Bool","insertText":"contains_key(${1:key})","insertTextFormat":2},
        {"label":"keys","kind":2,"detail":"Map.keys() -> [K]"},
        {"label":"values","kind":2,"detail":"Map.values() -> [V]"},
        {"label":"entries","kind":2,"detail":"Map.entries() -> [(K, V)]"},
        {"label":"from","kind":2,"detail":"Map.from([(k, v)]) -> Map<K, V>","insertText":"from(${1:pairs})","insertTextFormat":2}
    ]"#;

    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, items)
}

fn get_dot_prefix_alias(text: &str, line_idx: usize, char_idx: usize) -> Option<String> {
    let line = text.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    if char_idx == 0 || char_idx > chars.len() {
        return None;
    }

    let before: String = chars[..char_idx].iter().collect();
    let trimmed = before.trim_end();
    if trimmed.ends_with('.') {
        let without_dot = &trimmed[..trimmed.len() - 1];
        let mut start = without_dot.len();
        let bytes = without_dot.as_bytes();
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            start -= 1;
        }
        let alias = &without_dot[start..];
        if !alias.is_empty() {
            return Some(alias.to_string());
        }
    }
    None
}

fn get_words_at_position(text: &str, line_idx: usize, char_idx: usize) -> (String, String) {
    let line = match text.lines().nth(line_idx) {
        Some(l) => l,
        None => return (String::new(), String::new()),
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

    let simple_word: String = chars[start..end].iter().collect();

    let mut qstart = start;
    if qstart > 1 && chars[qstart - 1] == '.' {
        let mut prev = qstart - 1;
        while prev > 0 && (chars[prev - 1].is_alphanumeric() || chars[prev - 1] == '_') {
            prev -= 1;
        }
        qstart = prev;
    }

    let mut qend = end;
    if qend < chars.len() && chars[qend] == '.' {
        let mut next = qend + 1;
        while next < chars.len() && (chars[next].is_alphanumeric() || chars[next] == '_') {
            next += 1;
        }
        qend = next;
    }

    let qualified_word: String = chars[qstart..qend].iter().collect();
    (simple_word, qualified_word)
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let pos = json.find(&needle)?;
    let rest = &json[pos + needle.len()..].trim_start();
    if rest.starts_with('"') {
        let content = &rest[1..];
        let end = content.find('"')?;
        Some(content[..end].to_string())
    } else {
        None
    }
}

fn extract_json_num(json: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{}\":", key);
    let pos = json.find(&needle)?;
    let rest = &json[pos + needle.len()..].trim_start();
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

fn extract_json_id(json: &str) -> String {
    if let Some(pos) = json.find("\"id\":") {
        let rest = &json[pos + 5..].trim_start();
        if rest.starts_with('"') {
            let content = &rest[1..];
            if let Some(end) = content.find('"') {
                return format!("\"{}\"", &content[..end]);
            }
        } else {
            let val: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !val.is_empty() {
                return val;
            }
        }
    }
    "null".to_string()
}

fn extract_uri(json: &str) -> Option<String> {
    extract_json_string(json, "uri")
}

fn extract_open_text(json: &str) -> Option<String> {
    let needle = "\"text\":";
    let pos = json.find(needle)?;
    let rest = &json[pos + needle.len()..].trim_start();
    if rest.starts_with('"') {
        parse_escaped_string(&rest[1..])
    } else {
        None
    }
}

fn extract_change_text(json: &str) -> Option<String> {
    let needle = "\"contentChanges\":";
    let pos = json.find(needle)?;
    let rest = &json[pos + needle.len()..];
    let text_pos = rest.find("\"text\":")?;
    let text_rest = &rest[text_pos + 7..].trim_start();
    if text_rest.starts_with('"') {
        parse_escaped_string(&text_rest[1..])
    } else {
        None
    }
}

fn parse_escaped_string(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            return Some(out);
        }
        if c == '\\' {
            match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
    None
}

fn escape_json(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_read_and_send_message() {
        let payload = r#"{"jsonrpc":"2.0","method":"test"}"#;
        let mut buffer = Vec::new();
        send_message(&mut buffer, payload).unwrap();

        let mut cursor = Cursor::new(buffer);
        let received = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(received, payload);
    }

    #[test]
    fn test_validate_valid_code() {
        let code = "func add(a: Int, b: Int) -> Int {\n    return a + b\n}\n";
        let diag = validate_document("file:///main.cdt", code);
        assert!(diag.contains("\"diagnostics\":[]"));
    }

    #[test]
    fn test_validate_syntax_error() {
        let code = "func (a: Int) {}\n";
        let diag = validate_document("file:///main.cdt", code);
        assert!(diag.contains("\"diagnostics\":[{"));
        assert!(diag.contains("\"source\":\"conduit\""));
    }

    #[test]
    fn test_validate_type_error() {
        let code = "func test() -> Int {\n    return \"not an int\"\n}\n";
        let diag = validate_document("file:///main.cdt", code);
        assert!(diag.contains("\"diagnostics\":[{"));
        assert!(diag.contains("Type mismatch"));
    }

    #[test]
    fn test_server_loop_roundtrip() {
        let init_req = "Content-Length: 53\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}";
        let exit_req = "Content-Length: 41\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"exit\"}";
        let input = format!("{}{}", init_req, exit_req);

        let reader = Cursor::new(input.into_bytes());
        let mut writer = Vec::new();

        run_server_loop(reader, &mut writer).unwrap();

        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains("\"capabilities\""));
        assert!(output.contains("\"hoverProvider\":true"));
        assert!(output.contains("\"completionProvider\""));
    }
}
