use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::lexer::{Keyword, Operator, Punct, Token, TokenKind, lex_file};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repository_file(relative: &str) -> PathBuf {
    manifest_dir()
        .parent()
        .expect("crate must be inside the repository")
        .join(relative)
}

fn fixture_file(name: &str) -> PathBuf {
    manifest_dir().join("tests/fixtures/lexer").join(name)
}

fn lex_path(path: &Path) -> Vec<Token> {
    let input = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    lex_file(path, &input)
        .unwrap_or_else(|error| panic!("failed to lex {}: {error}", path.display()))
}

fn tokens_on_lines(tokens: Vec<Token>, first: usize, last: usize) -> Vec<Token> {
    tokens
        .into_iter()
        .filter(|token| (first..=last).contains(&token.span.line))
        .collect()
}

fn token_kind_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Identifier => "identifier",
        TokenKind::Keyword(keyword) => match keyword {
            Keyword::Module => "keyword.module",
            Keyword::Endmodule => "keyword.endmodule",
            Keyword::Parameter => "keyword.parameter",
            Keyword::Localparam => "keyword.localparam",
            Keyword::Real => "keyword.real",
            Keyword::Realtime => "keyword.realtime",
            Keyword::Input => "keyword.input",
            Keyword::Output => "keyword.output",
            Keyword::Inout => "keyword.inout",
            Keyword::Logic => "keyword.logic",
            Keyword::Tri => "keyword.tri",
            Keyword::Wire => "keyword.wire",
            Keyword::Import => "keyword.import",
            Keyword::Initial => "keyword.initial",
            Keyword::AlwaysLatch => "keyword.always_latch",
            Keyword::Assign => "keyword.assign",
            Keyword::Specify => "keyword.specify",
            Keyword::EndSpecify => "keyword.endspecify",
            Keyword::Specparam => "keyword.specparam",
        },
        TokenKind::Integer => "integer",
        TokenKind::Real => "real",
        TokenKind::ConstZero => "constant.zero",
        TokenKind::ConstOne => "constant.one",
        TokenKind::ConstZ => "constant.z",
        TokenKind::ConstX => "constant.x",
        TokenKind::Punct(punct) => match punct {
            Punct::LParen => "punct.lparen",
            Punct::RParen => "punct.rparen",
            Punct::LBracket => "punct.lbracket",
            Punct::RBracket => "punct.rbracket",
            Punct::LBrace => "punct.lbrace",
            Punct::RBrace => "punct.rbrace",
            Punct::Comma => "punct.comma",
            Punct::Semicolon => "punct.semicolon",
        },
        TokenKind::Operator(operator) => match operator {
            Operator::DoubleAnd => "operator.double_and",
            Operator::DoubleOr => "operator.double_or",
            Operator::EqualEqual => "operator.equal_equal",
            Operator::TripleEqual => "operator.triple_equal",
            Operator::NotEqual => "operator.not_equal",
            Operator::NotCaseEqual => "operator.not_case_equal",
            Operator::LessEqual => "operator.less_equal",
            Operator::Implies => "operator.implies",
            Operator::ColonColon => "operator.colon_colon",
            Operator::TildeCaret => "operator.tilde_caret",
            Operator::TildeAmpersand => "operator.tilde_ampersand",
            Operator::TildePipe => "operator.tilde_pipe",
            Operator::Tilde => "operator.tilde",
            Operator::Bang => "operator.bang",
            Operator::Ampersand => "operator.ampersand",
            Operator::Pipe => "operator.pipe",
            Operator::Caret => "operator.caret",
            Operator::Plus => "operator.plus",
            Operator::Minus => "operator.minus",
            Operator::Star => "operator.star",
            Operator::Slash => "operator.slash",
            Operator::Equals => "operator.equals",
            Operator::Less => "operator.less",
            Operator::Greater => "operator.greater",
            Operator::At => "operator.at",
            Operator::Dot => "operator.dot",
            Operator::Hash => "operator.hash",
            Operator::Question => "operator.question",
            Operator::Colon => "operator.colon",
        },
        TokenKind::Directive => "directive",
    }
}

fn escape_lexeme(lexeme: &str) -> String {
    let mut escaped = String::new();
    for ch in lexeme.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", ch as u32));
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn render_tokens(tokens: &[Token]) -> String {
    let mut rendered = String::new();
    for token in tokens {
        rendered.push_str(&format!(
            "{}:{} {:<24} \"{}\"\n",
            token.span.line,
            token.span.column,
            token_kind_name(&token.kind),
            escape_lexeme(&token.lexeme),
        ));
    }
    rendered
}

fn assert_snapshot(name: &str, tokens: &[Token]) {
    let snapshot = fixture_file(&format!("{name}.tokens"));
    let expected = fs::read_to_string(&snapshot)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", snapshot.display()));
    assert_eq!(render_tokens(tokens), expected, "snapshot {name}");
}

#[test]
fn snapshots_reference_cell() {
    let path = repository_file("sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv");
    assert_snapshot("reference_cell", &lex_path(&path));
}

#[test]
fn snapshots_generate_syntax_from_curated_dff() {
    let path = repository_file("sv-cells/dmg_cpu_b/cells/dffr.sv");
    let tokens = tokens_on_lines(lex_path(&path), 19, 28);
    assert_snapshot("generate_dffr", &tokens);
}

#[test]
fn snapshots_named_ports_and_parameter_overrides() {
    let path = repository_file("sv-cells/dmg_cpu_b/cells/half_add.sv");
    let tokens = tokens_on_lines(lex_path(&path), 11, 12);
    assert_snapshot("named_ports_half_add", &tokens);
}

#[test]
fn snapshots_strengths_and_two_entry_delays() {
    let path = repository_file("sv-cells/dmg_cpu_b/cells/pad_bidir_pu.sv");
    let tokens = tokens_on_lines(lex_path(&path), 14, 19);
    assert_snapshot("strengths_pad_bidir_pu", &tokens);
}

#[test]
fn snapshots_supply_strengths() {
    let path = repository_file("sv-cells/dmg_cpu_b/cells/tie.sv");
    let tokens = tokens_on_lines(lex_path(&path), 10, 11);
    assert_snapshot("supply_strengths_tie", &tokens);
}

#[test]
fn snapshots_direct_nmos_and_pmos_calls() {
    let path = repository_file("sv-cells/sm83/cells/irq_prio_bit0.sv");
    let tokens = tokens_on_lines(lex_path(&path), 42, 63);
    assert_snapshot("transistors_irq_prio_bit0", &tokens);
}

#[test]
fn snapshots_direct_rnmos_call() {
    let path = repository_file("sv-cells/sm83/cells/dlatch_ee_irq.sv");
    let tokens = tokens_on_lines(lex_path(&path), 23, 23);
    assert_snapshot("transistor_rnmos", &tokens);
}

#[test]
fn snapshots_one_entry_delay_and_all_unbased_literals() {
    let path = fixture_file("delay_and_literals.sv");
    assert_snapshot("delay_and_literals", &lex_path(&path));
}

#[test]
fn invalid_character_has_exact_diagnostic() {
    let path = fixture_file("invalid_character.sv");
    let error = lex_file(&path, "module bad;\n  assign q = @;\n  %\nendmodule\n").unwrap_err();

    assert_eq!(error.kind, DiagnosticKind::Error);
    assert_eq!(error.span.path, path);
    assert_eq!(error.span.line, 3);
    assert_eq!(error.span.column, 3);
    assert_eq!(error.message, "unexpected character `%`");
}
