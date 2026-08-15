#[allow(dead_code)]
mod analysis_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sexpr_fmt::format_source_default;

#[test]
fn every_cell_fixture_is_parseable_canonical_and_idempotent() {
    let fixture_root = manifest_root().join("tests/fixtures");
    let mut paths = Vec::new();
    collect_cell_files(&fixture_root, &mut paths);
    paths.sort();
    assert!(!paths.is_empty());

    for path in paths {
        let source = fs::read_to_string(&path).unwrap();
        let first = format_source_default(&source)
            .unwrap_or_else(|error| panic!("formatter rejected {}: {error}", path.display()));
        let second = format_source_default(&first).unwrap();
        assert_eq!(first, source, "non-canonical fixture {}", path.display());
        assert_eq!(second, first, "non-idempotent fixture {}", path.display());
    }
}

#[test]
fn sibling_formatter_cli_check_agrees_with_api_on_representative_files() {
    let root = repository_root();
    for relative in [
        "sv-to-sexpr/tests/fixtures/drivers/signal_high_z.cell",
        "sv-to-sexpr/tests/fixtures/lower/alu_cgen.cell",
        "sv-to-sexpr/tests/fixtures/stateful/block_latch.cell",
    ] {
        let path = root.join(relative);
        let source = fs::read_to_string(&path).unwrap();
        assert_eq!(
            format_source_default(&source).unwrap(),
            source,
            "{relative}"
        );

        let result = Command::new("cargo")
            .current_dir(&root)
            .args([
                "run",
                "--quiet",
                "--manifest-path",
                "sexpr-fmt/Cargo.toml",
                "--",
                "--check",
                path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "formatter CLI found non-canonical {relative}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
}

fn collect_cell_files(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_cell_files(&path, paths);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "cell")
        {
            paths.push(path);
        }
    }
}

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repository_root() -> PathBuf {
    manifest_root().parent().unwrap().to_path_buf()
}
