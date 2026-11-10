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
        let tok = self.peek();
        match tok.kind {
            TokenKind::KwFunc => self.parse_func_item(),
            TokenKind::KwType => self.parse_type_item(),
            _ => Err(ParserError {
                message: format!("Expected 'func' or 'type', got '{:?}'", tok.kind),
                span: tok.span,
            }),
        }
    }

    fn parse_func_item(&mut self) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwFunc)?.span;
        let (name, _) = self.expect_ident()?;

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

        let body = self.parse_block()?;

        Ok(Item::new(
            ItemKind::Func {
                name,
                params,
                return_type,
                body,
            },
            start_span,
        ))
    }

    fn parse_type_item(&mut self) -> Result<Item, ParserError> {
        let start_span = self.expect(&TokenKind::KwType)?.span;
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
                span: field_span,
            });

            self.match_token(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Item::new(ItemKind::Type { name, fields }, start_span))
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParserError> {
        let base_type = if self.match_token(&TokenKind::LBracket) {
            let inner = self.parse_type_expr()?;
            self.expect(&TokenKind::RBracket)?;
            TypeExpr::List(Box::new(inner))
        } else {
            let (name, _) = self.expect_ident()?;
            TypeExpr::Named(name)
        };

        if self.match_token(&TokenKind::Question) {
            Ok(TypeExpr::Optional(Box::new(base_type)))
        } else {
            Ok(base_type)
        }
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
            TokenKind::KwVar => self.parse_var_decl_stmt(),
            _ => {
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
        self.parse_or()
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
                self.bump();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            _ => Err(ParserError {
                message: format!("Expected expression, got '{:?}'", tok.kind),
                span: tok.span,
            }),
        }
    }
}

