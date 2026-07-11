use crate::analyze::analyze_file;
use crate::diagnostic::{Diagnostic, DiagnosticCollection, DiagnosticKind, DiagnosticPolicy};
use crate::lexer::{Keyword, Operator, Punct, TokenKind, lex_file};
use crate::lower::lower_file;
use crate::parser::parse_file;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct SurveyReport {
    pub files: usize,
    pub failed_files: usize,
    pub tokens: usize,
    pub kinds: BTreeMap<String, usize>,
}

impl SurveyReport {
    pub fn record(&mut self, tokens: &[crate::lexer::Token]) {
        self.tokens += tokens.len();
        for token in tokens {
            *self
                .kinds
                .entry(kind_label(&token.kind).to_string())
                .or_insert(0) += 1;
        }
    }

    pub fn record_failure(&mut self) {
        self.failed_files += 1;
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "survey summary: files={} failed={} tokens={}\n",
            self.files, self.failed_files, self.tokens
        ));
        for (kind, count) in &self.kinds {
            out.push_str(&format!("  {:<24} {}\n", kind, count));
        }
        out
    }
}

pub fn survey_dir(path: &Path) -> Result<SurveyReport, Diagnostic> {
    let mut report = SurveyReport::default();
    for file in collect_sv_files(path)? {
        report.files += 1;
        match fs::read_to_string(&file) {
            Ok(contents) => match lex_file(&file, &contents) {
                Ok(tokens) => report.record(&tokens),
                Err(_) => report.record_failure(),
            },
            Err(err) => {
                report.record_failure();
                let _ = err;
            }
        }
    }
    Ok(report)
}

pub fn check_lex_dir(path: &Path) -> Result<CheckReport, Diagnostic> {
    check_dir(path, "lex", |file, contents| {
        lex_file(file, contents).map(|_| ())
    })
}

pub fn check_parse_dir(path: &Path) -> Result<CheckReport, Diagnostic> {
    check_dir(path, "parse", |file, contents| {
        parse_file(file, contents).map(|_| ())
    })
}

pub fn check_analyze_dir(path: &Path) -> Result<CheckReport, Diagnostic> {
    check_dir(path, "analyze", |file, contents| {
        analyze_file(file, contents).map(|_| ())
    })
}

pub fn check_lower_dir(path: &Path) -> Result<CheckReport, Diagnostic> {
    check_dir(path, "lower", |file, contents| {
        lower_file(file, contents).map(|_| ())
    })
}

fn check_dir(
    path: &Path,
    stage: &str,
    check: impl Fn(&Path, &str) -> Result<(), Diagnostic>,
) -> Result<CheckReport, Diagnostic> {
    let mut report = CheckReport::new(stage);
    for file in collect_sv_files(path)? {
        report.processed += 1;
        match fs::read_to_string(&file) {
            Ok(contents) => {
                if let Err(diagnostic) = check(&file, &contents) {
                    report.record(diagnostic);
                }
            }
            Err(err) => report.record(Diagnostic::new(
                crate::diagnostic::Span::new(&file, 1, 1),
                format!("failed to read file: {}", err),
            )),
        }
    }
    Ok(report)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub stage: String,
    pub processed: usize,
    diagnostics: DiagnosticCollection,
}

impl CheckReport {
    pub fn new(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            ..Self::default()
        }
    }

    pub fn record(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn warned(&self) -> usize {
        self.diagnostics.warnings()
    }

    pub fn intentional_ignores(&self) -> usize {
        self.diagnostics.intentional_ignores()
    }

    pub fn failed(&self) -> usize {
        self.diagnostics.errors()
    }

    pub fn diagnostics(&self) -> &DiagnosticCollection {
        &self.diagnostics
    }

    pub fn fails(&self, policy: DiagnosticPolicy) -> bool {
        self.diagnostics.fails(policy)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{} check summary: processed={} warned={} intentional-ignored={} failed={}\n",
            self.stage,
            self.processed,
            self.warned(),
            self.intentional_ignores(),
            self.failed()
        ));
        for kind in [
            DiagnosticKind::Warning,
            DiagnosticKind::IntentionalIgnore,
            DiagnosticKind::Error,
        ] {
            let mut details = self
                .diagnostics
                .entries()
                .iter()
                .filter(|diagnostic| diagnostic.kind == kind)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            details.sort();
            for detail in details {
                out.push_str("  ");
                out.push_str(&detail);
                out.push('\n');
            }
        }
        out
    }
}

