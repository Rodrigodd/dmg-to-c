use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sv_to_sexpr::analyze::{DriverSource, InstantiationResolution, SignalRole};
use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, Expr, LoweredModule, ValueOperator};
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::{
    analyze_file_with_sibling_catalog_and_generate_mode,
    lower_file_with_sibling_catalog_and_generate_mode,
};

const IDU_BIT0: &str = "sv-cells/sm83/cells/idu_bit0.sv";

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    path: &'static str,
    target: &'static str,
    instance: &'static str,
}

fn cases() -> [Case; 4] {
    [
        Case {
            name: "mux",
            path: "sv-cells/dmg_cpu_b/cells/mux.sv",
            target: "mux",
            instance: "mux_keeper",
        },
        Case {
            name: "muxi",
            path: "sv-cells/dmg_cpu_b/cells/muxi.sv",
            target: "mux",
            instance: "mux_keeper",
        },
        Case {
            name: "pad_xtal",
            path: "sv-cells/dmg_cpu_b/cells/pad_xtal.sv",
            target: "clk",
            instance: "clk_keeper",
        },
        Case {
            name: "reg_wz_out",
            path: "sv-cells/sm83/cells/reg_wz_out.sv",
            target: "aoi_a_y",
            instance: "aoi_a_y_keeper",
        },
    ]
}

#[test]
fn reviewed_keeper_goldens_are_exact_flat_and_generate_mode_invariant() {
    for case in cases() {
        let delayful = configured(case.path, GenerateMode::Delayful).unwrap();
        let nodelay = configured(case.path, GenerateMode::Nodelay).unwrap();
        assert_eq!(
            delayful, nodelay,
            "{} changed with generate mode",
            case.path
        );
        let (analysis, lowered) = delayful;
        lowered.cell.validate().unwrap();
        assert_keeper_case(case, &analysis, &lowered);

        assert_or_update_fixture(
            case.name,
            "analysis",
            &normalize_repository_paths(analysis.render()),
        );
        assert_or_update_fixture(case.name, "ir", &render_cli_ir(&lowered));
        assert_or_update_fixture(case.name, "cell", &render_cell(&lowered.cell));
        assert_or_update_fixture(
            case.name,
            "diagnostics",
            &render_diagnostics(&lowered.diagnostics),
        );
    }
}

#[test]
fn idu_bit0_resolves_keeper_before_exact_nmos_blocker() {
    let root = repository_root();
    let physical = root.join(IDU_BIT0);
    let analysis =
        analyze_file_with_sibling_catalog_and_generate_mode(&physical, GenerateMode::Delayful)
            .unwrap();
    let module = &analysis.modules[0];
    let instance = module
        .instantiations
        .iter()
        .find(|instance| instance.module == "keeper")
        .unwrap();
    let InstantiationResolution::Special(special) = &instance.resolution else {
        panic!("idu_bit0 keeper was not specially resolved")
    };
    assert_eq!(special.keeper.instance, "aoi_y_keeper");
    assert_eq!(special.keeper.connection.target, "aoi_y");
    assert_eq!(instance.source_order, 4);
    assert!(
        module.signal_roles["aoi_y"]
            .roles
            .contains(&SignalRole::KeeperDriven)
    );
    assert!(!module.registers.iter().any(|name| name == "aoi_y"));
    assert!(matches!(
        module.drivers[4].source,
        DriverSource::Keeper { .. }
    ));
    assert!(matches!(
        module.drivers[5].source,
        DriverSource::Primitive { ref name } if name == "nmos"
    ));
    assert!(analysis.requirements.iter().all(|requirement| {
        requirement.capability_id != "hierarchy.keeper" && requirement.milestone.label() != "M10"
    }));

    let error =
        lower_file_with_sibling_catalog_and_generate_mode(&physical, GenerateMode::Delayful)
            .unwrap_err();
    assert_eq!(error.span.line, 37);
    assert_eq!(error.span.column, 2);
    assert_eq!(error.message, "unsupported primitive nmos");
    assert_or_update_fixture(
        "idu_bit0",
        "analysis",
        &normalize_repository_paths(analysis.render()),
    );
    assert_or_update_fixture(
        "idu_bit0",
        "diagnostics",
        &render_diagnostics(std::slice::from_ref(&error)),
    );
}

