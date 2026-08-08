use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, DelayTuple, TimingExpr, TimingOperator};
use sv_to_sexpr::lower::{
    LoweredDecomposedTimingModel, lower_design_with_decomposed_timing_and_generate_mode,
};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;

struct Case {
    name: &'static str,
    source: &'static str,
}

const CASES: &[Case] = &[Case {
    name: "ao21.delayful",
    source: "sv-cells/dmg_cpu_b/cells/ao21.sv",
}];

#[test]
fn reviewed_decomposed_timing_goldens_are_deterministic_and_structural() {
    for case in CASES {
        let design = parse_repository_source(case.source);
        let first =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap_or_else(|error| panic!("failed to lower {}: {error}", case.source));
        let second =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap_or_else(|error| panic!("failed to lower {} again: {error}", case.source));

        let first_rendered = render_cell(&first.lowered().cell);
        let second_rendered = render_cell(&second.lowered().cell);
        assert_eq!(
            first_rendered, second_rendered,
            "nondeterministic cell for {}",
            case.name
        );
        assert_eq!(first.decomposition(), second.decomposition());
        assert_eq!(first.applied_facts(), second.applied_facts());
        assert_eq!(first.actual_verification(), second.actual_verification());
        assert_or_update_golden(case.name, &first_rendered);

        match case.name {
            "ao21.delayful" => assert_ao21_distributes_its_shared_output_suffix(&first),
            _ => unreachable!(),
        }
    }
}

fn assert_ao21_distributes_its_shared_output_suffix(model: &LoweredDecomposedTimingModel) {
    let elmore = |length: &str, device, multiplier: Option<&str>| {
        let drive = TimingExpr::operation(device, vec![TimingExpr::atom("35").unwrap()]).unwrap();
        let drive = multiplier.map_or(drive.clone(), |multiplier| {
            TimingExpr::operation(
                TimingOperator::Multiply,
                vec![drive, TimingExpr::atom(multiplier).unwrap()],
            )
            .unwrap()
        });
        TimingExpr::operation(
            TimingOperator::Elmore,
            vec![
                TimingExpr::operation(
                    TimingOperator::Wire,
                    vec![TimingExpr::atom(length).unwrap()],
                )
                .unwrap(),
                drive,
            ],
        )
        .unwrap()
    };
    let zero = TimingExpr::atom("0").unwrap();
    let expected = [
        (
            "t0",
            DelayTuple::Two {
                rise: elmore("112", TimingOperator::Nmos, Some("2")),
                fall: zero.clone(),
            },
        ),
        (
            "d0",
            DelayTuple::Two {
                rise: elmore("112", TimingOperator::Nmos, None),
                fall: zero.clone(),
            },
        ),
        (
            "y",
            DelayTuple::Two {
                rise: elmore("L_y", TimingOperator::Pmos, None),
                fall: TimingExpr::operation(
                    TimingOperator::Add,
                    vec![
                        elmore("112", TimingOperator::Pmos, Some("2")),
                        elmore("L_y", TimingOperator::Nmos, None),
                    ],
                )
                .unwrap(),
            },
        ),
    ];
    for (target, delay) in expected {
        let assignment = assignments(model)
            .into_iter()
            .find(|assignment| assignment.target == target)
            .unwrap_or_else(|| panic!("missing ao21 assignment {target}"));
        assert_eq!(
            assignment.delay, delay,
            "{target} must retain only its source prefix before the shared output suffix"
        );
    }
}

fn assignments(model: &LoweredDecomposedTimingModel) -> Vec<&sv_to_sexpr::ir::Assignment> {
    model
        .lowered()
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect()
}

#[test]
fn cli_convert_file_honors_decompose_timing_and_matches_the_reviewed_golden() {
    let source = repository_root().join("sv-cells/dmg_cpu_b/cells/ao21.sv");
    let decomposed = convert_file_dry_run(&source, &["--decompose-timing"]);
    let ordinary = convert_file_dry_run(&source, &[]);

    // Read the golden directly; the update-capable helper would let
    // `UPDATE_FIXTURES` overwrite it with non-decomposed output.
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing_decomposition/ao21.delayful.cell");
    let expected = fs::read_to_string(&golden)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", golden.display()));
    assert_eq!(
        decomposed, expected,
        "convert-file --decompose-timing must match the reviewed decomposition golden"
    );
    assert_ne!(
        decomposed, ordinary,
        "--decompose-timing must change convert-file output for ao21"
    );
}

fn convert_file_dry_run(source: &Path, extra: &[&str]) -> String {
    let unused_output = std::env::temp_dir().join("sv-to-sexpr-convert-file-dry-run.cell");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .current_dir(repository_root())
        .arg("convert-file")
        .arg(source)
        .arg(&unused_output)
        .arg("--dry-run")
        .args(extra)
        .output()
        .expect("failed to run the converter binary");
    assert!(
        output.status.success(),
        "convert-file {extra:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("convert-file must emit UTF-8")
}

fn parse_repository_source(logical_path: &str) -> sv_to_sexpr::ast::Design {
    let path = repository_root().join(logical_path);
    let input = fs::read_to_string(&path).unwrap();
    parse_file(Path::new(logical_path), &input)
        .unwrap_or_else(|error| panic!("failed to parse {logical_path}: {error}"))
}

fn assert_or_update_golden(name: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing_decomposition")
        .join(format!("{name}.cell"));
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual, expected,
        "timing decomposition fixture changed: {name}"
    );
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
