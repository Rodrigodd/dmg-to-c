use std::path::Path;

use sv_to_sexpr::diagnostic::{DiagnosticKind, Span};
use sv_to_sexpr::parser::parse_file;

fn assert_parse_error(path: &str, source: &str, line: usize, column: usize, message: &str) {
    let error = parse_file(Path::new(path), source).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Error);
    assert_eq!(error.span, Span::new(path, line, column));
    assert_eq!(error.message, message);
}

#[test]
fn unterminated_module_reports_the_logical_eof() {
    assert_parse_error(
        "fixtures/truncated_module.sv",
        "module truncated;\n",
        2,
        1,
        "unterminated module body",
    );
}

#[test]
fn truncated_grouped_assignment_reports_expected_delimiter_at_eof() {
    assert_parse_error(
        "fixtures/truncated_group.sv",
        "module grouped;\n  assign y = (a",
        2,
        16,
        "expected punctuation `RParen`",
    );
}

#[test]
fn truncated_named_connection_reports_expected_delimiter_at_eof() {
    assert_parse_error(
        "fixtures/truncated_connection.sv",
        "module top;\n  child inst(.a(a",
        2,
        18,
        "expected punctuation `RParen`",
    );
}

#[test]
fn unterminated_specify_and_generate_report_their_logical_eof() {
    assert_parse_error(
        "fixtures/truncated_specify.sv",
        "module timed;\n  specify\n",
        3,
        1,
        "unterminated specify block",
    );
    assert_parse_error(
        "fixtures/truncated_generate.sv",
        "module generated;\n  generate\n",
        3,
        1,
        "unterminated generate block",
    );
}

#[test]
fn parser_success_requires_consuming_every_source_token() {
    assert_parse_error(
        "fixtures/trailing_token.sv",
        "module done; endmodule junk",
        1,
        24,
        "expected directive or `module` declaration",
    );
}
