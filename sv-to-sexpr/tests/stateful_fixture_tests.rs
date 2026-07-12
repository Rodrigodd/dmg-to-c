mod stateful_support;

use std::collections::BTreeSet;
use std::path::Path;

use stateful_support::{
    assert_or_update_fixture, lower_repository_file, read_repository_file, render_diagnostics,
    render_typed_ir, repository_root,
};
use sv_to_sexpr::ast::{ExprKind, ItemKind};
use sv_to_sexpr::diagnostic::{DiagnosticKind, Span};
use sv_to_sexpr::ir::{Assignment, Cell, CellItem, Expr, ValueOperator};
use sv_to_sexpr::lower::lower_file;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::{render_cell, render_expr};

const INITIAL_OMISSION: &str = "literal initial value/event is intentionally omitted because the cell model has no initial event queue";

struct FixtureCase {
    name: &'static str,
    source: &'static str,
    registers: &'static [&'static str],
    state_target_order: &'static [&'static str],
    temporary_indices: &'static [usize],
    initials: &'static [(&'static str, usize, usize)],
}

const CASES: &[FixtureCase] = &[
    FixtureCase {
        name: "simple_latch",
        source: "sv-cells/dmg_cpu_b/cells/dlatch.sv",
        registers: &["q"],
        state_target_order: &["q"],
        temporary_indices: &[],
        initials: &[("q", 13, 2)],
    },
    FixtureCase {
        name: "dff_cc_q",
        source: "sv-cells/sm83/cells/dff_cc_q.sv",
        registers: &["ff", "q"],
        state_target_order: &["ff", "q"],
        temporary_indices: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        initials: &[("ff", 14, 2), ("q", 15, 2)],
    },
    FixtureCase {
        name: "set_reset_latch",
        source: "sv-cells/sm83/cells/srlatch_r_n.sv",
        registers: &["q"],
        state_target_order: &["q"],
        temporary_indices: &[0, 1, 2],
        initials: &[],
    },
    FixtureCase {
        name: "block_latch",
        source: "sv-cells/dmg_cpu_b/cells/nand_latch.sv",
        registers: &["q", "q_n"],
        state_target_order: &["q", "q_n"],
        temporary_indices: &[0, 1, 2, 3, 4, 5, 6, 7],
        initials: &[],
    },
    FixtureCase {
        name: "nested_priority",
        source: "sv-to-sexpr/tests/fixtures/stateful/nested_priority.sv",
        registers: &["q"],
        state_target_order: &["q", "q"],
        temporary_indices: &[1, 2, 3, 4],
        initials: &[("q", 5, 5)],
    },
    FixtureCase {
        name: "combinational_procedure",
        source: "sv-to-sexpr/tests/fixtures/stateful/combinational_procedure.sv",
        registers: &[],
        state_target_order: &[],
        temporary_indices: &[],
        initials: &[],
    },
];

#[test]
fn stateful_goldens_are_flat_deterministic_and_contract_complete() {
    for case in CASES {
        let first = lower_repository_file(case.source);
        let second = lower_repository_file(case.source);
        assert_eq!(
            first, second,
            "nondeterministic lowered module for {}",
            case.source
        );
        first
            .cell
            .validate()
            .unwrap_or_else(|error| panic!("invalid cell for {}: {error}", case.source));
        assert_eq!(
            first.cell.registers, case.registers,
            "register classification changed in {}",
            case.source
        );

        let assignments = assignments(&first.cell);
        assert_flat_values(&assignments, case.source);
        assert_state_topology(&first.cell, &assignments, case);
        assert_temporary_order(&first.cell, &assignments, case);
        assert_initial_contract(case, &first);
        assert_case_semantics(case, &assignments);

        let typed_ir = render_typed_ir(case.source, &first);
        let serialized = render_cell(&first.cell);
        let diagnostics = render_diagnostics(&first.diagnostics);
        assert_eq!(typed_ir, render_typed_ir(case.source, &second));
        assert_eq!(serialized, render_cell(&second.cell));
        assert_eq!(diagnostics, render_diagnostics(&second.diagnostics));

        let absolute_root = repository_root().to_string_lossy().to_string();
        for rendered in [&typed_ir, &serialized, &diagnostics] {
            assert!(
                !rendered.contains(&absolute_root),
                "{} leaked an absolute repository path",
                case.source
            );
        }
        assert_or_update_fixture(case.name, "ir", &typed_ir);
        assert_or_update_fixture(case.name, "cell", &serialized);
        assert_or_update_fixture(case.name, "diagnostics", &diagnostics);
    }
}

