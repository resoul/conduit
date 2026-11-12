use super::*;

impl Parser {
    pub(super) fn parse_item(&mut self) -> Result<Item, ParserError> {
        let mut is_validator = false;
        let mut validator_span = None;

        if self.check(&TokenKind::LBracket) {
            let marker_span = self.peek().span;
            self.bump(); // [
            self.expect(&TokenKind::Hash)?;
            let (marker_name, _) = self.expect_ident()?;
            self.expect(&TokenKind::RBracket)?;

            if !marker_name.eq_ignore_ascii_case("validator") {
                return Err(ParserError {
                    message: format!("Unknown marker '[#{}]'", marker_name),
                    span: marker_span,
                });
            }

            if self.check(&TokenKind::LBracket) {
                return Err(ParserError {
                    message: "Duplicate marker '[#validator]'".to_string(),
                    span: self.peek().span,
                });
            }

            is_validator = true;
            validator_span = Some(marker_span);
        }

        let mut is_exported = false;
        let mut export_span = None;

        if self.match_token(&TokenKind::KwExport) {
            export_span = Some(self.tokens[self.cursor - 1].span);
            is_exported = true;

            if self.check(&TokenKind::KwExport) {
                return Err(ParserError {
                    message: "Repeated 'export' modifier".to_string(),
                    span: self.peek().span,
                });
            }

            if self.check(&TokenKind::KwImport) {
                return Err(ParserError {
                    message: "'export' cannot be applied to 'import'".to_string(),
                    span: self.peek().span,
                });
            }
        }

        let tok = self.peek();
        match tok.kind {
            TokenKind::KwFunc => self.parse_func_item(is_exported, is_validator),
            TokenKind::KwType => {
                if is_validator {
                    return Err(ParserError {
                        message: "Marker '[#validator]' is only allowed on function declarations".to_string(),
                        span: validator_span.unwrap_or(tok.span),
                    });
                }
                self.parse_type_item(is_exported)
            }
            TokenKind::KwError => {
                if is_validator {
                    return Err(ParserError {
                        message: "Marker '[#validator]' is only allowed on function declarations".to_string(),
                        span: validator_span.unwrap_or(tok.span),
                    });
                }
                self.parse_error_item(is_exported)
            }
            TokenKind::KwImport => {
                if is_validator {
                    return Err(ParserError {
                        message: "Marker '[#validator]' is only allowed on function declarations".to_string(),
                        span: validator_span.unwrap_or(tok.span),
                    });
                }
                if is_exported {
                    return Err(ParserError {
                        message: "'export' cannot be applied to 'import'".to_string(),
                        span: export_span.unwrap_or(tok.span),
                    });
                }
                self.parse_import_item()
            }
            _ => {
                if is_validator {
                    return Err(ParserError {
                        message: "Marker '[#validator]' is only allowed on function declarations".to_string(),
                        span: validator_span.unwrap_or(tok.span),
                    });
                }
                if is_exported {
                    return Err(ParserError {
                        message: format!("'export' can only be applied to 'func', 'type', or 'error', got '{:?}'", tok.kind),
                        span: tok.span,
                    });
                }
                Err(ParserError {
                    message: format!("Expected 'func', 'type', 'error', or 'import', got '{:?}'", tok.kind),
                    span: tok.span,
                })
            }
        }
    }

