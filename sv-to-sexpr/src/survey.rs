use crate::analyze::{
    AnalysisDisposition, AnalysisReport, CapabilityRequirement, ModuleCatalog,
    analyze_design_with_catalog_and_generate_mode, analyze_design_with_catalog_structural,
};
use crate::diagnostic::{Diagnostic, DiagnosticCollection, DiagnosticKind, DiagnosticPolicy};
use crate::elaborate::GenerateMode;
use crate::inventory::{
    CapabilityInventory, ClassificationKind, InventoryWalker, record_token_capabilities,
};
use crate::ir::LoweredModule;
use crate::lexer::{Keyword, Operator, Punct, TokenKind, lex_file};
use crate::lower::{lower_design_with_catalog_and_generate_mode, lower_file_structural};
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

pub fn check_analyze_dir(path: &Path) -> Result<AnalyzeCheckReport, Diagnostic> {
    check_analyze_dir_with_generate_mode(path, GenerateMode::default())
}

pub fn check_analyze_dir_with_generate_mode(
    path: &Path,
    mode: GenerateMode,
) -> Result<AnalyzeCheckReport, Diagnostic> {
    check_analyze_dir_impl(path, Some(mode))
}

/// Runs the M3 inventory analysis without selecting generate branches.
pub fn check_analyze_dir_structural(path: &Path) -> Result<AnalyzeCheckReport, Diagnostic> {
    check_analyze_dir_impl(path, None)
}

fn check_analyze_dir_impl(
    path: &Path,
    mode: Option<GenerateMode>,
) -> Result<AnalyzeCheckReport, Diagnostic> {
    let files = collect_sv_files(path)?;
    let mut report = AnalyzeCheckReport {
        processed: files.len(),
        ..AnalyzeCheckReport::default()
    };
    let mut parsed = Vec::new();
    for file in files {
        match fs::read_to_string(&file) {
            Ok(contents) => match parse_file(&file, &contents) {
                Ok(design) => parsed.push((file, design)),
                Err(diagnostic) => report
                    .files
                    .push(AnalyzeFileReport::failed(file, vec![diagnostic])),
            },
            Err(error) => report.files.push(AnalyzeFileReport::failed(
                file.clone(),
                vec![Diagnostic::new(
                    crate::diagnostic::Span::new(&file, 1, 1),
                    format!("failed to read file: {error}"),
                )],
            )),
        }
    }
    let designs = parsed
        .iter()
        .map(|(_, design)| design.clone())
        .collect::<Vec<_>>();
    let catalog = match ModuleCatalog::from_designs(&designs) {
        Ok(catalog) => catalog,
        Err(diagnostic) => {
            for (file, _) in parsed {
                let diagnostics = if diagnostic.span.path == file {
                    vec![diagnostic.clone()]
                } else {
                    vec![Diagnostic::new(
                        crate::diagnostic::Span::new(&file, 1, 1),
                        "module catalog construction failed",
                    )]
                };
                report
                    .files
                    .push(AnalyzeFileReport::failed(file, diagnostics));
            }
            report.sort_files();
            return Ok(report);
        }
    };
    for (file, design) in parsed {
        let analysis = match mode {
            Some(mode) => analyze_design_with_catalog_and_generate_mode(&design, &catalog, mode),
            None => analyze_design_with_catalog_structural(&design, &catalog),
        };
        match analysis {
            Ok(analysis) => report
                .files
                .push(AnalyzeFileReport::from_analysis(file, analysis)),
            Err(diagnostic) => report
                .files
                .push(AnalyzeFileReport::failed(file, vec![diagnostic])),
        }
    }
    report.sort_files();
    Ok(report)
}

pub fn analyze_file_with_sibling_catalog(path: &Path) -> Result<AnalysisReport, Diagnostic> {
    analyze_file_with_sibling_catalog_and_generate_mode(path, GenerateMode::default())
}

pub fn analyze_file_with_sibling_catalog_and_generate_mode(
    path: &Path,
    mode: GenerateMode,
) -> Result<AnalysisReport, Diagnostic> {
    analyze_file_with_sibling_catalog_impl(path, Some(mode))
}

/// Runs the M3 inventory analysis without selecting generate branches.
pub fn analyze_file_with_sibling_catalog_structural(
    path: &Path,
) -> Result<AnalysisReport, Diagnostic> {
    analyze_file_with_sibling_catalog_impl(path, None)
}

