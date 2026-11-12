use super::*;

impl Parser {
    pub(super) fn parse_block(&mut self) -> Result<Block, ParserError> {
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
            TokenKind::KwWhile => self.parse_while_stmt(),
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

                if let TokenKind::Ident(ref name) = self.peek().kind
                    && self.peek_ahead(1).kind == TokenKind::ColonAssign {
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
        if self.check(&TokenKind::RBrace)
            || self.check(&TokenKind::KwCase)
            || self.check(&TokenKind::Eof)
        {
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

    fn validate_for_pattern(pat: &Pattern) -> Result<(), ParserError> {
        match &pat.kind {
            PatternKind::Ident(name) if !name.contains('.') => Ok(()),
            PatternKind::Wildcard => Ok(()),
            PatternKind::Tuple(elems) => {
                for p in elems {
                    Self::validate_for_pattern(p)?;
                }
                Ok(())
            }
            _ => Err(ParserError {
                message: "Expected variable name, '_', or tuple pattern in 'for in' loop".to_string(),
                span: pat.span,
            }),
        }
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwFor)?.span;
        let pattern = self.parse_pattern()?;
        Self::validate_for_pattern(&pattern)?;
        self.expect(&TokenKind::KwIn)?;
        let iterable = self.parse_expr()?;
        let body = self.parse_block()?;

        Ok(Stmt::new(
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            },
            span,
        ))
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwWhile)?.span;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;

        Ok(Stmt::new(
            StmtKind::While { condition, body },
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

    pub(super) fn is_tuple_destructure(&self) -> bool {
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

}
