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
