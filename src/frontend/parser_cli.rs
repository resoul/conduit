use crate::frontend::ast::*;
use crate::frontend::lexer::{Span, TokenKind};
use crate::frontend::parser::{Parser, ParserError};

impl Parser {
    pub(super) fn parse_cli_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'cli'
        // Support both `cli App "desc" { ... }` and `cli group App "desc" { ... }`
        if matches!(&self.peek().kind, TokenKind::Ident(s) if s == "group") {
            self.bump();
        }
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_cli_body(&name, is_exported)?;
        self.expect(&TokenKind::RBrace)?;

        let mut item = Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        );
        item.doc = about;
        Ok(item)
    }

    fn try_take_string(&mut self) -> Option<String> {
        if let TokenKind::String(s) = &self.peek().kind {
            let val = s.clone();
            self.bump();
            Some(val)
        } else {
            None
        }
    }

    fn parse_cli_body(
        &mut self,
        parent_name: &str,
        is_exported: bool,
    ) -> Result<Vec<FieldDef>, ParserError> {
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Expected 'flag', 'opt', 'arg', 'rest', 'cmd', or 'group' in cli block, got '{:?}'",
                        tok.kind
                    ),
                    span: tok.span,
                });
            };
            let field = match kw.as_str() {
                "flag" => self.parse_cli_flag()?,
                "opt" => self.parse_cli_opt()?,
                "arg" => self.parse_cli_arg()?,
                "rest" => self.parse_cli_rest()?,
                "cmd" => self.parse_cli_sub(parent_name, is_exported, false)?,
                "group" => self.parse_cli_sub(parent_name, is_exported, true)?,
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown cli member '{}' (expected flag, opt, arg, rest, cmd, or group)",
                            other
                        ),
                        span: tok.span,
                    });
                }
            };
            fields.push(field);
        }
        Ok(fields)
    }

    fn parse_cli_flag(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let (short, help, default) = self.parse_cli_meta_and_default(true, span)?;
        let default = default.or(Some(Expr {
            kind: ExprKind::Bool(false),
            ty: crate::frontend::types::Type::Void,
            span,
        }));
        let ann = make_cli_ann("flag", short, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty: TypeExpr::Named("Bool".to_string()),
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_opt(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let (short, help, default) = self.parse_cli_meta_and_default(true, span)?;
        let ann = make_cli_ann("opt", short, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_arg(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type_expr()?;
        let (_, help, default) = self.parse_cli_meta_and_default(false, span)?;
        let ann = make_cli_ann("arg", None, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_rest(&mut self) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) {
            self.parse_type_expr()?
        } else {
            TypeExpr::List(Box::new(TypeExpr::Named("String".to_string())))
        };
        let help = self.try_take_string();
        let default = Some(Expr {
            kind: ExprKind::List(Vec::new()),
            ty: crate::frontend::types::Type::Void,
            span,
        });
        let ann = make_cli_ann("rest", None, help, span);
        Ok(FieldDef {
            name,
            json_name: None,
            ty,
            annotations: vec![ann],
            default,
            span,
        })
    }

    fn parse_cli_sub(
        &mut self,
        parent_name: &str,
        is_exported: bool,
        is_group: bool,
    ) -> Result<FieldDef, ParserError> {
        let span = self.bump().span;
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        let sub_struct_name = format!("{}_{}", parent_name, name);
        let sub_fields = if self.match_token(&TokenKind::LBrace) {
            let f = self.parse_cli_body(&sub_struct_name, is_exported)?;
            self.expect(&TokenKind::RBrace)?;
            f
        } else {
            Vec::new()
        };

        let mut sub_item = Item::new_exported(
            ItemKind::Struct {
                name: sub_struct_name.clone(),
                type_params: Vec::new(),
                fields: sub_fields,
            },
            span,
            is_exported,
        );
        sub_item.doc = about.clone();
        self.pending_items.push(sub_item);

        let ann_name = if is_group { "group" } else { "cmd" };
        let mut args = Vec::new();
        if let Some(h) = about {
            args.push(str_expr(h, span));
        }
        let ann = FieldAnnotation {
            target: format!("cli.{}", ann_name),
            args,
            span,
        };
        let mut annotations = vec![ann];
        if !is_group && self.match_token(&TokenKind::FatArrow) {
            let (handler_name, hspan) = self.expect_ident()?;
            annotations.push(FieldAnnotation {
                target: "cli.handler".to_string(),
                args: vec![Expr {
                    kind: ExprKind::Ident(handler_name),
                    ty: crate::frontend::types::Type::Void,
                    span: hspan,
                }],
                span: hspan,
            });
        }
        Ok(FieldDef {
            name,
            json_name: None,
            ty: TypeExpr::Optional(Box::new(TypeExpr::Named(sub_struct_name))),
            annotations,
            default: None,
            span,
        })
    }

    fn parse_cli_meta_and_default(
        &mut self,
        allow_short: bool,
        _span: Span,
    ) -> Result<CliMetaAndDefault, ParserError> {
        let mut short: Option<String> = None;
        let mut help: Option<String> = None;
        let mut default: Option<Expr> = None;

        for _ in 0..3 {
            if self.match_token(&TokenKind::Assign) {
                if default.is_some() {
                    return Err(ParserError {
                        message: "Duplicate default value in cli field".to_string(),
                        span: self.peek().span,
                    });
                }
                default = Some(self.parse_expr()?);
            } else if let Some(s) = self.try_take_string() {
                let normalized_short = s.strip_prefix('-').unwrap_or(&s);
                if allow_short && short.is_none() && normalized_short.chars().count() == 1 {
                    short = Some(normalized_short.to_string());
                } else if help.is_none() {
                    help = Some(s);
                } else {
                    return Err(ParserError {
                        message: "Unexpected extra string in cli field".to_string(),
                        span: self.peek().span,
                    });
                }
            } else {
                break;
            }
        }
        Ok((short, help, default))
    }
}

type CliMetaAndDefault = (Option<String>, Option<String>, Option<Expr>);

fn str_expr(s: String, span: Span) -> Expr {
    Expr {
        kind: ExprKind::String(s),
        ty: crate::frontend::types::Type::Void,
        span,
    }
}

fn make_cli_ann(target: &str, short: Option<String>, help: Option<String>, span: Span) -> FieldAnnotation {
    let mut args = Vec::new();
    match (short, help) {
        (Some(s), Some(h)) => {
            args.push(str_expr(s, span));
            args.push(str_expr(h, span));
        }
        (Some(s), None) => args.push(str_expr(s, span)),
        (None, Some(h)) => args.push(str_expr(h, span)),
        (None, None) => {}
    }
    FieldAnnotation {
        target: format!("cli.{}", target),
        args,
        span,
    }
}

impl Parser {
    pub(super) fn parse_router_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'router'
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        let mut idx = 0usize;
        self.parse_router_body("", &mut fields, &mut idx)?;
        self.expect(&TokenKind::RBrace)?;

        let mut item = Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        );
        item.doc = about;
        Ok(item)
    }

    fn parse_router_body(
        &mut self,
        prefix: &str,
        fields: &mut Vec<FieldDef>,
        idx: &mut usize,
    ) -> Result<(), ParserError> {
        self.parse_router_body_with_guard(prefix, None, fields, idx)
    }

    fn parse_router_body_with_guard(
        &mut self,
        prefix: &str,
        guard_fn: Option<(&str, Span)>,
        fields: &mut Vec<FieldDef>,
        idx: &mut usize,
    ) -> Result<(), ParserError> {
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            if matches!(tok.kind, TokenKind::KwGuard) {
                self.bump();
                let (g_name, g_span) = self.expect_ident()?;
                self.expect(&TokenKind::LBrace)?;
                self.parse_router_body_with_guard(prefix, Some((&g_name, g_span)), fields, idx)?;
                self.expect(&TokenKind::RBrace)?;
                continue;
            }
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Expected 'group', 'guard', 'static', 'cors', 'before', 'after', 'get', 'post', 'put', 'delete', 'patch', 'options', 'head', or 'any' in router block, got '{:?}'",
                        tok.kind
                    ),
                    span: tok.span,
                });
            };
            let span = self.bump().span;
            if kw == "group" {
                let gspan = self.peek().span;
                let Some(sub_prefix) = self.try_take_string() else {
                    return Err(ParserError {
                        message: "Expected path prefix string after 'group'".to_string(),
                        span: gspan,
                    });
                };
                let joined = join_route_prefix(prefix, &sub_prefix);
                self.expect(&TokenKind::LBrace)?;
                self.parse_router_body_with_guard(&joined, guard_fn, fields, idx)?;
                self.expect(&TokenKind::RBrace)?;
                continue;
            }
            if kw == "guard" {
                let (g_name, g_span) = self.expect_ident()?;
                self.expect(&TokenKind::LBrace)?;
                self.parse_router_body_with_guard(prefix, Some((&g_name, g_span)), fields, idx)?;
                self.expect(&TokenKind::RBrace)?;
                continue;
            }
            let ann = match kw.as_str() {
                "before" | "after" => {
                    let _ = self.match_token(&TokenKind::FatArrow);
                    let (handler_name, hspan) = self.expect_ident()?;
                    if prefix.is_empty() {
                        FieldAnnotation {
                            target: format!("router.{}", kw),
                            args: vec![Expr {
                                kind: ExprKind::Ident(handler_name),
                                ty: crate::frontend::types::Type::Void,
                                span: hspan,
                            }],
                            span,
                        }
                    } else {
                        FieldAnnotation {
                            target: format!("router.group_{}", kw),
                            args: vec![
                                str_expr(prefix.to_string(), span),
                                Expr {
                                    kind: ExprKind::Ident(handler_name),
                                    ty: crate::frontend::types::Type::Void,
                                    span: hspan,
                                },
                            ],
                            span,
                        }
                    }
                }
                "on_start" | "on_stop" | "on" => {
                    let hook_kw = if kw == "on" {
                        let (sub, sub_span) = self.expect_ident()?;
                        match sub.as_str() {
                            "start" => "on_start",
                            "stop" => "on_stop",
                            other => {
                                return Err(ParserError {
                                    message: format!(
                                        "Expected 'start' or 'stop' after 'on' in router block, got '{}'",
                                        other
                                    ),
                                    span: sub_span,
                                });
                            }
                        }
                    } else {
                        kw.as_str()
                    };
                    if !prefix.is_empty() || guard_fn.is_some() {
                        return Err(ParserError {
                            message: "on_start/on_stop must be declared at the root of a router block".to_string(),
                            span,
                        });
                    }
                    let _ = self.match_token(&TokenKind::FatArrow);
                    let (handler_name, hspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: format!("router.{}", hook_kw),
                        args: vec![Expr {
                            kind: ExprKind::Ident(handler_name),
                            ty: crate::frontend::types::Type::Void,
                            span: hspan,
                        }],
                        span,
                    }
                }
                "static" => {
                    let pspan = self.peek().span;
                    let Some(url_pat) = self.try_take_string() else {
                        return Err(ParserError {
                            message: "Expected URL prefix string after 'static'".to_string(),
                            span: pspan,
                        });
                    };
                    self.expect(&TokenKind::FatArrow)?;
                    let dspan = self.peek().span;
                    let Some(dir_path) = self.try_take_string() else {
                        return Err(ParserError {
                            message: "Expected directory path string after '=>'".to_string(),
                            span: dspan,
                        });
                    };
                    let full_prefix = join_route_prefix(prefix, &url_pat);
                    FieldAnnotation {
                        target: "router.static".to_string(),
                        args: vec![str_expr(full_prefix, pspan), str_expr(dir_path, dspan)],
                        span,
                    }
                }
                "cors" => {
                    let ospan = self.peek().span;
                    let Some(origin) = self.try_take_string() else {
                        return Err(ParserError {
                            message: "Expected origin string (e.g. \"*\") after 'cors'".to_string(),
                            span: ospan,
                        });
                    };
                    FieldAnnotation {
                        target: "router.cors".to_string(),
                        args: vec![str_expr(origin, ospan)],
                        span,
                    }
                }
                "get" | "post" | "put" | "delete" | "patch" | "options" | "head" | "any" => {
                    let pspan = self.peek().span;
                    let Some(pat) = self.try_take_string() else {
                        return Err(ParserError {
                            message: format!("Expected route pattern string after '{}'", kw),
                            span: pspan,
                        });
                    };
                    let full_pat = join_route_prefix(prefix, &pat);
                    self.expect(&TokenKind::FatArrow)?;
                    let (handler_name, hspan) = self.expect_ident()?;
                    let mut route_args = vec![
                        str_expr(full_pat, pspan),
                        Expr {
                            kind: ExprKind::Ident(handler_name),
                            ty: crate::frontend::types::Type::Void,
                            span: hspan,
                        },
                    ];
                    if let Some((g_name, g_span)) = guard_fn {
                        route_args.push(Expr {
                            kind: ExprKind::Ident(g_name.to_string()),
                            ty: crate::frontend::types::Type::Void,
                            span: g_span,
                        });
                    }
                    FieldAnnotation {
                        target: format!("router.{}", kw),
                        args: route_args,
                        span,
                    }
                }
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown router member '{}' (expected group, static, cors, before, after, get, post, put, delete, patch, options, head, or any)",
                            other
                        ),
                        span: tok.span,
                    });
                }
            };
            fields.push(FieldDef {
                name: format!("__route_{}", *idx),
                json_name: None,
                ty: TypeExpr::Named("Bool".to_string()),
                annotations: vec![ann],
                default: Some(Expr {
                    kind: ExprKind::Bool(false),
                    ty: crate::frontend::types::Type::Void,
                    span,
                }),
                span,
            });
            *idx += 1;
        }
        Ok(())
    }

    pub(super) fn parse_schedule_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'schedule'
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        let mut idx = 0usize;
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Expected 'every' or 'interval' in schedule block, got '{:?}'",
                        tok.kind
                    ),
                    span: tok.span,
                });
            };
            let span = self.bump().span;
            let ann = match kw.as_str() {
                "every" => {
                    let pspan = self.peek().span;
                    let Some(expr) = self.try_take_string() else {
                        return Err(ParserError {
                            message: "Expected cron expression string after 'every'".to_string(),
                            span: pspan,
                        });
                    };
                    self.expect(&TokenKind::FatArrow)?;
                    let (handler_name, hspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: "schedule.every".to_string(),
                        args: vec![
                            str_expr(expr, pspan),
                            Expr {
                                kind: ExprKind::Ident(handler_name),
                                ty: crate::frontend::types::Type::Void,
                                span: hspan,
                            },
                        ],
                        span,
                    }
                }
                "interval" => {
                    let dspan = self.peek().span;
                    let ms = match self.peek().kind.clone() {
                        TokenKind::Int(n) if n > 0 => {
                            self.bump();
                            let unit_tok = self.peek().clone();
                            let mult = match &unit_tok.kind {
                                TokenKind::Ident(u) if u == "ms" => {
                                    self.bump();
                                    1i64
                                }
                                TokenKind::Ident(u) if u == "s" => {
                                    self.bump();
                                    1_000i64
                                }
                                TokenKind::Ident(u) if u == "m" => {
                                    self.bump();
                                    60_000i64
                                }
                                TokenKind::Ident(u) if u == "h" => {
                                    self.bump();
                                    3_600_000i64
                                }
                                _ => {
                                    return Err(ParserError {
                                        message: "Expected duration unit ('ms', 's', 'm', or 'h') after interval integer".to_string(),
                                        span: unit_tok.span,
                                    });
                                }
                            };
                            n.saturating_mul(mult)
                        }
                        _ => {
                            return Err(ParserError {
                                message: "Expected positive integer duration (e.g. 10s, 500ms, 5m) after 'interval'".to_string(),
                                span: dspan,
                            });
                        }
                    };
                    self.expect(&TokenKind::FatArrow)?;
                    let (handler_name, hspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: "schedule.interval".to_string(),
                        args: vec![
                            Expr {
                                kind: ExprKind::Int(ms),
                                ty: crate::frontend::types::Type::Int,
                                span: dspan,
                            },
                            Expr {
                                kind: ExprKind::Ident(handler_name),
                                ty: crate::frontend::types::Type::Void,
                                span: hspan,
                            },
                        ],
                        span,
                    }
                }
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown schedule entry '{}' (expected 'every' or 'interval')",
                            other
                        ),
                        span: tok.span,
                    });
                }
            };
            fields.push(FieldDef {
                name: format!("__job_{}", idx),
                json_name: None,
                ty: TypeExpr::Named("Bool".to_string()),
                annotations: vec![ann],
                default: Some(Expr {
                    kind: ExprKind::Bool(false),
                    ty: crate::frontend::types::Type::Void,
                    span,
                }),
                span,
            });
            idx += 1;
        }
        self.expect(&TokenKind::RBrace)?;

        let mut item = Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        );
        item.doc = about;
        Ok(item)
    }

    pub(super) fn parse_query_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'query'
        let (name, _) = self.expect_ident()?;
        let (params, return_type) = self.parse_params_and_return_type()?;
        self.expect(&TokenKind::LBrace)?;

        let mut model_name: Option<(String, Span)> = None;
        let mut select_fields: Vec<(String, Span)> = Vec::new();
        let mut where_pred: Option<QueryPredicate> = None;
        let mut order_by: Vec<QueryOrderItem> = Vec::new();
        let mut limit: Option<Expr> = None;
        let mut offset: Option<Expr> = None;
        let mut mode: Option<QueryModeAst> = None;

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let tok = self.peek().clone();
            let kw = match &tok.kind {
                TokenKind::Ident(s) => s.clone(),
                _ => {
                    return Err(ParserError {
                        message: format!(
                            "Expected query clause ('from', 'select', 'where', 'order_by', 'limit', 'offset', 'first', 'count', 'exists', 'delete'), got '{:?}'",
                            tok.kind
                        ),
                        span: tok.span,
                    });
                }
            };
            self.bump();
            match kw.as_str() {
                "from" => {
                    if model_name.is_some() {
                        return Err(ParserError {
                            message: "Duplicate 'from' clause in query".to_string(),
                            span: tok.span,
                        });
                    }
                    let (m_name, m_span) = self.expect_ident()?;
                    model_name = Some((m_name, m_span));
                }
                "select" => {
                    loop {
                        let (f_name, f_span) = self.expect_ident()?;
                        select_fields.push((f_name, f_span));
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                "where" => {
                    let pred = self.parse_query_where_or()?;
                    where_pred = Some(match where_pred {
                        Some(existing) => QueryPredicate::And(Box::new(existing), Box::new(pred)),
                        None => pred,
                    });
                }
                "order_by" => {
                    loop {
                        let (f_name, f_span) = self.expect_ident()?;
                        let dir_tok = self.peek().clone();
                        let desc = match &dir_tok.kind {
                            TokenKind::Ident(d) if d.eq_ignore_ascii_case("asc") => {
                                self.bump();
                                false
                            }
                            TokenKind::Ident(d) if d.eq_ignore_ascii_case("desc") => {
                                self.bump();
                                true
                            }
                            _ => false,
                        };
                        order_by.push(QueryOrderItem {
                            field: f_name,
                            field_span: f_span,
                            desc,
                        });
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(ParserError {
                            message: "Duplicate 'limit' clause in query".to_string(),
                            span: tok.span,
                        });
                    }
                    limit = Some(self.parse_expr()?);
                }
                "offset" => {
                    if offset.is_some() {
                        return Err(ParserError {
                            message: "Duplicate 'offset' clause in query".to_string(),
                            span: tok.span,
                        });
                    }
                    offset = Some(self.parse_expr()?);
                }
                "all" | "first" | "count" | "exists" | "delete" => {
                    if mode.is_some() {
                        return Err(ParserError {
                            message: "Duplicate query mode ('all', 'first', 'count', 'exists', 'delete')".to_string(),
                            span: tok.span,
                        });
                    }
                    mode = Some(match kw.as_str() {
                        "first" => QueryModeAst::First,
                        "count" => QueryModeAst::Count,
                        "exists" => QueryModeAst::Exists,
                        "delete" => QueryModeAst::Delete,
                        _ => QueryModeAst::All,
                    });
                }
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown query clause '{}' (expected 'from', 'select', 'where', 'order_by', 'limit', 'offset', 'first', 'count', 'exists', or 'delete')",
                            other
                        ),
                        span: tok.span,
                    });
                }
            }
        }
        self.expect(&TokenKind::RBrace)?;

        let Some((model_name, model_span)) = model_name else {
            return Err(ParserError {
                message: format!("Query '{}' must contain a 'from <Model>' clause", name),
                span: start_span,
            });
        };

        Ok(Item::new_exported(
            ItemKind::Query(Box::new(QuerySpec {
                name,
                params,
                return_type,
                model_name,
                model_span,
                select_fields,
                where_pred,
                order_by,
                limit,
                offset,
                mode: mode.unwrap_or(QueryModeAst::All),
            })),
            start_span,
            is_exported,
        ))
    }

    fn parse_query_where_or(&mut self) -> Result<QueryPredicate, ParserError> {
        let mut left = self.parse_query_where_and()?;
        while self.match_token(&TokenKind::Or) {
            let right = self.parse_query_where_and()?;
            left = QueryPredicate::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_query_where_and(&mut self) -> Result<QueryPredicate, ParserError> {
        let mut left = self.parse_query_where_atom()?;
        while self.match_token(&TokenKind::And) {
            let right = self.parse_query_where_atom()?;
            left = QueryPredicate::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_query_where_atom(&mut self) -> Result<QueryPredicate, ParserError> {
        if self.match_token(&TokenKind::LParen) {
            let inner = self.parse_query_where_or()?;
            self.expect(&TokenKind::RParen)?;
            return Ok(inner);
        }
        let (field, field_span) = self.expect_ident()?;
        let op_tok = self.peek().clone();
        let op = match &op_tok.kind {
            TokenKind::Eq => {
                self.bump();
                QueryWhereOp::Eq
            }
            TokenKind::NotEq => {
                self.bump();
                QueryWhereOp::Ne
            }
            TokenKind::Gt => {
                self.bump();
                QueryWhereOp::Gt
            }
            TokenKind::GtEq => {
                self.bump();
                QueryWhereOp::Ge
            }
            TokenKind::Lt => {
                self.bump();
                QueryWhereOp::Lt
            }
            TokenKind::LtEq => {
                self.bump();
                QueryWhereOp::Le
            }
            TokenKind::KwIn => {
                self.bump();
                QueryWhereOp::In
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("like") => {
                self.bump();
                QueryWhereOp::Like
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("ilike") => {
                self.bump();
                QueryWhereOp::ILike
            }
            _ => {
                return Err(ParserError {
                    message: format!(
                        "Expected comparison operator ('==', '!=', '>', '>=', '<', '<=', 'in', 'like', 'ilike') after '{}', got '{:?}'",
                        field, op_tok.kind
                    ),
                    span: op_tok.span,
                });
            }
        };
        let value = self.parse_query_value_expr()?;
        Ok(QueryPredicate::Cmp {
            field,
            field_span,
            op,
            value: Box::new(value),
        })
    }

    pub(super) fn parse_job_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'job'
        let (name, _) = self.expect_ident()?;
        let tok = self.peek().clone();
        let TokenKind::Ident(kw) = &tok.kind else {
            return Err(ParserError {
                message: format!(
                    "Expected 'every' or 'interval' after job name '{}', got '{:?}'",
                    name, tok.kind
                ),
                span: tok.span,
            });
        };
        let mut cron_expr = None;
        let mut interval_ms = None;
        let expr_span;
        match kw.as_str() {
            "every" => {
                self.bump();
                expr_span = self.peek().span;
                let Some(expr) = self.try_take_string() else {
                    return Err(ParserError {
                        message: "Expected cron expression string after 'every'".to_string(),
                        span: expr_span,
                    });
                };
                cron_expr = Some(expr);
            }
            "interval" => {
                self.bump();
                expr_span = self.peek().span;
                let ms = match self.peek().kind.clone() {
                    TokenKind::Int(n) if n > 0 => {
                        self.bump();
                        let unit_tok = self.peek().clone();
                        let mult = match &unit_tok.kind {
                            TokenKind::Ident(u) if u == "ms" => {
                                self.bump();
                                1i64
                            }
                            TokenKind::Ident(u) if u == "s" => {
                                self.bump();
                                1_000i64
                            }
                            TokenKind::Ident(u) if u == "m" => {
                                self.bump();
                                60_000i64
                            }
                            TokenKind::Ident(u) if u == "h" => {
                                self.bump();
                                3_600_000i64
                            }
                            _ => {
                                return Err(ParserError {
                                    message: "Expected duration unit ('ms', 's', 'm', or 'h') after interval integer".to_string(),
                                    span: unit_tok.span,
                                });
                            }
                        };
                        n.saturating_mul(mult) as u64
                    }
                    _ => {
                        return Err(ParserError {
                            message: "Expected positive integer duration (e.g. 10s, 500ms, 5m) after 'interval'".to_string(),
                            span: expr_span,
                        });
                    }
                };
                interval_ms = Some(ms);
            }
            other => {
                return Err(ParserError {
                    message: format!(
                        "Expected 'every' or 'interval' in job '{}', got '{}'",
                        name, other
                    ),
                    span: tok.span,
                });
            }
        }

        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Item::new_exported(
            ItemKind::Job(Box::new(JobSpec {
                name,
                cron_expr,
                interval_ms,
                expr_span,
                return_type,
                body,
            })),
            start_span,
            is_exported,
        ))
    }

    pub(super) fn parse_lifecycle_item(
        &mut self,
        is_exported: bool,
        export_span: Option<Span>,
    ) -> Result<Item, ParserError> {
        let start_tok = self.bump();
        let start_span = start_tok.span;
        if is_exported {
            return Err(ParserError {
                message: "'export' cannot be applied to lifecycle hooks".to_string(),
                span: export_span.unwrap_or(start_span),
            });
        }
        let is_start = match &start_tok.kind {
            TokenKind::Ident(kw) if kw == "on_start" => true,
            TokenKind::Ident(kw) if kw == "on_stop" => false,
            TokenKind::Ident(kw) if kw == "on" => {
                let (sub, sub_span) = self.expect_ident()?;
                match sub.as_str() {
                    "start" => true,
                    "stop" => false,
                    other => {
                        return Err(ParserError {
                            message: format!(
                                "Expected 'start' or 'stop' after 'on', got '{}'",
                                other
                            ),
                            span: sub_span,
                        });
                    }
                }
            }
            _ => unreachable!(),
        };
        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        let idx = self.lifecycle_counter;
        self.lifecycle_counter += 1;
        let name = if is_start {
            format!("__cdt_on_start_{}", idx)
        } else {
            format!("__cdt_on_stop_{}", idx)
        };
        Ok(Item::new_exported(
            ItemKind::Func {
                name,
                type_params: Vec::new(),
                params: Vec::new(),
                return_type,
                body,
                is_validator: false,
            },
            start_span,
            false,
        ))
    }

    pub(super) fn parse_app_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'app'
        let (name, _) = self.expect_ident()?;
        let about = self.try_take_string();
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        let mut idx = 0usize;
        let mut seen_config = false;
        let mut seen_http = false;
        let mut seen_schedule = false;
        let mut seen_shutdown = false;

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Expected app clause ('config', 'http', 'schedule', 'shutdown', 'on start', or 'on stop'), got '{:?}'",
                        tok.kind
                    ),
                    span: tok.span,
                });
            };
            let span = self.bump().span;
            let ann = match kw.as_str() {
                "config" => {
                    if seen_config {
                        return Err(ParserError {
                            message: format!("Duplicate 'config' clause in app '{}'", name),
                            span,
                        });
                    }
                    seen_config = true;
                    let (cfg_name, cspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: "app.config".to_string(),
                        args: vec![ident_expr(cfg_name, cspan)],
                        span,
                    }
                }
                "http" => {
                    if seen_http {
                        return Err(ParserError {
                            message: format!("Duplicate 'http' clause in app '{}'", name),
                            span,
                        });
                    }
                    seen_http = true;
                    let (router_name, rspan) = self.expect_ident()?;
                    let on_tok = self.peek().clone();
                    match &on_tok.kind {
                        TokenKind::Ident(s) if s == "on" => {
                            self.bump();
                        }
                        _ => {
                            return Err(ParserError {
                                message: format!(
                                    "Expected 'on' after router name '{}' in app '{}'",
                                    router_name, name
                                ),
                                span: on_tok.span,
                            });
                        }
                    }
                    let aspan = self.peek().span;
                    let Some(addr) = self.try_take_string() else {
                        return Err(ParserError {
                            message: format!(
                                "Expected listen address string after 'on' in app '{}'",
                                name
                            ),
                            span: aspan,
                        });
                    };
                    FieldAnnotation {
                        target: "app.http".to_string(),
                        args: vec![ident_expr(router_name, rspan), str_expr(addr, aspan)],
                        span,
                    }
                }
                "schedule" => {
                    if seen_schedule {
                        return Err(ParserError {
                            message: format!("Duplicate 'schedule' clause in app '{}'", name),
                            span,
                        });
                    }
                    seen_schedule = true;
                    let (sched_name, sspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: "app.schedule".to_string(),
                        args: vec![ident_expr(sched_name, sspan)],
                        span,
                    }
                }
                "shutdown" => {
                    if seen_shutdown {
                        return Err(ParserError {
                            message: format!("Duplicate 'shutdown' clause in app '{}'", name),
                            span,
                        });
                    }
                    seen_shutdown = true;
                    let dspan = self.peek().span;
                    let ms = self.parse_positive_duration_ms(&format!("shutdown in app '{}'", name))?;
                    FieldAnnotation {
                        target: "app.shutdown".to_string(),
                        args: vec![int_expr(ms, dspan)],
                        span,
                    }
                }
                "on" => {
                    let (sub, sub_span) = self.expect_ident()?;
                    let target = match sub.as_str() {
                        "start" => "app.on_start",
                        "stop" => "app.on_stop",
                        other => {
                            return Err(ParserError {
                                message: format!(
                                    "Expected 'start' or 'stop' after 'on' in app '{}', got '{}'",
                                    name, other
                                ),
                                span: sub_span,
                            });
                        }
                    };
                    let _ = self.match_token(&TokenKind::FatArrow);
                    let (handler_name, hspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: target.to_string(),
                        args: vec![ident_expr(handler_name, hspan)],
                        span,
                    }
                }
                "on_start" | "on_stop" => {
                    let target = if kw == "on_start" {
                        "app.on_start"
                    } else {
                        "app.on_stop"
                    };
                    let _ = self.match_token(&TokenKind::FatArrow);
                    let (handler_name, hspan) = self.expect_ident()?;
                    FieldAnnotation {
                        target: target.to_string(),
                        args: vec![ident_expr(handler_name, hspan)],
                        span,
                    }
                }
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unknown app clause '{}' in app '{}' (expected 'config', 'http', 'schedule', 'shutdown', 'on start', or 'on stop')",
                            other, name
                        ),
                        span: tok.span,
                    });
                }
            };
            fields.push(FieldDef {
                name: format!("__app_{}", idx),
                json_name: None,
                ty: TypeExpr::Named("Bool".to_string()),
                annotations: vec![ann],
                default: Some(Expr {
                    kind: ExprKind::Bool(false),
                    ty: crate::frontend::types::Type::Void,
                    span,
                }),
                span,
            });
            idx += 1;
        }
        self.expect(&TokenKind::RBrace)?;

        let mut item = Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        );
        item.doc = about;
        Ok(item)
    }

    fn parse_positive_duration_ms(&mut self, ctx: &str) -> Result<i64, ParserError> {
        let dspan = self.peek().span;
        let TokenKind::Int(n) = self.peek().kind else {
            return Err(ParserError {
                message: format!(
                    "Expected positive integer duration (e.g. 100ms, 5s, 1m) for {}",
                    ctx
                ),
                span: dspan,
            });
        };
        if n <= 0 {
            return Err(ParserError {
                message: format!("Expected positive integer duration for {}", ctx),
                span: dspan,
            });
        }
        self.bump();
        let unit_tok = self.peek().clone();
        let mult = match &unit_tok.kind {
            TokenKind::Ident(u) if u == "ms" => {
                self.bump();
                1i64
            }
            TokenKind::Ident(u) if u == "s" => {
                self.bump();
                1_000i64
            }
            TokenKind::Ident(u) if u == "m" => {
                self.bump();
                60_000i64
            }
            TokenKind::Ident(u) if u == "h" => {
                self.bump();
                3_600_000i64
            }
            _ => {
                return Err(ParserError {
                    message: format!(
                        "Expected duration unit ('ms', 's', 'm', or 'h') for {}",
                        ctx
                    ),
                    span: unit_tok.span,
                });
            }
        };
        Ok(n.saturating_mul(mult))
    }

    pub(super) fn parse_worker_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'worker'
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        let params = self.parse_param_list()?;
        if params.is_empty() {
            return Err(ParserError {
                message: format!(
                    "Worker '{}' must declare at least a message parameter (e.g. 'worker {}(msg: T) {{ ... }}')",
                    name, name
                ),
                span: start_span,
            });
        }
        if params[0].default.is_some() {
            return Err(ParserError {
                message: format!(
                    "Worker '{}' message parameter '{}' cannot have a default value",
                    name, params[0].name
                ),
                span: params[0].span,
            });
        }

        let mut return_type: Option<TypeExpr> = None;
        let mut concurrency: Option<i64> = None;
        let mut retry: Option<i64> = None;
        let mut backoff_ms: Option<i64> = None;
        while !self.check(&TokenKind::LBrace) && !self.check(&TokenKind::Eof) {
            if self.match_token(&TokenKind::Arrow) {
                let arrow_span = self.tokens[self.cursor - 1].span;
                if return_type.is_some() {
                    return Err(ParserError {
                        message: format!("Duplicate return type '->' in worker '{}'", name),
                        span: arrow_span,
                    });
                }
                return_type = Some(self.parse_type_expr()?);
                continue;
            }
            let tok = self.peek().clone();
            let TokenKind::Ident(kw) = &tok.kind else {
                return Err(ParserError {
                    message: format!(
                        "Unexpected '{:?}' in worker '{}' (expected 'concurrency', 'retry', 'backoff', '->', or '{{')",
                        tok.kind, name
                    ),
                    span: tok.span,
                });
            };
            self.bump();
            match kw.as_str() {
                "concurrency" => {
                    if concurrency.is_some() {
                        return Err(ParserError {
                            message: format!("Duplicate 'concurrency' in worker '{}'", name),
                            span: tok.span,
                        });
                    }
                    let ntok = self.peek().clone();
                    let TokenKind::Int(n) = ntok.kind else {
                        return Err(ParserError {
                            message: format!(
                                "Expected positive integer after 'concurrency' in worker '{}'",
                                name
                            ),
                            span: ntok.span,
                        });
                    };
                    self.bump();
                    if n < 1 {
                        return Err(ParserError {
                            message: format!(
                                "Worker '{}' concurrency must be at least 1, got {}",
                                name, n
                            ),
                            span: ntok.span,
                        });
                    }
                    concurrency = Some(n);
                }
                "retry" => {
                    if retry.is_some() {
                        return Err(ParserError {
                            message: format!("Duplicate 'retry' in worker '{}'", name),
                            span: tok.span,
                        });
                    }
                    let ntok = self.peek().clone();
                    let TokenKind::Int(n) = ntok.kind else {
                        return Err(ParserError {
                            message: format!(
                                "Expected non-negative integer after 'retry' in worker '{}'",
                                name
                            ),
                            span: ntok.span,
                        });
                    };
                    self.bump();
                    if n < 0 {
                        return Err(ParserError {
                            message: format!(
                                "Worker '{}' retry count must be >= 0, got {}",
                                name, n
                            ),
                            span: ntok.span,
                        });
                    }
                    retry = Some(n);
                }
                "backoff" => {
                    if backoff_ms.is_some() {
                        return Err(ParserError {
                            message: format!("Duplicate 'backoff' in worker '{}'", name),
                            span: tok.span,
                        });
                    }
                    backoff_ms = Some(
                        self.parse_positive_duration_ms(&format!("backoff in worker '{}'", name))?,
                    );
                }
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Unexpected '{}' in worker '{}' (expected 'concurrency', 'retry', 'backoff', '->', or '{{')",
                            other, name
                        ),
                        span: tok.span,
                    });
                }
            }
        }

        let body = self.parse_block()?;
        let step_name = format!("__cdt_worker_step_{}", name);
        let step_params: Vec<Param> = params
            .iter()
            .map(|p| Param {
                name: p.name.clone(),
                ty: p.ty.clone(),
                default: None,
                span: p.span,
            })
            .collect();
        self.pending_items.push(Item::new_exported(
            ItemKind::Func {
                name: step_name.clone(),
                type_params: type_params.clone(),
                params: step_params,
                return_type,
                body,
                is_validator: false,
            },
            start_span,
            false,
        ));

        let msg_ty = params[0].ty.clone();
        let mut runner_params = Vec::with_capacity(params.len());
        runner_params.push(Param {
            name: "__cdt_ch".to_string(),
            ty: TypeExpr::Generic {
                name: "Chan".to_string(),
                type_args: vec![msg_ty],
            },
            default: None,
            span: params[0].span,
        });
        runner_params.extend(params.into_iter().skip(1));

        let runner_body = build_worker_runner_body(
            &step_name,
            &type_params,
            &runner_params[1..],
            concurrency.unwrap_or(1),
            retry.unwrap_or(0),
            backoff_ms.unwrap_or(0),
            start_span,
        );

        Ok(Item::new_exported(
            ItemKind::Func {
                name,
                type_params,
                params: runner_params,
                return_type: Some(TypeExpr::Named("Int".to_string())),
                body: runner_body,
                is_validator: false,
            },
            start_span,
            is_exported,
        ))
    }
}

