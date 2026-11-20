use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use crate::frontend::ast::{Block, Expr, ExprKind, ItemKind, Program, Stmt, StmtKind};
use crate::frontend::check::{CheckedProgram, HttpRouterDeclEntry, HttpRouterDeclKind};
use crate::frontend::types::{ModuleId, Type};
use crate::native::http::route_prefix_matches;
use crate::native::validation::CompiledValidationRule;

pub fn clean_symbol_name(name: &str) -> &str {
    if let Some(after_prefix) = name.strip_prefix("__m")
        && let Some((digits, tail)) = after_prefix.split_once('_')
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !tail.is_empty()
    {
        return tail;
    }
    name
}

fn pattern_to_openapi_path(pattern: &str) -> (String, Vec<String>) {
    let trimmed = pattern.trim_matches('/');
    if trimmed.is_empty() {
        return ("/".to_string(), Vec::new());
    }
    let mut segs = Vec::new();
    let mut params = Vec::new();
    for seg in trimmed.split('/').filter(|s| !s.is_empty()) {
        if let Some(name) = seg.strip_prefix(':').or_else(|| seg.strip_prefix('*')) {
            params.push(name.to_string());
            segs.push(format!("{{{}}}", name));
        } else {
            segs.push(seg.to_string());
        }
    }
    if segs.is_empty() {
        ("/".to_string(), params)
    } else {
        (format!("/{}", segs.join("/")), params)
    }
}

fn normalize_display_path(pattern: &str) -> String {
    let trimmed = pattern.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", trimmed)
    }
}

fn select_routers<'a>(
    checked: &'a CheckedProgram,
    router_filter: Option<&str>,
) -> Result<Vec<&'a (String, Vec<HttpRouterDeclEntry>)>, String> {
    if checked.http_router_plans.is_empty() {
        return Err("No 'router' declarations found in program".to_string());
    }
    let Some(filter) = router_filter else {
        return Ok(checked.http_router_plans.iter().collect());
    };
    let selected: Vec<_> = checked
        .http_router_plans
        .iter()
        .filter(|(name, _)| name == filter || clean_symbol_name(name) == filter)
        .collect();
    if selected.is_empty() {
        let available: Vec<_> = checked
            .http_router_plans
            .iter()
            .map(|(n, _)| clean_symbol_name(n))
            .collect();
        return Err(format!(
            "Unknown router '{}'. Available: {}",
            filter,
            available.join(", ")
        ));
    }
    Ok(selected)
}

fn collect_route_middleware(
    entries: &[HttpRouterDeclEntry],
    route_path: &str,
) -> (Vec<String>, Vec<String>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for e in entries {
        match &e.kind {
            HttpRouterDeclKind::Before => {
                before.push(clean_symbol_name(&e.handler_name).to_string());
            }
            HttpRouterDeclKind::GroupBefore(prefix) if route_prefix_matches(prefix, route_path) => {
                before.push(clean_symbol_name(&e.handler_name).to_string());
            }
            HttpRouterDeclKind::After => {
                after.push(clean_symbol_name(&e.handler_name).to_string());
            }
            HttpRouterDeclKind::GroupAfter(prefix) if route_prefix_matches(prefix, route_path) => {
                after.push(clean_symbol_name(&e.handler_name).to_string());
            }
            _ => {}
        }
    }
    (before, after)
}

