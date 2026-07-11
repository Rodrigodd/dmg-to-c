use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    UnexpectedClose,
    UnclosedOpen,
    UnexpectedToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub location: Location,
    pub message: String,
}

impl ParseError {
    pub fn new(kind: ParseErrorKind, location: Location, message: impl Into<String>) -> Self {
        Self {
            kind,
            location,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at byte {} (line {}, column {})",
            self.message, self.location.offset, self.location.line, self.location.column
        )
    }
}

impl std::error::Error for ParseError {}
