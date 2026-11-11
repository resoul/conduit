#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Int(i64),
    Float(f64),
    String(String),

    KwFunc,
    KwReturn,
    KwIf,
    KwElse,
    KwFor,
    KwIn,
    KwVar,
    KwType,
    KwTrue,
    KwFalse,
    KwNil,
    KwGuard,
    KwBreak,
    KwContinue,
    KwError,
    KwSwitch,
    KwCase,
    KwExport,
    KwImport,
    KwAs,

    ColonAssign,
    Arrow,
    Pipe,
    Bar,
    Underscore,
    Eq,
    NotEq,
    LtEq,
    GtEq,
    Lt,
    Gt,
    And,
    Or,
    Not,
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    QuestionQuestion,
    Question,
    DotDot,
    Dot,
    Hash,
    At,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexerError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for LexerError {}

pub struct Lexer<'a> {
    input: &'a str,
    byte_offset: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            byte_offset: 0,
            line: 1,
            col: 1,
        }
    }

    fn current_span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.byte_offset..].chars().next()
    }

    fn peek_next_char(&self) -> Option<char> {
        let mut chars = self.input[self.byte_offset..].chars();
        chars.next()?;
        chars.next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.byte_offset += ch.len_utf8();

        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }

        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek_char() {
                Some(ch) if ch.is_whitespace() => {
                    self.bump_char();
                }
                Some('/') if self.peek_next_char() == Some('/') => {
                    self.bump_char();
                    self.bump_char();
                    while let Some(c) = self.peek_char() {
                        if c == '\n' {
                            self.bump_char();
                            break;
                        }
                        self.bump_char();
                    }
                }
                _ => break,
            }
        }
    }

    fn read_number(&mut self, start_span: Span) -> Result<Token, LexerError> {
        let start_offset = self.byte_offset;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                self.bump_char();
            } else {
                break;
            }
        }

        if self.peek_char() == Some('.')
            && self.peek_next_char().map_or(false, |c| c.is_ascii_digit())
        {
            self.bump_char();
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() {
                    self.bump_char();
                } else {
                    break;
                }
            }
            let num_str = &self.input[start_offset..self.byte_offset];
            return match num_str.parse::<f64>() {
                Ok(value) if value.is_finite() => {
                    Ok(Token::new(TokenKind::Float(value), start_span))
                }
                _ => Err(LexerError {
                    message: format!("Invalid float literal: '{}'", num_str),
                    span: start_span,
                }),
            };
        }

        let num_str = &self.input[start_offset..self.byte_offset];
        match num_str.parse::<i64>() {
            Ok(value) => Ok(Token::new(TokenKind::Int(value), start_span)),
            Err(_) => Err(LexerError {
                message: format!("Number too large: '{}'", num_str),
                span: start_span,
            }),
        }
    }

    fn read_string(&mut self, start_span: Span) -> Result<Token, LexerError> {
        self.bump_char();
        let mut content = String::new();

        loop {
            match self.peek_char() {
                None => {
                    return Err(LexerError {
                        message: "Unterminated string literal".to_string(),
                        span: start_span,
                    });
                }
                Some('"') => {
                    self.bump_char();
                    break;
                }
                Some('\\') => {
                    self.bump_char();
                    match self.bump_char() {
                        Some('n') => content.push('\n'),
                        Some('t') => content.push('\t'),
                        Some('r') => content.push('\r'),
                        Some('"') => content.push('"'),
                        Some('\\') => content.push('\\'),
                        Some(other) => {
                            content.push('\\');
                            content.push(other);
                        }
                        None => {
                            return Err(LexerError {
                                message: "Unterminated escape sequence".to_string(),
                                span: self.current_span(),
                            });
                        }
                    }
                }
                Some(ch) => {
                    content.push(ch);
                    self.bump_char();
                }
            }
        }

        Ok(Token::new(TokenKind::String(content), start_span))
    }

    fn read_identifier_or_keyword(&mut self, start_span: Span) -> Token {
        let start_offset = self.byte_offset;
        while let Some(ch) = self.peek_char() {
            if ch.is_alphanumeric() || ch == '_' {
                self.bump_char();
            } else {
                break;
            }
        }

        let word = &self.input[start_offset..self.byte_offset];
        let kind = match word {
            "func" => TokenKind::KwFunc,
            "return" => TokenKind::KwReturn,
            "if" => TokenKind::KwIf,
            "else" => TokenKind::KwElse,
            "for" => TokenKind::KwFor,
            "in" => TokenKind::KwIn,
            "var" => TokenKind::KwVar,
            "type" => TokenKind::KwType,
            "true" => TokenKind::KwTrue,
            "false" => TokenKind::KwFalse,
            "nil" => TokenKind::KwNil,
            "guard" => TokenKind::KwGuard,
            "break" => TokenKind::KwBreak,
            "continue" => TokenKind::KwContinue,
            "error" => TokenKind::KwError,
            "switch" => TokenKind::KwSwitch,
            "case" => TokenKind::KwCase,
            "export" => TokenKind::KwExport,
            "import" => TokenKind::KwImport,
            "as" => TokenKind::KwAs,
            "_" => TokenKind::Underscore,
            _ => TokenKind::Ident(word.to_string()),
        };

        Token::new(kind, start_span)
    }

    pub fn next_token(&mut self) -> Result<Token, LexerError> {
        self.skip_whitespace_and_comments();

        let start_span = self.current_span();
        let ch = match self.peek_char() {
            Some(c) => c,
            None => return Ok(Token::new(TokenKind::Eof, start_span)),
        };

        if ch.is_alphabetic() || ch == '_' {
            return Ok(self.read_identifier_or_keyword(start_span));
        }

        if ch.is_ascii_digit() {
            return self.read_number(start_span);
        }

        if ch == '"' {
            return self.read_string(start_span);
        }

        self.bump_char();
        let kind = match ch {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            '#' => TokenKind::Hash,
            '@' => TokenKind::At,
            ',' => TokenKind::Comma,
            '?' => {
                if self.peek_char() == Some('?') {
                    self.bump_char();
                    TokenKind::QuestionQuestion
                } else {
                    TokenKind::Question
                }
            }
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '.' => {
                if self.peek_char() == Some('.') {
                    self.bump_char();
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            ':' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::ColonAssign
                } else {
                    TokenKind::Colon
                }
            }
            '-' => {
                if self.peek_char() == Some('>') {
                    self.bump_char();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '=' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Eq
                } else {
                    TokenKind::Assign
                }
            }
            '!' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::NotEq
                } else {
                    TokenKind::Not
                }
            }
            '<' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.peek_char() == Some('&') {
                    self.bump_char();
                    TokenKind::And
                } else {
                    return Err(LexerError {
                        message: "Unexpected character '&', expected '&&'".to_string(),
                        span: start_span,
                    });
                }
            }
            '|' => {
                if self.peek_char() == Some('|') {
                    self.bump_char();
                    TokenKind::Or
                } else if self.peek_char() == Some('>') {
                    self.bump_char();
                    TokenKind::Pipe
                } else {
                    TokenKind::Bar
                }
            }
            other => {
                return Err(LexerError {
                    message: format!("Unexpected character: '{}'", other),
                    span: start_span,
                });
            }
        };

        Ok(Token::new(kind, start_span))
    }

    pub fn tokenize_all(&mut self) -> Result<Vec<Token>, LexerError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_guard_and_coalesce() {
        let mut lexer = Lexer::new("guard x > 0 else { return a ?? 10 }");
        let tokens = lexer.tokenize_all().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::KwGuard);
        assert_eq!(tokens[1].kind, TokenKind::Ident("x".to_string()));
        assert_eq!(tokens[2].kind, TokenKind::Gt);
        assert_eq!(tokens[3].kind, TokenKind::Int(0));
        assert_eq!(tokens[4].kind, TokenKind::KwElse);
        assert_eq!(tokens[5].kind, TokenKind::LBrace);
        assert_eq!(tokens[6].kind, TokenKind::KwReturn);
        assert_eq!(tokens[7].kind, TokenKind::Ident("a".to_string()));
        assert_eq!(tokens[8].kind, TokenKind::QuestionQuestion);
        assert_eq!(tokens[9].kind, TokenKind::Int(10));
        assert_eq!(tokens[10].kind, TokenKind::RBrace);
    }

    #[test]
    fn test_lex_break_continue_pipe() {
        let mut lexer = Lexer::new("for x in list { if x == 1 { continue } break } data |> process()");
        let tokens = lexer.tokenize_all().unwrap();
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwContinue));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwBreak));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_lex_error_switch_tuples_bar() {
        let code = "error NotFound { id: Int } switch res { case (val, _): return val } A | B";
        let mut lexer = Lexer::new(code);
        let tokens = lexer.tokenize_all().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::KwError);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwSwitch));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwCase));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Underscore));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Bar));
    }

    #[test]
    fn test_lex_export_import_as() {
        let mut lexer = Lexer::new("export func foo() {}\nimport cdt.time as clock\nimport \"./users\" as users");
        let tokens = lexer.tokenize_all().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::KwExport);
        assert_eq!(tokens[1].kind, TokenKind::KwFunc);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwImport));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::KwAs));
    }

    #[test]
    fn test_lex_validator_marker_and_annotation() {
        let code = "[#validator] func check(v: String) {} name: String @validation.min(3)";
        let mut lexer = Lexer::new(code);
        let tokens = lexer.tokenize_all().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::LBracket);
        assert_eq!(tokens[1].kind, TokenKind::Hash);
        assert_eq!(tokens[2].kind, TokenKind::Ident("validator".to_string()));
        assert_eq!(tokens[3].kind, TokenKind::RBracket);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::At));
    }
}

