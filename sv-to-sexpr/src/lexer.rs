use crate::diagnostic::{Diagnostic, Span};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Module,
    Endmodule,
    Parameter,
    Localparam,
    Real,
    Realtime,
    Input,
    Output,
    Inout,
    Logic,
    Tri,
    Wire,
    Import,
    Initial,
    AlwaysLatch,
    Assign,
    Specify,
    EndSpecify,
    Specparam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operator {
    DoubleAnd,
    DoubleOr,
    EqualEqual,
    NotEqual,
    NotCaseEqual,
    LessEqual,
    Implies,
    ColonColon,
    TildeCaret,
    TildeAmpersand,
    TildePipe,
    Tilde,
    Bang,
    Ampersand,
    Pipe,
    Caret,
    Plus,
    Minus,
    Star,
    Slash,
    Equals,
    Less,
    Greater,
    At,
    Dot,
    Hash,
    Question,
    Colon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Punct {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Identifier,
    Keyword(Keyword),
    Integer,
    Real,
    ConstZero,
    ConstOne,
    ConstZ,
    ConstX,
    Punct(Punct),
    Operator(Operator),
    Directive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub span: Span,
}

pub type LexResult<T> = Result<T, Diagnostic>;

pub fn lex_file(path: &Path, input: &str) -> LexResult<Vec<Token>> {
    Lexer::new(path, input).lex_all()
}

pub struct Lexer<'a> {
    path: &'a Path,
    input: &'a str,
    bytes: &'a [u8],
    index: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(path: &'a Path, input: &'a str) -> Self {
        Self {
            path,
            input,
            bytes: input.as_bytes(),
            index: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn lex_all(mut self) -> LexResult<Vec<Token>> {
        let mut tokens = Vec::new();
        while self.skip_ws_and_comments()? {
            if let Some(token) = self.next_token()? {
                tokens.push(token);
            }
        }
        Ok(tokens)
    }

    fn next_token(&mut self) -> LexResult<Option<Token>> {
        let Some(byte) = self.peek_byte() else {
            return Ok(None);
        };
        let ch = byte as char;
        if ch == '`' {
            return self.lex_directive().map(Some);
        }
        if ch == '\\' {
            return self.lex_escaped_identifier().map(Some);
        }
        if is_ident_start(ch) {
            return self.lex_identifier_or_keyword().map(Some);
        }
        if ch.is_ascii_digit() {
            return self.lex_number().map(Some);
        }
        if ch == '\'' {
            return self.lex_constant().map(Some);
        }
        if let Some(op) = self.lex_operator_or_punct()? {
            return Ok(Some(op));
        }
        Err(self.error_here(format!("unexpected character `{}`", ch)))
    }

    fn lex_directive(&mut self) -> LexResult<Token> {
        let span = self.span_here();
        self.advance_char();
        let start = self.index;
        while let Some(byte) = self.peek_byte() {
            let ch = byte as char;
            if is_ident_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }
        let name = &self.input[start..self.index];
        if name == "default_nettype" {
            while let Some(byte) = self.peek_byte() {
                let ch = byte as char;
                if ch == '\n' {
                    break;
                }
                self.advance_char();
            }
            Ok(Token {
                kind: TokenKind::Directive,
                lexeme: format!("`{}", name),
                span,
            })
        } else {
            while let Some(byte) = self.peek_byte() {
                let ch = byte as char;
                if ch == '\n' {
                    break;
                }
                self.advance_char();
            }
            Ok(Token {
                kind: TokenKind::Directive,
                lexeme: format!("`{}", name),
                span,
            })
        }
    }

    fn lex_escaped_identifier(&mut self) -> LexResult<Token> {
        let span = self.span_here();
        self.advance_char();
        let start = self.index;
        while let Some(byte) = self.peek_byte() {
            let ch = byte as char;
            if ch.is_whitespace() {
                break;
            }
            self.advance_char();
        }
        if self.index == start {
            return Err(self.error_here("expected escaped identifier".to_string()));
        }
        Ok(Token {
            kind: TokenKind::Identifier,
            lexeme: self.input[start..self.index].to_string(),
            span,
        })
    }

    fn lex_identifier_or_keyword(&mut self) -> LexResult<Token> {
        let span = self.span_here();
        let start = self.index;
        self.advance_char();
        while let Some(byte) = self.peek_byte() {
            let ch = byte as char;
            if is_ident_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }
        let lexeme = &self.input[start..self.index];
        let kind = match lexeme {
            "module" => TokenKind::Keyword(Keyword::Module),
            "endmodule" => TokenKind::Keyword(Keyword::Endmodule),
            "parameter" => TokenKind::Keyword(Keyword::Parameter),
            "localparam" => TokenKind::Keyword(Keyword::Localparam),
            "real" => TokenKind::Keyword(Keyword::Real),
            "realtime" => TokenKind::Keyword(Keyword::Realtime),
            "input" => TokenKind::Keyword(Keyword::Input),
            "output" => TokenKind::Keyword(Keyword::Output),
            "inout" => TokenKind::Keyword(Keyword::Inout),
            "logic" => TokenKind::Keyword(Keyword::Logic),
            "tri" => TokenKind::Keyword(Keyword::Tri),
            "wire" => TokenKind::Keyword(Keyword::Wire),
            "import" => TokenKind::Keyword(Keyword::Import),
            "initial" => TokenKind::Keyword(Keyword::Initial),
            "always_latch" => TokenKind::Keyword(Keyword::AlwaysLatch),
            "assign" => TokenKind::Keyword(Keyword::Assign),
            "specify" => TokenKind::Keyword(Keyword::Specify),
            "endspecify" => TokenKind::Keyword(Keyword::EndSpecify),
            "specparam" => TokenKind::Keyword(Keyword::Specparam),
            _ => TokenKind::Identifier,
        };
        Ok(Token {
            kind,
            lexeme: lexeme.to_string(),
            span,
        })
    }

    fn lex_number(&mut self) -> LexResult<Token> {
        let span = self.span_here();
        let start = self.index;
        while matches!(self.peek_byte().map(char::from), Some(ch) if ch.is_ascii_digit()) {
            self.advance_char();
        }
        let mut is_real = false;
        if self.peek_char() == Some('.')
            && self.peek_next_char().map_or(false, |c| c.is_ascii_digit())
        {
            is_real = true;
            self.advance_char();
            while matches!(self.peek_byte().map(char::from), Some(ch) if ch.is_ascii_digit()) {
                self.advance_char();
            }
        }
        if matches!(self.peek_char(), Some('e' | 'E')) {
            is_real = true;
            self.advance_char();
            if matches!(self.peek_char(), Some('+' | '-')) {
                self.advance_char();
            }
            let mut saw_digit = false;
            while matches!(self.peek_byte().map(char::from), Some(ch) if ch.is_ascii_digit()) {
                saw_digit = true;
                self.advance_char();
            }
            if !saw_digit {
                return Err(self.error_here("invalid real literal exponent".to_string()));
            }
        }
        Ok(Token {
            kind: if is_real {
                TokenKind::Real
            } else {
                TokenKind::Integer
            },
            lexeme: self.input[start..self.index].to_string(),
            span,
        })
    }

    fn lex_constant(&mut self) -> LexResult<Token> {
        let span = self.span_here();
        let start = self.index;
        self.advance_char();
        let Some(byte) = self.peek_byte() else {
            return Err(self.error_here("unterminated constant".to_string()));
        };
        let ch = byte as char;
        let kind = match ch {
            '0' => TokenKind::ConstZero,
            '1' => TokenKind::ConstOne,
            'z' | 'Z' => TokenKind::ConstZ,
            'x' | 'X' => TokenKind::ConstX,
            _ => return Err(self.error_here(format!("unsupported constant `'{}'", ch))),
        };
        self.advance_char();
        Ok(Token {
            kind,
            lexeme: self.input[start..self.index].to_string(),
            span,
        })
    }

    fn lex_operator_or_punct(&mut self) -> LexResult<Option<Token>> {
        let span = self.span_here();
        let start = self.index;
        let rest = &self.input[self.index..];
        let kind = if rest.starts_with("!==") {
            Some(TokenKind::Operator(Operator::NotCaseEqual))
        } else if rest.starts_with("~^") {
            Some(TokenKind::Operator(Operator::TildeCaret))
        } else if rest.starts_with("~&") {
            Some(TokenKind::Operator(Operator::TildeAmpersand))
        } else if rest.starts_with("~|") {
            Some(TokenKind::Operator(Operator::TildePipe))
        } else if rest.starts_with("&&") {
            Some(TokenKind::Operator(Operator::DoubleAnd))
        } else if rest.starts_with("||") {
            Some(TokenKind::Operator(Operator::DoubleOr))
        } else if rest.starts_with("==") {
            Some(TokenKind::Operator(Operator::EqualEqual))
        } else if rest.starts_with("!=") {
            Some(TokenKind::Operator(Operator::NotEqual))
        } else if rest.starts_with("<=") {
            Some(TokenKind::Operator(Operator::LessEqual))
        } else if rest.starts_with("*>") {
            Some(TokenKind::Operator(Operator::Implies))
        } else if rest.starts_with("::") {
            Some(TokenKind::Operator(Operator::ColonColon))
        } else {
            None
        };
        if let Some(kind) = kind {
            let len = match kind {
                TokenKind::Operator(Operator::NotCaseEqual) => 3,
                TokenKind::Operator(Operator::TildeCaret)
                | TokenKind::Operator(Operator::TildeAmpersand)
                | TokenKind::Operator(Operator::TildePipe) => 2,
                _ => 2,
            };
            for _ in 0..len {
                self.advance_char();
            }
            return Ok(Some(Token {
                kind,
                lexeme: self.input[start..self.index].to_string(),
                span,
            }));
        }
        let ch = self
            .peek_char()
            .ok_or_else(|| self.error_here("unexpected end of input".to_string()))?;
        let kind = match ch {
            '(' => Some(TokenKind::Punct(Punct::LParen)),
            ')' => Some(TokenKind::Punct(Punct::RParen)),
            '[' => Some(TokenKind::Punct(Punct::LBracket)),
            ']' => Some(TokenKind::Punct(Punct::RBracket)),
            '{' => Some(TokenKind::Punct(Punct::LBrace)),
            '}' => Some(TokenKind::Punct(Punct::RBrace)),
            ',' => Some(TokenKind::Punct(Punct::Comma)),
            ';' => Some(TokenKind::Punct(Punct::Semicolon)),
            '!' => Some(TokenKind::Operator(Operator::Bang)),
            '~' => Some(TokenKind::Operator(Operator::Tilde)),
            '&' => Some(TokenKind::Operator(Operator::Ampersand)),
            '|' => Some(TokenKind::Operator(Operator::Pipe)),
            '^' => Some(TokenKind::Operator(Operator::Caret)),
            '+' => Some(TokenKind::Operator(Operator::Plus)),
            '-' => Some(TokenKind::Operator(Operator::Minus)),
            '*' => Some(TokenKind::Operator(Operator::Star)),
            '/' => Some(TokenKind::Operator(Operator::Slash)),
            '=' => Some(TokenKind::Operator(Operator::Equals)),
            '<' => Some(TokenKind::Operator(Operator::Less)),
            '>' => Some(TokenKind::Operator(Operator::Greater)),
            '@' => Some(TokenKind::Operator(Operator::At)),
            '.' => Some(TokenKind::Operator(Operator::Dot)),
            '#' => Some(TokenKind::Operator(Operator::Hash)),
            '?' => Some(TokenKind::Operator(Operator::Question)),
            ':' => Some(TokenKind::Operator(Operator::Colon)),
            _ => None,
        };
        if let Some(kind) = kind {
            self.advance_char();
            return Ok(Some(Token {
                kind,
                lexeme: self.input[start..self.index].to_string(),
                span,
            }));
        }
        Ok(None)
    }

    fn skip_ws_and_comments(&mut self) -> LexResult<bool> {
        loop {
            while matches!(self.peek_char(), Some(ch) if ch.is_whitespace()) {
                self.advance_char();
            }
            if self.peek_char() == Some('/') && self.peek_next_char() == Some('/') {
                while let Some(ch) = self.peek_char() {
                    self.advance_char();
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek_char() == Some('/') && self.peek_next_char() == Some('*') {
                self.advance_char();
                self.advance_char();
                let mut closed = false;
                while let Some(ch) = self.peek_char() {
                    if ch == '*' && self.peek_next_char() == Some('/') {
                        self.advance_char();
                        self.advance_char();
                        closed = true;
                        break;
                    }
                    self.advance_char();
                }
                if !closed {
                    return Err(self.error_here("unterminated block comment".to_string()));
                }
                continue;
            }
            break;
        }
        Ok(self.peek_char().is_some())
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.peek_byte().map(char::from)
    }

    fn peek_next_char(&self) -> Option<char> {
        self.bytes.get(self.index + 1).copied().map(char::from)
    }

    fn advance_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.index += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn span_here(&self) -> Span {
        Span::new(self.path, self.line, self.column)
    }

    fn error_here(&self, message: String) -> Diagnostic {
        Diagnostic::new(self.span_here(), message)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn kinds(input: &str) -> Vec<TokenKind> {
        lex_file(Path::new("test.sv"), input)
            .unwrap()
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn lexes_reference_assign_and_constants() {
        let tokens = kinds(
            "assign q = !q_n; bufif0 (strong1, highz0) #(T_rise_d, T_Z_d, T_Z_d) (d, '1, pch_n);",
        );
        assert_eq!(
            tokens,
            vec![
                TokenKind::Keyword(Keyword::Assign),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::Equals),
                TokenKind::Operator(Operator::Bang),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Semicolon),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Comma),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Operator(Operator::Hash),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Comma),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Comma),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Comma),
                TokenKind::ConstOne,
                TokenKind::Punct(Punct::Comma),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Punct(Punct::Semicolon),
            ]
        );
    }

    #[test]
    fn lexes_multichar_operators_and_comments() {
        let tokens = kinds(
            "`default_nettype none\n// comment\nalways_latch if ((a && b) || (c !== 1) || (d == 0) || (e ~^ f)) q <= !q; /* block */",
        );
        assert_eq!(
            tokens,
            vec![
                TokenKind::Directive,
                TokenKind::Keyword(Keyword::AlwaysLatch),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::LParen),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::DoubleAnd),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Operator(Operator::DoubleOr),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::NotCaseEqual),
                TokenKind::Integer,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Operator(Operator::DoubleOr),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::EqualEqual),
                TokenKind::Integer,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Operator(Operator::DoubleOr),
                TokenKind::Punct(Punct::LParen),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::TildeCaret),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::RParen),
                TokenKind::Punct(Punct::RParen),
                TokenKind::Identifier,
                TokenKind::Operator(Operator::LessEqual),
                TokenKind::Operator(Operator::Bang),
                TokenKind::Identifier,
                TokenKind::Punct(Punct::Semicolon),
            ]
        );
    }

    #[test]
    fn reports_bad_character_location() {
        let err = lex_file(Path::new("bad.sv"), "module x %").unwrap_err();
        assert_eq!(err.span.line, 1);
        assert_eq!(err.span.column, 10);
        assert!(err.message.contains("unexpected character"));
    }
}
