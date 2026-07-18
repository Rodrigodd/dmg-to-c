use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::analyze::ModuleCatalog;
use crate::ast::Design;
use crate::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticPolicy, Span};
use crate::elaborate::GenerateMode;
use crate::lower::lower_design_with_catalog_and_generate_mode;
use crate::parser::parse_file;
use crate::serialize::render_cell;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertOptions {
    pub input_root: PathBuf,
    pub output_root: PathBuf,
    pub dry_run: bool,
    pub strict: bool,
    pub overwrite: bool,
    pub filter: Option<String>,
    pub generate_mode: GenerateMode,
}

impl ConvertOptions {
    pub fn new(input_root: impl Into<PathBuf>, output_root: impl Into<PathBuf>) -> Self {
        Self {
            input_root: input_root.into(),
            output_root: output_root.into(),
            dry_run: false,
            strict: false,
            overwrite: false,
            filter: None,
            generate_mode: GenerateMode::default(),
        }
    }

    fn policy(&self) -> DiagnosticPolicy {
        DiagnosticPolicy::new(self.strict)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertDisposition {
    FilterExcluded,
    Pending,
    Prepared,
    SkippedExisting,
    WouldWrite,
    Written,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertFileResult {
    pub relative_source: String,
    pub relative_output: PathBuf,
    pub output_path: PathBuf,
    pub selected: bool,
    pub disposition: ConvertDisposition,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConvertReport {
    pub processed: usize,
    pub selected: usize,
    pub skipped: usize,
    pub warned: usize,
    pub intentional_ignored: usize,
    pub written: usize,
    pub would_write: usize,
    pub failed: usize,
    pub files: Vec<ConvertFileResult>,
    pub global_diagnostics: Vec<Diagnostic>,
}

impl ConvertReport {
    pub fn succeeded(&self) -> bool {
        self.failed == 0
    }

    pub fn render_summary(&self) -> String {
        format!(
            "convert summary: processed={} selected={} skipped={} warned={} intentional-ignored={} written={} would-write={} failed={}\n",
            self.processed,
            self.selected,
            self.skipped,
            self.warned,
            self.intentional_ignored,
            self.written,
            self.would_write,
            self.failed
        )
    }

    pub fn diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.files
            .iter()
            .flat_map(|file| file.diagnostics.iter())
            .chain(self.global_diagnostics.iter())
    }

    fn fail_file(&mut self, index: usize, diagnostic: Diagnostic) {
        let file = &mut self.files[index];
        if file.disposition != ConvertDisposition::Failed {
            self.failed += 1;
            file.disposition = ConvertDisposition::Failed;
        }
        file.diagnostics.push(diagnostic);
        sort_diagnostics(&mut file.diagnostics);
    }

    fn fail_global(&mut self, diagnostic: Diagnostic) {
        self.failed += 1;
        self.global_diagnostics.push(diagnostic);
        sort_diagnostics(&mut self.global_diagnostics);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredSource {
    physical_path: PathBuf,
    relative_path: String,
}

#[derive(Debug)]
struct PreparedOutput {
    file_index: usize,
    output_path: PathBuf,
    rendered: String,
}

/// Preflights and, when requested, transactionally converts a source tree.
///
/// Every failure discoverable before output mutation is recorded in the
/// returned report and prevents all writes.
pub fn convert(options: &ConvertOptions) -> ConvertReport {
    let discovered = match discover_sources(&options.input_root) {
        Ok(discovered) => discovered,
        Err(diagnostic) => {
            let mut report = ConvertReport::default();
            report.fail_global(diagnostic);
            return report;
        }
    };

    let mut report = initialize_report(options, &discovered);
    if report.selected > 0 {
        if let Err(diagnostic) = validate_output_root(&options.output_root) {
            report.fail_global(diagnostic);
            return report;
        }
        mark_output_collisions(&mut report);
        if !report.succeeded() {
            return report;
        }
    }

    let mut parsed = discovered.iter().map(|_| None).collect::<Vec<_>>();
    for (index, source) in discovered.iter().enumerate() {
        match fs::read_to_string(&source.physical_path) {
            Ok(contents) => match parse_file(Path::new(&source.relative_path), &contents) {
                Ok(design) => parsed[index] = Some(design),
                Err(diagnostic) => report.fail_file(index, diagnostic),
            },
            Err(error) => report.fail_file(
                index,
                Diagnostic::new(
                    Span::new(&source.relative_path, 1, 1),
                    format!("failed to read file: {error}"),
                ),
            ),
        }
    }
    if !report.succeeded() {
        return report;
    }

    let designs = parsed
        .iter()
        .map(|design| design.clone().expect("all discovered sources parsed"))
        .collect::<Vec<Design>>();
    let catalog = match ModuleCatalog::from_designs(&designs) {
        Ok(catalog) => catalog,
        Err(diagnostic) => {
            if let Some(index) = report
                .files
                .iter()
                .position(|file| diagnostic.span.path == Path::new(&file.relative_source))
            {
                report.fail_file(index, diagnostic);
            } else {
                report.fail_global(diagnostic);
            }
            return report;
        }
    };

    let mut prepared = Vec::new();
    for (index, design) in designs.iter().enumerate() {
        if !report.files[index].selected {
            continue;
        }
        let lowered = match lower_design_with_catalog_and_generate_mode(
            design,
            &catalog,
            options.generate_mode,
        ) {
            Ok(lowered) => lowered,
            Err(diagnostic) => {
                report.fail_file(index, diagnostic);
                continue;
            }
        };

        let mut diagnostics = lowered.diagnostics;
        sort_diagnostics(&mut diagnostics);
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
        {
            report.warned += 1;
        }
        report.intentional_ignored += diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
            .count();
        let policy_failed = diagnostics
            .iter()
            .any(|diagnostic| options.policy().is_failure(diagnostic));
        report.files[index].diagnostics = diagnostics;
        if policy_failed {
            report.files[index].disposition = ConvertDisposition::Failed;
            report.failed += 1;
            continue;
        }
        if let Err(error) = lowered.cell.validate() {
            report.fail_file(
                index,
                Diagnostic::new(
                    Span::new(&report.files[index].relative_source, 1, 1),
                    format!("lowered cell validation failed: {error}"),
                ),
            );
            continue;
        }
        let rendered = render_cell(&lowered.cell);

        let output_path = report.files[index].output_path.clone();
        if let Err(diagnostic) = validate_output_parent_chain(
            &options.output_root,
            &report.files[index].relative_output,
            &report.files[index].relative_source,
        ) {
            report.fail_file(index, diagnostic);
            continue;
        }
        match inspect_output_target(&output_path, &report.files[index].relative_source) {
            Ok(OutputTarget::ExistingRegular) if !options.overwrite => {
                report.files[index].disposition = ConvertDisposition::SkippedExisting;
                report.skipped += 1;
                continue;
            }
            Ok(OutputTarget::ExistingRegular | OutputTarget::Missing) => {}
            Err(diagnostic) => {
                report.fail_file(index, diagnostic);
                continue;
            }
        }

        report.files[index].disposition = ConvertDisposition::Prepared;
        prepared.push(PreparedOutput {
            file_index: index,
            output_path,
            rendered,
        });
    }

    execute_prepared(options, &mut report, prepared);
    report
}

fn initialize_report(options: &ConvertOptions, discovered: &[DiscoveredSource]) -> ConvertReport {
    let mut report = ConvertReport {
        processed: discovered.len(),
        ..ConvertReport::default()
    };
    for source in discovered {
        let selected = options
            .filter
            .as_ref()
            .is_none_or(|filter| source.relative_path.contains(filter));
        let relative_output = output_relative_path(&source.relative_path)
            .expect("discovered normalized .sv path must map safely");
        if selected {
            report.selected += 1;
        } else {
            report.skipped += 1;
        }
        report.files.push(ConvertFileResult {
            relative_source: source.relative_path.clone(),
            output_path: options.output_root.join(&relative_output),
            relative_output,
            selected,
            disposition: if selected {
                ConvertDisposition::Pending
            } else {
                ConvertDisposition::FilterExcluded
            },
            diagnostics: Vec::new(),
        });
    }
    report
}

fn execute_prepared(
    options: &ConvertOptions,
    report: &mut ConvertReport,
    prepared: Vec<PreparedOutput>,
) {
    if !report.succeeded() {
        return;
    }
    if options.dry_run {
        report.would_write = prepared.len();
        for output in prepared {
            report.files[output.file_index].disposition = ConvertDisposition::WouldWrite;
        }
        return;
    }

    for output in prepared {
        let parent = output
            .output_path
            .parent()
            .expect("prepared output must have an output-root parent");
        if let Err(error) = fs::create_dir_all(parent) {
            report.fail_file(
                output.file_index,
                Diagnostic::new(
                    Span::new(&report.files[output.file_index].relative_source, 1, 1),
                    format!(
                        "failed to create output directory `{}`: {error}",
                        parent.display()
                    ),
                ),
            );
            return;
        }
        if let Err(error) = fs::write(&output.output_path, output.rendered) {
            report.fail_file(
                output.file_index,
                Diagnostic::new(
                    Span::new(&report.files[output.file_index].relative_source, 1, 1),
                    format!(
                        "failed to write output file `{}`: {error}",
                        output.output_path.display()
                    ),
                ),
            );
            return;
        }
        report.files[output.file_index].disposition = ConvertDisposition::Written;
        report.written += 1;
    }
}

fn discover_sources(input_root: &Path) -> Result<Vec<DiscoveredSource>, Diagnostic> {
    if input_root.as_os_str().is_empty() {
        return Err(Diagnostic::new(
            Span::new(input_root, 1, 1),
            "input root must not be empty",
        ));
    }
    let metadata = fs::symlink_metadata(input_root).map_err(|error| {
        Diagnostic::new(
            Span::new(input_root, 1, 1),
            format!("failed to stat input root: {error}"),
        )
    })?;
    if !metadata.is_dir() {
        return Err(Diagnostic::new(
            Span::new(input_root, 1, 1),
            "input root must be a directory",
        ));
    }

    let mut physical_paths = Vec::new();
    discover_regular_sv_files(input_root, &mut physical_paths)?;
    let mut discovered = Vec::with_capacity(physical_paths.len());
    for physical_path in physical_paths {
        let relative = physical_path.strip_prefix(input_root).map_err(|_| {
            Diagnostic::new(
                Span::new(&physical_path, 1, 1),
                "discovered source escaped the input root",
            )
        })?;
        let relative_path = normalize_relative_path(relative)
            .map_err(|message| Diagnostic::new(Span::new(&physical_path, 1, 1), message))?;
        discovered.push(DiscoveredSource {
            physical_path,
            relative_path,
        });
    }
    discovered.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(discovered)
}

fn discover_regular_sv_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), Diagnostic> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| {
            Diagnostic::new(
                Span::new(path, 1, 1),
                format!("failed to read input directory: {error}"),
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            Diagnostic::new(
                Span::new(path, 1, 1),
                format!("failed to read input directory entry: {error}"),
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path).map_err(|error| {
            Diagnostic::new(
                Span::new(&entry_path, 1, 1),
                format!("failed to stat input path: {error}"),
            )
        })?;
        if metadata.is_dir() {
            discover_regular_sv_files(&entry_path, output)?;
        } else if metadata.is_file()
            && entry_path
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("sv")
        {
            output.push(entry_path);
        }
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<String, String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => segments.push(
                segment
                    .to_str()
                    .ok_or_else(|| "source path is not valid UTF-8".to_string())?,
            ),
            _ => return Err("source path is not a safe relative path".to_string()),
        }
    }
    if segments.is_empty() {
        return Err("source path must not be empty".to_string());
    }
    Ok(segments.join("/"))
}

fn output_relative_path(source_relative: &str) -> Result<PathBuf, String> {
    if source_relative.starts_with('/') || source_relative.split('/').any(|part| part.is_empty()) {
        return Err("source path is not a normalized relative path".to_string());
    }
    let parts = source_relative.split('/').collect::<Vec<_>>();
    if parts.iter().any(|part| matches!(*part, "." | "..")) {
        return Err("source path contains an unsafe component".to_string());
    }
    if !parts.last().is_some_and(|name| name.ends_with(".sv")) {
        return Err("source path must end in .sv".to_string());
    }
    let mut output = PathBuf::new();
    for part in parts {
        output.push(part);
    }
    output.set_extension("cell");
    Ok(output)
}

fn mark_output_collisions(report: &mut ConvertReport) {
    let collisions = find_output_collisions(
        report
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.selected)
            .map(|(index, file)| (index, file.relative_output.clone())),
    );
    for (first, second, target) in collisions {
        let message = format!(
            "output path collision with `{}` at `{}`",
            report.files[first].relative_source,
            target.display()
        );
        report.fail_file(
            second,
            Diagnostic::new(
                Span::new(&report.files[second].relative_source, 1, 1),
                message,
            ),
        );
    }
}

fn find_output_collisions(
    targets: impl IntoIterator<Item = (usize, PathBuf)>,
) -> Vec<(usize, usize, PathBuf)> {
    let mut seen = BTreeMap::new();
    let mut collisions = Vec::new();
    for (index, target) in targets {
        if let Some(first) = seen.get(&target).copied() {
            collisions.push((first, index, target));
        } else {
            seen.insert(target, index);
        }
    }
    collisions
}

fn validate_output_root(output_root: &Path) -> Result<(), Diagnostic> {
    if output_root.as_os_str().is_empty() {
        return Err(Diagnostic::new(
            Span::new(output_root, 1, 1),
            "output root must not be empty",
        ));
    }
    match fs::symlink_metadata(output_root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(Diagnostic::new(
            Span::new(output_root, 1, 1),
            "output root exists but is not a directory",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            validate_existing_ancestors(output_root)
                .map_err(|message| Diagnostic::new(Span::new(output_root, 1, 1), message))
        }
        Err(error) => Err(Diagnostic::new(
            Span::new(output_root, 1, 1),
            format!("failed to stat output root: {error}"),
        )),
    }
}

fn validate_existing_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => {
                return Err(format!(
                    "output ancestor `{}` is not a directory",
                    ancestor.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "failed to stat output ancestor `{}`: {error}",
                    ancestor.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_output_parent_chain(
    output_root: &Path,
    relative_output: &Path,
    relative_source: &str,
) -> Result<(), Diagnostic> {
    let Some(relative_parent) = relative_output.parent() else {
        return Ok(());
    };
    let mut current = output_root.to_path_buf();
    for component in relative_parent.components() {
        let Component::Normal(segment) = component else {
            return Err(Diagnostic::new(
                Span::new(relative_source, 1, 1),
                "output parent contains an unsafe path component",
            ));
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(Diagnostic::new(
                    Span::new(relative_source, 1, 1),
                    format!(
                        "output parent `{}` exists but is not a directory",
                        current.display()
                    ),
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(
                    Span::new(relative_source, 1, 1),
                    format!(
                        "failed to stat output parent `{}`: {error}",
                        current.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

enum OutputTarget {
    Missing,
    ExistingRegular,
}

fn inspect_output_target(
    output_path: &Path,
    relative_source: &str,
) -> Result<OutputTarget, Diagnostic> {
    match fs::symlink_metadata(output_path) {
        Ok(metadata) if metadata.is_file() => Ok(OutputTarget::ExistingRegular),
        Ok(_) => Err(Diagnostic::new(
            Span::new(relative_source, 1, 1),
            format!(
                "output target `{}` exists but is not a regular file",
                output_path.display()
            ),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(OutputTarget::Missing),
        Err(error) => Err(Diagnostic::new(
            Span::new(relative_source, 1, 1),
            format!(
                "failed to stat output target `{}`: {error}",
                output_path.display()
            ),
        )),
    }
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        left.span
            .path
            .cmp(&right.span.path)
            .then_with(|| left.span.line.cmp(&right.span.line))
            .then_with(|| left.span.column.cmp(&right.span.column))
            .then_with(|| diagnostic_kind_order(left.kind).cmp(&diagnostic_kind_order(right.kind)))
            .then_with(|| left.message.cmp(&right.message))
    });
}

fn diagnostic_kind_order(kind: DiagnosticKind) -> usize {
    match kind {
        DiagnosticKind::Warning => 0,
        DiagnosticKind::IntentionalIgnore => 1,
        DiagnosticKind::Error => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mapping_replaces_only_final_extension_and_rejects_escapes() {
        assert_eq!(
            output_relative_path("nested/name.with.sv").unwrap(),
            PathBuf::from("nested/name.with.cell")
        );
        for unsafe_path in ["/absolute.sv", "../escape.sv", "a/../escape.sv", "a//b.sv"] {
            assert!(output_relative_path(unsafe_path).is_err(), "{unsafe_path}");
        }
        assert!(output_relative_path("nested/not-sv.txt").is_err());
    }

    #[test]
    fn collision_detection_is_stable_and_identifies_later_sources() {
        assert_eq!(
            find_output_collisions([
                (0, PathBuf::from("a.cell")),
                (1, PathBuf::from("b.cell")),
                (2, PathBuf::from("a.cell")),
            ]),
            vec![(0, 2, PathBuf::from("a.cell"))]
        );
    }

    #[test]
    fn strict_warning_failure_aborts_an_injected_earlier_write_plan() {
        let root = std::env::temp_dir().join(format!(
            "sv-to-sexpr-convert-warning-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let output = root.join("valid.cell");
        let options = ConvertOptions {
            strict: true,
            output_root: root.clone(),
            ..ConvertOptions::new("input", &root)
        };
        let mut report = ConvertReport {
            processed: 2,
            selected: 2,
            warned: 1,
            failed: 1,
            files: vec![
                ConvertFileResult {
                    relative_source: "a_valid.sv".into(),
                    relative_output: "a_valid.cell".into(),
                    output_path: output.clone(),
                    selected: true,
                    disposition: ConvertDisposition::Prepared,
                    diagnostics: Vec::new(),
                },
                ConvertFileResult {
                    relative_source: "z_warning.sv".into(),
                    relative_output: "z_warning.cell".into(),
                    output_path: root.join("z_warning.cell"),
                    selected: true,
                    disposition: ConvertDisposition::Failed,
                    diagnostics: vec![Diagnostic::warning(
                        Span::new("z_warning.sv", 1, 1),
                        "injected warning",
                    )],
                },
            ],
            ..ConvertReport::default()
        };
        execute_prepared(
            &options,
            &mut report,
            vec![PreparedOutput {
                file_index: 0,
                output_path: output.clone(),
                rendered: "(cell valid)\n".into(),
            }],
        );
        assert!(!root.exists());
        assert_eq!(report.written, 0);
    }
}
