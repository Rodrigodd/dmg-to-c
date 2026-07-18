#[allow(dead_code)]
mod analysis_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use analysis_support::corpus;
use sexpr_fmt::format_source_default;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;
use sv_to_sexpr::serialize::render_cell;

const REPRESENTATIVE_CASES: &[(&str, &str, &str)] = &[
    (
        "combinational",
        "sv-cells/sm83/cells/and3.sv",
        "tests/fixtures/lower/and3.cell",
    ),
    (
        "drivers",
        "sv-cells/dmg_cpu_b/cells/buf_if0.sv",
        "tests/fixtures/drivers/signal_high_z.cell",
    ),
    (
        "stateful",
        "sv-cells/dmg_cpu_b/cells/dlatch.sv",
        "tests/fixtures/stateful/simple_latch.cell",
    ),
    (
        "generate",
        "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
        "tests/fixtures/generate/dffr_cc_delayful.cell",
    ),
    (
        "hierarchy",
        "sv-cells/dmg_cpu_b/cells/full_add.sv",
        "tests/fixtures/hierarchy/full_add.cell",
    ),
    (
        "keeper",
        "sv-cells/dmg_cpu_b/cells/mux.sv",
        "tests/fixtures/keeper/mux.cell",
    ),
    (
        "timing",
        "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        "tests/fixtures/timing/reference.cell",
    ),
    (
        "transistor",
        "sv-cells/sm83/cells/idu_bit0.sv",
        "tests/fixtures/transistor/idu_bit0.cell",
    ),
];

#[test]
fn every_cell_fixture_is_parseable_canonical_and_idempotent() {
    let fixture_root = manifest_root().join("tests/fixtures");
    let mut paths = Vec::new();
    collect_cell_files(&fixture_root, &mut paths);
    paths.sort();
    assert_eq!(paths.len(), 38);

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
fn representative_lowered_families_match_canonical_goldens_and_preserve_ir() {
    let corpus = corpus();
    let mut rendered_reference = None;

    for (family, logical_path, golden_path) in REPRESENTATIVE_CASES {
        let design = &corpus.designs[*logical_path];
        let lowered = lower_design_with_catalog_and_generate_mode(
            design,
            &corpus.catalog,
            GenerateMode::Delayful,
        )
        .unwrap_or_else(|error| panic!("failed to lower {family} case {logical_path}: {error}"));
        lowered.cell.validate().unwrap();
        let unchanged_cell = lowered.cell.clone();

        let first = render_cell(&lowered.cell);
        let second = render_cell(&lowered.cell);
        let expected = fs::read_to_string(manifest_root().join(golden_path)).unwrap();

        assert_eq!(
            lowered.cell, unchanged_cell,
            "serializer mutated {family} IR"
        );
        assert_eq!(first, second, "nondeterministic {family} serialization");
        assert_eq!(first, expected, "canonical {family} golden changed");
        assert_eq!(format_source_default(&first).unwrap(), first);
        if *family == "timing" {
            rendered_reference = Some(first);
        }
    }

    let checked_reference = fs::read_to_string(
        repository_root().join("sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell"),
    )
    .unwrap();
    assert_eq!(rendered_reference.unwrap(), checked_reference);
}

#[test]
fn sibling_formatter_cli_check_agrees_with_api_on_representative_files() {
    let root = repository_root();
    for relative in [
        "sv-to-sexpr/tests/fixtures/drivers/signal_high_z.cell",
        "sv-to-sexpr/tests/fixtures/lower/alu_cgen.cell",
        "sv-to-sexpr/tests/fixtures/stateful/simple_latch.cell",
        "sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell",
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
