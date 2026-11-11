use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};

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
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    fn peek(&self) -> &Token {
        if self.cursor < self.tokens.len() {
            &self.tokens[self.cursor]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    fn peek_ahead(&self, n: usize) -> &Token {
        let idx = self.cursor + n;
        if idx < self.tokens.len() {
            &self.tokens[idx]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.cursor < self.tokens.len() {
            self.cursor += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Token, ParserError> {
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

    fn expect_ident(&mut self) -> Result<(String, Span), ParserError> {
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
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParserError> {
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
            let default_alias = path.split('.').last().unwrap_or(&path).to_string();
            let alias = if self.match_token(&TokenKind::KwAs) {
                let (custom_alias, _) = self.expect_ident()?;
                custom_alias
            } else {
                default_alias
            };
            Ok(Item::new_exported(ItemKind::Import { path, alias }, start_span, false))
        }
    }

    fn parse_params_and_return_type(&mut self) -> Result<(Vec<Param>, Option<TypeExpr>), ParserError> {
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

    fn parse_func_item(&mut self, is_exported: bool, is_validator: bool) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwFunc)?.span;
        let (name, _) = self.expect_ident()?;

        let (params, return_type) = self.parse_params_and_return_type()?;
        let body = self.parse_block()?;

        Ok(Item::new_exported(
            ItemKind::Func {
                name,
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

        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let (field_name, field_span) = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let field_ty = self.parse_type_expr()?;

            let mut annotations = Vec::new();
            while self.match_token(&TokenKind::At) {
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
            }

            fields.push(FieldDef {
                name: field_name,
                ty: field_ty,
                annotations,
                span: field_span,
            });

            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Item::new_exported(ItemKind::Type { name, fields }, start_span, is_exported))
    }

    fn parse_annotation_arg(&mut self) -> Result<Expr, ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Int(n) => {
                self.bump();
                Ok(Expr::new(ExprKind::Int(n), tok.span))
            }
            TokenKind::Minus => {
                self.bump();
                let next = self.peek().clone();
                if let TokenKind::Int(n) = next.kind {
                    self.bump();
                    Ok(Expr::new(ExprKind::Int(-n), tok.span))
                } else {
                    Err(ParserError {
                        message: "Expected integer literal after '-' in annotation argument".to_string(),
                        span: next.span,
                    })
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
                annotations: Vec::new(),
                span: field_span,
            });

            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Item::new_exported(ItemKind::Error { name, fields }, start_span, is_exported))
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParserError> {
        let mut types = vec![self.parse_primary_type()?];
        while self.match_token(&TokenKind::Bar) {
            types.push(self.parse_primary_type()?);
        }
        if types.len() == 1 {
            Ok(types.pop().unwrap())
        } else {
            Ok(TypeExpr::Union(types))
        }
    }

    fn parse_primary_type(&mut self) -> Result<TypeExpr, ParserError> {
        let mut base_type = if self.match_token(&TokenKind::LBracket) {
            let inner = self.parse_type_expr()?;
            self.expect(&TokenKind::RBracket)?;
            TypeExpr::List(Box::new(inner))
        } else if self.match_token(&TokenKind::LParen) {
            let mut types = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                types.push(self.parse_type_expr()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
            if self.match_token(&TokenKind::Arrow) {
                let ret = self.parse_type_expr()?;
                TypeExpr::Func {
                    params: types,
                    return_type: Box::new(ret),
                }
            } else if types.len() == 1 {
                types.pop().unwrap()
            } else {
                TypeExpr::Tuple(types)
            }
        } else {
            let (name, _) = self.expect_ident()?;
            let full_name = if self.match_token(&TokenKind::Dot) {
                let (sub, _) = self.expect_ident()?;
                format!("{}.{}", name, sub)
            } else {
                name
            };
            TypeExpr::Named(full_name)
        };

        while self.match_token(&TokenKind::Question) {
            base_type = TypeExpr::Optional(Box::new(base_type));
        }

        Ok(base_type)
    }

    fn parse_block(&mut self) -> Result<Block, ParserError> {
        let start_span = self.expect(&TokenKind::LBrace)?.span;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block {
            stmts,
            span: start_span,
        })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParserError> {
        let tok = self.peek();
        match tok.kind {
            TokenKind::KwReturn => self.parse_return_stmt(),
            TokenKind::KwIf => self.parse_if_stmt(),
            TokenKind::KwGuard => self.parse_guard_stmt(),
            TokenKind::KwFor => self.parse_for_stmt(),
            TokenKind::KwSwitch => self.parse_switch_stmt(),
            TokenKind::KwBreak => {
                let span = self.bump().span;
                Ok(Stmt::new(StmtKind::Break, span))
            }
            TokenKind::KwContinue => {
                let span = self.bump().span;
                Ok(Stmt::new(StmtKind::Continue, span))
            }
            TokenKind::KwVar => self.parse_var_decl_stmt(),
            _ => {
                if self.is_tuple_destructure() {
                    let span = self.peek().span;
                    let pattern = self.parse_pattern()?;
                    self.expect(&TokenKind::ColonAssign)?;
                    let value = self.parse_expr()?;
                    return Ok(Stmt::new(StmtKind::DestructureDefine { pattern, value }, span));
                }

                if let TokenKind::Ident(ref name) = self.peek().kind {
                    if self.peek_ahead(1).kind == TokenKind::ColonAssign {
                        let span = self.peek().span;
                        let var_name = name.clone();
                        self.bump();
                        self.bump();
                        let value = self.parse_expr()?;
                        return Ok(Stmt::new(
                            StmtKind::Define {
                                name: var_name,
                                value,
                            },
                            span,
                        ));
                    }
                }

                let expr = self.parse_expr()?;
                if self.match_token(&TokenKind::Assign) {
                    let val = self.parse_expr()?;
                    let span = expr.span;
                    Ok(Stmt::new(
                        StmtKind::Assign {
                            target: expr,
                            value: val,
                        },
                        span,
                    ))
                } else {
                    let span = expr.span;
                    Ok(Stmt::new(StmtKind::Expr(expr), span))
                }
            }
        }
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwReturn)?.span;
        if self.check(&TokenKind::RBrace) || self.check(&TokenKind::Eof) {
            Ok(Stmt::new(StmtKind::Return(None), span))
        } else {
            let expr = self.parse_expr()?;
            Ok(Stmt::new(StmtKind::Return(Some(expr)), span))
        }
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwIf)?.span;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;

        let else_block = if self.match_token(&TokenKind::KwElse) {
            if self.check(&TokenKind::KwIf) {
                let nested_if = self.parse_if_stmt()?;
                let nested_span = nested_if.span;
                Some(Block {
                    stmts: vec![nested_if],
                    span: nested_span,
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        Ok(Stmt::new(
            StmtKind::If {
                condition,
                then_block,
                else_block,
            },
            span,
        ))
    }

    fn parse_guard_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwGuard)?.span;
        let condition = self.parse_expr()?;
        self.expect(&TokenKind::KwElse)?;
        let else_block = self.parse_block()?;

        Ok(Stmt::new(
            StmtKind::Guard {
                condition,
                else_block,
            },
            span,
        ))
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwFor)?.span;
        let (variable, _) = self.expect_ident()?;
        self.expect(&TokenKind::KwIn)?;
        let iterable = self.parse_expr()?;
        let body = self.parse_block()?;

        Ok(Stmt::new(
            StmtKind::ForIn {
                variable,
                iterable,
                body,
            },
            span,
        ))
    }

    fn parse_switch_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwSwitch)?.span;
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let mut cases = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let case_span = self.expect(&TokenKind::KwCase)?.span;
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::Colon)?;

            let mut stmts = Vec::new();
            while !self.check(&TokenKind::KwCase) && !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
                stmts.push(self.parse_stmt()?);
            }
            let body = Block { stmts, span: case_span };
            cases.push(SwitchCase { pattern, body, span: case_span });
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::new(StmtKind::Switch { expr, cases }, span))
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::KwNil => {
                self.bump();
                Ok(Pattern::new(PatternKind::Nil, tok.span))
            }
            TokenKind::Underscore => {
                self.bump();
                Ok(Pattern::new(PatternKind::Wildcard, tok.span))
            }
            TokenKind::LParen => {
                self.bump();
                let mut pats = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                    pats.push(self.parse_pattern()?);
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::new(PatternKind::Tuple(pats), tok.span))
            }
            TokenKind::Ident(name) => {
                self.bump();
                let full_name = if self.match_token(&TokenKind::Dot) {
                    let (sub, _) = self.expect_ident()?;
                    format!("{}.{}", name, sub)
                } else {
                    name
                };
                if self.match_token(&TokenKind::LParen) {
                    let (binding, _) = self.expect_ident()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Pattern::new(PatternKind::Type { name: full_name, binding }, tok.span))
                } else {
                    Ok(Pattern::new(PatternKind::Ident(full_name), tok.span))
                }
            }
            _ => Err(ParserError {
                message: format!("Expected pattern, got '{:?}'", tok.kind),
                span: tok.span,
            }),
        }
    }

    fn is_tuple_destructure(&self) -> bool {
        if !self.check(&TokenKind::LParen) {
            return false;
        }
        let mut depth = 0;
        let mut idx = self.cursor;
        while idx < self.tokens.len() {
            match &self.tokens[idx].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return idx + 1 < self.tokens.len()
                            && self.tokens[idx + 1].kind == TokenKind::ColonAssign;
                    }
                }
                _ => {}
            }
            idx += 1;
        }
        false
    }

    fn parse_var_decl_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwVar)?.span;
        let (name, _) = self.expect_ident()?;

        let ty = if self.match_token(&TokenKind::Colon) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        let init = if self.match_token(&TokenKind::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Stmt::new(StmtKind::VarDecl { name, ty, init }, span))
    }

    pub fn parse_expr(&mut self) -> Result<Expr, ParserError> {
        self.parse_pipe()
    }

    fn parse_pipe(&mut self) -> Result<Expr, ParserError> {
        let mut left = self.parse_or()?;
        while self.match_token(&TokenKind::Pipe) {
            let span = left.span;
            let right = self.parse_or()?;
            left = self.desugar_pipe(left, right, span)?;
        }
        Ok(left)
    }

    fn desugar_pipe(&self, left: Expr, right: Expr, span: Span) -> Result<Expr, ParserError> {
        match right.kind {
            ExprKind::Call { callee, mut args, named_args } => {
                args.insert(0, left);
                Ok(Expr::new(
                    ExprKind::Call { callee, args, named_args },
                    span,
                ))
            }
            ExprKind::Ident(_) | ExprKind::Closure { .. } => {
                Ok(Expr::new(
                    ExprKind::Call {
                        callee: Box::new(right),
                        args: vec![left],
                        named_args: Vec::new(),
                    },
                    span,
                ))
            }
            _ => Err(ParserError {
                message: "Pipe operator '|>' requires a function or call on the right side".to_string(),
                span: right.span,
            }),
        }
    }

    fn parse_binary_op(
        &mut self,
        parse_next: fn(&mut Self) -> Result<Expr, ParserError>,
        ops: &[(TokenKind, BinaryOp)],
    ) -> Result<Expr, ParserError> {
        let mut left = parse_next(self)?;
        'outer: loop {
            for (kind, op) in ops {
                if self.match_token(kind) {
                    let span = left.span;
                    let right = parse_next(self)?;
                    left = Expr::new(
                        ExprKind::Binary {
                            op: *op,
                            left: Box::new(left),
                            right: Box::new(right),
                        },
                        span,
                    );
                    continue 'outer;
                }
            }
            break;
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(Self::parse_and, &[(TokenKind::Or, BinaryOp::Or)])
    }

    fn parse_and(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(Self::parse_coalesce, &[(TokenKind::And, BinaryOp::And)])
    }

    fn parse_coalesce(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_equality,
            &[(TokenKind::QuestionQuestion, BinaryOp::Coalesce)],
        )
    }

    fn parse_equality(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_comparison,
            &[
                (TokenKind::Eq, BinaryOp::Eq),
                (TokenKind::NotEq, BinaryOp::NotEq),
            ],
        )
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_range,
            &[
                (TokenKind::Lt, BinaryOp::Lt),
                (TokenKind::LtEq, BinaryOp::LtEq),
                (TokenKind::Gt, BinaryOp::Gt),
                (TokenKind::GtEq, BinaryOp::GtEq),
            ],
        )
    }

    fn parse_range(&mut self) -> Result<Expr, ParserError> {
        let left = self.parse_term()?;
        if self.match_token(&TokenKind::DotDot) {
            let span = left.span;
            let right = self.parse_term()?;
            Ok(Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Range,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            ))
        } else {
            Ok(left)
        }
    }

    fn parse_term(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_factor,
            &[
                (TokenKind::Plus, BinaryOp::Add),
                (TokenKind::Minus, BinaryOp::Sub),
            ],
        )
    }

    fn parse_factor(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_unary,
            &[
                (TokenKind::Star, BinaryOp::Mul),
                (TokenKind::Slash, BinaryOp::Div),
                (TokenKind::Percent, BinaryOp::Mod),
            ],
        )
    }

    fn parse_unary(&mut self) -> Result<Expr, ParserError> {
        if self.match_token(&TokenKind::Minus) {
            let span = self.peek().span;
            let expr = self.parse_unary()?;
            Ok(Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                },
                span,
            ))
        } else if self.match_token(&TokenKind::Not) {
            let span = self.peek().span;
            let expr = self.parse_unary()?;
            Ok(Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                },
                span,
            ))
        } else {
            self.parse_postfix()
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParserError> {
        let mut expr = self.parse_primary()?;

        loop {
            if self.check(&TokenKind::LParen) && self.is_tuple_destructure() {
                break;
            }
            if self.match_token(&TokenKind::LParen) {
                let span = expr.span;
                let mut args = Vec::new();
                let mut named_args = Vec::new();

                while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                    if let TokenKind::Ident(ref arg_name) = self.peek().kind {
                        if self.peek_ahead(1).kind == TokenKind::Colon {
                            let name = arg_name.clone();
                            self.bump();
                            self.bump();
                            let val = self.parse_expr()?;
                            named_args.push(NamedArg { name, value: val });
                            if !self.match_token(&TokenKind::Comma) {
                                break;
                            }
                            continue;
                        }
                    }

                    args.push(self.parse_expr()?);
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
                expr = Expr::new(
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                        named_args,
                    },
                    span,
                );
            } else if self.match_token(&TokenKind::LBracket) {
                let span = expr.span;
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                expr = Expr::new(
                    ExprKind::Index {
                        target: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                );
            } else if self.match_token(&TokenKind::Dot) {
                let span = expr.span;
                let (field, _) = self.expect_ident()?;
                expr = Expr::new(
                    ExprKind::Member {
                        target: Box::new(expr),
                        field,
                    },
                    span,
                );
            } else if self.match_token(&TokenKind::Question) {
                let span = expr.span;
                expr = Expr::new(ExprKind::Try(Box::new(expr)), span);
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Int(val) => {
                self.bump();
                Ok(Expr::new(ExprKind::Int(val), tok.span))
            }
            TokenKind::String(val) => {
                self.bump();
                Ok(Expr::new(ExprKind::String(val), tok.span))
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
            TokenKind::KwFunc => {
                self.bump();
                let (params, return_type) = self.parse_params_and_return_type()?;
                let body = self.parse_block()?;
                Ok(Expr::new(
                    ExprKind::Closure {
                        params,
                        return_type,
                        body,
                    },
                    tok.span,
                ))
            }
            TokenKind::Ident(name) => {
                self.bump();
                Ok(Expr::new(ExprKind::Ident(name), tok.span))
            }
            TokenKind::LBracket => {
                let span = self.bump().span;
                let mut elements = Vec::new();
                while !self.check(&TokenKind::RBracket) && !self.check(&TokenKind::Eof) {
                    elements.push(self.parse_expr()?);
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::new(ExprKind::List(elements), span))
            }
            TokenKind::LParen => {
                let span = self.bump().span;
                if self.match_token(&TokenKind::RParen) {
                    return Ok(Expr::new(ExprKind::Tuple(Vec::new()), span));
                }
                let first = self.parse_expr()?;
                if self.match_token(&TokenKind::Comma) {
                    let mut elements = vec![first];
                    while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                        elements.push(self.parse_expr()?);
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::new(ExprKind::Tuple(elements), span))
                } else {
                    self.expect(&TokenKind::RParen)?;
                    Ok(first)
                }
            }
            _ => Err(ParserError {
                message: format!("Expected expression, got '{:?}'", tok.kind),
                span: tok.span,
            }),
        }
    }
}

