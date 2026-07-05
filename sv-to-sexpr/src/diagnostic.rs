use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl Span {
    pub fn new(path: impl Into<PathBuf>, line: usize, column: usize) -> Self {
        Self {
            path: path.into(),
            line,
            column,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.span.path.display(),
            self.span.line,
            self.span.column,
            self.message
        )
    }
}

impl std::error::Error for Diagnostic {}
