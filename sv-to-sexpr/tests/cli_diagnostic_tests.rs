use std::fs;
use std::process::Command;

#[test]
fn strict_dry_run_serializes_initial_metadata_without_a_diagnostic() {
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
    assert!(stdout.contains("(registers (q 0))"));
    assert!(stderr.is_empty());
    assert!(!output.exists());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ambiguous_specify_intentional_ignore_succeeds_normally_and_in_strict_mode() {
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
    assert_eq!(ordinary_stderr.matches(": warning:").count(), 0);
    assert_eq!(ordinary_stderr.matches(": intentional-ignore:").count(), 1);
    assert!(ordinary_stderr.contains(
        "additional control-dependent specify path for target `y` is intentionally ignored because delay-tuple lowering temporarily selects the first source-ordered path for the target"
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
    assert!(strict.status.success());
    let strict_stdout = String::from_utf8(strict.stdout).unwrap();
    assert_eq!(strict_stdout, ordinary_stdout);
    let strict_stderr = String::from_utf8(strict.stderr).unwrap();
    assert_eq!(strict_stderr.matches(": warning:").count(), 0);
    assert_eq!(strict_stderr.matches(": intentional-ignore:").count(), 1);
    assert!(strict_stderr.contains(
        "intentional-ignore: additional control-dependent specify path for target `y` is intentionally ignored because delay-tuple lowering temporarily selects the first source-ordered path for the target"
    ));
    assert!(!output.exists());
}