fn ident_expr(name: String, span: Span) -> Expr {
    Expr::new(ExprKind::Ident(name), span)
}

fn int_expr(val: i64, span: Span) -> Expr {
    Expr::new(ExprKind::Int(val), span)
}

fn bin_expr(op: BinaryOp, left: Expr, right: Expr, span: Span) -> Expr {
    Expr::new(
        ExprKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
        span,
    )
}

fn call_expr(callee: Expr, type_args: Vec<TypeExpr>, args: Vec<Expr>, span: Span) -> Expr {
    Expr::new(
        ExprKind::Call {
            callee: Box::new(callee),
            type_args,
            args,
            named_args: Vec::new(),
        },
        span,
    )
}

fn method_call(target: &str, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    let member = Expr::new(
        ExprKind::Member {
            target: Box::new(ident_expr(target.to_string(), span)),
            field: method.to_string(),
        },
        span,
    );
    call_expr(member, Vec::new(), args, span)
}

fn build_worker_step_stmts(
    step_name: &str,
    type_params: &[String],
    extra_params: &[Param],
    retry: i64,
    backoff_ms: i64,
    span: Span,
) -> Vec<Stmt> {
    let step_type_args: Vec<TypeExpr> = type_params
        .iter()
        .map(|tp| TypeExpr::Named(tp.clone()))
        .collect();
    let mut step_args = Vec::with_capacity(1 + extra_params.len());
    step_args.push(ident_expr("__cdt_msg".to_string(), span));
    for p in extra_params {
        step_args.push(ident_expr(p.name.clone(), p.span));
    }
    let step_call = call_expr(
        ident_expr(step_name.to_string(), span),
        step_type_args,
        step_args,
        span,
    );
    let ok_call = call_expr(
        ident_expr("__cdt_worker_ok".to_string(), span),
        Vec::new(),
        vec![step_call],
        span,
    );
    let inc_count = Stmt::new(
        StmtKind::Assign {
            target: ident_expr("__cdt_count".to_string(), span),
            value: bin_expr(
                BinaryOp::Add,
                ident_expr("__cdt_count".to_string(), span),
                int_expr(1, span),
                span,
            ),
        },
        span,
    );

    if retry == 0 {
        return vec![Stmt::new(
            StmtKind::If {
                condition: ok_call,
                then_block: Block {
                    stmts: vec![inc_count],
                    span,
                },
                else_block: None,
            },
            span,
        )];
    }

    let mut loop_stmts = Vec::new();
    if backoff_ms > 0 {
        loop_stmts.push(Stmt::new(
            StmtKind::If {
                condition: bin_expr(
                    BinaryOp::Gt,
                    ident_expr("__cdt_attempt".to_string(), span),
                    int_expr(0, span),
                    span,
                ),
                then_block: Block {
                    stmts: vec![Stmt::new(
                        StmtKind::Expr(call_expr(
                            ident_expr("__cdt_sleep_ms".to_string(), span),
                            Vec::new(),
                            vec![int_expr(backoff_ms, span)],
                            span,
                        )),
                        span,
                    )],
                    span,
                },
                else_block: None,
            },
            span,
        ));
    }
    loop_stmts.push(Stmt::new(
        StmtKind::If {
            condition: ok_call,
            then_block: Block {
                stmts: vec![
                    Stmt::new(
                        StmtKind::Assign {
                            target: ident_expr("__cdt_ok".to_string(), span),
                            value: Expr::new(ExprKind::Bool(true), span),
                        },
                        span,
                    ),
                    inc_count,
                ],
                span,
            },
            else_block: Some(Block {
                stmts: vec![Stmt::new(
                    StmtKind::Assign {
                        target: ident_expr("__cdt_attempt".to_string(), span),
                        value: bin_expr(
                            BinaryOp::Add,
                            ident_expr("__cdt_attempt".to_string(), span),
                            int_expr(1, span),
                            span,
                        ),
                    },
                    span,
                )],
                span,
            }),
        },
        span,
    ));

    vec![
        Stmt::new(
            StmtKind::VarDecl {
                name: "__cdt_attempt".to_string(),
                ty: None,
                init: Some(int_expr(0, span)),
            },
            span,
        ),
        Stmt::new(
            StmtKind::VarDecl {
                name: "__cdt_ok".to_string(),
                ty: None,
                init: Some(Expr::new(ExprKind::Bool(false), span)),
            },
            span,
        ),
        Stmt::new(
            StmtKind::While {
                condition: bin_expr(
                    BinaryOp::And,
                    bin_expr(
                        BinaryOp::LtEq,
                        ident_expr("__cdt_attempt".to_string(), span),
                        int_expr(retry, span),
                        span,
                    ),
                    Expr::new(
                        ExprKind::Unary {
                            op: UnaryOp::Not,
                            expr: Box::new(ident_expr("__cdt_ok".to_string(), span)),
                        },
                        span,
                    ),
                    span,
                ),
                body: Block {
                    stmts: loop_stmts,
                    span,
                },
            },
            span,
        ),
    ]
}