#[test]
fn fixture_sources_prove_blocking_and_nonblocking_normalize_identically() {
    for (path, nonblocking) in [
        ("sv-cells/sm83/cells/dff_cc_q.sv", "ff <= !d;"),
        (
            "sv-to-sexpr/tests/fixtures/stateful/nested_priority.sv",
            "q <= 1;",
        ),
    ] {
        let source = read_repository_file(path);
        assert!(source.contains(nonblocking));
        let blocking_source = source.replace("<=", "=");
        assert_ne!(source, blocking_source);
        let nonblocking_lowered = lower_file(Path::new(path), &source).unwrap();
        let blocking_lowered = lower_file(Path::new(path), &blocking_source).unwrap();
        assert_eq!(nonblocking_lowered.cell, blocking_lowered.cell);
        assert_eq!(
            nonblocking_lowered.diagnostics,
            blocking_lowered.diagnostics
        );
    }
}

fn assignments(cell: &Cell) -> Vec<&Assignment> {
    cell.items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect()
}

fn value_operation(expr: &Expr) -> Option<(ValueOperator, &[Expr])> {
    let Expr::List(items) = expr else {
        return None;
    };
    let (head, operands) = items.split_first().expect("validated operator list");
    let Expr::Atom(head) = head else {
        panic!("validated operator head must be an atom");
    };
    Some((
        ValueOperator::parse(head).expect("validated contracted operator"),
        operands,
    ))
}

fn assert_flat_values(assignments: &[&Assignment], source: &str) {
    for assignment in assignments {
        assignment
            .expr
            .validate_value(&format!("{source}:{}", assignment.target))
            .unwrap();
        if let Some((operator, operands)) = value_operation(&assignment.expr) {
            assert!(
                !operands.is_empty(),
                "{} contains an empty value operation",
                assignment.target
            );
            assert!(operator.accepts_arity(operands.len()));
            assert!(
                operands
                    .iter()
                    .all(|operand| matches!(operand, Expr::Atom(atom) if !atom.is_empty())),
                "{} contains a nested or empty operand",
                assignment.target
            );
            if operator == ValueOperator::Mux {
                assert_eq!(
                    operands.len(),
                    3,
                    "{} has a non-flat mux",
                    assignment.target
                );
            }
        }
        assignment
            .delay
            .validate_timing(&format!("{source}:{} delay", assignment.target))
            .unwrap();
    }
}

