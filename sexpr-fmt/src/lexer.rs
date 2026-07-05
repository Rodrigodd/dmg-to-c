use crate::error::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    LParen,
    RParen,
    Atom(String),
    Comment(String),
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub location: Location,
}

pub struct Lexer<'a> {
    source: &'a str,
    index: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            index: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_horizontal_whitespace();

        let location = Location {
            offset: self.index,
            line: self.line,
            column: self.column,
        };

        match self.peek_char() {
            None => Token {
                kind: TokenKind::Eof,
                span: Span {
                    start: self.index,
                    end: self.index,
                },
                location,
            },
            Some('(') => {
                self.bump_char();
                Token {
                    kind: TokenKind::LParen,
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
            Some(')') => {
                self.bump_char();
                Token {
                    kind: TokenKind::RParen,
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
            Some('\n') => {
                self.bump_char();
                Token {
                    kind: TokenKind::Newline,
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
            Some('\r') => {
                self.bump_char();
                if self.peek_char() == Some('\n') {
                    self.bump_char();
                }
                Token {
                    kind: TokenKind::Newline,
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
            Some(';') => {
                self.bump_char();
                while let Some(ch) = self.peek_char() {
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                    self.bump_char();
                }
                let text = self.source[location.offset..self.index].to_string();
                Token {
                    kind: TokenKind::Comment(text),
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
            Some(_) => {
                while let Some(ch) = self.peek_char() {
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == ';' {
                        break;
                    }
                    self.bump_char();
                }
                let text = self.source[location.offset..self.index].to_string();
                Token {
                    kind: TokenKind::Atom(text),
                    span: Span {
                        start: location.offset,
                        end: self.index,
                    },
                    location,
                }
            }
        }
    }

    fn skip_horizontal_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() && ch != '\n' && ch != '\r' {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.index..].chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.index += ch.len_utf8();
        if ch == '\r' {
            if self.peek_char() == Some('\n') {
                self.index += '\n'.len_utf8();
            }
            self.line += 1;
            self.column = 1;
            return Some(ch);
        }
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }
}