    fn parse_import_item(&mut self) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwImport)?.span;
        let (path, is_local) = match &self.peek().kind {
            TokenKind::String(s) => {
                let s = s.clone();
                self.bump();
                (s, true)
            }
            TokenKind::Ident(first) => {
                let mut path = first.clone();
                self.bump();
                while self.match_token(&TokenKind::Dot) {
                    let (seg, _) = self.expect_ident()?;
                    path.push('.');
                    path.push_str(&seg);
                }
                (path, false)
            }
            other => {
                return Err(ParserError {
                    message: format!("Expected module path or string literal in import, got '{:?}'", other),
                    span: self.peek().span,
                });
            }
        };

        if is_local {
            if path.ends_with(".cdt") {
                return Err(ParserError {
                    message: format!("Local import path '{}' must not include '.cdt' extension", path),
                    span: start_span,
                });
            }

            if !self.match_token(&TokenKind::KwAs) {
                return Err(ParserError {
                    message: format!("Local import '{}' requires an explicit alias using 'as'", path),
                    span: start_span,
                });
            }
            let (alias, _) = self.expect_ident()?;
            Ok(Item::new_exported(ItemKind::Import { path, alias }, start_span, false))
        } else {
            let default_alias = path.split('.').next_back().unwrap_or(&path).to_string();
            let alias = if self.match_token(&TokenKind::KwAs) {
                let (custom_alias, _) = self.expect_ident()?;
                custom_alias
            } else {
                default_alias
            };
            Ok(Item::new_exported(ItemKind::Import { path, alias }, start_span, false))
        }
    }

    pub(super) fn parse_params_and_return_type(&mut self) -> Result<(Vec<Param>, Option<TypeExpr>), ParserError> {
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
            let (param_name, param_span) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let param_ty = self.parse_type_expr()?;
            params.push(Param {
                name: param_name,
                ty: param_ty,
                span: param_span,
            });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;

        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        Ok((params, return_type))
    }

    fn parse_type_params(&mut self) -> Result<Vec<String>, ParserError> {
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
        let mut seen = std::collections::HashSet::new();
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

    pub(super) fn parse_type_arg_list(&mut self) -> Result<Vec<TypeExpr>, ParserError> {
        let start_span = self.expect(&TokenKind::Lt)?.span;
        if self.check(&TokenKind::Gt) {
            return Err(ParserError {
                message: "Type argument list cannot be empty".to_string(),
                span: start_span,
            });
        }
        let mut args = Vec::new();
        while !self.check(&TokenKind::Gt) && !self.check(&TokenKind::Eof) {
            args.push(self.parse_type_expr()?);
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        if args.is_empty() {
            return Err(ParserError {
                message: "Type argument list cannot be empty".to_string(),
                span: start_span,
            });
        }
        self.expect(&TokenKind::Gt)?;
        Ok(args)
    }

    fn parse_func_item(&mut self, is_exported: bool, is_validator: bool) -> Result<Item, ParserError> {
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

    fn parse_type_item(&mut self, is_exported: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwType)?.span;
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;

        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        let mut field_names = HashSet::new();
        let mut json_names = HashSet::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
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
            let mut annotations = Vec::new();
            loop {
                if self.match_token(&TokenKind::At) {
                    let ann_span = self.tokens[self.cursor - 1].span;
                    let (target_first, _) = self.expect_ident()?;
                    let target = if self.match_token(&TokenKind::Dot) {
                        let (sub, _) = self.expect_ident()?;
                        format!("{}.{}", target_first, sub)
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
                    annotations.push(FieldAnnotation {
                        target,
                        args,
                        span: ann_span,
                    });
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

            fields.push(FieldDef {
                name: field_name,
                ty: field_ty,
                json_name,
                annotations,
                span: field_span,
            });

            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Item::new_exported(
            ItemKind::Type {
                name,
                type_params,
                fields,
            },
            start_span,
            is_exported,
        ))
    }

    fn parse_json_field_tag(&mut self) -> Result<(String, Span), ParserError> {
        let token = self.bump();
        let span = token.span;
        let raw = match token.kind {
            TokenKind::FieldTag(raw) => raw,
            _ => unreachable!(),
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
                            message: "JSON tag options are not supported; specify only the field name".to_string(),
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
                                message: format!("Unsupported escape sequence '\\{}' in JSON field tag", other),
                                span,
                            });
                        }
                        None => {
                            return Err(ParserError {
                                message: "Unterminated escape sequence in JSON field tag".to_string(),
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

    fn parse_annotation_arg(&mut self) -> Result<Expr, ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Int(n) => {
                self.bump();
                Ok(Expr::new(ExprKind::Int(n), tok.span))
            }
            TokenKind::IntMinMagnitude => Err(ParserError {
                message: "Integer literal is too large unless preceded by '-'".to_string(),
                span: tok.span,
            }),
            TokenKind::Float(f) => {
                self.bump();
                Ok(Expr::new(ExprKind::Float(f), tok.span))
            }
            TokenKind::Minus => {
                self.bump();
                let next = self.peek().clone();
                match next.kind {
                    TokenKind::IntMinMagnitude => {
                        self.bump();
                        Ok(Expr::new(ExprKind::Int(i64::MIN), tok.span))
                    }
                    TokenKind::Int(n) => {
                        self.bump();
                        Ok(Expr::new(ExprKind::Int(-n), tok.span))
                    }
                    TokenKind::Float(f) => {
                        self.bump();
                        Ok(Expr::new(ExprKind::Float(-f), tok.span))
                    }
                    _ => Err(ParserError {
                        message: "Expected numeric literal after '-' in annotation argument".to_string(),
                        span: next.span,
                    }),
                }
            }
            TokenKind::String(ref s) => {
                let s_val = s.clone();
                self.bump();
                Ok(Expr::new(ExprKind::String(s_val), tok.span))
            }
            TokenKind::KwTrue => {
                self.bump();
                Ok(Expr::new(ExprKind::Bool(true), tok.span))
            }
            TokenKind::KwFalse => {
                self.bump();
                Ok(Expr::new(ExprKind::Bool(false), tok.span))
            }
            TokenKind::KwNil => {
                self.bump();
                Ok(Expr::new(ExprKind::Nil, tok.span))
            }
            TokenKind::Ident(ref id) => {
                let id_val = id.clone();
                self.bump();
                Ok(Expr::new(ExprKind::Ident(id_val), tok.span))
            }
            _ => Err(ParserError {
                message: format!("Annotation arguments must be literal constants, got '{:?}'", tok.kind),
                span: tok.span,
            }),
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
                span: field_span,
            });

            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Item::new_exported(ItemKind::Error { name, fields }, start_span, is_exported))
    }

}
