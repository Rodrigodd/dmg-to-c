use std::fs;
use std::process::Command;

#[test]
fn strict_dry_run_keeps_cell_stdout_clean_and_surfaces_initial_omission_on_stderr() {
    let directory = std::env::temp_dir().join(format!(
        "sv-to-sexpr-cli-diagnostic-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if directory.exists() {
        fs::remove_dir_all(&directory).unwrap();
    }
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("state.sv");
    let output = directory.join("state.cell");
    fs::write(
        &input,
        "module state(output logic q);\n  initial q = '0;\nendmodule\n",
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args([
            "convert-file",
            "--dry-run",
            "--strict",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.starts_with("(cell\n  state\n"));
    assert!(!stdout.contains("intentional-ignore"));
    assert_eq!(stderr.matches("intentional-ignore:").count(), 1);
    assert!(stderr.contains(
        "literal initial value/event is intentionally omitted because the cell model has no initial event queue"
    ));
    assert!(!output.exists());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ambiguous_specify_warning_succeeds_normally_and_fails_in_strict_mode() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let input = root.join("sv-to-sexpr/tests/fixtures/timing/ambiguous_paths.sv");
    let output = std::env::temp_dir().join(format!(
        "sv-to-sexpr-cli-ambiguous-{}-{:?}.cell",
        std::process::id(),
        std::thread::current().id()
    ));
    if output.exists() {
        fs::remove_file(&output).unwrap();
    }

    let ordinary = Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args([
            "convert-file",
            "--dry-run",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(ordinary.status.success());
    let ordinary_stdout = String::from_utf8(ordinary.stdout).unwrap();
    let ordinary_stderr = String::from_utf8(ordinary.stderr).unwrap();
    assert!(ordinary_stdout.starts_with("(cell\n  timing_ambiguous_paths\n"));
    assert_eq!(ordinary_stderr.matches(": warning:").count(), 1);
    assert_eq!(ordinary_stderr.matches(": intentional-ignore:").count(), 2);
    assert!(ordinary_stderr.contains(
        "multiple control-dependent specify paths target `y`; the one-delay cell DSL selects the first source-ordered path"
    ));
    assert!(!output.exists());

    let strict = Command::new(env!("CARGO_BIN_EXE_sv-to-sexpr"))
        .args([
            "convert-file",
            "--dry-run",
            "--strict",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!strict.status.success());
    assert!(strict.stdout.is_empty());
    let strict_stderr = String::from_utf8(strict.stderr).unwrap();
    assert_eq!(strict_stderr.matches(": warning:").count(), 2);
    assert_eq!(strict_stderr.matches(": intentional-ignore:").count(), 2);
    assert!(strict_stderr.lines().last().unwrap().contains(
        "warning: multiple control-dependent specify paths target `y`; the one-delay cell DSL selects the first source-ordered path"
    ));
    assert!(!output.exists());
}