pub fn format_routes(
    checked: &CheckedProgram,
    router_filter: Option<&str>,
) -> Result<String, String> {
    let routers = select_routers(checked, router_filter)?;
    let mut out = String::new();

    for (r_idx, (rname, entries)) in routers.iter().enumerate() {
        if r_idx > 0 {
            out.push('\n');
        }
        let clean_name = clean_symbol_name(rname);
        if let Some(about) = checked.struct_abouts.get(rname)
            && !about.is_empty()
        {
            let _ = writeln!(out, "router {} — {}", clean_name, about);
        } else {
            let _ = writeln!(out, "router {}", clean_name);
        }

        for e in entries {
            match &e.kind {
                HttpRouterDeclKind::Cors(origin) => {
                    let _ = writeln!(out, "  cors: {}", origin);
                }
                HttpRouterDeclKind::Static(url_prefix, dir_path) => {
                    let _ = writeln!(out, "  static: {} -> {}", url_prefix, dir_path);
                }
                _ => {}
            }
        }

        let mut rows: Vec<[String; 5]> = Vec::new();
        for e in entries {
            let HttpRouterDeclKind::Route(method, pattern) = &e.kind else {
                continue;
            };
            let norm_path = normalize_display_path(pattern);
            let (before, after) = collect_route_middleware(entries, &norm_path);
            let mut mw_parts = Vec::new();
            if let Some(g) = &e.guard_handler {
                mw_parts.push(format!("guard: {}", clean_symbol_name(g)));
            }
            if !before.is_empty() {
                mw_parts.push(format!("before: {}", before.join(", ")));
            }
            if !after.is_empty() {
                mw_parts.push(format!("after: {}", after.join(", ")));
            }
            let mw_str = if mw_parts.is_empty() {
                "-".to_string()
            } else {
                mw_parts.join("; ")
            };
            let body_str = e
                .body_struct
                .as_deref()
                .map(clean_symbol_name)
                .unwrap_or("-")
                .to_string();
            rows.push([
                method.to_ascii_uppercase(),
                norm_path,
                clean_symbol_name(&e.handler_name).to_string(),
                body_str,
                mw_str,
            ]);
        }

        if rows.is_empty() {
            let _ = writeln!(out, "  (no routes)");
            continue;
        }

        let headers = ["METHOD", "PATH", "HANDLER", "BODY", "MIDDLEWARE"];
        let mut widths = [
            headers[0].len(),
            headers[1].len(),
            headers[2].len(),
            headers[3].len(),
            headers[4].len(),
        ];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.len());
            }
        }
        let _ = writeln!(
            out,
            "  {:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            headers[0],
            headers[1],
            headers[2],
            headers[3],
            headers[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
        for row in &rows {
            let _ = writeln!(
                out,
                "  {:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
                row[0],
                row[1],
                row[2],
                row[3],
                row[4],
                w0 = widths[0],
                w1 = widths[1],
                w2 = widths[2],
                w3 = widths[3],
            );
        }
    }

    if out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct InferredResponse {
    status: u16,
    media_type: Option<&'static str>,
    body_ty: Option<Type>,
}

fn match_http_helper<'a>(checked: &'a CheckedProgram, callee: &'a Expr) -> Option<&'a str> {
    match &callee.kind {
        ExprKind::Member { target, field } => {
            if let ExprKind::Ident(alias) = &target.kind
                && checked.is_native_alias(alias, "cdt.http")
            {
                Some(field.as_str())
            } else {
                None
            }
        }
        ExprKind::Ident(name) => checked.native_direct_fn(name, "cdt.http"),
        _ => None,
    }
}

fn visit_expr_for_responses(
    expr: &Expr,
    checked: &CheckedProgram,
    out: &mut Vec<InferredResponse>,
) {
    match &expr.kind {
        ExprKind::Call {
            callee,
            args,
            named_args,
            ..
        } => {
            if let Some(sym) = match_http_helper(checked, callee) {
                match sym {
                    "ok_json" if args.len() == 1 => {
                        out.push(InferredResponse {
                            status: 200,
                            media_type: Some("application/json"),
                            body_ty: Some(args[0].ty.clone()),
                        });
                    }
                    "status_json" if args.len() == 2 => {
                        if let ExprKind::Int(code) = args[0].kind
                            && (100..=599).contains(&code)
                        {
                            out.push(InferredResponse {
                                status: code as u16,
                                media_type: Some("application/json"),
                                body_ty: Some(args[1].ty.clone()),
                            });
                        }
                    }
                    "no_content" if args.is_empty() => {
                        out.push(InferredResponse {
                            status: 204,
                            media_type: None,
                            body_ty: None,
                        });
                    }
                    "json" if args.len() == 1 => {
                        out.push(InferredResponse {
                            status: 200,
                            media_type: Some("application/json"),
                            body_ty: Some(Type::String),
                        });
                    }
                    "text" if args.len() == 1 => {
                        out.push(InferredResponse {
                            status: 200,
                            media_type: Some("text/plain"),
                            body_ty: Some(Type::String),
                        });
                    }
                    "html" if args.len() == 1 => {
                        out.push(InferredResponse {
                            status: 200,
                            media_type: Some("text/html"),
                            body_ty: Some(Type::String),
                        });
                    }
                    "response" | "response_with" if args.len() >= 2 => {
                        if let ExprKind::Int(code) = args[0].kind
                            && (100..=599).contains(&code)
                        {
                            out.push(InferredResponse {
                                status: code as u16,
                                media_type: Some("text/plain"),
                                body_ty: Some(Type::String),
                            });
                        }
                    }
                    "redirect" if (1..=2).contains(&args.len()) => {
                        let code = if args.len() == 2 {
                            match args[1].kind {
                                ExprKind::Int(c) if (300..=399).contains(&c) => c as u16,
                                _ => 302,
                            }
                        } else {
                            302
                        };
                        out.push(InferredResponse {
                            status: code,
                            media_type: None,
                            body_ty: None,
                        });
                    }
                    "Response" => {
                        let status_expr = named_args
                            .iter()
                            .find(|na| na.name == "status")
                            .map(|na| &na.value)
                            .or_else(|| args.first());
                        if let Some(Expr {
                            kind: ExprKind::Int(code),
                            ..
                        }) = status_expr
                            && (100..=599).contains(code)
                        {
                            out.push(InferredResponse {
                                status: *code as u16,
                                media_type: None,
                                body_ty: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            visit_expr_for_responses(callee, checked, out);
            for a in args {
                visit_expr_for_responses(a, checked, out);
            }
            for na in named_args {
                visit_expr_for_responses(&na.value, checked, out);
            }
        }
        ExprKind::Member { target, .. }
        | ExprKind::OptionalMember { target, .. }
        | ExprKind::Try(target)
        | ExprKind::Unary { expr: target, .. } => {
            visit_expr_for_responses(target, checked, out);
        }
        ExprKind::Binary { left, right, .. } => {
            visit_expr_for_responses(left, checked, out);
            visit_expr_for_responses(right, checked, out);
        }
        ExprKind::Index { target, index } => {
            visit_expr_for_responses(target, checked, out);
            visit_expr_for_responses(index, checked, out);
        }
        ExprKind::List(es) | ExprKind::Tuple(es) => {
            for e in es {
                visit_expr_for_responses(e, checked, out);
            }
        }
        ExprKind::Closure { body, .. } => visit_block_for_responses(body, checked, out),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil
        | ExprKind::Ident(_) => {}
    }
}

fn visit_stmt_for_responses(
    stmt: &Stmt,
    checked: &CheckedProgram,
    out: &mut Vec<InferredResponse>,
) {
    match &stmt.kind {
        StmtKind::Define { value, .. }
        | StmtKind::DestructureDefine { value, .. }
        | StmtKind::Expr(value) => visit_expr_for_responses(value, checked, out),
        StmtKind::VarDecl { init: Some(value), .. } => {
            visit_expr_for_responses(value, checked, out)
        }
        StmtKind::VarDecl { init: None, .. } | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Assign { target, value } => {
            visit_expr_for_responses(target, checked, out);
            visit_expr_for_responses(value, checked, out);
        }
        StmtKind::Return(Some(e)) => visit_expr_for_responses(e, checked, out),
        StmtKind::Return(None) => {}
        StmtKind::If {
            condition,
            then_block,
            else_block,
        } => {
            visit_expr_for_responses(condition, checked, out);
            visit_block_for_responses(then_block, checked, out);
            if let Some(eb) = else_block {
                visit_block_for_responses(eb, checked, out);
            }
        }
        StmtKind::Guard {
            condition,
            else_block,
        } => {
            visit_expr_for_responses(condition, checked, out);
            visit_block_for_responses(else_block, checked, out);
        }
        StmtKind::ForIn { iterable, body, .. } => {
            visit_expr_for_responses(iterable, checked, out);
            visit_block_for_responses(body, checked, out);
        }
        StmtKind::While { condition, body } => {
            visit_expr_for_responses(condition, checked, out);
            visit_block_for_responses(body, checked, out);
        }
        StmtKind::Defer(block) => visit_block_for_responses(block, checked, out),
        StmtKind::Switch { expr, cases } => {
            visit_expr_for_responses(expr, checked, out);
            for case in cases {
                visit_block_for_responses(&case.body, checked, out);
            }
        }
    }
}

fn visit_block_for_responses(
    block: &Block,
    checked: &CheckedProgram,
    out: &mut Vec<InferredResponse>,
) {
    for stmt in &block.stmts {
        visit_stmt_for_responses(stmt, checked, out);
    }
}

fn infer_handler_responses(
    program: &Program,
    checked: &CheckedProgram,
    handler_name: &str,
) -> Vec<InferredResponse> {
    let mut found = Vec::new();
    for item in &program.items {
        if let ItemKind::Func { name, body, .. } = &item.kind
            && name == handler_name
        {
            visit_block_for_responses(body, checked, &mut found);
            break;
        }
    }
    let mut dedup: BTreeMap<u16, InferredResponse> = BTreeMap::new();
    for resp in found {
        match dedup.get_mut(&resp.status) {
            Some(existing) => {
                if existing.body_ty.is_none() && resp.body_ty.is_some() {
                    *existing = resp;
                }
            }
            None => {
                dedup.insert(resp.status, resp);
            }
        }
    }
    if dedup.is_empty() {
        vec![InferredResponse {
            status: 200,
            media_type: None,
            body_ty: None,
        }]
    } else {
        dedup.into_values().collect()
    }
}

fn infer_auto_return_responses(
    ret_ty: &Type,
    checked: &CheckedProgram,
    explicit: Vec<InferredResponse>,
) -> Vec<InferredResponse> {
    let mut out: BTreeMap<u16, InferredResponse> = BTreeMap::new();
    let mut push_leaf = |ty: &Type| {
        if let Type::Custom(tid) = ty
            && let ModuleId::Native(m) = &tid.module
            && m == "cdt.http"
            && tid.name == "Response"
        {
            for r in &explicit {
                out.entry(r.status).or_insert_with(|| r.clone());
            }
            return;
        }
        if *ty == Type::Void {
            out.entry(204).or_insert(InferredResponse {
                status: 204,
                media_type: None,
                body_ty: None,
            });
            return;
        }
        let status = match ty {
            Type::Custom(tid) if tid.module == ModuleId::Program => {
                if let Some(&st) = checked.http_status_by_struct.get(&tid.name) {
                    st
                } else if checked.error_structs.contains(&tid.name) {
                    500
                } else {
                    200
                }
            }
            _ => 200,
        };
        out.entry(status).or_insert_with(|| InferredResponse {
            status,
            media_type: Some("application/json"),
            body_ty: Some(ty.clone()),
        });
    };

    match ret_ty {
        Type::Optional(inner) => {
            push_leaf(inner);
            out.entry(404).or_insert(InferredResponse {
                status: 404,
                media_type: Some("application/json"),
                body_ty: None,
            });
        }
        Type::Union(vars) => {
            for v in vars {
                push_leaf(v);
            }
        }
        other => push_leaf(other),
    }

    if out.is_empty() {
        explicit
    } else {
        out.into_values().collect()
    }
}

#[derive(Debug, Clone)]
enum JsonVal {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(Vec<(String, JsonVal)>),
}

impl JsonVal {
    fn to_pretty_string(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_escaped_str(out: &mut String, s: &str) {
        out.push('"');
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            JsonVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonVal::Int(n) => {
                let _ = write!(out, "{}", n);
            }
            JsonVal::Float(f) => {
                if f.is_finite() {
                    let s = f.to_string();
                    out.push_str(&s);
                } else {
                    out.push_str("null");
                }
            }
            JsonVal::Str(s) => Self::write_escaped_str(out, s),
            JsonVal::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    for _ in 0..(indent + 2) {
                        out.push(' ');
                    }
                    item.write_pretty(out, indent + 2);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                for _ in 0..indent {
                    out.push(' ');
                }
                out.push(']');
            }
            JsonVal::Obj(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in entries.iter().enumerate() {
                    for _ in 0..(indent + 2) {
                        out.push(' ');
                    }
                    Self::write_escaped_str(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, indent + 2);
                    if i + 1 < entries.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                for _ in 0..indent {
                    out.push(' ');
                }
                out.push('}');
            }
        }
    }
}

struct SchemaRegistry<'a> {
    checked: &'a CheckedProgram,
    schemas: BTreeMap<String, JsonVal>,
    visiting: HashSet<String>,
    raw_to_schema_name: HashMap<String, String>,
}

impl<'a> SchemaRegistry<'a> {
    fn new(checked: &'a CheckedProgram) -> Self {
        let mut raw_to_schema_name = HashMap::new();
        let mut used_clean: HashSet<String> = HashSet::new();

        let mut raw_names: Vec<String> = checked
            .struct_by_name
            .keys()
            .filter(|k| !k.starts_with("__enum_"))
            .chain(checked.enums.keys())
            .chain(checked.data_enums.keys())
            .cloned()
            .collect();
        raw_names.sort();

        for raw in raw_names {
            let clean = clean_symbol_name(&raw).to_string();
            let chosen = if used_clean.insert(clean.clone()) {
                clean
            } else {
                raw.clone()
            };
            raw_to_schema_name.insert(raw, chosen);
        }

        Self {
            checked,
            schemas: BTreeMap::new(),
            visiting: HashSet::new(),
            raw_to_schema_name,
        }
    }

    fn schema_name(&self, raw: &str) -> String {
        self.raw_to_schema_name
            .get(raw)
            .cloned()
            .unwrap_or_else(|| clean_symbol_name(raw).to_string())
    }

    fn type_to_schema(&mut self, ty: &Type) -> JsonVal {
        match ty {
            Type::Int => JsonVal::Obj(vec![
                ("type".to_string(), JsonVal::Str("integer".to_string())),
                ("format".to_string(), JsonVal::Str("int64".to_string())),
            ]),
            Type::Float => JsonVal::Obj(vec![
                ("type".to_string(), JsonVal::Str("number".to_string())),
                ("format".to_string(), JsonVal::Str("double".to_string())),
            ]),
            Type::Bool => JsonVal::Obj(vec![("type".to_string(), JsonVal::Str("boolean".to_string()))]),
            Type::String => JsonVal::Obj(vec![("type".to_string(), JsonVal::Str("string".to_string()))]),
            Type::List(inner) => {
                let items = self.type_to_schema(inner);
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("array".to_string())),
                    ("items".to_string(), items),
                ])
            }
            Type::Map(_, val) => {
                let add_props = self.type_to_schema(val);
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("object".to_string())),
                    ("additionalProperties".to_string(), add_props),
                ])
            }
            Type::Optional(inner) => {
                let inner_schema = self.type_to_schema(inner);
                match inner_schema {
                    JsonVal::Obj(mut fields) => {
                        let is_ref = fields.iter().any(|(k, _)| k == "$ref");
                        if is_ref {
                            JsonVal::Obj(vec![
                                ("allOf".to_string(), JsonVal::Arr(vec![JsonVal::Obj(fields)])),
                                ("nullable".to_string(), JsonVal::Bool(true)),
                            ])
                        } else {
                            fields.push(("nullable".to_string(), JsonVal::Bool(true)));
                            JsonVal::Obj(fields)
                        }
                    }
                    other => other,
                }
            }
            Type::Enum(ename) => {
                let sname = self.ensure_enum_schema(ename);
                JsonVal::Obj(vec![(
                    "$ref".to_string(),
                    JsonVal::Str(format!("#/components/schemas/{}", sname)),
                )])
            }
            Type::DataEnum(ename) => {
                let sname = self.ensure_data_enum_schema(ename);
                JsonVal::Obj(vec![(
                    "$ref".to_string(),
                    JsonVal::Str(format!("#/components/schemas/{}", sname)),
                )])
            }
            Type::Custom(tid) if tid.module == ModuleId::Program => {
                if self.checked.struct_by_name.contains_key(&tid.name) {
                    let sname = self.ensure_struct_schema(&tid.name);
                    JsonVal::Obj(vec![(
                        "$ref".to_string(),
                        JsonVal::Str(format!("#/components/schemas/{}", sname)),
                    )])
                } else if self.checked.enums.contains_key(&tid.name) {
                    let sname = self.ensure_enum_schema(&tid.name);
                    JsonVal::Obj(vec![(
                        "$ref".to_string(),
                        JsonVal::Str(format!("#/components/schemas/{}", sname)),
                    )])
                } else if self.checked.data_enums.contains_key(&tid.name) {
                    let sname = self.ensure_data_enum_schema(&tid.name);
                    JsonVal::Obj(vec![(
                        "$ref".to_string(),
                        JsonVal::Str(format!("#/components/schemas/{}", sname)),
                    )])
                } else {
                    JsonVal::Obj(vec![("type".to_string(), JsonVal::Str("object".to_string()))])
                }
            }
            _ => JsonVal::Obj(vec![("type".to_string(), JsonVal::Str("object".to_string()))]),
        }
    }

    fn ensure_enum_schema(&mut self, raw_name: &str) -> String {
        let sname = self.schema_name(raw_name);
        if !self.schemas.contains_key(&sname)
            && let Some(variants) = self.checked.enums.get(raw_name)
        {
            let vals = variants.iter().cloned().map(JsonVal::Str).collect();
            self.schemas.insert(
                sname.clone(),
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("string".to_string())),
                    ("enum".to_string(), JsonVal::Arr(vals)),
                ]),
            );
        }
        sname
    }

    fn apply_validation_rules(
        schema: JsonVal,
        ty: &Type,
        rules: &[CompiledValidationRule],
    ) -> JsonVal {
        let JsonVal::Obj(mut pairs) = schema else {
            return schema;
        };
        let unwrapped = match ty {
            Type::Optional(inner) => inner.as_ref(),
            other => other,
        };
        let is_list = matches!(unwrapped, Type::List(_));

        for rule in rules {
            match rule {
                CompiledValidationRule::Email => {
                    pairs.push(("format".to_string(), JsonVal::Str("email".to_string())));
                }
                CompiledValidationRule::Url => {
                    pairs.push(("format".to_string(), JsonVal::Str("uri".to_string())));
                }
                CompiledValidationRule::Uuid => {
                    pairs.push(("format".to_string(), JsonVal::Str("uuid".to_string())));
                }
                CompiledValidationRule::NotEmpty => {
                    let key = if is_list { "minItems" } else { "minLength" };
                    pairs.push((key.to_string(), JsonVal::Int(1)));
                }
                CompiledValidationRule::MinInt(n) => {
                    pairs.push(("minimum".to_string(), JsonVal::Int(*n)));
                }
                CompiledValidationRule::MaxInt(n) => {
                    pairs.push(("maximum".to_string(), JsonVal::Int(*n)));
                }
                CompiledValidationRule::RangeInt(lo, hi) => {
                    pairs.push(("minimum".to_string(), JsonVal::Int(*lo)));
                    pairs.push(("maximum".to_string(), JsonVal::Int(*hi)));
                }
                CompiledValidationRule::MinFloat(n) => {
                    pairs.push(("minimum".to_string(), JsonVal::Float(*n)));
                }
                CompiledValidationRule::MaxFloat(n) => {
                    pairs.push(("maximum".to_string(), JsonVal::Float(*n)));
                }
                CompiledValidationRule::RangeFloat(lo, hi) => {
                    pairs.push(("minimum".to_string(), JsonVal::Float(*lo)));
                    pairs.push(("maximum".to_string(), JsonVal::Float(*hi)));
                }
                CompiledValidationRule::MinObj(n) => {
                    let key = if is_list { "minItems" } else { "minLength" };
                    pairs.push((key.to_string(), JsonVal::Int(*n)));
                }
                CompiledValidationRule::MaxObj(n) => {
                    let key = if is_list { "maxItems" } else { "maxLength" };
                    pairs.push((key.to_string(), JsonVal::Int(*n)));
                }
                CompiledValidationRule::OneOf(vals) => {
                    pairs.push((
                        "enum".to_string(),
                        JsonVal::Arr(vals.iter().cloned().map(JsonVal::Str).collect()),
                    ));
                }
                CompiledValidationRule::UserFunc { .. } => {}
            }
        }
        JsonVal::Obj(pairs)
    }

    fn build_struct_object_schema(
        &mut self,
        struct_name: &str,
        enum_tag: Option<&str>,
    ) -> Option<JsonVal> {
        let &sid = self.checked.struct_by_name.get(struct_name)?;
        let st = &self.checked.structs[sid];
        let val_plan = self
            .checked
            .validation_plans
            .iter()
            .find(|vp| vp.schema_id == st.schema_id);

        let mut props = Vec::new();
        let mut required = Vec::new();

        if let Some(tag) = enum_tag {
            props.push((
                "type".to_string(),
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("string".to_string())),
                    (
                        "enum".to_string(),
                        JsonVal::Arr(vec![JsonVal::Str(tag.to_string())]),
                    ),
                ]),
            ));
            required.push(JsonVal::Str("type".to_string()));
        }

        for f in &st.fields {
            let mut fschema = self.type_to_schema(&f.ty);
            if let Some(vp) = val_plan
                && let Some(fp) = vp.fields.iter().find(|pf| pf.name == f.name)
            {
                fschema = Self::apply_validation_rules(fschema, &f.ty, &fp.rules);
            }
            props.push((f.json_name.clone(), fschema));
            if !matches!(f.ty, Type::Optional(_)) {
                required.push(JsonVal::Str(f.json_name.clone()));
            }
        }

        let mut obj = vec![
            ("type".to_string(), JsonVal::Str("object".to_string())),
            ("properties".to_string(), JsonVal::Obj(props)),
        ];
        if !required.is_empty() {
            obj.push(("required".to_string(), JsonVal::Arr(required)));
        }
        Some(JsonVal::Obj(obj))
    }

    fn ensure_struct_schema(&mut self, raw_name: &str) -> String {
        let sname = self.schema_name(raw_name);
        if self.schemas.contains_key(&sname) || !self.visiting.insert(raw_name.to_string()) {
            return sname;
        }
        if let Some(schema_obj) = self.build_struct_object_schema(raw_name, None) {
            self.schemas.insert(sname.clone(), schema_obj);
        }
        self.visiting.remove(raw_name);
        sname
    }

    fn ensure_data_enum_schema(&mut self, raw_name: &str) -> String {
        let sname = self.schema_name(raw_name);
        if self.schemas.contains_key(&sname) || !self.visiting.insert(raw_name.to_string()) {
            return sname;
        }
        if let Some(variants) = self.checked.data_enums.get(raw_name) {
            let mut one_of = Vec::with_capacity(variants.len());
            let mut mapping = Vec::with_capacity(variants.len());
            for dv in variants {
                let mut v_sname = format!("{}_{}", sname, dv.name);
                while self.schemas.contains_key(&v_sname) {
                    v_sname.push('_');
                }
                if let Some(v_obj) = self.build_struct_object_schema(&dv.struct_name, Some(&dv.name)) {
                    self.schemas.insert(v_sname.clone(), v_obj);
                }
                let ref_str = format!("#/components/schemas/{}", v_sname);
                one_of.push(JsonVal::Obj(vec![(
                    "$ref".to_string(),
                    JsonVal::Str(ref_str.clone()),
                )]));
                mapping.push((dv.name.clone(), JsonVal::Str(ref_str)));
            }
            self.schemas.insert(
                sname.clone(),
                JsonVal::Obj(vec![
                    ("oneOf".to_string(), JsonVal::Arr(one_of)),
                    (
                        "discriminator".to_string(),
                        JsonVal::Obj(vec![
                            (
                                "propertyName".to_string(),
                                JsonVal::Str("type".to_string()),
                            ),
                            ("mapping".to_string(), JsonVal::Obj(mapping)),
                        ]),
                    ),
                ]),
            );
        }
        self.visiting.remove(raw_name);
        sname
    }

    fn register_standard_error_schemas(&mut self) {
        self.schemas
            .entry("InvalidJsonErrorResponse".to_string())
            .or_insert_with(|| {
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("object".to_string())),
                    (
                        "properties".to_string(),
                        JsonVal::Obj(vec![
                            (
                                "error".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                            (
                                "message".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                        ]),
                    ),
                    (
                        "required".to_string(),
                        JsonVal::Arr(vec![
                            JsonVal::Str("error".to_string()),
                            JsonVal::Str("message".to_string()),
                        ]),
                    ),
                ])
            });

        self.schemas
            .entry("ValidationIssue".to_string())
            .or_insert_with(|| {
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("object".to_string())),
                    (
                        "properties".to_string(),
                        JsonVal::Obj(vec![
                            (
                                "path".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                            (
                                "code".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                            (
                                "message".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                        ]),
                    ),
                    (
                        "required".to_string(),
                        JsonVal::Arr(vec![
                            JsonVal::Str("path".to_string()),
                            JsonVal::Str("code".to_string()),
                            JsonVal::Str("message".to_string()),
                        ]),
                    ),
                ])
            });

        self.schemas
            .entry("ValidationErrorResponse".to_string())
            .or_insert_with(|| {
                JsonVal::Obj(vec![
                    ("type".to_string(), JsonVal::Str("object".to_string())),
                    (
                        "properties".to_string(),
                        JsonVal::Obj(vec![
                            (
                                "error".to_string(),
                                JsonVal::Obj(vec![(
                                    "type".to_string(),
                                    JsonVal::Str("string".to_string()),
                                )]),
                            ),
                            (
                                "issues".to_string(),
                                JsonVal::Obj(vec![
                                    ("type".to_string(), JsonVal::Str("array".to_string())),
                                    (
                                        "items".to_string(),
                                        JsonVal::Obj(vec![(
                                            "$ref".to_string(),
                                            JsonVal::Str(
                                                "#/components/schemas/ValidationIssue".to_string(),
                                            ),
                                        )]),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                    (
                        "required".to_string(),
                        JsonVal::Arr(vec![
                            JsonVal::Str("error".to_string()),
                            JsonVal::Str("issues".to_string()),
                        ]),
                    ),
                ])
            });
    }
}

fn status_description(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Response",
    }
}

pub fn generate_openapi_json(
    program: &Program,
    checked: &CheckedProgram,
    title: &str,
    version: &str,
    router_filter: Option<&str>,
    server_url: Option<&str>,
) -> Result<String, String> {
    let routers = select_routers(checked, router_filter)?;
    let mut registry = SchemaRegistry::new(checked);
    let mut paths_order: Vec<String> = Vec::new();
    let mut paths_map: HashMap<String, Vec<(String, JsonVal)>> = HashMap::new();
    let mut used_op_ids: HashSet<String> = HashSet::new();
    let mut has_body_structs = false;

    for (rname, entries) in routers {
        let clean_router = clean_symbol_name(rname).to_string();
        for e in entries {
            let HttpRouterDeclKind::Route(method, pattern) = &e.kind else {
                continue;
            };
            let norm_path = normalize_display_path(pattern);
            let (openapi_path, path_params) = pattern_to_openapi_path(pattern);
            let (before, after) = collect_route_middleware(entries, &norm_path);

            let base_op = clean_symbol_name(&e.handler_name).to_string();
            let mut op_id = base_op.clone();
            if !used_op_ids.insert(op_id.clone()) {
                let mut n = 2usize;
                loop {
                    let cand = format!("{}_{}", base_op, n);
                    if used_op_ids.insert(cand.clone()) {
                        op_id = cand;
                        break;
                    }
                    n += 1;
                }
            }

            let mut op_fields: Vec<(String, JsonVal)> = vec![
                ("operationId".to_string(), JsonVal::Str(op_id)),
                ("tags".to_string(), JsonVal::Arr(vec![JsonVal::Str(clean_router.clone())])),
            ];

            let mut path_param_schemas: HashMap<String, JsonVal> = HashMap::new();
            let mut query_params_arr: Vec<JsonVal> = Vec::new();
            let mut has_query_struct = false;
            if let Some(qname) = &e.query_struct
                && let Some(&qsid) = checked.struct_by_name.get(qname)
            {
                has_query_struct = true;
                let qst = &checked.structs[qsid];
                let path_set: HashSet<&str> = path_params.iter().map(|s| s.as_str()).collect();
                for f in &qst.fields {
                    let (is_opt, inner_ty) = match &f.ty {
                        Type::Optional(inner) => (true, inner.as_ref()),
                        other => (false, other),
                    };
                    let schema = registry.type_to_schema(inner_ty);
                    if path_set.contains(f.json_name.as_str()) {
                        path_param_schemas.insert(f.json_name.clone(), schema);
                    } else {
                        query_params_arr.push(JsonVal::Obj(vec![
                            ("name".to_string(), JsonVal::Str(f.json_name.clone())),
                            ("in".to_string(), JsonVal::Str("query".to_string())),
                            (
                                "required".to_string(),
                                JsonVal::Bool(!is_opt && f.default.is_none()),
                            ),
                            ("schema".to_string(), schema),
                        ]));
                    }
                }
            }

            if !path_params.is_empty() || !query_params_arr.is_empty() {
                let mut params_arr: Vec<JsonVal> = path_params
                    .into_iter()
                    .map(|pname| {
                        let schema = path_param_schemas.remove(&pname).unwrap_or_else(|| {
                            JsonVal::Obj(vec![(
                                "type".to_string(),
                                JsonVal::Str("string".to_string()),
                            )])
                        });
                        JsonVal::Obj(vec![
                            ("name".to_string(), JsonVal::Str(pname)),
                            ("in".to_string(), JsonVal::Str("path".to_string())),
                            ("required".to_string(), JsonVal::Bool(true)),
                            ("schema".to_string(), schema),
                        ])
                    })
                    .collect();
                params_arr.extend(query_params_arr);
                op_fields.push(("parameters".to_string(), JsonVal::Arr(params_arr)));
            }

            if !before.is_empty() || !after.is_empty() {
                let mut mw_obj = Vec::new();
                if !before.is_empty() {
                    mw_obj.push((
                        "before".to_string(),
                        JsonVal::Arr(before.into_iter().map(JsonVal::Str).collect()),
                    ));
                }
                if !after.is_empty() {
                    mw_obj.push((
                        "after".to_string(),
                        JsonVal::Arr(after.into_iter().map(JsonVal::Str).collect()),
                    ));
                }
                op_fields.push(("x-conduit-middleware".to_string(), JsonVal::Obj(mw_obj)));
            }

            let method_lower = method.to_ascii_lowercase();
            let (http_method_key, is_any) = if method_lower == "any" {
                ("get".to_string(), true)
            } else {
                (method_lower, false)
            };
            if is_any {
                op_fields.push(("x-conduit-any".to_string(), JsonVal::Bool(true)));
            }

            if let Some(body_struct) = &e.body_struct {
                has_body_structs = true;
                let sname = if registry.checked.data_enums.contains_key(body_struct) {
                    registry.ensure_data_enum_schema(body_struct)
                } else {
                    registry.ensure_struct_schema(body_struct)
                };
                op_fields.push((
                    "requestBody".to_string(),
                    JsonVal::Obj(vec![
                        ("required".to_string(), JsonVal::Bool(true)),
                        (
                            "content".to_string(),
                            JsonVal::Obj(vec![(
                                "application/json".to_string(),
                                JsonVal::Obj(vec![(
                                    "schema".to_string(),
                                    JsonVal::Obj(vec![(
                                        "$ref".to_string(),
                                        JsonVal::Str(format!("#/components/schemas/{}", sname)),
                                    )]),
                                )]),
                            )]),
                        ),
                    ]),
                ));
            }

            let mut inferred = infer_handler_responses(program, checked, &e.handler_name);
            if e.auto_response
                && let Some(&fid) = checked.func_by_name.get(&e.handler_name)
            {
                let sig = &checked.funcs[fid as usize];
                inferred = infer_auto_return_responses(&sig.return_type, checked, inferred);
            }
            let mut responses_map: BTreeMap<u16, JsonVal> = BTreeMap::new();
            for resp in inferred {
                let mut resp_obj = vec![(
                    "description".to_string(),
                    JsonVal::Str(status_description(resp.status).to_string()),
                )];
                if let Some(media) = resp.media_type {
                    let schema = match &resp.body_ty {
                        Some(ty) => registry.type_to_schema(ty),
                        None => {
                            JsonVal::Obj(vec![("type".to_string(), JsonVal::Str("object".to_string()))])
                        }
                    };
                    resp_obj.push((
                        "content".to_string(),
                        JsonVal::Obj(vec![(
                            media.to_string(),
                            JsonVal::Obj(vec![("schema".to_string(), schema)]),
                        )]),
                    ));
                }
                responses_map.insert(resp.status, JsonVal::Obj(resp_obj));
            }

            if e.body_struct.is_some() || has_query_struct {
                has_body_structs = true;
                responses_map.entry(400).or_insert_with(|| {
                    JsonVal::Obj(vec![
                        ("description".to_string(), JsonVal::Str("Bad Request".to_string())),
                        (
                            "content".to_string(),
                            JsonVal::Obj(vec![(
                                "application/json".to_string(),
                                JsonVal::Obj(vec![(
                                    "schema".to_string(),
                                    JsonVal::Obj(vec![(
                                        "$ref".to_string(),
                                        JsonVal::Str(
                                            "#/components/schemas/InvalidJsonErrorResponse"
                                                .to_string(),
                                        ),
                                    )]),
                                )]),
                            )]),
                        ),
                    ])
                });
                responses_map.entry(422).or_insert_with(|| {
                    JsonVal::Obj(vec![
                        (
                            "description".to_string(),
                            JsonVal::Str("Validation Error".to_string()),
                        ),
                        (
                            "content".to_string(),
                            JsonVal::Obj(vec![(
                                "application/json".to_string(),
                                JsonVal::Obj(vec![(
                                    "schema".to_string(),
                                    JsonVal::Obj(vec![(
                                        "$ref".to_string(),
                                        JsonVal::Str(
                                            "#/components/schemas/ValidationErrorResponse"
                                                .to_string(),
                                        ),
                                    )]),
                                )]),
                            )]),
                        ),
                    ])
                });
            }

            let responses_entries: Vec<(String, JsonVal)> = responses_map
                .into_iter()
                .map(|(code, val)| (code.to_string(), val))
                .collect();
            op_fields.push(("responses".to_string(), JsonVal::Obj(responses_entries)));

            let entry = paths_map.entry(openapi_path.clone()).or_insert_with(|| {
                paths_order.push(openapi_path.clone());
                Vec::new()
            });
            entry.push((http_method_key, JsonVal::Obj(op_fields)));
        }
    }

    if has_body_structs {
        registry.register_standard_error_schemas();
    }

    let paths_val = JsonVal::Obj(
        paths_order
            .into_iter()
            .filter_map(|p| paths_map.remove(&p).map(|ops| (p, JsonVal::Obj(ops))))
            .collect(),
    );

    let mut root = vec![
        ("openapi".to_string(), JsonVal::Str("3.0.3".to_string())),
        (
            "info".to_string(),
            JsonVal::Obj(vec![
                ("title".to_string(), JsonVal::Str(title.to_string())),
                ("version".to_string(), JsonVal::Str(version.to_string())),
            ]),
        ),
    ];
    if let Some(url) = server_url {
        root.push((
            "servers".to_string(),
            JsonVal::Arr(vec![JsonVal::Obj(vec![(
                "url".to_string(),
                JsonVal::Str(url.to_string()),
            )])]),
        ));
    }
    root.push(("paths".to_string(), paths_val));

    if !registry.schemas.is_empty() {
        let schemas_entries: Vec<(String, JsonVal)> = registry.schemas.into_iter().collect();
        root.push((
            "components".to_string(),
            JsonVal::Obj(vec![(
                "schemas".to_string(),
                JsonVal::Obj(schemas_entries),
            )]),
        ));
    }

    Ok(JsonVal::Obj(root).to_pretty_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::check_source;

    #[test]
    fn formats_routes_and_generates_openapi_with_validation_and_responses() {
        let src = r#"
import * as http from "cdt.http"
import * as validation from "cdt.validation"

enum Role {
    Admin
    Member
}

struct Tag {
    name: String @validation.not_empty
}

struct CreateUser {
    email: String @validation.email
    age: Int @validation.range(18, 120)
    role: Role
    nickname: String? @validation.min_len(2)
    tags: [Tag] = []
    tier: String @validation.one_of("free", "pro") = "free"
}

struct UserReply {
    id: String
    email: String
}

struct ErrReply {
    code: String
}

router UsersApi "User management API" {
    cors "*"
    group "/admin" {
        before require_admin
        post "/users/:id/promote" => promote_user
    }
    post "/users" => create_user
    get "/users/:id" => get_user
    delete "/users/:id" => delete_user
    static "/assets" => "./public"
}

func require_admin(req: http.Request) -> http.Response? {
    return nil
}

func promote_user(req: http.Request) -> http.Response {
    return http.ok_json(UserReply(id: "1", email: "a@b.com"))
}

func create_user(req: http.Request, body: CreateUser) -> http.Response {
    if body.age < 21 {
        return http.status_json(409, ErrReply(code: "too_young"))
    }
    return http.status_json(201, UserReply(id: "42", email: body.email))
}

func get_user(req: http.Request) -> http.Response {
    return http.ok_json(UserReply(id: req.param("id") ?? "0", email: "a@b.com"))
}

func delete_user(req: http.Request) -> http.Response {
    return http.no_content()
}
"#;
        let (prog, checked) = check_source(src).unwrap();
        let routes_txt = format_routes(&checked, None).unwrap();
        assert!(routes_txt.contains("router UsersApi — User management API"));
        assert!(routes_txt.contains("cors: *"));
        assert!(routes_txt.contains("static: /assets -> ./public"));
        assert!(routes_txt.contains("/admin/users/:id/promote"));
        assert!(routes_txt.contains("before: require_admin"));
        assert!(routes_txt.contains("create_user"));
        assert!(routes_txt.contains("CreateUser"));

        let spec = generate_openapi_json(
            &prog,
            &checked,
            "Users API",
            "1.0.0",
            None,
            Some("http://localhost:8080"),
        )
        .unwrap();
        assert!(spec.contains(r#""openapi": "3.0.3""#));
        assert!(spec.contains(r#""url": "http://localhost:8080""#));
        assert!(spec.contains(r#""/admin/users/{id}/promote""#));
        assert!(spec.contains(r#""/users/{id}""#));
        assert!(spec.contains(r#""format": "email""#));
        assert!(spec.contains(r#""minimum": 18"#));
        assert!(spec.contains(r#""maximum": 120"#));
        assert!(spec.contains(r#""minLength": 2"#));
        assert!(spec.contains(r#""nullable": true"#));
        assert!(spec.contains(r#""201":"#));
        assert!(spec.contains(r#""409":"#));
        assert!(spec.contains(r#""204":"#));
        assert!(spec.contains(r#""400":"#));
        assert!(spec.contains(r#""422":"#));
        assert!(spec.contains("InvalidJsonErrorResponse"));
        assert!(spec.contains("ValidationErrorResponse"));
        assert!(spec.contains(r#""Role""#));
    }

    #[test]
    fn errors_when_no_router_exists() {
        let src = "export func main() -> Void { return }\n";
        let (prog, checked) = check_source(src).unwrap();
        let err_routes = format_routes(&checked, None).unwrap_err();
        assert!(err_routes.contains("No 'router' declarations found"));
        let err_spec = generate_openapi_json(&prog, &checked, "App", "0.1.0", None, None).unwrap_err();
        assert!(err_spec.contains("No 'router' declarations found"));
    }

    #[test]
    fn wildcard_param_converts_to_braces() {
        let (path, params) = pattern_to_openapi_path("/files/*filepath/meta/:id");
        assert_eq!(path, "/files/{filepath}/meta/{id}");
        assert_eq!(params, vec!["filepath".to_string(), "id".to_string()]);
    }
}
