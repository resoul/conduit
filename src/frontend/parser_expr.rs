use crate::frontend::ast::*;
use crate::frontend::lexer::{Span, TokenKind};
use crate::frontend::parser::{Parser, ParserError};

impl Parser {
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
            ExprKind::Call {
                callee,
                type_args,
                mut args,
                named_args,
            } => {
                args.insert(0, left);
                Ok(Expr::new(
                    ExprKind::Call {
                        callee,
                        type_args,
                        args,
                        named_args,
                    },
                    span,
                ))
            }
            ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Closure { .. } => {
                Ok(Expr::new(
                    ExprKind::Call {
                        callee: Box::new(right),
                        type_args: Vec::new(),
                        args: vec![left],
                        named_args: Vec::new(),
                    },
                    span,
                ))
            }
            _ => Err(ParserError {
                message: "Pipe operator '|>' requires a function or call on the right side"
                    .to_string(),
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
        self.parse_binary_op(Self::parse_equality, &[(TokenKind::And, BinaryOp::And)])
    }

    pub(super) fn parse_query_value_expr(&mut self) -> Result<Expr, ParserError> {
        self.parse_equality()
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
            Self::parse_coalesce,
            &[
                (TokenKind::Lt, BinaryOp::Lt),
                (TokenKind::LtEq, BinaryOp::LtEq),
                (TokenKind::Gt, BinaryOp::Gt),
                (TokenKind::GtEq, BinaryOp::GtEq),
            ],
        )
    }