fn build_worker_runner_body(
    step_name: &str,
    type_params: &[String],
    extra_params: &[Param],
    concurrency: i64,
    retry: i64,
    backoff_ms: i64,
    span: Span,
) -> Block {
    let per_msg_stmts =
        build_worker_step_stmts(step_name, type_params, extra_params, retry, backoff_ms, span);
    let init_count = Stmt::new(
        StmtKind::VarDecl {
            name: "__cdt_count".to_string(),
            ty: None,
            init: Some(int_expr(0, span)),
        },
        span,
    );
    let msg_loop = Stmt::new(
        StmtKind::ForIn {
            pattern: Pattern::new(PatternKind::Ident("__cdt_msg".to_string()), span),
            iterable: ident_expr("__cdt_ch".to_string(), span),
            body: Block {
                stmts: per_msg_stmts,
                span,
            },
        },
        span,
    );

    if concurrency <= 1 {
        return Block {
            stmts: vec![
                init_count,
                msg_loop,
                Stmt::new(
                    StmtKind::Return(Some(ident_expr("__cdt_count".to_string(), span))),
                    span,
                ),
            ],
            span,
        };
    }

    let done_chan = Stmt::new(
        StmtKind::Define {
            name: "__cdt_done".to_string(),
            value: call_expr(
                ident_expr("Chan".to_string(), span),
                vec![TypeExpr::Named("Int".to_string())],
                vec![int_expr(concurrency, span)],
                span,
            ),
        },
        span,
    );
    let send_count = Stmt::new(
        StmtKind::Expr(method_call(
            "__cdt_done",
            "send",
            vec![ident_expr("__cdt_count".to_string(), span)],
            span,
        )),
        span,
    );
    let spawn_workers = Stmt::new(
        StmtKind::ForIn {
            pattern: Pattern::new(PatternKind::Ident("__cdt_w".to_string()), span),
            iterable: int_expr(concurrency, span),
            body: Block {
                stmts: vec![Stmt::new(
                    StmtKind::Spawn(Block {
                        stmts: vec![init_count, msg_loop, send_count],
                        span,
                    }),
                    span,
                )],
                span,
            },
        },
        span,
    );
    let init_total = Stmt::new(
        StmtKind::VarDecl {
            name: "__cdt_total".to_string(),
            ty: None,
            init: Some(int_expr(0, span)),
        },
        span,
    );
    let join_workers = Stmt::new(
        StmtKind::ForIn {
            pattern: Pattern::new(PatternKind::Ident("__cdt_w".to_string()), span),
            iterable: int_expr(concurrency, span),
            body: Block {
                stmts: vec![Stmt::new(
                    StmtKind::Switch {
                        expr: method_call("__cdt_done", "recv", Vec::new(), span),
                        cases: vec![
                            SwitchCase {
                                pattern: Pattern::new(
                                    PatternKind::Type {
                                        name: "Int".to_string(),
                                        binding: "__cdt_c".to_string(),
                                    },
                                    span,
                                ),
                                body: Block {
                                    stmts: vec![Stmt::new(
                                        StmtKind::Assign {
                                            target: ident_expr("__cdt_total".to_string(), span),
                                            value: bin_expr(
                                                BinaryOp::Add,
                                                ident_expr("__cdt_total".to_string(), span),
                                                ident_expr("__cdt_c".to_string(), span),
                                                span,
                                            ),
                                        },
                                        span,
                                    )],
                                    span,
                                },
                                span,
                            },
                            SwitchCase {
                                pattern: Pattern::new(PatternKind::Nil, span),
                                body: Block {
                                    stmts: Vec::new(),
                                    span,
                                },
                                span,
                            },
                        ],
                    },
                    span,
                )],
                span,
            },
        },
        span,
    );
    let close_done = Stmt::new(
        StmtKind::Expr(method_call("__cdt_done", "close", Vec::new(), span)),
        span,
    );
    let ret_total = Stmt::new(
        StmtKind::Return(Some(ident_expr("__cdt_total".to_string(), span))),
        span,
    );

    Block {
        stmts: vec![
            done_chan,
            spawn_workers,
            init_total,
            join_workers,
            close_done,
            ret_total,
        ],
        span,
    }
}

fn join_route_prefix(prefix: &str, pat: &str) -> String {
    let p = prefix.trim_end_matches('/');
    let s = pat.trim_start_matches('/');
    if p.is_empty() {
        if s.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", s)
        }
    } else if s.is_empty() {
        p.to_string()
    } else {
        format!("{}/{}", p, s)
    }
}