pub fn collect_sv_files(path: &Path) -> Result<Vec<PathBuf>, Diagnostic> {
    let mut files = Vec::new();
    collect_sv_files_inner(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_sv_files_inner(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), Diagnostic> {
    let meta = fs::metadata(path).map_err(|err| {
        Diagnostic::new(
            crate::diagnostic::Span::new(path, 1, 1),
            format!("failed to stat path: {}", err),
        )
    })?;
    if meta.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("sv") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|err| {
        Diagnostic::new(
            crate::diagnostic::Span::new(path, 1, 1),
            format!("failed to read directory: {}", err),
        )
    })? {
        let entry = entry.map_err(|err| {
            Diagnostic::new(
                crate::diagnostic::Span::new(path, 1, 1),
                format!("failed to read directory entry: {}", err),
            )
        })?;
        collect_sv_files_inner(&entry.path(), out)?;
    }
    Ok(())
}

fn kind_label(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Identifier => "identifier",
        TokenKind::Keyword(keyword) => match keyword {
            Keyword::Module => "keyword:module",
            Keyword::Endmodule => "keyword:endmodule",
            Keyword::Parameter => "keyword:parameter",
            Keyword::Localparam => "keyword:localparam",
            Keyword::Real => "keyword:real",
            Keyword::Realtime => "keyword:realtime",
            Keyword::Input => "keyword:input",
            Keyword::Output => "keyword:output",
            Keyword::Inout => "keyword:inout",
            Keyword::Logic => "keyword:logic",
            Keyword::Tri => "keyword:tri",
            Keyword::Wire => "keyword:wire",
            Keyword::Import => "keyword:import",
            Keyword::Initial => "keyword:initial",
            Keyword::AlwaysLatch => "keyword:always_latch",
            Keyword::Assign => "keyword:assign",
            Keyword::Specify => "keyword:specify",
            Keyword::EndSpecify => "keyword:endspecify",
            Keyword::Specparam => "keyword:specparam",
        },
        TokenKind::Integer => "integer",
        TokenKind::Real => "real",
        TokenKind::ConstZero => "const:'0",
        TokenKind::ConstOne => "const:'1",
        TokenKind::ConstZ => "const:'z",
        TokenKind::ConstX => "const:'x",
        TokenKind::Punct(punct) => match punct {
            Punct::LParen => "punct:(",
            Punct::RParen => "punct:)",
            Punct::LBracket => "punct:[",
            Punct::RBracket => "punct:]",
            Punct::LBrace => "punct:{",
            Punct::RBrace => "punct:}",
            Punct::Comma => "punct:,",
            Punct::Semicolon => "punct:;",
        },
        TokenKind::Operator(op) => match op {
            Operator::DoubleAnd => "op:&&",
            Operator::DoubleOr => "op:||",
            Operator::EqualEqual => "op:==",
            Operator::TripleEqual => "op:===",
            Operator::NotEqual => "op:!=",
            Operator::NotCaseEqual => "op:!==",
            Operator::LessEqual => "op:<=",
            Operator::Implies => "op:*>",
            Operator::ColonColon => "op:::",
            Operator::TildeCaret => "op:~^",
            Operator::TildeAmpersand => "op:~&",
            Operator::TildePipe => "op:~|",
            Operator::Tilde => "op:~",
            Operator::Bang => "op:!",
            Operator::Ampersand => "op:&",
            Operator::Pipe => "op:|",
            Operator::Caret => "op:^",
            Operator::Plus => "op:+",
            Operator::Minus => "op:-",
            Operator::Star => "op:*",
            Operator::Slash => "op:/",
            Operator::Equals => "op:=",
            Operator::Less => "op:<",
            Operator::Greater => "op:>",
            Operator::At => "op:@",
            Operator::Dot => "op:.",
            Operator::Hash => "op:#",
            Operator::Question => "op:?",
            Operator::Colon => "op::",
        },
        TokenKind::Directive => "directive",
    }
}

#[cfg(test)]
mod check_report_tests {
    use super::*;

    fn span(path: &str) -> crate::diagnostic::Span {
        crate::diagnostic::Span::new(path, 1, 1)
    }

    #[test]
    fn warnings_fail_only_in_strict_mode_and_errors_always_fail() {
        let mut warning = CheckReport::new("lex");
        warning.record(Diagnostic::warning(span("warning.sv"), "approximation"));
        assert!(!warning.fails(DiagnosticPolicy::new(false)));
        assert!(warning.fails(DiagnosticPolicy::new(true)));
        assert_eq!(warning.warned(), 1);

        let mut error = CheckReport::new("lex");
        error.record(Diagnostic::error(span("error.sv"), "unsupported"));
        assert!(error.fails(DiagnosticPolicy::new(false)));
        assert!(error.fails(DiagnosticPolicy::new(true)));
        assert_eq!(error.failed(), 1);
    }

    #[test]
    fn intentional_ignores_are_visible_but_do_not_fail() {
        let mut report = CheckReport::new("lower");
        report.record(Diagnostic::intentional_ignore(
            span("ignored.sv"),
            "later delay entry",
        ));
        assert!(!report.fails(DiagnosticPolicy::new(false)));
        assert!(!report.fails(DiagnosticPolicy::new(true)));
        assert_eq!(report.intentional_ignores(), 1);
        assert!(report.render().contains("intentional-ignored=1"));
    }

    #[test]
    fn summary_and_grouped_details_render_deterministically() {
        let mut report = CheckReport::new("lower");
        report.processed = 4;
        report.record(Diagnostic::warning(span("b.sv"), "second warning"));
        report.record(Diagnostic::error(span("d.sv"), "failure"));
        report.record(Diagnostic::intentional_ignore(
            span("c.sv"),
            "ignored source",
        ));
        report.record(Diagnostic::warning(span("a.sv"), "first warning"));

        assert_eq!(
            report.render(),
            concat!(
                "lower check summary: processed=4 warned=2 intentional-ignored=1 failed=1\n",
                "  a.sv:1:1: warning: first warning\n",
                "  b.sv:1:1: warning: second warning\n",
                "  c.sv:1:1: intentional-ignore: ignored source\n",
                "  d.sv:1:1: error: failure\n",
            )
        );
    }
}
