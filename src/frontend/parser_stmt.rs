use crate::frontend::ast::*;
use crate::frontend::lexer::TokenKind;
use crate::frontend::parser::{Parser, ParserError};

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
        match self.peek().kind {
            TokenKind::KwReturn => self.parse_return_stmt(),
            TokenKind::KwIf => self.parse_if_stmt(),
            TokenKind::KwGuard => self.parse_guard_stmt(),
            TokenKind::KwFor => self.parse_for_stmt(),
            TokenKind::KwWhile => self.parse_while_stmt(),
            TokenKind::KwSwitch => self.parse_switch_stmt(),
            TokenKind::KwDefer => self.parse_defer_stmt(),
            TokenKind::KwSpawn => self.parse_spawn_stmt(),
            TokenKind::KwBreak => Ok(Stmt::new(StmtKind::Break, self.bump().span)),
            TokenKind::KwContinue => Ok(Stmt::new(StmtKind::Continue, self.bump().span)),
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
                    && self.peek_ahead(1).kind == TokenKind::ColonAssign
                {
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
                let span = expr.span;
                if self.match_token(&TokenKind::Assign) {
                    let value = self.parse_expr()?;
                    Ok(Stmt::new(StmtKind::Assign { target: expr, value }, span))
                } else {
                    Ok(Stmt::new(StmtKind::Expr(expr), span))
                }
            }
        }
    }

    fn parse_defer_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwDefer)?.span;
        let block = if self.check(&TokenKind::LBrace) {
            self.parse_block()?
        } else {
            let expr = self.parse_expr()?;
            let expr_span = expr.span;
            Block {
                stmts: vec![Stmt::new(StmtKind::Expr(expr), expr_span)],
                span,
            }
        };
        Ok(Stmt::new(StmtKind::Defer(block), span))
    }

    fn parse_spawn_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwSpawn)?.span;
        let block = if self.check(&TokenKind::LBrace) {
            self.parse_block()?
        } else {
            let expr = self.parse_expr()?;
            let expr_span = expr.span;
            Block {
                stmts: vec![Stmt::new(StmtKind::Expr(expr), expr_span)],
                span,
            }
        };
        Ok(Stmt::new(StmtKind::Spawn(block), span))
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwReturn)?.span;
        if self.check(&TokenKind::RBrace)
            || self.check(&TokenKind::KwCase)
            || self.check(&TokenKind::Eof)
        {
            Ok(Stmt::new(StmtKind::Return(None), span))
        } else {
            Ok(Stmt::new(StmtKind::Return(Some(self.parse_expr()?)), span))
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
            PatternKind::Tuple(elems) => elems.iter().try_for_each(Self::validate_for_pattern),
            _ => Err(ParserError {
                message: "Expected variable name, '_', or tuple pattern in 'for in' loop"
                    .to_string(),
                span: pat.span,
            }),
        }
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParserError> {
        let span = self.expect(&TokenKind::KwFor)?.span;
        if self.match_token(&TokenKind::KwIn)
            || (matches!(&self.peek().kind, TokenKind::Ident(kw) if kw == "range")
                && self.peek_ahead(1).kind != TokenKind::KwIn
                && {
                    self.bump();
                    true
                })
        {
            let iterable = self.parse_expr()?;
            let body = self.parse_block()?;
            return Ok(Stmt::new(
                StmtKind::ForIn {
                    pattern: Pattern::new(PatternKind::Wildcard, span),
                    iterable,
                    body,
                },
                span,
            ));
        }
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
        Ok(Stmt::new(StmtKind::While { condition, body }, span))
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
            while !self.check(&TokenKind::KwCase)
                && !self.check(&TokenKind::RBrace)
                && !self.check(&TokenKind::Eof)
            {
                stmts.push(self.parse_stmt()?);
            }
            cases.push(SwitchCase {
                pattern,
                body: Block {
                    stmts,
                    span: case_span,
                },
                span: case_span,
            });
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
                let mut full_name = name;
                while self.match_token(&TokenKind::Dot) {
                    full_name.push('.');
                    full_name.push_str(&self.expect_ident()?.0);
                }
                if self.check(&TokenKind::Lt) {
                    let args = self.parse_type_arg_list()?;
                    let rendered: Vec<String> = args.iter().map(format_type_expr).collect();
                    full_name = format!("{}<{}>", full_name, rendered.join(", "));
                }
                if self.match_token(&TokenKind::LParen) {
                    let mut bindings: Vec<(Option<String>, String)> = Vec::new();
                    while !self.check(&TokenKind::RParen) && !self.check(&TokenKind::Eof) {
                        let b = if self.match_token(&TokenKind::Underscore) {
                            (None, "_".to_string())
                        } else {
                            let (first, _) = self.expect_ident()?;
                            if self.match_token(&TokenKind::Colon) {
                                let target = if self.match_token(&TokenKind::Underscore) {
                                    "_".to_string()
                                } else {
                                    self.expect_ident()?.0
                                };
                                (Some(first), target)
                            } else {
                                (None, first)
                            }
                        };
                        bindings.push(b);
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    if bindings.len() == 1 && bindings[0].0.is_none() {
                        Ok(Pattern::new(
                            PatternKind::Type {
                                name: full_name,
                                binding: bindings.into_iter().next().unwrap().1,
                            },
                            tok.span,
                        ))
                    } else if let Some((ename, vname)) = full_name.rsplit_once('.') {
                        Ok(Pattern::new(
                            PatternKind::EnumVariant {
                                enum_name: ename.to_string(),
                                variant_name: vname.to_string(),
                                bindings,
                            },
                            tok.span,
                        ))
                    } else {
                        Err(ParserError {
                            message: format!(
                                "Pattern '{}' expects a single binding variable",
                                full_name
                            ),
                            span: tok.span,
                        })
                    }
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

pub(crate) fn format_type_expr(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Generic { name, type_args } => {
            let s: Vec<String> = type_args.iter().map(format_type_expr).collect();
            format!("{}<{}>", name, s.join(", "))
        }
        TypeExpr::List(inner) => format!("[{}]", format_type_expr(inner)),
        TypeExpr::Optional(inner) => format!("{}?", format_type_expr(inner)),
        TypeExpr::Union(vars) => vars.iter().map(format_type_expr).collect::<Vec<_>>().join(" | "),
        TypeExpr::Tuple(es) => {
            let s: Vec<String> = es.iter().map(format_type_expr).collect();
            format!("({})", s.join(", "))
        }
        TypeExpr::Func { params, return_type } => {
            let ps: Vec<String> = params.iter().map(format_type_expr).collect();
            format!("({}) -> {}", ps.join(", "), format_type_expr(return_type))
        }
    }
}