fn analyze_file_with_sibling_catalog_impl(
    path: &Path,
    mode: Option<GenerateMode>,
) -> Result<AnalysisReport, Diagnostic> {
    let target = load_sibling_catalog_target(path)?;
    match mode {
        Some(mode) => analyze_design_with_catalog_and_generate_mode(
            &target.designs[target.target_index],
            &target.catalog,
            mode,
        ),
        None => analyze_design_with_catalog_structural(
            &target.designs[target.target_index],
            &target.catalog,
        ),
    }
}

pub fn lower_file_with_sibling_catalog(path: &Path) -> Result<LoweredModule, Diagnostic> {
    lower_file_with_sibling_catalog_and_generate_mode(path, GenerateMode::default())
}

pub fn lower_file_with_sibling_catalog_and_generate_mode(
    path: &Path,
    mode: GenerateMode,
) -> Result<LoweredModule, Diagnostic> {
    let target = load_sibling_catalog_target(path)?;
    lower_design_with_catalog_and_generate_mode(
        &target.designs[target.target_index],
        &target.catalog,
        mode,
    )
}

struct SiblingCatalogTarget {
    designs: Vec<crate::ast::Design>,
    target_index: usize,
    catalog: ModuleCatalog,
}

fn load_sibling_catalog_target(path: &Path) -> Result<SiblingCatalogTarget, Diagnostic> {
    let parent = sibling_catalog_parent(path);
    let mut files = collect_sv_files(parent)?;
    // Preserve the caller's exact path in spans while avoiding a second copy
    // such as `./parent.sv` when the requested path is bare.
    files.retain(|candidate| !lexically_same_path(candidate, path));
    files.push(path.to_path_buf());
    files.sort();
    files.dedup();
    let mut parsed = Vec::new();
    let mut target_index = None;
    for file in files {
        let contents = fs::read_to_string(&file).map_err(|error| {
            Diagnostic::new(
                crate::diagnostic::Span::new(&file, 1, 1),
                format!("failed to read file: {error}"),
            )
        })?;
        let design = parse_file(&file, &contents)?;
        if file == path {
            target_index = Some(parsed.len());
        }
        parsed.push(design);
    }
    let target_index = target_index.ok_or_else(|| {
        Diagnostic::new(
            crate::diagnostic::Span::new(path, 1, 1),
            "input SystemVerilog file was not included in its sibling catalog",
        )
    })?;
    let catalog = ModuleCatalog::from_designs(&parsed)?;
    Ok(SiblingCatalogTarget {
        designs: parsed,
        target_index,
        catalog,
    })
}

