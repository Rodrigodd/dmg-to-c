use sexpr_fmt::{FormatOptions, ParseErrorKind, format_source, format_source_default};

#[test]
fn default_api_is_canonical_and_idempotent() {
    let source = "(alpha   beta)\n\n\n; note\n(gamma\n delta)";
    let canonical = "(alpha beta)\n\n; note\n(gamma delta)\n";

    let first = format_source_default(source).unwrap();
    let second = format_source_default(&first).unwrap();

    assert_eq!(first, canonical);
    assert_eq!(second, first);
    assert_eq!(
        format_source(source, FormatOptions::default()).unwrap(),
        first
    );
}

#[test]
fn api_returns_typed_parse_failure_with_exact_location() {
    let error = format_source_default("(alpha").unwrap_err();

    assert_eq!(error.kind, ParseErrorKind::UnclosedOpen);
    assert_eq!(error.location.offset, 0);
    assert_eq!(error.location.line, 1);
    assert_eq!(error.location.column, 1);
    assert_eq!(error.message, "unclosed '('");
}
