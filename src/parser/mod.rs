mod expr;
mod items;
mod stmt;
mod types;

use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};
use std::collections::HashSet;

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
}
