use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::ir::{
    CellItem, DelayTuple, Expr, LoweredModule, TimingExpr, TimingOperator, ValueOperator,
};
use sv_to_sexpr::lower::lower_file;

pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("converter crate must be inside the repository")
        .to_path_buf()
}

pub fn read_repository_file(logical_path: &str) -> String {
    fs::read_to_string(repository_root().join(logical_path))
        .unwrap_or_else(|error| panic!("failed to read {logical_path}: {error}"))
}

pub fn lower_repository_file(logical_path: &str) -> LoweredModule {
    let input = read_repository_file(logical_path);
    lower_file(Path::new(logical_path), &input)
        .unwrap_or_else(|error| panic!("failed to lower {logical_path}: {error}"))
}

pub fn assert_or_update_fixture(name: &str, extension: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stateful")
        .join(format!("{name}.{extension}"));
    if std::env::var_os("UPDATE_STATEFUL_GOLDENS").is_some() {
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual,
        expected,
        "stateful fixture {} changed",
        fixture.display()
    );
}

pub fn render_typed_ir(logical_path: &str, lowered: &LoweredModule) -> String {
    let cell = &lowered.cell;
    let mut output = String::new();
    writeln!(&mut output, "source: {logical_path}").unwrap();
    writeln!(&mut output, "cell: {}", cell.name).unwrap();
    writeln!(&mut output, "inputs: [{}]", cell.inputs.join(", ")).unwrap();
    writeln!(&mut output, "outputs: [{}]", cell.outputs.join(", ")).unwrap();
    writeln!(
        &mut output,
        "registers: [{}]",
        cell.registers
            .iter()
            .map(|register| format!("{}={}", register.name, register.initial.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(&mut output, "assignments:").unwrap();
    for (index, item) in cell.items.iter().enumerate() {
        match item {
            CellItem::Assignment(assignment) => {
                writeln!(&mut output, "  {index}: {}", assignment.target).unwrap();
                writeln!(&mut output, "    value: {}", render_value(&assignment.expr)).unwrap();
                writeln!(
                    &mut output,
                    "    delay: {}",
                    render_delay(&assignment.delay)
                )
                .unwrap();
            }
            CellItem::Blank => writeln!(&mut output, "  {index}: blank").unwrap(),
            CellItem::Comment(comment) => {
                writeln!(&mut output, "  {index}: comment({comment})").unwrap()
            }
        }
    }
    output
}

pub fn render_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "diagnostics: []\n".to_string();
    }
    let mut output = "diagnostics:\n".to_string();
    for diagnostic in diagnostics {
        writeln!(
            &mut output,
            "  {} | {}:{}:{} | {}",
            diagnostic.kind,
            diagnostic.span.path.display(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.message
        )
        .unwrap();
    }
    output
}

fn render_value(expr: &Expr) -> String {
    match expr {
        Expr::Atom(atom) => format!("atom({atom})"),
        Expr::List(items) => {
            let (head, operands) = items.split_first().expect("validated value operator");
            let Expr::Atom(head) = head else {
                panic!("validated value operator head must be an atom");
            };
            let operator = ValueOperator::parse(head).expect("validated value operator");
            let operands = operands
                .iter()
                .map(|operand| match operand {
                    Expr::Atom(atom) => format!("atom({atom})"),
                    Expr::List(_) => panic!("validated value operand must be flat"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("op({}; {operands})", operator.as_str())
        }
    }
}

fn render_delay(delay: &DelayTuple) -> String {
    format!(
        "tuple({})",
        delay
            .components()
            .map(render_timing)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_timing(expr: &TimingExpr) -> String {
    render_timing_tree(expr.as_expr())
}

fn render_timing_tree(expr: &Expr) -> String {
    match expr {
        Expr::Atom(atom) => format!("atom({atom})"),
        Expr::List(items) => {
            let (head, operands) = items.split_first().expect("validated timing operator");
            let Expr::Atom(head) = head else {
                panic!("validated timing operator head must be an atom");
            };
            let operator = TimingOperator::parse(head).expect("validated timing operator");
            let operands = operands
                .iter()
                .map(render_timing_tree)
                .collect::<Vec<_>>()
                .join(", ");
            format!("op({}; {operands})", operator.as_str())
        }
    }
}
