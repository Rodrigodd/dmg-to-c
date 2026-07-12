mod lowering_support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use lowering_support::{
    assert_or_update_fixture, lower_repository_file, render_typed_ir, repository_root,
};
use sv_to_sexpr::diagnostic::Span;
use sv_to_sexpr::ir::{Assignment, Cell, CellItem, Expr, ValueOperator};
use sv_to_sexpr::lower::lower_file;
use sv_to_sexpr::serialize::{render_cell, render_expr};

struct FixtureCase {
    name: &'static str,
    source: &'static str,
    source_targets: &'static [&'static str],
    direct_operator: Option<ValueOperator>,
    direct_operands: &'static [&'static str],
    temporary_count: usize,
}

const FIXTURES: &[FixtureCase] = &[
    FixtureCase {
        name: "and3",
        source: "sv-cells/sm83/cells/and3.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::And),
        direct_operands: &["in1", "in2", "in3"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "or3",
        source: "sv-cells/sm83/cells/or3_b.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Or),
        direct_operands: &["in1", "in2", "in3"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "xor",
        source: "sv-cells/sm83/cells/xor_idu_l.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Xor),
        direct_operands: &["in1", "in2"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "inverter",
        source: "sv-cells/sm83/cells/not_a.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Not),
        direct_operands: &["in"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "nand",
        source: "sv-cells/dmg_cpu_b/cells/nand2.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Nand),
        direct_operands: &["in1", "in2"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "nor",
        source: "sv-cells/sm83/cells/nor2_a.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Nor),
        direct_operands: &["in1", "in2"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "xnor",
        source: "sv-cells/dmg_cpu_b/cells/xnor.sv",
        source_targets: &["y"],
        direct_operator: Some(ValueOperator::Xnor),
        direct_operands: &["in1", "in2"],
        temporary_count: 0,
    },
    FixtureCase {
        name: "alu_cgen",
        source: "sv-cells/sm83/cells/alu_cgen.sv",
        source_targets: &[
            "cout0_n_p",
            "cout0_n_n",
            "cout1_n_p",
            "cout1_n_n",
            "cout2_n_p",
            "cout2_n_n",
            "cout3_n_p",
            "cout3_n_n",
            "cout0",
            "cout1",
            "cout2",
            "cout3",
        ],
        direct_operator: None,
        direct_operands: &[],
        temporary_count: 40,
    },
];

#[test]
fn combinational_operator_family_goldens_are_flat_stable_and_contracted() {
    for case in FIXTURES {
        let first = lower_repository_file(case.source);
        let second = lower_repository_file(case.source);
        assert_eq!(
            first, second,
            "nondeterministic lowering for {}",
            case.source
        );
        first
            .validate()
            .unwrap_or_else(|error| panic!("invalid cell for {}: {error}", case.source));
        assert!(
            first.registers.is_empty(),
            "unexpected state in {}",
            case.source
        );

        let assignments = assignments(&first);
        assert_flat_values_and_temporary_delays(&assignments, case);
        assert_source_and_dependency_order(&assignments, case);

        if let Some(expected) = case.direct_operator {
            assert_eq!(assignments.len(), 1, "unexpected SSA in {}", case.source);
            let (operator, operands) = value_operation(&assignments[0].expr).unwrap();
            assert_eq!(operator, expected);
            assert_eq!(
                operands
                    .iter()
                    .map(|operand| match operand {
                        Expr::Atom(atom) => atom.as_str(),
                        Expr::List(_) => unreachable!("flatness checked above"),
                    })
                    .collect::<Vec<_>>(),
                case.direct_operands,
                "operand order changed in {}",
                case.source
            );
        } else {
            assert_alu_case_equality_muxes(&assignments);
        }

        let typed_ir = render_typed_ir(&first);
        let serialized = render_cell(&first);
        assert_eq!(typed_ir, render_typed_ir(&second));
        assert_eq!(serialized, render_cell(&second));
        let absolute_root = repository_root().to_string_lossy().to_string();
        assert!(!typed_ir.contains(&absolute_root));
        assert!(!serialized.contains(&absolute_root));
        for uncontracted in ["&&", "||", "~^", "===", " ? "] {
            assert!(
                !serialized.contains(uncontracted),
                "serialized {} contains source operator {uncontracted}",
                case.source
            );
        }
        assert_or_update_fixture(case.name, "ir", &typed_ir);
        assert_or_update_fixture(case.name, "cell", &serialized);
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

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

fn assert_flat_values_and_temporary_delays(assignments: &[&Assignment], case: &FixtureCase) {
    let source = case.source;
    let source_targets = case.source_targets.iter().copied().collect::<BTreeSet<_>>();
    for assignment in assignments {
        assignment
            .expr
            .validate_value(&format!("{source}:{}", assignment.target))
            .unwrap();
        if let Some((operator, operands)) = value_operation(&assignment.expr) {
            assert!(operator.accepts_arity(operands.len()));
            assert!(
                operands
                    .iter()
                    .all(|operand| { matches!(operand, Expr::Atom(atom) if !atom.is_empty()) })
            );
        }
        assignment
            .delay
            .validate_timing(&format!("{source}:{} delay", assignment.target))
            .unwrap();
        if !source_targets.contains(assignment.target.as_str()) {
            assert_eq!(
                assignment.delay,
                Expr::atom("0"),
                "generated SSA timing leaked onto {} in {source}",
                assignment.target
            );
        }
    }
}

fn assert_source_and_dependency_order(assignments: &[&Assignment], case: &FixtureCase) {
    let source_target_set = case.source_targets.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        assignments
            .iter()
            .filter(|assignment| source_target_set.contains(assignment.target.as_str()))
            .map(|assignment| assignment.target.as_str())
            .collect::<Vec<_>>(),
        case.source_targets,
        "source assignment order changed in {}",
        case.source
    );

    let temp_indices = assignments
        .iter()
        .filter_map(|assignment| temp_index(&assignment.target))
        .collect::<Vec<_>>();
    assert_eq!(
        temp_indices,
        (0..case.temporary_count).collect::<Vec<_>>(),
        "temporary sequence changed in {}",
        case.source
    );

    let mut available_temps = BTreeSet::new();
    for assignment in assignments {
        if let Some((_, operands)) = value_operation(&assignment.expr) {
            for operand in operands {
                let Expr::Atom(atom) = operand else {
                    unreachable!("flatness checked above");
                };
                if temp_index(atom).is_some() {
                    assert!(
                        available_temps.contains(atom),
                        "{} uses temporary {atom} before its definition",
                        assignment.target
                    );
                }
            }
        }
        if temp_index(&assignment.target).is_some() {
            available_temps.insert(assignment.target.clone());
        }
    }
}

fn assert_alu_case_equality_muxes(assignments: &[&Assignment]) {
    let operations = assignments
        .iter()
        .filter_map(|assignment| {
            value_operation(&assignment.expr)
                .map(|(operator, _)| (assignment.target.as_str(), operator))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        operations
            .values()
            .filter(|operator| **operator == ValueOperator::CaseEq)
            .count(),
        4
    );
    assert_eq!(
        operations
            .values()
            .filter(|operator| **operator == ValueOperator::Mux)
            .count(),
        4
    );

    for target in ["cout0", "cout1", "cout2", "cout3"] {
        let assignment = assignments
            .iter()
            .find(|assignment| assignment.target == target)
            .unwrap();
        let (operator, operands) = value_operation(&assignment.expr).unwrap();
        assert_eq!(operator, ValueOperator::Mux);
        let [
            Expr::Atom(condition),
            Expr::Atom(then_value),
            Expr::Atom(else_value),
        ] = operands
        else {
            panic!("{target} mux must have three atom operands");
        };
        assert_eq!(
            operations.get(condition.as_str()),
            Some(&ValueOperator::CaseEq)
        );
        assert_eq!(
            operations.get(then_value.as_str()),
            Some(&ValueOperator::Not)
        );
        assert_eq!(else_value, "x");
    }
}

fn lower_error(path: &str, input: &str) -> sv_to_sexpr::diagnostic::Diagnostic {
    lower_file(Path::new(path), input).unwrap_err()
}

#[test]
fn unsupported_value_and_timing_forms_report_exact_expression_spans() {
    let arithmetic = lower_error(
        "diagnostics/arithmetic.sv",
        "module sample(input logic a, output logic y);\n  assign y = a + 1;\nendmodule",
    );
    assert_eq!(
        arithmetic.span,
        Span::new("diagnostics/arithmetic.sv", 2, 14)
    );
    assert!(
        arithmetic
            .message
            .contains("not contracted value expressions")
    );

    let call = lower_error(
        "diagnostics/value_call.sv",
        "module sample(input logic a, output logic y);\n  assign y = mystery(a);\nendmodule",
    );
    assert_eq!(call.span, Span::new("diagnostics/value_call.sv", 2, 14));
    assert!(call.message.contains("function calls"));

    let timing_call = lower_error(
        "diagnostics/timing_call.sv",
        "module sample(input logic a, output logic y);\n  assign #(mystery(a)) y = a;\nendmodule",
    );
    assert_eq!(
        timing_call.span,
        Span::new("diagnostics/timing_call.sv", 2, 12)
    );
    assert!(
        timing_call
            .message
            .contains("uncontracted timing function `mystery`")
    );

    let delayed = lower_file(
        Path::new("diagnostics/delayed.sv"),
        "module sample(input logic a, b, c, output logic y);\n  assign #((rise > minimum) ? (rise + extra) : minimum) y = a & (b | c);\nendmodule",
    )
    .unwrap();
    let delayed = assignments(&delayed.cell);
    assert_eq!(delayed.len(), 2);
    assert_eq!(delayed[0].target, "t0");
    assert_eq!(render_expr(&delayed[0].delay), "0");
    assert_eq!(delayed[1].target, "y");
    assert_eq!(
        render_expr(&delayed[1].delay),
        "(mux (gt rise minimum) (+ rise extra) minimum)"
    );
    delayed
        .iter()
        .for_each(|assignment| assignment.validate().unwrap());
}

#[test]
fn later_driver_forms_fail_at_their_source_constructs() {
    for primitive in ["nmos", "pmos", "rnmos"] {
        let path = format!("diagnostics/{primitive}.sv");
        let input = format!(
            "module sample(input logic a, c, output logic y);\n  {primitive} (y, a, c);\nendmodule"
        );
        let error = lower_error(&path, &input);
        assert_eq!(error.span, Span::new(&path, 2, 3));
        assert_eq!(error.message, format!("unsupported primitive {primitive}"));
    }

    let path = "diagnostics/hierarchy.sv";
    let input =
        "module sample(input logic a, output logic y);\n  child u0(.a(a), .y(y));\nendmodule";
    let error = lower_error(path, input);
    assert_eq!(error.span, Span::new(path, 2, 3));
    assert_eq!(error.message, "unsupported item for lowering");

    let keeper = lower_file(
        Path::new("diagnostics/keeper.sv"),
        "module sample(output logic y);\n  keeper held(y);\nendmodule",
    )
    .unwrap();
    let assignments = assignments(&keeper.cell);
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].target, "y");
    assert_eq!(render_expr(&assignments[0].expr), "(keeper)");
    assert_eq!(render_expr(&assignments[0].delay), "0");
}
