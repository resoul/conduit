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