fn sibling_catalog_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn lexically_same_path(left: &Path, right: &Path) -> bool {
    left.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .eq(right
            .components()
            .filter(|component| !matches!(component, std::path::Component::CurDir)))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AnalyzeCheckReport {
    pub processed: usize,
    pub files: Vec<AnalyzeFileReport>,
}

impl AnalyzeCheckReport {
    fn sort_files(&mut self) {
        self.files.sort_by(|left, right| left.path.cmp(&right.path));
    }

    pub fn supported(&self) -> usize {
        self.count(AnalysisDisposition::Supported)
    }

    pub fn deferred(&self) -> usize {
        self.count(AnalysisDisposition::Deferred)
    }

    pub fn warned(&self) -> usize {
        self.count(AnalysisDisposition::Warned)
    }

    pub fn failed(&self) -> usize {
        self.count(AnalysisDisposition::Failed)
    }

    fn count(&self, disposition: AnalysisDisposition) -> usize {
        self.files
            .iter()
            .filter(|file| file.disposition == disposition)
            .count()
    }

    pub fn fails(&self, policy: DiagnosticPolicy) -> bool {
        self.failed() > 0 || (policy.strict && self.warned() > 0)
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "analyze check summary: processed={} supported={} deferred={} warned={} failed={}\n",
            self.processed,
            self.supported(),
            self.deferred(),
            self.warned(),
            self.failed()
        );
        for file in &self.files {
            out.push_str(&format!(
                "  {}: {}\n",
                file.path.display(),
                file.disposition.label()
            ));
            for requirement in &file.requirements {
                out.push_str(&format!(
                    "    {} | {} | {}:{} | {}\n",
                    requirement.capability_id,
                    requirement.milestone.label(),
                    requirement.span.line,
                    requirement.span.column,
                    requirement.reason
                ));
            }
            for diagnostic in &file.diagnostics {
                out.push_str(&format!("    {diagnostic}\n"));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzeFileReport {
    pub path: PathBuf,
    pub disposition: AnalysisDisposition,
    pub requirements: Vec<CapabilityRequirement>,
    pub diagnostics: Vec<Diagnostic>,
}

impl AnalyzeFileReport {
    fn from_analysis(path: PathBuf, analysis: AnalysisReport) -> Self {
        Self {
            path,
            disposition: analysis.disposition,
            requirements: analysis.requirements,
            diagnostics: analysis.diagnostics,
        }
    }

    fn failed(path: PathBuf, mut diagnostics: Vec<Diagnostic>) -> Self {
        diagnostics.sort_by(|left, right| {
            left.span
                .line
                .cmp(&right.span.line)
                .then_with(|| left.span.column.cmp(&right.span.column))
                .then_with(|| left.message.cmp(&right.message))
        });
        Self {
            path,
            disposition: AnalysisDisposition::Failed,
            requirements: Vec::new(),
            diagnostics,
        }
    }
}

pub fn check_lower_dir(path: &Path) -> Result<CheckReport, Diagnostic> {
    check_lower_dir_with_generate_mode(path, GenerateMode::default())
}

pub fn check_lower_dir_with_generate_mode(
    path: &Path,
    mode: GenerateMode,
) -> Result<CheckReport, Diagnostic> {
    let files = collect_sv_files(path)?;
    let mut report = CheckReport::new("lower");
    report.processed = files.len();
    let mut parsed = Vec::new();
    for file in files {
        match fs::read_to_string(&file) {
            Ok(contents) => match parse_file(&file, &contents) {
                Ok(design) => parsed.push((file, design)),
                Err(diagnostic) => report.record(diagnostic),
            },
            Err(error) => report.record(Diagnostic::new(
                crate::diagnostic::Span::new(&file, 1, 1),
                format!("failed to read file: {error}"),
            )),
        }
    }
    let designs = parsed
        .iter()
        .map(|(_, design)| design.clone())
        .collect::<Vec<_>>();
    let catalog = match ModuleCatalog::from_designs(&designs) {
        Ok(catalog) => catalog,
        Err(diagnostic) => {
            for (file, _) in parsed {
                if diagnostic.span.path == file {
                    report.record(diagnostic.clone());
                } else {
                    report.record(Diagnostic::new(
                        crate::diagnostic::Span::new(&file, 1, 1),
                        "module catalog construction failed",
                    ));
                }
            }
            return Ok(report);
        }
    };
    for (_, design) in parsed {
        match lower_design_with_catalog_and_generate_mode(&design, &catalog, mode) {
            Ok(lowered) => {
                for diagnostic in lowered.diagnostics {
                    report.record(diagnostic);
                }
            }
            Err(diagnostic) => report.record(diagnostic),
        }
    }
    Ok(report)
}

/// Runs the M3-M7 lowering inventory without selecting generate branches.
pub fn check_lower_dir_structural(path: &Path) -> Result<CheckReport, Diagnostic> {
    let mut report = CheckReport::new("lower");
    for file in collect_sv_files(path)? {
        report.processed += 1;
        match fs::read_to_string(&file) {
            Ok(contents) => match lower_file_structural(&file, &contents) {
                Ok(lowered) => {
                    for diagnostic in lowered.diagnostics {
                        report.record(diagnostic);
                    }
                }
                Err(diagnostic) => report.record(diagnostic),
            },
            Err(err) => report.record(Diagnostic::new(
                crate::diagnostic::Span::new(&file, 1, 1),
                format!("failed to read file: {err}"),
            )),
        }
    }
    Ok(report)
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
    use crate::analyze::{InstantiationResolution, TargetMilestone};

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
    fn lower_check_captures_successful_initial_values_without_diagnostics() {
        let directory = temporary_directory("lower-success-diagnostics");
        std::fs::write(
            directory.join("a.sv"),
            "module a(output logic q0, q1);\n  initial q0 = 0;\n  initial q1 = '1;\nendmodule\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("b.sv"),
            "module b(input logic d, output logic q);\n  always_latch if (d) q = d;\nendmodule\n",
        )
        .unwrap();

        let report = check_lower_dir(&directory).unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(report.processed, 2);
        assert_eq!(report.warned(), 0);
        assert_eq!(report.intentional_ignores(), 0);
        assert_eq!(report.failed(), 0);
        assert!(!report.fails(DiagnosticPolicy::new(false)));
        assert!(!report.fails(DiagnosticPolicy::new(true)));
        assert!(report.diagnostics().entries().is_empty());
        let rendered = report.render();
        assert_eq!(rendered.matches("intentional-ignore:").count(), 0);
        assert_eq!(rendered, report.render());
    }

    #[test]
    fn analyze_report_counts_each_file_once_and_obeys_policy() {
        let requirement = CapabilityRequirement {
            span: span("deferred.sv"),
            capability_id: "hierarchy.ordinary".to_string(),
            milestone: TargetMilestone::M9OrdinaryHierarchy,
            disposition: AnalysisDisposition::Deferred,
            reason: "ordinary hierarchy lowering is scheduled for Milestone 9".to_string(),
        };
        let report = AnalyzeCheckReport {
            processed: 4,
            files: vec![
                AnalyzeFileReport {
                    path: PathBuf::from("supported.sv"),
                    disposition: AnalysisDisposition::Supported,
                    requirements: Vec::new(),
                    diagnostics: Vec::new(),
                },
                AnalyzeFileReport {
                    path: PathBuf::from("deferred.sv"),
                    disposition: AnalysisDisposition::Deferred,
                    requirements: vec![requirement],
                    diagnostics: Vec::new(),
                },
                AnalyzeFileReport {
                    path: PathBuf::from("warned.sv"),
                    disposition: AnalysisDisposition::Warned,
                    requirements: Vec::new(),
                    diagnostics: vec![Diagnostic::warning(span("warned.sv"), "review required")],
                },
                AnalyzeFileReport {
                    path: PathBuf::from("failed.sv"),
                    disposition: AnalysisDisposition::Failed,
                    requirements: Vec::new(),
                    diagnostics: vec![Diagnostic::error(span("failed.sv"), "invalid source")],
                },
            ],
        };
        assert_eq!(report.supported(), 1);
        assert_eq!(report.deferred(), 1);
        assert_eq!(report.warned(), 1);
        assert_eq!(report.failed(), 1);
        assert_eq!(
            report.supported() + report.deferred() + report.warned() + report.failed(),
            report.processed
        );
        let rendered = report.render();
        assert_eq!(rendered, report.render());
        assert!(rendered.starts_with(
            "analyze check summary: processed=4 supported=1 deferred=1 warned=1 failed=1\n"
        ));

        let deferred_only = AnalyzeCheckReport {
            processed: 1,
            files: vec![report.files[1].clone()],
        };
        assert!(!deferred_only.fails(DiagnosticPolicy::new(false)));
        assert!(!deferred_only.fails(DiagnosticPolicy::new(true)));
        let warned_only = AnalyzeCheckReport {
            processed: 1,
            files: vec![report.files[2].clone()],
        };
        assert!(!warned_only.fails(DiagnosticPolicy::new(false)));
        assert!(warned_only.fails(DiagnosticPolicy::new(true)));
        let failed_only = AnalyzeCheckReport {
            processed: 1,
            files: vec![report.files[3].clone()],
        };
        assert!(failed_only.fails(DiagnosticPolicy::new(false)));
        assert!(failed_only.fails(DiagnosticPolicy::new(true)));
    }

    #[test]
    fn catalog_aware_analyze_check_resolves_known_hierarchy_and_fails_unknown_module() {
        let directory = temporary_directory("analyze-check");
        std::fs::write(
            directory.join("child.sv"),
            "module child(input logic i, output logic o); endmodule\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("parent.sv"),
            "module parent(input logic i, output logic o);\n  child u(.i(i), .o(o));\nendmodule\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("unknown.sv"),
            "module unknown(input logic i, output logic o);\n  missing u(i, o);\nendmodule\n",
        )
        .unwrap();

        let report = check_analyze_dir(&directory).unwrap();
        assert_eq!(report.processed, 3);
        assert_eq!(report.supported(), 2);
        assert_eq!(report.deferred(), 0);
        assert_eq!(report.warned(), 0);
        assert_eq!(report.failed(), 1);
        assert_eq!(
            report.supported() + report.deferred() + report.warned() + report.failed(),
            report.processed
        );
        let child = report
            .files
            .iter()
            .find(|file| file.path.ends_with("child.sv"))
            .unwrap();
        assert_eq!(child.disposition, AnalysisDisposition::Supported);
        let parent = report
            .files
            .iter()
            .find(|file| file.path.ends_with("parent.sv"))
            .unwrap();
        assert_eq!(parent.disposition, AnalysisDisposition::Supported);
        assert!(parent.requirements.is_empty());
        let unknown = report
            .files
            .iter()
            .find(|file| file.path.ends_with("unknown.sv"))
            .unwrap();
        assert_eq!(unknown.disposition, AnalysisDisposition::Failed);
        assert_eq!(unknown.diagnostics.len(), 1);
        assert_eq!(unknown.diagnostics[0].span.line, 2);
        assert_eq!(unknown.diagnostics[0].span.column, 3);
        assert_eq!(
            unknown.diagnostics[0].message,
            "unknown instantiated module `missing` for instance `u`"
        );
        assert_eq!(report.render(), report.render());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn configured_checks_analyze_and_lower_only_the_selected_generate_branch() {
        let directory = temporary_directory("configured-generate-check");
        std::fs::write(
            directory.join("generated.sv"),
            "module generated(input logic a, output logic y);\n  generate\n    if (nodelay) begin\n      logic selected;\n    end else begin\n      missing u(.a(a), .y(y));\n    end\n  endgenerate\nendmodule\n",
        )
        .unwrap();

        let default_analysis = check_analyze_dir(&directory).unwrap();
        let delayful_analysis =
            check_analyze_dir_with_generate_mode(&directory, GenerateMode::Delayful).unwrap();
        let nodelay_analysis =
            check_analyze_dir_with_generate_mode(&directory, GenerateMode::Nodelay).unwrap();
        assert_eq!(default_analysis, delayful_analysis);
        assert_eq!(delayful_analysis.failed(), 1);
        assert_eq!(nodelay_analysis.supported(), 1);
        assert_eq!(nodelay_analysis.failed(), 0);
        assert!(nodelay_analysis.files[0].requirements.is_empty());
        assert!(nodelay_analysis.files[0].diagnostics.is_empty());

        let default_lower = check_lower_dir(&directory).unwrap();
        let delayful_lower =
            check_lower_dir_with_generate_mode(&directory, GenerateMode::Delayful).unwrap();
        let nodelay_lower =
            check_lower_dir_with_generate_mode(&directory, GenerateMode::Nodelay).unwrap();
        assert_eq!(default_lower, delayful_lower);
        assert_eq!(delayful_lower.failed(), 1);
        assert_eq!(nodelay_lower.failed(), 0);
        assert_eq!(nodelay_lower.intentional_ignores(), 0);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sibling_catalog_helper_resolves_dependencies_and_normalizes_bare_parent() {
        assert_eq!(
            sibling_catalog_parent(Path::new("parent.sv")),
            Path::new(".")
        );
        let directory = temporary_directory("sibling-catalog");
        std::fs::write(
            directory.join("child.sv"),
            "module child(input logic i, output logic o); assign o = i; endmodule\n",
        )
        .unwrap();
        let parent_path = directory.join("parent.sv");
        std::fs::write(
            &parent_path,
            "module parent(input logic i, output logic o);\n  child u(.i(i), .o(o));\nendmodule\n",
        )
        .unwrap();

        let analysis = analyze_file_with_sibling_catalog(&parent_path).unwrap();
        assert_eq!(analysis.disposition, AnalysisDisposition::Supported);
        assert!(analysis.requirements.is_empty());
        assert!(matches!(
            analysis.modules[0].instantiations[0].resolution,
            InstantiationResolution::Resolved(_)
        ));
        let rendered = analysis.render();
        assert!(rendered.contains("child u @"));
        assert!(rendered.contains("resolution=resolved"));
        assert!(rendered.contains("connection i direction=input source=Named value=i local=i"));
        assert!(rendered.contains("connection o direction=output source=Named value=o local=o"));
        let default_lower = lower_file_with_sibling_catalog(&parent_path).unwrap();
        let explicit_lower =
            lower_file_with_sibling_catalog_and_generate_mode(&parent_path, GenerateMode::Nodelay)
                .unwrap();
        assert_eq!(default_lower, explicit_lower);
        assert_eq!(default_lower.cell.items.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "sv-to-sexpr-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        directory
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
