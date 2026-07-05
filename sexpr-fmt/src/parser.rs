use crate::error::{Location, ParseError, ParseErrorKind};
use crate::lexer::{Lexer, Span, Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Expr(Expr),
    Comment(Comment),
    BlankLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Atom(Atom),
    List(List),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom {
    pub text: String,
    pub span: Span,
    pub trailing_comment: Option<Comment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub children: Vec<Item>,
    pub span: Span,
    pub trailing_comment: Option<Comment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub span: Span,
}

pub fn parse_document(source: &str) -> Result<Document, ParseError> {
    Parser::new(source).parse_document()
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    next: Token,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token();
        let next = lexer.next_token();
        Self {
            lexer,
            current,
            next,
        }
    }

    fn parse_document(mut self) -> Result<Document, ParseError> {
        let items = self.parse_items(None)?;
        Ok(Document { items })
    }

    fn parse_items(&mut self, stop_on_close: Option<Location>) -> Result<Vec<Item>, ParseError> {
        let mut items = Vec::new();
        let mut newline_run = 0usize;

        loop {
            while matches!(self.current.kind, TokenKind::Newline) {
                newline_run += 1;
                self.bump();
            }

            if matches!(self.current.kind, TokenKind::Eof) {
                if let Some(location) = stop_on_close {
                    return Err(ParseError::new(
                        ParseErrorKind::UnclosedOpen,
                        location,
                        "unclosed '('",
                    ));
                }
                break;
            }

            if matches!(self.current.kind, TokenKind::RParen) {
                if stop_on_close.is_some() {
                    break;
                }
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedClose,
                    self.current.location,
                    "unexpected ')'",
                ));
            }

            if newline_run >= 2 && !items.is_empty() {
                items.push(Item::BlankLine);
            }
            newline_run = 0;

            match &self.current.kind {
                TokenKind::Comment(_) => {
                    let comment = self.parse_comment();
                    items.push(Item::Comment(comment));
                }
                TokenKind::Atom(_) | TokenKind::LParen => {
                    let expr = self.parse_expr()?;
                    items.push(Item::Expr(expr));
                }
                TokenKind::RParen => unreachable!(),
                TokenKind::Newline | TokenKind::Eof => unreachable!(),
            }
        }

        Ok(trim_items(items))
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let mut expr = match &self.current.kind {
            TokenKind::Atom(text) => {
                let atom = Atom {
                    text: text.clone(),
                    span: self.current.span,
                    trailing_comment: None,
                };
                self.bump();
                Expr::Atom(atom)
            }
            TokenKind::LParen => self.parse_list()?,
            _ => {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken,
                    self.current.location,
                    "unexpected token",
                ));
            }
        };

        if let TokenKind::Comment(_) = self.current.kind {
            let comment = self.parse_comment();
            match &mut expr {
                Expr::Atom(atom) => atom.trailing_comment = Some(comment),
                Expr::List(list) => list.trailing_comment = Some(comment),
            }
        }

        Ok(expr)
    }

    fn parse_list(&mut self) -> Result<Expr, ParseError> {
        let open = self.current.clone();
        self.bump();
        let children = self.parse_items(Some(open.location))?;
        if !matches!(self.current.kind, TokenKind::RParen) {
            return Err(ParseError::new(
                ParseErrorKind::UnclosedOpen,
                open.location,
                "unclosed '('",
            ));
        }
        let close = self.current.clone();
        self.bump();
        Ok(Expr::List(List {
            children,
            span: Span {
                start: open.span.start,
                end: close.span.end,
            },
            trailing_comment: None,
        }))
    }

    fn parse_comment(&mut self) -> Comment {
        let token = self.current.clone();
        let text = match token.kind {
            TokenKind::Comment(text) => text,
            _ => unreachable!(),
        };
        self.bump();
        Comment {
            text,
            span: token.span,
        }
    }

    fn bump(&mut self) {
        self.current = std::mem::replace(&mut self.next, self.lexer.next_token());
    }
}

fn trim_items(mut items: Vec<Item>) -> Vec<Item> {
    while matches!(items.first(), Some(Item::BlankLine)) {
        items.remove(0);
    }
    while matches!(items.last(), Some(Item::BlankLine)) {
        items.pop();
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_regular_atom_characters() {
        let doc = parse_document("(\"foo\" bar)").unwrap();
        match &doc.items[0] {
            Item::Expr(Expr::List(list)) => match &list.children[0] {
                Item::Expr(Expr::Atom(atom)) => assert_eq!(atom.text, "\"foo\""),
                _ => panic!("expected atom"),
            },
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn unexpected_close_reports_location() {
        let err = parse_document(")").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedClose));
        assert!(err.to_string().contains("byte 0"));
    }

    #[test]
    fn unclosed_open_reports_location() {
        let err = parse_document("(").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnclosedOpen));
        assert!(err.to_string().contains("byte 0"));
    }
}
