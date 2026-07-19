use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sexpr_fmt::format_source_default;
use sv_to_sexpr::convert::{ConvertDisposition, ConvertOptions, convert};

static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

const REFERENCE_RELATIVE: &str = "sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell";

struct TempTree {
    root: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let sequence = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "sv-to-sexpr-release-{}-{sequence}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        Self { root }
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
fn strict_release_conversion_is_complete_canonical_and_byte_deterministic() {
    let tree = TempTree::new();
    let repository = repository_root();
    let input = repository.join("sv-cells");
    let output = tree.root.join("sexpr-cells");
    let expected_paths = expected_output_paths(&input);
    assert_eq!(expected_paths.len(), 206);

    let mut options = ConvertOptions::new(&input, &output);
    options.strict = true;
    options.overwrite = true;

    let first_report = convert(&options);
    assert_release_write_report(&first_report);
    assert_eq!(
        first_report
            .files
            .iter()
            .map(|file| normalize_relative(&file.relative_output))
            .collect::<BTreeSet<_>>(),
        expected_paths
    );

    let first_manifest = canonical_output_manifest(&output);
    assert_eq!(first_manifest.len(), 206);
    assert_eq!(
        first_manifest.keys().cloned().collect::<BTreeSet<_>>(),
        expected_paths
    );
    assert_eq!(
        first_manifest.get(REFERENCE_RELATIVE).unwrap(),
        &fs::read(repository.join("sexpr-cells").join(REFERENCE_RELATIVE)).unwrap()
    );

    let second_report = convert(&options);
    assert_release_write_report(&second_report);
    let second_manifest = canonical_output_manifest(&output);
    assert_eq!(second_manifest, first_manifest);

    options.overwrite = false;
    options.dry_run = true;
    let skipped = convert(&options);
    assert!(skipped.succeeded());
    assert_eq!(skipped.processed, 206);
    assert_eq!(skipped.selected, 206);
    assert_eq!(skipped.skipped, 206);
    assert_eq!(skipped.warned, 0);
    assert_eq!(skipped.intentional_ignored, 49);
    assert_eq!(skipped.written, 0);
    assert_eq!(skipped.would_write, 0);
    assert_eq!(skipped.failed, 0);
    assert!(
        skipped.files.iter().all(|file| {
            file.disposition == ConvertDisposition::SkippedExisting && file.selected
        })
    );
    assert_eq!(canonical_output_manifest(&output), first_manifest);
}

fn assert_release_write_report(report: &sv_to_sexpr::convert::ConvertReport) {
    assert!(
        report.succeeded(),
        "release conversion diagnostics: {:#?}",
        report.diagnostics().collect::<Vec<_>>()
    );
    assert_eq!(report.processed, 206);
    assert_eq!(report.selected, 206);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.warned, 0);
    assert_eq!(report.intentional_ignored, 49);
    assert_eq!(report.written, 206);
    assert_eq!(report.would_write, 0);
    assert_eq!(report.failed, 0);
    assert!(
        report
            .files
            .iter()
            .all(|file| file.disposition == ConvertDisposition::Written && file.selected)
    );
}

fn expected_output_paths(input_root: &Path) -> BTreeSet<String> {
    let mut sources = Vec::new();
    collect_regular_files(input_root, "sv", &mut sources);
    sources
        .into_iter()
        .map(|path| {
            let mut relative = path.strip_prefix(input_root).unwrap().to_path_buf();
            relative.set_extension("cell");
            normalize_relative(&relative)
        })
        .collect()
}

fn canonical_output_manifest(output_root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut outputs = Vec::new();
    collect_regular_files(output_root, "cell", &mut outputs);
    outputs.sort();

    let mut manifest = BTreeMap::new();
    for path in outputs {
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes.is_empty(), "empty generated cell {}", path.display());
        let source = std::str::from_utf8(&bytes).unwrap();
        let first = format_source_default(source)
            .unwrap_or_else(|error| panic!("formatter rejected {}: {error}", path.display()));
        let second = format_source_default(&first).unwrap();
        assert_eq!(first, source, "non-canonical output {}", path.display());
        assert_eq!(second, first, "non-idempotent output {}", path.display());
        let relative = normalize_relative(path.strip_prefix(output_root).unwrap());
        assert!(manifest.insert(relative, bytes).is_none());
    }
    manifest
}

fn collect_regular_files(directory: &Path, extension: &str, output: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            collect_regular_files(&path, extension, output);
        } else if metadata.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some(extension)
        {
            output.push(path);
        }
    }
}

fn normalize_relative(path: &Path) -> String {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().unwrap(),
            _ => panic!("expected normalized relative path: {}", path.display()),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
