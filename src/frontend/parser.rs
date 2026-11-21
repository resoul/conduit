use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::lexer::{Span, Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParserError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Syntax error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for ParserError {}

pub struct Parser {
    pub(super) tokens: Vec<Token>,
    pub(super) cursor: usize,
    pub(super) pending_items: Vec<Item>,
    pub(super) lifecycle_counter: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0, pending_items: Vec::new(), lifecycle_counter: 0 }
    }

    pub(super) fn peek(&self) -> &Token {
        if self.cursor < self.tokens.len() {
            &self.tokens[self.cursor]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    pub(super) fn peek_ahead(&self, n: usize) -> &Token {
        let idx = self.cursor + n;
        if idx < self.tokens.len() {
            &self.tokens[idx]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    pub(super) fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.cursor < self.tokens.len() {
            self.cursor += 1;
        }
        tok
    }

    pub(super) fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    pub(super) fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn expect(&mut self, expected: &TokenKind) -> Result<Token, ParserError> {
        if self.check(expected) {
            Ok(self.bump())
        } else {
            let current = self.peek();
            Err(ParserError {
                message: format!("Expected '{:?}', got '{:?}'", expected, current.kind),
                span: current.span,
            })
        }
    }

    pub(super) fn expect_ident(&mut self) -> Result<(String, Span), ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Ident(name) => {
                self.bump();
                Ok((name, tok.span))
            }
            _ => Err(ParserError {
                message: format!("Expected identifier, got '{:?}'", tok.kind),
                span: tok.span,
            }),
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, ParserError> {
        let mut items = Vec::new();
        while !self.check(&TokenKind::Eof) {
            let item = self.parse_item()?;
            items.append(&mut self.pending_items);
            items.push(item);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParserError> {
        let mut is_validator = false;
        let mut validator_span = None;
        let mut is_exported = false;
        let mut export_span = None;
        let mut item_annotations = Vec::new();

        loop {
            if self.check(&TokenKind::LBracket) {
                let marker_span = self.peek().span;
                self.bump();
                self.expect(&TokenKind::Hash)?;
                let (marker_name, _) = self.expect_ident()?;
                self.expect(&TokenKind::RBracket)?;
                if !marker_name.eq_ignore_ascii_case("validator") {
                    return Err(ParserError {
                        message: format!("Unknown marker '[#{}]'", marker_name),
                        span: marker_span,
                    });
                }
                if is_validator {
                    return Err(ParserError {
                        message: "Duplicate marker '[#validator]'".to_string(),
                        span: marker_span,
                    });
                }
                is_validator = true;
                validator_span = Some(marker_span);
            } else if self.match_token(&TokenKind::At) {
                item_annotations.push(self.parse_at_annotation()?);
            } else if self.match_token(&TokenKind::KwExport) {
                let sp = self.tokens[self.cursor - 1].span;
                if is_exported {
                    return Err(ParserError {
                        message: "Repeated 'export' modifier".to_string(),
                        span: sp,
                    });
                }
                is_exported = true;
                export_span = Some(sp);
            } else {
                break;
            }
        }

        if is_exported && self.check(&TokenKind::KwImport) {
            return Err(ParserError {
                message: "'export' cannot be applied to 'import'".to_string(),
                span: self.peek().span,
            });
        }

        let tok = self.peek();
        if is_validator && !matches!(tok.kind, TokenKind::KwFunc) {
            return Err(ParserError {
                message: "Marker '[#validator]' is only allowed on function declarations"
                    .to_string(),
                span: validator_span.unwrap_or(tok.span),
            });
        }
        if is_exported && matches!(tok.kind, TokenKind::LBrace) {
            let fallback_span = tok.span;
            let names = self.parse_specifier_list()?;
            return Ok(Item::new_exported(
                ItemKind::ExportList { names },
                export_span.unwrap_or(fallback_span),
                true,
            ));
        }
        if !item_annotations.is_empty() && !matches!(tok.kind, TokenKind::KwStruct | TokenKind::KwError) {
            return Err(ParserError {
                message: "Item annotations (e.g. '@db.table(...)', '@http.status(...)') are only allowed on struct or error declarations"
                    .to_string(),
                span: item_annotations[0].span,
            });
        }
        let mut item = match &tok.kind {
            TokenKind::KwFunc => self.parse_func_item(is_exported, is_validator),
            TokenKind::KwStruct => self.parse_struct_item(is_exported),
            TokenKind::KwType => self.parse_type_alias_item(is_exported),
            TokenKind::KwConst => self.parse_const_item(is_exported),
            TokenKind::KwEnum => self.parse_enum_item(is_exported),
            TokenKind::KwError => self.parse_error_item(is_exported),
            TokenKind::Ident(kw) if kw == "cli" => self.parse_cli_item(is_exported),
            TokenKind::Ident(kw) if kw == "config" => self.parse_config_item(is_exported),
            TokenKind::Ident(kw) if kw == "router" => self.parse_router_item(is_exported),
            TokenKind::Ident(kw) if kw == "schedule" => self.parse_schedule_item(is_exported),
            TokenKind::Ident(kw) if kw == "app" => self.parse_app_item(is_exported),
            TokenKind::Ident(kw) if kw == "job" => self.parse_job_item(is_exported),
            TokenKind::Ident(kw) if kw == "worker" => self.parse_worker_item(is_exported),
            TokenKind::Ident(kw) if kw == "migration" => self.parse_migration_item(),
            TokenKind::Ident(kw) if kw == "query" => self.parse_query_item(is_exported),
            TokenKind::Ident(kw) if matches!(kw.as_str(), "on" | "on_start" | "on_stop") => {
                self.parse_lifecycle_item(is_exported, export_span)
            }
            TokenKind::Ident(kw) if kw == "test" => self.parse_test_item(is_exported, export_span),
            TokenKind::KwImport => {
                if is_exported {
                    return Err(ParserError {
                        message: "'export' cannot be applied to 'import'".to_string(),
                        span: export_span.unwrap_or(tok.span),
                    });
                }
                self.parse_import_item()
            }
            _ => Err(ParserError {
                message: format!(
                    "Expected 'func', 'struct', 'type', 'error', or 'import', got '{:?}'",
                    tok.kind
                ),
                span: tok.span,
            }),
        }?;
        item.annotations = item_annotations;
        Ok(item)
    }

    fn parse_migration_item(&mut self) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'migration'
        let ver_tok = self.peek().clone();
        let version = match ver_tok.kind {
            TokenKind::Int(v) if v >= 1 => {
                self.bump();
                v
            }
            _ => {
                return Err(ParserError {
                    message: "Expected positive integer migration version after 'migration'"
                        .to_string(),
                    span: ver_tok.span,
                });
            }
        };
        let name_tok = self.peek().clone();
        let name = match name_tok.kind {
            TokenKind::String(s) => {
                self.bump();
                s
            }
            _ => {
                return Err(ParserError {
                    message: "Expected migration name string after version".to_string(),
                    span: name_tok.span,
                });
            }
        };
        self.expect(&TokenKind::LBrace)?;
        let mut up_block: Option<Block> = None;
        let mut down_block: Option<Block> = None;
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let kw_tok = self.peek().clone();
            let TokenKind::Ident(kw) = &kw_tok.kind else {
                return Err(ParserError {
                    message: "Expected 'up' or 'down' inside migration block".to_string(),
                    span: kw_tok.span,
                });
            };
            let is_up = match kw.as_str() {
                "up" => true,
                "down" => false,
                other => {
                    return Err(ParserError {
                        message: format!(
                            "Expected 'up' or 'down' in migration block, got '{}'",
                            other
                        ),
                        span: kw_tok.span,
                    });
                }
            };
            self.bump();
            let block = if let TokenKind::String(sql) = &self.peek().kind {
                let sql_str = sql.clone();
                let sp = self.bump().span;
                // Desugar `up "SQL"` -> `{ db.exec("SQL")? }`
                let call = Expr::new(
                    ExprKind::Call {
                        callee: Box::new(Expr::new(
                            ExprKind::Member {
                                target: Box::new(Expr::new(
                                    ExprKind::Ident("db".to_string()),
                                    sp,
                                )),
                                field: "exec".to_string(),
                            },
                            sp,
                        )),
                        type_args: Vec::new(),
                        args: vec![Expr::new(ExprKind::String(sql_str), sp)],
                        named_args: Vec::new(),
                    },
                    sp,
                );
                Block {
                    stmts: vec![Stmt::new(
                        StmtKind::Expr(Expr::new(ExprKind::Try(Box::new(call)), sp)),
                        sp,
                    )],
                    span: sp,
                }
            } else {
                self.parse_block()?
            };
            if is_up {
                if up_block.is_some() {
                    return Err(ParserError {
                        message: "Duplicate 'up' block in migration".to_string(),
                        span: kw_tok.span,
                    });
                }
                up_block = Some(block);
            } else {
                if down_block.is_some() {
                    return Err(ParserError {
                        message: "Duplicate 'down' block in migration".to_string(),
                        span: kw_tok.span,
                    });
                }
                down_block = Some(block);
            }
        }
        self.expect(&TokenKind::RBrace)?;
        let up = up_block.ok_or_else(|| ParserError {
            message: "Migration block must define 'up'".to_string(),
            span: start_span,
        })?;
        let down = down_block.unwrap_or(Block {
            stmts: Vec::new(),
            span: start_span,
        });
        Ok(Item::new_exported(
            ItemKind::Migration {
                version,
                name,
                up,
                down,
            },
            start_span,
            false,
        ))
    }

    fn parse_test_item(
        &mut self,
        is_exported: bool,
        export_span: Option<Span>,
    ) -> Result<Item, ParserError> {
        let start_span = self.bump().span; // consume 'test'
        if is_exported {
            return Err(ParserError {
                message: "'export' cannot be applied to 'test'".to_string(),
                span: export_span.unwrap_or(start_span),
            });
        }
        let name_tok = self.peek().clone();
        let name = match name_tok.kind {
            TokenKind::String(s) if !s.is_empty() => {
                self.bump();
                s
            }
            _ => {
                return Err(ParserError {
                    message: "Expected non-empty test name string after 'test'".to_string(),
                    span: name_tok.span,
                });
            }
        };
        let body = self.parse_block()?;
        Ok(Item::new_exported(
            ItemKind::Test { name, body },
            start_span,
            false,
        ))
    }

    fn expect_from_keyword(&mut self) -> Result<(), ParserError> {
        let tok = self.peek().clone();
        if matches!(&tok.kind, TokenKind::Ident(kw) if kw == "from") {
            self.bump();
            Ok(())
        } else {
            Err(ParserError {
                message: format!("Expected 'from', got '{:?}'", tok.kind),
                span: tok.span,
            })
        }
    }

    fn parse_specifier_list(&mut self) -> Result<Vec<ImportSpecifier>, ParserError> {
        self.expect(&TokenKind::LBrace)?;
        let mut specs = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let (name, _) = self.expect_ident()?;
            let alias = if self.match_token(&TokenKind::KwAs) {
                self.expect_ident()?.0
            } else {
                name.clone()
            };
            specs.push(ImportSpecifier { name, alias });
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(specs)
    }

    fn parse_module_path(&mut self, start_span: Span) -> Result<(String, bool), ParserError> {
        match &self.peek().kind {
            TokenKind::String(s) => {
                let s = s.clone();
                self.bump();
                if s.ends_with(".cdt") {
                    return Err(ParserError {
                        message: format!("Local import path '{}' must not include '.cdt' extension", s),
                        span: start_span,
                    });
                }
                Ok((s, true))
            }
            TokenKind::Ident(first) => {
                let mut path = first.clone();
                self.bump();
                while self.match_token(&TokenKind::Dot) {
                    let (seg, _) = self.expect_ident()?;
                    path.push('.');
                    path.push_str(&seg);
                }
                Ok((path, false))
            }
            other => Err(ParserError {
                message: format!(
                    "Expected module path or string literal in import, got '{:?}'",
                    other
                ),
                span: self.peek().span,
            }),
        }
    }

    fn parse_import_item(&mut self) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwImport)?.span;
        if self.check(&TokenKind::LBrace) {
            let names = self.parse_specifier_list()?;
            self.expect_from_keyword()?;
            let (path, _) = self.parse_module_path(start_span)?;
            return Ok(Item::new_exported(
                ItemKind::Import {
                    path,
                    alias: None,
                    names,
                },
                start_span,
                false,
            ));
        }

        if self.match_token(&TokenKind::Star) {
            self.expect(&TokenKind::KwAs)?;
            let (alias, _) = self.expect_ident()?;
            self.expect_from_keyword()?;
            let (path, _) = self.parse_module_path(start_span)?;
            return Ok(Item::new_exported(
                ItemKind::Import {
                    path,
                    alias: Some(alias),
                    names: Vec::new(),
                },
                start_span,
                false,
            ));
        }

        if matches!(&self.peek().kind, TokenKind::Ident(_))
            && matches!(&self.peek_ahead(1).kind, TokenKind::Ident(kw) if kw == "from")
        {
            let (alias, _) = self.expect_ident()?;
            self.expect_from_keyword()?;
            let (path, _) = self.parse_module_path(start_span)?;
            return Ok(Item::new_exported(
                ItemKind::Import {
                    path,
                    alias: Some(alias),
                    names: Vec::new(),
                },
                start_span,
                false,
            ));
        }

        let (path, is_local) = self.parse_module_path(start_span)?;
        let alias = if is_local {
            if !self.match_token(&TokenKind::KwAs) {
                return Err(ParserError {
                    message: format!("Local import '{}' requires an explicit alias using 'as'", path),
                    span: start_span,
                });
            }
            self.expect_ident()?.0
        } else {
            let default_alias = path.split('.').next_back().unwrap_or(&path).to_string();
            if self.match_token(&TokenKind::KwAs) {
                self.expect_ident()?.0
            } else {
                default_alias
            }
        };
        Ok(Item::new_exported(
            ItemKind::Import {
                path,
                alias: Some(alias),
                names: Vec::new(),
            },
            start_span,
            false,
        ))
    }

    pub(super) fn parse_param_list(&mut self) -> Result<Vec<Param>, ParserError> {
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        let mut seen_default = false;
        while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
            let (name, span) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type_expr()?;
            let default = if self.match_token(&TokenKind::Assign) {
                seen_default = true;
                Some(self.parse_expr()?)
            } else {
                if seen_default {
                    return Err(ParserError {
                        message: format!(
                            "Required parameter '{}' cannot follow a parameter with a default value",
                            name
                        ),
                        span,
                    });
                }
                None
            };
            params.push(Param { name, ty, default, span });
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(params)
    }

    pub(super) fn parse_params_and_return_type(
        &mut self,
    ) -> Result<(Vec<Param>, Option<TypeExpr>), ParserError> {
        let params = self.parse_param_list()?;
        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        Ok((params, return_type))
    }

    pub(super) fn parse_type_params(&mut self) -> Result<Vec<String>, ParserError> {
        if !self.match_token(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        if self.check(&TokenKind::Gt) {
            return Err(ParserError {
                message: "Type parameter list cannot be empty".to_string(),
                span: self.peek().span,
            });
        }
        let mut params = Vec::new();
        let mut seen = HashSet::new();
        while !self.check(&TokenKind::Gt) && !self.check(&TokenKind::Eof) {
            let (param_name, param_span) = self.expect_ident()?;
            if !seen.insert(param_name.clone()) {
                return Err(ParserError {
                    message: format!("Duplicate type parameter '{}'", param_name),
                    span: param_span,
                });
            }
            params.push(param_name);
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        if params.is_empty() {
            return Err(ParserError {
                message: "Type parameter list cannot be empty".to_string(),
                span: self.peek().span,
            });
        }
        self.expect(&TokenKind::Gt)?;
        Ok(params)
    }

    fn parse_func_item(
        &mut self,
        is_exported: bool,
        is_validator: bool,
    ) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwFunc)?.span;
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        let (params, return_type) = self.parse_params_and_return_type()?;
        let body = self.parse_block()?;
        Ok(Item::new_exported(
            ItemKind::Func {
                name,
                type_params,
                params,
                return_type,
                body,
                is_validator,
            },
            start_span,
            is_exported,
        ))
    }

    fn parse_struct_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwStruct)?.span;
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;

        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_field_list(&TokenKind::RBrace)?;
        self.expect(&TokenKind::RBrace)?;
        Ok(Item::new_exported(
            ItemKind::Struct {
                name,
                type_params,
                fields,
            },
            start_span,
            is_exported,
        ))
    }

    fn parse_config_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let (_, start_span) = self.expect_ident()?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = self.parse_field_list(&TokenKind::RBrace)?;
        self.expect(&TokenKind::RBrace)?;
        for f in &mut fields {
            if !f.annotations.iter().any(|a| a.target == "env") {
                f.annotations.push(FieldAnnotation {
                    target: "env".to_string(),
                    args: Vec::new(),
                    span: f.span,
                });
            }
        }
        Ok(Item::new_exported(
            ItemKind::Struct {
                name,
                type_params: Vec::new(),
                fields,
            },
            start_span,
            is_exported,
        ))
    }

    fn parse_field_list(&mut self, closing: &TokenKind) -> Result<Vec<FieldDef>, ParserError> {
        let mut fields = Vec::new();
        let mut field_names = HashSet::new();
        let mut json_names = HashSet::new();
        while !self.check(closing) && !self.check(&TokenKind::Eof) {
            let mut annotations = Vec::new();
            while self.match_token(&TokenKind::At) {
                annotations.push(self.parse_at_annotation()?);
            }
            let (field_name, field_span) = self.expect_ident()?;
            if !field_names.insert(field_name.clone()) {
                return Err(ParserError {
                    message: format!("Duplicate field name '{}'", field_name),
                    span: field_span,
                });
            }
            self.expect(&TokenKind::Colon)?;
            let field_ty = self.parse_type_expr()?;

            let mut json_name = None;
            let mut json_tag_span = None;
            let mut last_line = self.tokens[self.cursor - 1].span.line;
            loop {
                if self.check(&TokenKind::At) && self.peek().span.line == last_line {
                    self.bump();
                    let ann = self.parse_at_annotation()?;
                    last_line = self.tokens[self.cursor - 1].span.line;
                    if ann.target == "json" {
                        if json_tag_span.is_some() {
                            return Err(ParserError {
                                message: "A field can have only one JSON tag".to_string(),
                                span: ann.span,
                            });
                        }
                        let [Expr { kind: ExprKind::String(s), .. }] = ann.args.as_slice() else {
                            return Err(ParserError {
                                message: "@json(...) expects a single string argument".to_string(),
                                span: ann.span,
                            });
                        };
                        if s.contains(',') {
                            return Err(ParserError {
                                message:
                                    "JSON tag options are not supported; specify only the field name"
                                        .to_string(),
                                span: ann.span,
                            });
                        }
                        if !s.is_empty() {
                            json_name = Some(s.clone());
                        }
                        json_tag_span = Some(ann.span);
                    }
                    annotations.push(ann);
                } else if matches!(&self.peek().kind, TokenKind::FieldTag(_)) {
                    if json_tag_span.is_some() {
                        return Err(ParserError {
                            message: "A field can have only one JSON tag".to_string(),
                            span: self.peek().span,
                        });
                    }
                    let (parsed_name, tag_span) = self.parse_json_field_tag()?;
                    json_tag_span = Some(tag_span);
                    if !parsed_name.is_empty() {
                        json_name = Some(parsed_name);
                    }
                } else {
                    break;
                }
            }

            let effective_json_name = json_name.as_deref().unwrap_or(&field_name);
            if !json_names.insert(effective_json_name.to_string()) {
                return Err(ParserError {
                    message: format!("Duplicate JSON field name '{}'", effective_json_name),
                    span: json_tag_span.unwrap_or(field_span),
                });
            }

            let default = if self.match_token(&TokenKind::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            fields.push(FieldDef {
                name: field_name,
                ty: field_ty,
                json_name,
                annotations,
                default,
                span: field_span,
            });
            self.match_token(&TokenKind::Comma);
        }
        Ok(fields)
    }

    fn parse_at_annotation(&mut self) -> Result<FieldAnnotation, ParserError> {
        let ann_span = self.tokens[self.cursor - 1].span;
        let (target_first, _) = self.expect_ident()?;
        let target = if self.match_token(&TokenKind::Dot) {
            format!("{}.{}", target_first, self.expect_ident()?.0)
        } else {
            target_first
        };
        let mut args = Vec::new();
        if self.match_token(&TokenKind::LParen) {
            while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                args.push(self.parse_annotation_arg()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
        }
        Ok(FieldAnnotation {
            target,
            args,
            span: ann_span,
        })
    }

    fn parse_type_alias_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwType)?.span;
        let (name, _) = self.expect_ident()?;
        if self.check(&TokenKind::LBrace) {
            return Err(ParserError {
                message: "Use 'struct' to declare a struct, or 'type Name = ...' for a type alias"
                    .to_string(),
                span: self.peek().span,
            });
        }
        self.expect(&TokenKind::Assign)?;
        let target = self.parse_type_expr()?;
        Ok(Item::new_exported(
            ItemKind::TypeAlias { name, target },
            start_span,
            is_exported,
        ))
    }

    fn parse_const_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwConst)?.span;
        let (name, _) = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;
        Ok(Item::new_exported(
            ItemKind::Const { name, ty, value },
            start_span,
            is_exported,
        ))
    }

    fn parse_enum_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwEnum)?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let (v, vspan) = self.expect_ident()?;
            let fields = if self.match_token(&TokenKind::LParen) {
                let parsed = self.parse_field_list(&TokenKind::RParen)?;
                self.expect(&TokenKind::RParen)?;
                for f in &parsed {
                    let eff = f.json_name.as_deref().unwrap_or(&f.name);
                    if f.name == "type" || eff == "type" {
                        return Err(ParserError {
                            message: format!(
                                "Field '{}' in enum variant '{}.{}' cannot use reserved name 'type'",
                                f.name, name, v
                            ),
                            span: f.span,
                        });
                    }
                }
                parsed
            } else {
                Vec::new()
            };
            variants.push(EnumVariantDef {
                name: v,
                fields,
                span: vspan,
            });
            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;
        if variants.is_empty() {
            return Err(ParserError {
                message: format!("Enum '{}' must have at least one variant", name),
                span: start_span,
            });
        }
        Ok(Item::new_exported(
            ItemKind::Enum { name, variants },
            start_span,
            is_exported,
        ))
    }

    fn parse_json_field_tag(&mut self) -> Result<(String, Span), ParserError> {
        let token = self.bump();
        let span = token.span;
        let TokenKind::FieldTag(raw) = token.kind else {
            unreachable!()
        };
        let Some(mut chars) = raw.strip_prefix("json:\"").map(str::chars) else {
            return Err(ParserError {
                message: "Field tags must use the form `json:\"name\"`".to_string(),
                span,
            });
        };
        let mut name = String::new();
        loop {
            match chars.next() {
                None => {
                    return Err(ParserError {
                        message: "Unterminated JSON field name in tag".to_string(),
                        span,
                    });
                }
                Some('"') => {
                    if chars.next().is_some() {
                        return Err(ParserError {
                            message: "Only a JSON field name is supported in field tags".to_string(),
                            span,
                        });
                    }
                    if name.contains(',') {
                        return Err(ParserError {
                            message: "JSON tag options are not supported; specify only the field name"
                                .to_string(),
                            span,
                        });
                    }
                    return Ok((name, span));
                }
                Some('\\') => {
                    let escaped = match chars.next() {
                        Some('\\') => '\\',
                        Some('"') => '"',
                        Some('a') => '\u{0007}',
                        Some('b') => '\u{0008}',
                        Some('f') => '\u{000C}',
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('v') => '\u{000B}',
                        Some(other) => {
                            return Err(ParserError {
                                message: format!(
                                    "Unsupported escape sequence '\\{}' in JSON field tag",
                                    other
                                ),
                                span,
                            });
                        }
                        None => {
                            return Err(ParserError {
                                message: "Unterminated escape sequence in JSON field tag"
                                    .to_string(),
                                span,
                            });
                        }
                    };
                    name.push(escaped);
                }
                Some(ch) => name.push(ch),
            }
        }
    }

    fn parse_error_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwError)?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let (field_name, field_span) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let field_ty = self.parse_type_expr()?;
            fields.push(FieldDef {
                name: field_name,
                ty: field_ty,
                json_name: None,
                annotations: Vec::new(),
                default: None,
                span: field_span,
            });
            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Item::new_exported(
            ItemKind::Error { name, fields },
            start_span,
            is_exported,
        ))
    }
}