#[test]
fn cli_lower_and_convert_match_keeper_goldens_in_both_modes() {
    for case in cases() {
        for nodelay in [false, true] {
            let mut lower_args = vec!["lower", case.path];
            if nodelay {
                lower_args.push("--nodelay");
            }
            let lower = run_cli(&lower_args);
            assert!(
                lower.status.success(),
                "lower failed for {}: {}",
                case.path,
                String::from_utf8_lossy(&lower.stderr)
            );
            assert_eq!(
                String::from_utf8(lower.stdout).unwrap(),
                fixture(case.name, "ir")
            );

            let output = temporary_output(case.name, nodelay);
            let mut convert_args = vec![
                "convert-file",
                "--dry-run",
                case.path,
                output.to_str().unwrap(),
            ];
            if nodelay {
                convert_args.push("--nodelay");
            }
            let convert = run_cli(&convert_args);
            assert!(
                convert.status.success(),
                "convert failed for {}: {}",
                case.path,
                String::from_utf8_lossy(&convert.stderr)
            );
            assert_eq!(
                String::from_utf8(convert.stdout).unwrap(),
                fixture(case.name, "cell")
            );
            assert!(!output.exists());
        }
    }

    let idu = run_cli(&["lower", IDU_BIT0]);
    assert!(!idu.status.success());
    assert!(idu.stdout.is_empty());
    let stderr = String::from_utf8(idu.stderr).unwrap();
    assert!(stderr.contains("idu_bit0.sv:37:2: error: unsupported primitive nmos"));
    assert!(!stderr.contains("keeper"));
}

#[test]
fn keeper_cell_goldens_parse_with_sibling_formatter() {
    for case in cases() {
        let cell = fixture_path(case.name, "cell");
        let result = Command::new("cargo")
            .current_dir(repository_root())
            .args([
                "run",
                "--quiet",
                "--manifest-path",
                "sexpr-fmt/Cargo.toml",
                "--",
                cell.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "formatter rejected {}: {}",
            cell.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn configured(
    path: &str,
    mode: GenerateMode,
) -> Result<
    (sv_to_sexpr::analyze::AnalysisReport, LoweredModule),
    sv_to_sexpr::diagnostic::Diagnostic,
> {
    let physical = repository_root().join(path);
    let analysis = analyze_file_with_sibling_catalog_and_generate_mode(&physical, mode)?;
    let lowered = lower_file_with_sibling_catalog_and_generate_mode(&physical, mode)?;
    Ok((analysis, lowered))
}

fn assert_keeper_case(
    case: Case,
    analysis: &sv_to_sexpr::analyze::AnalysisReport,
    lowered: &LoweredModule,
) {
    let module = &analysis.modules[0];
    assert!(!module.registers.iter().any(|name| name == case.target));
    assert!(
        module.signal_roles[case.target]
            .roles
            .contains(&SignalRole::KeeperDriven)
    );
    let keeper_drivers = module
        .drivers
        .iter()
        .filter(|driver| {
            driver.target == case.target
                && matches!(
                    &driver.source,
                    DriverSource::Keeper { instance } if instance == case.instance
                )
        })
        .count();
    assert_eq!(keeper_drivers, 1);
    let target_assignments = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) if assignment.target == case.target => {
                Some(assignment)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        target_assignments.len() >= 2,
        "{} lost independent drivers",
        case.path
    );
    let keepers = target_assignments
        .iter()
        .filter(|assignment| {
            assignment.expr == Expr::value(ValueOperator::Keeper, vec![])
                && assignment.delay == Expr::atom("0")
        })
        .count();
    assert_eq!(keepers, 1);
    for item in &lowered.cell.items {
        let CellItem::Assignment(assignment) = item else {
            continue;
        };
        if let Expr::List(items) = &assignment.expr {
            assert!(
                items
                    .iter()
                    .skip(1)
                    .all(|operand| matches!(operand, Expr::Atom(_)))
            );
        }
    }
}

fn render_cli_ir(lowered: &LoweredModule) -> String {
    format!(
        "cell:\n{:#?}\ntiming aliases:\n{:#?}\n",
        lowered.cell, lowered.timing_aliases
    )
}

fn render_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "diagnostics: []\n".to_string();
    }
    let mut output = String::from("diagnostics:\n");
    for diagnostic in diagnostics {
        writeln!(
            &mut output,
            "  {} | {}:{}:{} | {}",
            diagnostic.kind,
            logical_path(&diagnostic.span.path),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.message
        )
        .unwrap();
    }
    output
}

fn logical_path(path: &Path) -> String {
    path.strip_prefix(repository_root())
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_repository_paths(rendered: String) -> String {
    rendered.replace(&format!("{}/", repository_root().display()), "")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture_path(name: &str, extension: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/keeper")
        .join(format!("{name}.{extension}"))
}

fn fixture(name: &str, extension: &str) -> String {
    fs::read_to_string(fixture_path(name, extension)).unwrap()
}

fn assert_or_update_fixture(name: &str, extension: &str, actual: &str) {
    let path = fixture_path(name, extension);
    if std::env::var_os("UPDATE_KEEPER_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert_eq!(
        actual,
        expected,
        "keeper fixture {} changed",
        path.display()
    );
}

fn temporary_output(name: &str, nodelay: bool) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sv-to-sexpr-keeper-{name}-{nodelay}-{}-{:?}.cell",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    path
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .current_dir(repository_root())
        .args(args)
        .output()
        .unwrap()
}