    fn parse_coalesce(&mut self) -> Result<Expr, ParserError> {
        self.parse_binary_op(
            Self::parse_range,
            &[(TokenKind::QuestionQuestion, BinaryOp::Coalesce)],
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
            if self.check(&TokenKind::IntMinMagnitude) {
                self.bump();
                return Ok(Expr::new(ExprKind::Int(i64::MIN), span));
            }
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
            if matches!(expr.kind, ExprKind::Ident(_) | ExprKind::Member { .. })
                && self.check(&TokenKind::Lt)
            {
                if self.peek_ahead(1).kind == TokenKind::Gt
                    && self.peek_ahead(2).kind == TokenKind::LParen
                {
                    return Err(ParserError {
                        message: "Type argument list cannot be empty".to_string(),
                        span: self.peek().span,
                    });
                }
                let saved_cursor = self.cursor;
                if let Ok(parsed_type_args) = self.parse_type_arg_list()
                    && self.match_token(&TokenKind::LParen)
                {
                    let span = expr.span;
                    let (args, named_args) = self.parse_call_args()?;
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            type_args: parsed_type_args,
                            args,
                            named_args,
                        },
                        span,
                    );
                    continue;
                }
                self.cursor = saved_cursor;
            }
            if self.match_token(&TokenKind::LParen) {
                let span = expr.span;
                let (args, named_args) = self.parse_call_args()?;
                expr = Expr::new(
                    ExprKind::Call {
                        callee: Box::new(expr),
                        type_args: Vec::new(),
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
            } else if self.match_token(&TokenKind::QuestionDot) {
                let span = expr.span;
                let (field, _) = self.expect_ident()?;
                expr = Expr::new(
                    ExprKind::OptionalMember {
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

    fn parse_call_args(&mut self) -> Result<(Vec<Expr>, Vec<NamedArg>), ParserError> {
        let mut args = Vec::new();
        let mut named_args = Vec::new();

        while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
            if self.match_token(&TokenKind::DotDot) {
                if !args.is_empty() || !named_args.is_empty() {
                    return Err(ParserError {
                        message: "Record base '..expr' must be the first argument".to_string(),
                        span: self.peek().span,
                    });
                }
                let base = self.parse_expr()?;
                named_args.push(NamedArg {
                    name: "..".to_string(),
                    value: base,
                });
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
                continue;
            }
            if let TokenKind::Ident(ref arg_name) = self.peek().kind
                && self.peek_ahead(1).kind == TokenKind::Colon
            {
                let name = arg_name.clone();
                self.bump();
                self.bump();
                let value = self.parse_expr()?;
                named_args.push(NamedArg { name, value });
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
                continue;
            }

            args.push(self.parse_expr()?);
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok((args, named_args))
    }

    fn parse_primary(&mut self) -> Result<Expr, ParserError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Int(val) => {
                self.bump();
                Ok(Expr::new(ExprKind::Int(val), tok.span))
            }
            TokenKind::IntMinMagnitude => Err(ParserError {
                message: "Integer literal is too large unless preceded by '-'".to_string(),
                span: tok.span,
            }),
            TokenKind::Float(val) => {
                self.bump();
                Ok(Expr::new(ExprKind::Float(val), tok.span))
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
                if self.match_token(&TokenKind::FatArrow) {
                    let body = self.parse_arrow_body(tok.span)?;
                    Ok(Expr::new(
                        ExprKind::Closure {
                            params: vec![Param {
                                name,
                                ty: TypeExpr::Named("_".to_string()),
                                default: None,
                                span: tok.span,
                            }],
                            return_type: None,
                            body,
                        },
                        tok.span,
                    ))
                } else {
                    Ok(Expr::new(ExprKind::Ident(name), tok.span))
                }
            }
            TokenKind::LBracket => {
                let saved_cursor = self.cursor;
                let span = self.bump().span;
                if let Ok(k_ty) = self.parse_type_expr()
                    && self.match_token(&TokenKind::Colon)
                    && let Ok(v_ty) = self.parse_type_expr()
                    && self.match_token(&TokenKind::RBracket)
                    && self.match_token(&TokenKind::LParen)
                    && self.match_token(&TokenKind::RParen)
                {
                    return Ok(Expr::new(
                        ExprKind::Call {
                            callee: Box::new(Expr::new(ExprKind::Ident("Map".to_string()), span)),
                            type_args: vec![k_ty, v_ty],
                            args: Vec::new(),
                            named_args: Vec::new(),
                        },
                        span,
                    ));
                }
                self.cursor = saved_cursor + 1;
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
                if let Some(closure_expr) = self.try_parse_arrow_closure()? {
                    return Ok(closure_expr);
                }
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

    fn parse_arrow_body(&mut self, span: Span) -> Result<Block, ParserError> {
        if self.check(&TokenKind::LBrace) {
            self.parse_block()
        } else {
            let expr = self.parse_expr()?;
            let expr_span = expr.span;
            Ok(Block {
                stmts: vec![Stmt::new(StmtKind::Return(Some(expr)), expr_span)],
                span,
            })
        }
    }

    fn try_parse_arrow_closure(&mut self) -> Result<Option<Expr>, ParserError> {
        let saved_cursor = self.cursor;
        let span = self.expect(&TokenKind::LParen)?.span;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                let Ok((name, p_span)) = self.expect_ident() else {
                    self.cursor = saved_cursor;
                    return Ok(None);
                };
                let ty = if self.match_token(&TokenKind::Colon) {
                    match self.parse_type_expr() {
                        Ok(t) => t,
                        Err(_) => {
                            self.cursor = saved_cursor;
                            return Ok(None);
                        }
                    }
                } else {
                    TypeExpr::Named("_".to_string())
                };
                params.push(Param { name, ty, default: None, span: p_span });
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
        }
        if !self.match_token(&TokenKind::RParen) {
            self.cursor = saved_cursor;
            return Ok(None);
        }
        let return_type = if self.match_token(&TokenKind::Arrow) {
            match self.parse_type_expr() {
                Ok(t) => Some(t),
                Err(_) => {
                    self.cursor = saved_cursor;
                    return Ok(None);
                }
            }
        } else {
            None
        };
        if !self.match_token(&TokenKind::FatArrow) {
            self.cursor = saved_cursor;
            return Ok(None);
        }
        let body = self.parse_arrow_body(span)?;
        Ok(Some(Expr::new(
            ExprKind::Closure {
                params,
                return_type,
                body,
            },
            span,
        )))
    }

    pub(super) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParserError> {
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
            let first = self.parse_type_expr()?;
            if self.match_token(&TokenKind::Colon) {
                let val = self.parse_type_expr()?;
                self.expect(&TokenKind::RBracket)?;
                TypeExpr::Generic {
                    name: "Map".to_string(),
                    type_args: vec![first, val],
                }
            } else {
                self.expect(&TokenKind::RBracket)?;
                TypeExpr::List(Box::new(first))
            }
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
            if self.check(&TokenKind::Lt) {
                let type_args = self.parse_type_arg_list()?;
                TypeExpr::Generic {
                    name: full_name,
                    type_args,
                }
            } else {
                TypeExpr::Named(full_name)
            }
        };

        while self.match_token(&TokenKind::Question) {
            base_type = TypeExpr::Optional(Box::new(base_type));
        }

        Ok(base_type)
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

    pub(super) fn parse_annotation_arg(&mut self) -> Result<Expr, ParserError> {
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
                        message: "Expected numeric literal after '-' in annotation argument"
                            .to_string(),
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
                message: format!(
                    "Annotation arguments must be literal constants, got '{:?}'",
                    tok.kind
                ),
                span: tok.span,
            }),
        }
    }
}
