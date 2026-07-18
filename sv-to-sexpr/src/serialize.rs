use crate::ir::{Assignment, Cell, CellItem, Expr};
use std::fmt::Write as _;

pub fn render_cell(cell: &Cell) -> String {
    let source = render_cell_source(cell);
    sexpr_fmt::format_source_default(&source)
        .expect("typed cell serializer must emit valid S-expression source")
}

fn render_cell_source(cell: &Cell) -> String {
    let mut out = String::new();
    writeln!(&mut out, "(cell").unwrap();
    writeln!(&mut out, "  {}", cell.name).unwrap();
    render_list_section(&mut out, "inputs", &cell.inputs);
    out.push('\n');
    render_inline_section(&mut out, "outputs", &cell.outputs);
    out.push('\n');
    render_inline_section(&mut out, "registers", &cell.registers);
    out.push('\n');
    writeln!(&mut out, "  (assignments").unwrap();
    writeln!(&mut out).unwrap();
    for item in &cell.items {
        match item {
            CellItem::Blank => writeln!(&mut out).unwrap(),
            CellItem::Comment(text) => writeln!(&mut out, "    ;; {}", text).unwrap(),
            CellItem::Assignment(assignment) => render_assignment(&mut out, assignment),
        }
    }
    writeln!(&mut out, "  )").unwrap();
    writeln!(&mut out, ")").unwrap();
    out
}

fn render_list_section(out: &mut String, label: &str, items: &[String]) {
    if items.len() <= 3 {
        writeln!(out, "  ({} {})", label, items.join(" ")).unwrap();
    } else {
        writeln!(out, "  ({label}", label = label).unwrap();
        for item in items {
            writeln!(out, "    {}", item).unwrap();
        }
        writeln!(out, "  )").unwrap();
    }
}

fn render_inline_section(out: &mut String, label: &str, items: &[String]) {
    writeln!(out, "  ({} {})", label, items.join(" ")).unwrap();
}

fn render_assignment(out: &mut String, assignment: &Assignment) {
    let expr = render_expr(&assignment.expr);
    let delay = render_expr(&assignment.delay);
    if should_wrap_assignment(&assignment.target, &expr, &delay) {
        writeln!(out, "    ({target}", target = assignment.target).unwrap();
        writeln!(out, "      {}", expr).unwrap();
        writeln!(out, "      {}", delay).unwrap();
        writeln!(out, "    )").unwrap();
    } else {
        writeln!(out, "    ({} {} {})", assignment.target, expr, delay).unwrap();
    }
}

fn should_wrap_assignment(target: &str, expr: &str, delay: &str) -> bool {
    if target == "q_n" && delay != "0" {
        return true;
    }
    expr.len() + delay.len() > 48
}

pub fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Atom(atom) => atom.clone(),
        Expr::List(items) => {
            let rendered = items.iter().map(render_expr).collect::<Vec<_>>().join(" ");
            format!("({})", rendered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{TimingOperator, ValueOperator};

    #[test]
    fn representative_value_and_nested_delay_forms_serialize_canonically() {
        let cell = Cell {
            name: "serializer_sample".into(),
            inputs: vec!["a".into(), "b".into()],
            outputs: vec!["y".into()],
            registers: Vec::new(),
            items: vec![CellItem::Assignment(Assignment {
                target: "y".into(),
                expr: Expr::value(ValueOperator::And, vec![Expr::atom("a"), Expr::atom("b")]),
                delay: Expr::timing(
                    TimingOperator::Add,
                    vec![
                        Expr::timing(
                            TimingOperator::Elmore,
                            vec![
                                Expr::timing(TimingOperator::Wire, vec![Expr::atom("L_y")]),
                                Expr::timing(TimingOperator::Pmos, vec![Expr::atom("5")]),
                            ],
                        ),
                        Expr::atom("T_extra"),
                    ],
                ),
            })],
        };
        cell.validate().unwrap();

        let first = render_cell(&cell);
        let second = render_cell(&cell);
        assert_eq!(first, second);
        assert_eq!(sexpr_fmt::format_source_default(&first).unwrap(), first);
        assert!(first.contains("(y (and a b) (+ (elmore (wire L_y) (pmos 5)) T_extra))"));
    }
}
