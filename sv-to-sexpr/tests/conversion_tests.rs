use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use sexpr_fmt::format_source_default;
use sv_to_sexpr::convert::{ConvertDisposition, ConvertOptions, ConvertReport, convert};
use sv_to_sexpr::elaborate::GenerateMode;

static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

const LEAF: &str = "module leaf(input logic a, output logic y);\n  assign y = a;\nendmodule\n";
const OTHER: &str = "module other(input logic a, output logic y);\n  assign y = !a;\nendmodule\n";

struct TempTree {
    root: PathBuf,
}

impl TempTree {
    fn new(name: &str) -> Self {
        let sequence = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "sv-to-sexpr-{name}-{}-{sequence}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

#[test]
fn conversion_is_sorted_mirrored_deterministic_and_canonical() {
    let tree = TempTree::new("convert-mirror");
    let input = tree.path("input");
    let output = tree.path("output");
    tree.write("input/zeta.sv", OTHER);
    tree.write("input/nested/alpha.sv", LEAF);
    tree.write("input/nested/not-source.txt", "ignored");
    #[cfg(unix)]
    std::os::unix::fs::symlink(input.join("zeta.sv"), input.join("alias.sv")).unwrap();

    let mut options = ConvertOptions::new(&input, &output);
    options.dry_run = true;
    let first = convert(&options);
    let second = convert(&options);

    assert_eq!(first, second);
    assert_eq!(first.processed, 2);
    assert_eq!(first.selected, 2);
    assert_eq!(first.skipped, 0);
    assert_eq!(first.written, 0);
    assert_eq!(first.would_write, 2);
    assert_eq!(first.failed, 0);
    assert_eq!(
        first
            .files
            .iter()
            .map(|file| file.relative_source.as_str())
            .collect::<Vec<_>>(),
        ["nested/alpha.sv", "zeta.sv"]
    );
    assert!(
        first
            .files
            .iter()
            .all(|file| file.disposition == ConvertDisposition::WouldWrite)
    );
    assert!(!output.exists());

    options.dry_run = false;
    let written = convert(&options);
    assert_success_counts(&written, 2, 2, 0, 2, 0);
    for relative in ["nested/alpha.cell", "zeta.cell"] {
        let contents = fs::read_to_string(output.join(relative)).unwrap();
        assert_eq!(format_source_default(&contents).unwrap(), contents);
    }
}

#[test]
fn filtering_uses_the_complete_catalog_and_zero_match_still_parses_every_source() {
    let tree = TempTree::new("convert-filter");
    let input = tree.path("input");
    let output = tree.path("output");
    tree.write(
        "input/child.sv",
        "module child(input logic a, output logic y);\n  logic inner;\n  assign inner = !a;\n  assign y = inner;\nendmodule\n",
    );
    tree.write(
        "input/nested/parent.sv",
        "module parent(input logic a, output logic y);\n  child instance(.a(a), .y(y));\nendmodule\n",
    );

    let mut selected = ConvertOptions::new(&input, &output);
    selected.filter = Some("nested/parent.sv".into());
    let report = convert(&selected);
    assert_success_counts(&report, 2, 1, 1, 1, 0);
    assert!(!output.join("child.cell").exists());
    let parent = fs::read_to_string(output.join("nested/parent.cell")).unwrap();
    assert!(
        parent.contains("instance__inner"),
        "child module was not flattened: {parent}"
    );

    let no_match_output = tree.path("no-match-output");
    let mut no_match = ConvertOptions::new(&input, &no_match_output);
    no_match.filter = Some("NESTED/".into());
    let report = convert(&no_match);
    assert_success_counts(&report, 2, 0, 2, 0, 0);
    assert_eq!(
        report.render_summary(),
        "convert summary: processed=2 selected=0 skipped=2 warned=0 intentional-ignored=0 written=0 would-write=0 failed=0\n"
    );
    assert!(!no_match_output.exists());

    tree.write("input/z_bad.sv", "module bad(input logic a);\n");
    let report = convert(&no_match);
    assert!(!report.succeeded());
    assert_eq!(report.processed, 3);
    assert_eq!(report.selected, 0);
    assert_eq!(report.skipped, 3);
    assert_eq!(report.failed, 1);
    assert_eq!(report.files[2].disposition, ConvertDisposition::Failed);
    assert_eq!(report.files[2].relative_source, "z_bad.sv");
    assert!(!no_match_output.exists());

    tree.write(
        "input/z_bad.sv",
        "module child(input logic a, output logic y);\n  assign y = a;\nendmodule\n",
    );
    let report = convert(&no_match);
    assert_eq!(report.failed, 1);
    assert!(
        report
            .diagnostics()
            .any(|diagnostic| diagnostic.message.contains("duplicate module `child`"))
    );
    assert!(!no_match_output.exists());
}

#[test]
fn existing_outputs_skip_or_overwrite_only_after_successful_conversion() {
    let tree = TempTree::new("convert-overwrite");
    let input = tree.path("input");
    let output = tree.path("output");
    tree.write("input/leaf.sv", LEAF);

    let options = ConvertOptions::new(&input, &output);
    assert_success_counts(&convert(&options), 1, 1, 0, 1, 0);
    let target = output.join("leaf.cell");
    fs::write(&target, "sentinel\n").unwrap();

    let skipped = convert(&options);
    assert_success_counts(&skipped, 1, 1, 1, 0, 0);
    assert_eq!(
        skipped.files[0].disposition,
        ConvertDisposition::SkippedExisting
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "sentinel\n");

    tree.write(
        "input/leaf.sv",
        "module leaf(input logic a, output logic y);\n  missing child(.a(a), .y(y));\nendmodule\n",
    );
    let failed = convert(&options);
    assert_eq!(failed.failed, 1);
    assert_eq!(failed.skipped, 0);
    assert!(failed.diagnostics().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown instantiated module `missing`")
    }));
    assert_eq!(fs::read_to_string(&target).unwrap(), "sentinel\n");

