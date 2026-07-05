use crate::diagnostic::Diagnostic;
use crate::lexer::{Keyword, Operator, Punct, TokenKind, lex_file};
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
    let mut report = CheckReport::default();
    for file in collect_sv_files(path)? {
        report.processed += 1;
        match fs::read_to_string(&file) {
            Ok(contents) => match lex_file(&file, &contents) {
                Ok(_) => {}
                Err(err) => {
                    report.failed += 1;
                    report.failures.push(err.to_string());
                }
            },
            Err(err) => {
                report.failed += 1;
                report.failures.push(
                    Diagnostic::new(
                        crate::diagnostic::Span::new(&file, 1, 1),
                        format!("failed to read file: {}", err),
                    )
                    .to_string(),
                );
            }
        }
    }
    Ok(report)
}

#[derive(Debug, Default, Clone)]
pub struct CheckReport {
    pub processed: usize,
    pub failed: usize,
    pub failures: Vec<String>,
}

impl CheckReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "lex check summary: processed={} failed={}\n",
            self.processed, self.failed
        ));
        for failure in &self.failures {
            out.push_str("  ");
            out.push_str(failure);
            out.push('\n');
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
