use super::*;

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
            ExprKind::Call { callee, type_args, mut args, named_args } => {
                args.insert(0, left);
                Ok(Expr::new(
                    ExprKind::Call { callee, type_args, args, named_args },
                    span,
                ))
            }
            ExprKind::Ident(_) | ExprKind::Closure { .. } => {
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
        self.parse_binary_op(Self::parse_equality, &[(TokenKind::And, BinaryOp::And)])
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
            if matches!(expr.kind, ExprKind::Ident(_) | ExprKind::Member { .. }) && self.check(&TokenKind::Lt) {
                if self.peek_ahead(1).kind == TokenKind::Gt && self.peek_ahead(2).kind == TokenKind::LParen {
                    return Err(ParserError {
                        message: "Type argument list cannot be empty".to_string(),
                        span: self.peek().span,
                    });
                }
                let saved_cursor = self.cursor;
                if let Ok(parsed_type_args) = self.parse_type_arg_list()
                    && self.match_token(&TokenKind::LParen) {
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
            if let TokenKind::Ident(ref arg_name) = self.peek().kind
                && self.peek_ahead(1).kind == TokenKind::Colon {
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