    tree.write("input/leaf.sv", OTHER);
    let mut overwrite = options.clone();
    overwrite.overwrite = true;
    overwrite.dry_run = true;
    let dry_run = convert(&overwrite);
    assert_success_counts(&dry_run, 1, 1, 0, 0, 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "sentinel\n");

    overwrite.dry_run = false;
    let replaced = convert(&overwrite);
    assert_success_counts(&replaced, 1, 1, 0, 1, 0);
    let contents = fs::read_to_string(&target).unwrap();
    assert_ne!(contents, "sentinel\n");
    assert_eq!(format_source_default(&contents).unwrap(), contents);
}

#[test]
fn parse_lower_and_output_conflicts_abort_every_preflighted_write() {
    let tree = TempTree::new("convert-transaction");
    let input = tree.path("input");
    let output = tree.path("output");
    tree.write("input/a_valid.sv", LEAF);
    tree.write("input/z_bad.sv", "module bad(input logic a);\n");

    let options = ConvertOptions::new(&input, &output);
    let parse_failure = convert(&options);
    assert_eq!(parse_failure.failed, 1);
    assert!(!output.exists());
    assert_eq!(
        parse_failure.files[0].disposition,
        ConvertDisposition::Pending
    );
    assert_eq!(parse_failure.files[1].relative_source, "z_bad.sv");

    tree.write(
        "input/z_bad.sv",
        "module bad(input logic a, output logic y);\n  missing child(.a(a), .y(y));\nendmodule\n",
    );
    let lower_failure = convert(&options);
    assert_eq!(lower_failure.failed, 1);
    assert!(!output.exists());
    assert_eq!(
        lower_failure.files[0].disposition,
        ConvertDisposition::Prepared
    );
    assert!(lower_failure.diagnostics().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown instantiated module `missing`")
    }));

    tree.write("input/z_bad.sv", OTHER);
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("nested"), "parent sentinel\n").unwrap();
    tree.write(
        "input/nested/b.sv",
        "module nested_leaf(input logic a, output logic y);\n  assign y = a;\nendmodule\n",
    );
    let parent_conflict = convert(&options);
    assert_eq!(parent_conflict.failed, 1);
    assert!(!output.join("a_valid.cell").exists());
    assert!(!output.join("z_bad.cell").exists());
    assert_eq!(
        fs::read_to_string(output.join("nested")).unwrap(),
        "parent sentinel\n"
    );
    assert!(
        parent_conflict
            .diagnostics()
            .any(|diagnostic| diagnostic.message.contains("exists but is not a directory"))
    );
}

#[test]
fn non_directory_output_root_and_nonregular_target_fail_without_mutation() {
    let tree = TempTree::new("convert-path-conflict");
    let input = tree.path("input");
    tree.write("input/leaf.sv", LEAF);

    let output_file = tree.write("output-file", "root sentinel\n");
    let root_failure = convert(&ConvertOptions::new(&input, &output_file));
    assert_eq!(root_failure.failed, 1);
    assert_eq!(fs::read_to_string(&output_file).unwrap(), "root sentinel\n");
    assert!(
        root_failure.global_diagnostics[0]
            .message
            .contains("output root exists but is not a directory")
    );

    let output = tree.path("output");
    fs::create_dir_all(output.join("leaf.cell")).unwrap();
    let target_failure = convert(&ConvertOptions::new(&input, &output));
    assert_eq!(target_failure.failed, 1);
    assert_eq!(
        target_failure.files[0].disposition,
        ConvertDisposition::Failed
    );
    assert!(
        target_failure.files[0].diagnostics[0]
            .message
            .contains("exists but is not a regular file")
    );
    assert!(output.join("leaf.cell").is_dir());
}

