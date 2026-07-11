use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Error,
    Warning,
    IntentionalIgnore,
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::IntentionalIgnore => "intentional-ignore",
        })
    }
}

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
    pub kind: DiagnosticKind,
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    /// Constructs an error, preserving the behavior of the original API.
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self::error(span, message)
    }

    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticKind::Error,
            span,
            message: message.into(),
        }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticKind::Warning,
            span,
            message: message.into(),
        }
    }

    pub fn intentional_ignore(span: Span, message: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticKind::IntentionalIgnore,
            span,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {}",
            self.span.path.display(),
            self.span.line,
            self.span.column,
            self.kind,
            self.message
        )
    }
}

impl std::error::Error for Diagnostic {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticPolicy {
    pub strict: bool,
}

impl DiagnosticPolicy {
    pub fn new(strict: bool) -> Self {
        Self { strict }
    }

    pub fn is_failure(self, diagnostic: &Diagnostic) -> bool {
        match diagnostic.kind {
            DiagnosticKind::Error => true,
            DiagnosticKind::Warning => self.strict,
            DiagnosticKind::IntentionalIgnore => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticCollection {
    entries: Vec<Diagnostic>,
}

impl DiagnosticCollection {
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.entries.push(diagnostic);
    }

    pub fn entries(&self) -> &[Diagnostic] {
        &self.entries
    }

    pub fn warnings(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == DiagnosticKind::Warning)
            .count()
    }

    pub fn errors(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == DiagnosticKind::Error)
            .count()
    }

    pub fn intentional_ignores(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == DiagnosticKind::IntentionalIgnore)
            .count()
    }

    pub fn fails(&self, policy: DiagnosticPolicy) -> bool {
        self.entries.iter().any(|entry| policy.is_failure(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span::new("gate.sv", 3, 7)
    }

    #[test]
    fn display_identifies_error_warning_and_intentional_ignore() {
        let error = Diagnostic::new(span(), "bad gate");
        assert_eq!(error.kind, DiagnosticKind::Error);
        assert_eq!(error.to_string(), "gate.sv:3:7: error: bad gate");
        let warning = Diagnostic::warning(span(), "approximation");
        assert_eq!(warning.kind, DiagnosticKind::Warning);
        assert_eq!(warning.to_string(), "gate.sv:3:7: warning: approximation");
        let ignore = Diagnostic::intentional_ignore(span(), "later tuple entry");
        assert_eq!(ignore.kind, DiagnosticKind::IntentionalIgnore);
        assert_eq!(
            ignore.to_string(),
            "gate.sv:3:7: intentional-ignore: later tuple entry"
        );
    }

    #[test]
    fn strict_policy_promotes_warnings_but_not_intentional_ignores() {
        let mut diagnostics = DiagnosticCollection::default();
        diagnostics.push(Diagnostic::warning(span(), "approximation"));
        diagnostics.push(Diagnostic::intentional_ignore(span(), "later tuple entry"));
        assert!(!diagnostics.fails(DiagnosticPolicy::new(false)));
        assert!(diagnostics.fails(DiagnosticPolicy::new(true)));
        assert_eq!(diagnostics.warnings(), 1);
        assert_eq!(diagnostics.intentional_ignores(), 1);
    }

    #[test]
    fn intentional_ignores_never_fail() {
        let diagnostic = Diagnostic::intentional_ignore(span(), "later tuple entry");
        assert!(!DiagnosticPolicy::new(false).is_failure(&diagnostic));
        assert!(!DiagnosticPolicy::new(true).is_failure(&diagnostic));
    }

    #[test]
    fn errors_always_fail() {
        let mut diagnostics = DiagnosticCollection::default();
        diagnostics.push(Diagnostic::error(span(), "unsupported"));
        assert!(diagnostics.fails(DiagnosticPolicy::new(false)));
        assert!(diagnostics.fails(DiagnosticPolicy::new(true)));
        assert_eq!(diagnostics.errors(), 1);
    }
}
