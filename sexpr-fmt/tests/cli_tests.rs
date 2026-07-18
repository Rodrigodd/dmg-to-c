use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sexpr_fmt::{FormatOptions, format_source, format_source_default};

#[test]
fn cli_print_check_write_and_width_modes_match_library_api() {
    let path = temporary_file("modes");
    let source = "(alpha   beta)\n\n\n; note\n(gamma delta epsilon zeta)";
    fs::write(&path, source).unwrap();

    let print = run(&[path.to_str().unwrap()]);
    assert!(print.status.success());
    assert_eq!(
        String::from_utf8(print.stdout).unwrap(),
        format_source_default(source).unwrap()
    );
    assert!(print.stderr.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), source);

    let check_before = run(&["--check", path.to_str().unwrap()]);
    assert_eq!(check_before.status.code(), Some(1));
    assert!(check_before.stdout.is_empty());
    assert!(check_before.stderr.is_empty());

    let width = run(&["--width", "16", path.to_str().unwrap()]);
    assert!(width.status.success());
    assert_eq!(
        String::from_utf8(width.stdout).unwrap(),
        format_source(
            source,
            FormatOptions {
                width: 16,
                max_inline_items: 4,
            }
        )
        .unwrap()
    );
    assert!(width.stderr.is_empty());

    let write = run(&["--write", path.to_str().unwrap()]);
    assert!(write.status.success());
    assert!(write.stdout.is_empty());
    assert!(write.stderr.is_empty());
    let canonical = fs::read_to_string(&path).unwrap();
    assert_eq!(canonical, format_source_default(source).unwrap());
    assert!(canonical.contains("\n\n; note\n"));

    let check_after = run(&[path.to_str().unwrap(), "--check"]);
    assert!(check_after.status.success());
    assert!(check_after.stdout.is_empty());
    assert!(check_after.stderr.is_empty());

    fs::remove_file(path).unwrap();
}

#[test]
fn cli_parse_failure_preserves_error_exit_code_and_location() {
    let path = temporary_file("parse-error");
    fs::write(&path, "(alpha").unwrap();

    let result = run(&[path.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert_eq!(
        String::from_utf8(result.stderr).unwrap(),
        "unclosed '(' at byte 0 (line 1, column 1)\n"
    );

    fs::remove_file(path).unwrap();
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sexpr-fmt"))
        .args(args)
        .output()
        .unwrap()
}

fn temporary_file(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sexpr-fmt-{label}-{}-{:?}.cell",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    path
}