#[test]
fn nodelay_changes_selected_generate_output_and_strict_keeps_ignores_non_failing() {
    let tree = TempTree::new("convert-mode-policy");
    let input = tree.path("input");
    let dffr = repository_root().join("sv-cells/dmg_cpu_b/cells/dffr_cc.sv");
    tree.write("input/dffr_cc.sv", &fs::read_to_string(dffr).unwrap());

    let delayful_output = tree.path("delayful");
    let mut delayful = ConvertOptions::new(&input, &delayful_output);
    delayful.strict = true;
    let delayful_report = convert(&delayful);
    assert!(delayful_report.succeeded());
    assert_eq!(delayful_report.warned, 0);
    assert!(delayful_report.intentional_ignored > 0);

    let nodelay_output = tree.path("nodelay");
    let mut nodelay = ConvertOptions::new(&input, &nodelay_output);
    nodelay.strict = true;
    nodelay.generate_mode = GenerateMode::Nodelay;
    let nodelay_report = convert(&nodelay);
    assert!(nodelay_report.succeeded());
    assert_eq!(nodelay_report.warned, 0);
    assert!(nodelay_report.intentional_ignored > 0);

    assert_ne!(
        fs::read_to_string(delayful_output.join("dffr_cc.cell")).unwrap(),
        fs::read_to_string(nodelay_output.join("dffr_cc.cell")).unwrap()
    );
}

#[test]
fn cli_convert_emits_one_exact_summary_and_sorted_diagnostics() {
    let tree = TempTree::new("convert-cli");
    let input = tree.path("input");
    let output = tree.path("output");
    tree.write("input/z.sv", OTHER);
    tree.write("input/a.sv", LEAF);

    let success = run_cli(&[
        "convert",
        input.to_str().unwrap(),
        "--strict",
        output.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(success.status.success());
    assert_eq!(
        String::from_utf8(success.stdout).unwrap(),
        "convert summary: processed=2 selected=2 skipped=0 warned=0 intentional-ignored=0 written=0 would-write=2 failed=0\n"
    );
    assert!(success.stderr.is_empty());
    assert!(!output.exists());

    tree.write("input/a.sv", "module a_broken(input logic a);\n");
    tree.write("input/z.sv", "module z_broken(input logic a);\n");
    let failure = run_cli(&[
        "convert",
        "--dry-run",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    assert!(!failure.status.success());
    assert_eq!(
        String::from_utf8(failure.stdout).unwrap(),
        "convert summary: processed=2 selected=2 skipped=0 warned=0 intentional-ignored=0 written=0 would-write=0 failed=2\n"
    );
    let stderr = String::from_utf8(failure.stderr).unwrap();
    let lines = stderr.lines().collect::<Vec<_>>();
    assert!(lines[0].starts_with("a.sv:"));
    assert!(lines[1].starts_with("z.sv:"));
    assert!(lines[2].contains("conversion failed with 2 failed source(s)"));
    assert!(!output.exists());
}

#[test]
fn cli_convert_file_requires_overwrite_but_dry_run_remains_a_preview() {
    let tree = TempTree::new("convert-file-overwrite");
    let input = tree.write("leaf.sv", LEAF);
    let output = tree.write("leaf.cell", "sentinel\n");

    let refused = run_cli(&[
        "convert-file",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("output file already exists; pass --overwrite to replace it")
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "sentinel\n");

    let preview = run_cli(&[
        "convert-file",
        "--dry-run",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    assert!(preview.status.success());
    assert!(
        String::from_utf8(preview.stdout)
            .unwrap()
            .starts_with("(cell\n  leaf\n")
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "sentinel\n");

    let replaced = run_cli(&[
        "convert-file",
        input.to_str().unwrap(),
        "--overwrite",
        output.to_str().unwrap(),
    ]);
    assert!(replaced.status.success());
    let contents = fs::read_to_string(&output).unwrap();
    assert_ne!(contents, "sentinel\n");
    assert_eq!(format_source_default(&contents).unwrap(), contents);
}

fn assert_success_counts(
    report: &ConvertReport,
    processed: usize,
    selected: usize,
    skipped: usize,
    written: usize,
    would_write: usize,
) {
    assert!(
        report.succeeded(),
        "{:#?}",
        report.diagnostics().collect::<Vec<_>>()
    );
    assert_eq!(report.processed, processed);
    assert_eq!(report.selected, selected);
    assert_eq!(report.skipped, skipped);
    assert_eq!(report.written, written);
    assert_eq!(report.would_write, would_write);
    assert_eq!(report.failed, 0);
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args(args)
        .output()
        .unwrap()
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
