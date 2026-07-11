use crate::analyze::analyze_file;
use crate::diagnostic::{Diagnostic, DiagnosticCollection, DiagnosticKind, DiagnosticPolicy};
use crate::inventory::{
    CapabilityInventory, ClassificationKind, InventoryWalker, record_token_capabilities,
};
use crate::lexer::{Keyword, Operator, Punct, TokenKind, lex_file};
use crate::lower::lower_file;
use crate::parser::parse_file;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveyFailure {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub kind: DiagnosticKind,
    pub message: String,
}

impl SurveyFailure {
    fn from_diagnostic(path: String, diagnostic: Diagnostic) -> Self {
        Self {
            path,
            line: diagnostic.span.line,
            column: diagnostic.span.column,
            kind: diagnostic.kind,
            message: diagnostic.message,
        }
    }

    fn read_error(path: String, message: String) -> Self {
        Self {
            path,
            line: 1,
            column: 1,
            kind: DiagnosticKind::Error,
            message,
        }
    }

    fn render(&self) -> String {
        format!(
            "{}:{}:{}: {}: {}",
            self.path, self.line, self.column, self.kind, self.message
        )
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SurveyReport {
    pub files: usize,
    pub failed_files: usize,
    pub tokens: usize,
    pub kinds: BTreeMap<String, usize>,
    pub inventory: CapabilityInventory,
    pub failures: Vec<SurveyFailure>,
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

    pub fn record_failure(&mut self, failure: SurveyFailure) {
        self.failed_files += 1;
        self.failures.push(failure);
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "survey summary: files={} failed={} tokens={} supported={} deferred={} intentional-ignored={} unsupported={}\n",
            self.files,
            self.failed_files,
            self.tokens,
            self.inventory
                .classification_count(ClassificationKind::Supported),
            self.inventory
                .classification_count(ClassificationKind::Deferred),
            self.inventory
                .classification_count(ClassificationKind::IntentionalIgnore),
            self.inventory.unsupported_count(),
        ));
        out.push_str("token kinds:\n");
        for (kind, count) in &self.kinds {
            out.push_str(&format!("  {:<24} {}\n", kind, count));
        }
        let mut failures = self
            .failures
            .iter()
            .map(SurveyFailure::render)
            .collect::<Vec<_>>();
        failures.sort();
        out.push_str(&format!("failures: {}\n", failures.len()));
        for failure in failures {
            out.push_str("  ");
            out.push_str(&failure);
            out.push('\n');
        }
        out.push_str(&format!(
            "capabilities: {}\n",
            self.inventory.records().len()
        ));
        for (id, record) in self.inventory.records() {
            out.push_str(&format!(
                "  {id} | {} | occurrences={} | files={}\n",
                record.classification.label(),
                record.occurrences,
                record
                    .files
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        out.push_str(&format!(
            "unsupported capabilities: {}\n",
            self.inventory.unsupported_count()
        ));
        for (id, record) in
            self.inventory.records().iter().filter(|(_, record)| {
                record.classification.kind() == ClassificationKind::Unsupported
            })
        {
            out.push_str(&format!(
                "  {id} | {} | occurrences={} | files={}\n",
                record.classification.label(),
                record.occurrences,
                record
                    .files
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        out
    }
}

pub fn survey_dir(path: &Path) -> Result<SurveyReport, Diagnostic> {
    let mut report = SurveyReport::default();
    let mut parsed_files = Vec::new();
    for file in collect_sv_files(path)? {
        report.files += 1;
        let normalized = normalize_survey_path(path, &file);
        match fs::read_to_string(&file) {
            Ok(contents) => match lex_file(&file, &contents) {
                Ok(tokens) => {
                    report.record(&tokens);
                    record_token_capabilities(&mut report.inventory, &tokens, &normalized);
                    match parse_file(&file, &contents) {
                        Ok(design) => parsed_files.push((normalized, design)),
                        Err(diagnostic) => report
                            .record_failure(SurveyFailure::from_diagnostic(normalized, diagnostic)),
                    }
                }
                Err(diagnostic) => {
                    report.record_failure(SurveyFailure::from_diagnostic(normalized, diagnostic))
                }
            },
            Err(err) => {
                report.record_failure(SurveyFailure::read_error(
                    normalized,
                    format!("failed to read file: {err}"),
                ));
            }
        }
    }
    let known_modules = parsed_files
        .iter()
        .flat_map(|(_, design)| design.modules().map(|module| module.name.clone()))
        .collect::<BTreeSet<_>>();
    for (normalized, design) in &parsed_files {
        let mut walker = InventoryWalker::new(&mut report.inventory, &known_modules, normalized);
        walker.record_design(design);
    }
    Ok(report)
}

fn normalize_survey_path(root: &Path, file: &Path) -> String {
    let relative = if root.is_file() {
        file.file_name().map(Path::new).unwrap_or(file)
    } else {
        file.strip_prefix(root).unwrap_or(file)
    };
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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

    #[test]
    fn survey_surfaces_lex_and_parse_failures_with_normalized_paths() {
        let directory = std::env::temp_dir().join(format!(
            "sv-to-sexpr-survey-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("bad_lex.sv"), "module bad; % endmodule\n").unwrap();
        std::fs::write(directory.join("bad_parse.sv"), "module truncated;\n").unwrap();
        std::fs::write(
            directory.join("unknown_directive.sv"),
            "`mystery setting\nmodule okay; endmodule\n",
        )
        .unwrap();

        let report = survey_dir(&directory).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();

        assert_eq!(report.files, 3);
        assert_eq!(report.failed_files, 3);
        assert_eq!(report.failures.len(), 3);
        let rendered = report.render();
        assert!(rendered.contains("bad_lex.sv:1:13: error: unexpected character `%`"));
        assert!(rendered.contains("bad_parse.sv:2:1: error: unterminated module body"));
        assert!(rendered.contains(
            "unknown_directive.sv:1:1: error: unsupported directive `mystery` at design scope"
        ));
        assert!(!rendered.contains(&directory.to_string_lossy().to_string()));

        let directive = report
            .inventory
            .record_by_id("directive.`mystery")
            .expect("unknown directive must remain inventoried after parse failure");
        assert_eq!(
            directive.classification.kind(),
            ClassificationKind::Unsupported
        );
        assert_eq!(directive.occurrences, 1);
        assert_eq!(
            directive.files,
            BTreeSet::from(["unknown_directive.sv".to_string()])
        );
    }
}