fn assert_state_topology(cell: &Cell, assignments: &[&Assignment], case: &FixtureCase) {
    let registers = cell
        .registers
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let state_assignments = assignments
        .iter()
        .copied()
        .filter(|assignment| registers.contains(assignment.target.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        state_assignments
            .iter()
            .map(|assignment| assignment.target.as_str())
            .collect::<Vec<_>>(),
        case.state_target_order,
        "state assignment order changed in {}",
        case.source
    );
    for assignment in state_assignments {
        let Some((operator, operands)) = value_operation(&assignment.expr) else {
            panic!(
                "state assignment {} must be a retained mux",
                assignment.target
            );
        };
        assert_eq!(operator, ValueOperator::Mux);
        let [Expr::Atom(_), Expr::Atom(_), Expr::Atom(false_value)] = operands else {
            panic!(
                "state mux {} must have three atom operands",
                assignment.target
            );
        };
        assert_eq!(false_value, &assignment.target);
    }
}

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

fn assert_temporary_order(cell: &Cell, assignments: &[&Assignment], case: &FixtureCase) {
    let source_names = cell
        .inputs
        .iter()
        .chain(&cell.outputs)
        .chain(&cell.registers)
        .cloned()
        .collect::<BTreeSet<_>>();
    let generated = assignments
        .iter()
        .filter(|assignment| !source_names.contains(&assignment.target))
        .filter_map(|assignment| temp_index(&assignment.target))
        .collect::<Vec<_>>();
    assert_eq!(
        generated, case.temporary_indices,
        "temporary numbering changed in {}",
        case.source
    );

    let mut available = BTreeSet::new();
    for assignment in assignments {
        if let Some((_, operands)) = value_operation(&assignment.expr) {
            for operand in operands {
                let Expr::Atom(atom) = operand else {
                    unreachable!("flatness checked above");
                };
                if temp_index(atom).is_some() && !source_names.contains(atom) {
                    assert!(
                        available.contains(atom),
                        "{} uses generated temporary {atom} before its definition",
                        assignment.target
                    );
                }
            }
        }
        if temp_index(&assignment.target).is_some() && !source_names.contains(&assignment.target) {
            assert_eq!(
                assignment.delay,
                Expr::atom("0"),
                "generated SSA temporary {} acquired source timing",
                assignment.target
            );
            available.insert(assignment.target.clone());
        }
    }

    if case.name == "nested_priority" {
        assert!(source_names.contains("t0"));
        assert!(
            !assignments
                .iter()
                .any(|assignment| assignment.target == "t0")
        );
        assert_eq!(generated.first(), Some(&1));
    }
}

fn assert_initial_contract(case: &FixtureCase, lowered: &sv_to_sexpr::ir::LoweredModule) {
    let source = read_repository_file(case.source);
    let design = parse_file(Path::new(case.source), &source).unwrap();
    let module = design.first_module().unwrap();
    let initial_targets = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Initial(stmt) => match &stmt.target.kind {
                ExprKind::Path(segments) if segments.len() == 1 => Some(segments[0].as_str()),
                _ => panic!("fixture initial target must be a scalar local signal"),
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        initial_targets,
        case.initials
            .iter()
            .map(|(target, _, _)| *target)
            .collect::<Vec<_>>()
    );
    let initial_diagnostics = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message == INITIAL_OMISSION)
        .collect::<Vec<_>>();
    assert_eq!(initial_diagnostics.len(), case.initials.len());
    for (diagnostic, (target, line, column)) in initial_diagnostics.iter().zip(case.initials.iter())
    {
        assert!(
            lowered
                .cell
                .registers
                .iter()
                .any(|register| register == target)
        );
        assert_eq!(diagnostic.kind, DiagnosticKind::IntentionalIgnore);
        assert_eq!(diagnostic.span, Span::new(case.source, *line, *column));
        assert_eq!(diagnostic.message, INITIAL_OMISSION);
    }
}

fn assignment_triplets(assignments: &[&Assignment]) -> Vec<(String, String, String)> {
    assignments
        .iter()
        .map(|assignment| {
            (
                assignment.target.clone(),
                render_expr(&assignment.expr),
                render_expr(&assignment.delay),
            )
        })
        .collect()
}

fn assert_case_semantics(case: &FixtureCase, assignments: &[&Assignment]) {
    let triplets = assignment_triplets(assignments);
    match case.name {
        "simple_latch" => {
            assert_eq!(assignments.len(), 2);
            assert_eq!(
                triplets[0],
                (
                    "q".into(),
                    "(mux ena d q)".into(),
                    "(+ (+ (elmore (wire 73) (pmos 10)) (elmore (wire 101) (nmos 10))) (elmore (wire L_q) (pmos 35)))".into(),
                )
            );
            assert_eq!(assignments[1].target, "q_n");
            assert_eq!(render_expr(&assignments[1].expr), "(not q)");
        }
        "set_reset_latch" => assert_eq!(
            triplets,
            vec![
                ("t0".into(), "(not r_n)".into(), "0".into()),
                ("t1".into(), "(or t0 s)".into(), "0".into()),
                ("t2".into(), "(and s r_n)".into(), "0".into()),
                (
                    "q".into(),
                    "(mux t1 t2 q)".into(),
                    "(+ (elmore (wire 51) (* (nmos 16.5) 2)) (elmore (wire L_q) (pmos 11.0)))"
                        .into(),
                ),
            ]
        ),
        "nested_priority" => assert_eq!(
            triplets,
            vec![
                ("t1".into(), "(not reset_n)".into(), "0".into()),
                ("t2".into(), "(and enable t1)".into(), "0".into()),
                ("q".into(), "(mux t2 0 q)".into(), "0".into()),
                ("t3".into(), "(and set t0)".into(), "0".into()),
                ("t4".into(), "(and enable t3)".into(), "0".into()),
                ("q".into(), "(mux t4 1 q)".into(), "0".into()),
            ]
        ),
        "combinational_procedure" => {
            assert_eq!(triplets, vec![("y".into(), "(and a b)".into(), "0".into())])
        }
        "dff_cc_q" | "block_latch" => {}
        other => panic!("unreviewed stateful fixture case {other}"),
    }
}
