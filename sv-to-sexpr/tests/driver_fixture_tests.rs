mod driver_support;

use std::collections::BTreeSet;
use std::path::{Component, Path};

use driver_support::{
    assert_or_update_fixture, lower_repository_file, read_repository_file, render_diagnostics,
    render_typed_ir, repository_root,
};
use sv_to_sexpr::analyze::{DriverSource, analyze_design_structural};
use sv_to_sexpr::diagnostic::DiagnosticKind;
use sv_to_sexpr::ir::{Assignment, Cell, CellItem, Expr, StrengthPair, ValueOperator};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::{render_cell, render_expr};

struct FixtureCase {
    name: &'static str,
    source: &'static str,
    inputs: &'static [&'static str],
    outputs: &'static [&'static str],
    source_targets: &'static [&'static str],
    temporary_indices: &'static [usize],
    expected_values: &'static [(&'static str, &'static str)],
    expected_delay_ignores: usize,
    expected_warnings: usize,
}

const CASES: &[FixtureCase] = &[
    FixtureCase {
        name: "signal_high_z",
        source: "sv-cells/dmg_cpu_b/cells/buf_if0.sv",
        inputs: &["in", "ena_n"],
        outputs: &["y"],
        source_targets: &["y"],
        temporary_indices: &[0, 1],
        expected_values: &[
            ("t0", "(not in)"),
            ("t1", "(not t0)"),
            ("y", "(bufif0 t1 ena_n)"),
        ],
        expected_delay_ignores: 3,
        expected_warnings: 1,
    },
    FixtureCase {
        name: "open_drain",
        source: "sv-cells/sm83/cells/nand2_od_a_dbus.sv",
        inputs: &["in1", "in2"],
        outputs: &["y"],
        source_targets: &["y"],
        temporary_indices: &[0],
        expected_values: &[
            ("t0", "(and in1 in2)"),
            ("y", "(bufif1-strength 0 t0 highz1 strong0)"),
        ],
        expected_delay_ignores: 2,
        expected_warnings: 0,
    },
    FixtureCase {
        name: "precharge",
        source: "sv-cells/sm83/cells/pch_dec2_a.sv",
        inputs: &["pch_n"],
        outputs: &["y"],
        source_targets: &["y"],
        temporary_indices: &[],
        expected_values: &[("y", "(bufif0-strength 1 pch_n strong1 highz0)")],
        expected_delay_ignores: 2,
        expected_warnings: 0,
    },
    FixtureCase {
        name: "pad_bidir_pu",
        source: "sv-cells/dmg_cpu_b/cells/pad_bidir_pu.sv",
        inputs: &["ndrv", "pdrv_n", "ena_n_pu", "pad"],
        outputs: &["i_n", "pad"],
        source_targets: &["pad", "pad", "pad", "i_n"],
        temporary_indices: &[],
        expected_values: &[
            ("pad", "(bufif1-strength 0 ndrv highz1 strong0)"),
            ("pad", "(bufif0-strength 1 pdrv_n strong1 highz0)"),
            ("pad", "(bufif0-strength 1 ena_n_pu pull1 highz0)"),
            ("i_n", "(not pad)"),
        ],
        expected_delay_ignores: 4,
        expected_warnings: 0,
    },
    FixtureCase {
        name: "repeated_bus",
        source: "sv-cells/sm83/cells/reg_pc_out_bit67.sv",
        inputs: &[
            "in1", "in2", "in3", "in4", "in5", "in6", "in7", "in8", "in9", "in10", "in11", "in12",
            "in13", "in14", "in15", "in16", "in17", "in18",
        ],
        outputs: &["y1", "y2", "y3", "y4", "y5", "y6"],
        source_targets: &["y1", "y2", "y3", "y4", "y4", "y5", "y5", "y6"],
        temporary_indices: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        expected_values: &[
            ("t0", "(and in1 in2)"),
            ("t1", "(and in3 in4)"),
            ("t2", "(or t0 t1)"),
            ("y1", "(bufif1-strength 0 t2 highz1 strong0)"),
            ("t3", "(and in5 in6)"),
            ("t4", "(and in7 in6)"),
            ("t5", "(or t3 t4)"),
            ("y2", "(bufif1-strength 0 t5 highz1 strong0)"),
            ("t6", "(and in5 in8)"),
            ("y3", "(bufif1-strength 0 t6 highz1 strong0)"),
            ("t7", "(and in7 in8)"),
            ("y4", "(bufif1-strength 0 t7 highz1 strong0)"),
            ("y4", "(bufif1-strength 0 in9 highz1 strong0)"),
            ("t8", "(and in11 in12)"),
            ("t9", "(and in13 in14)"),
            ("t10", "(or t8 t9)"),
            ("t11", "(and in10 t10)"),
            ("y5", "(bufif1-strength 0 t11 highz1 strong0)"),
            ("t12", "(and in10 in18)"),
            ("y5", "(bufif1-strength 0 t12 highz1 strong0)"),
            ("t13", "(and in10 in15 in16)"),
            ("t14", "(and in10 in13 in17)"),
            ("t15", "(or t13 t14)"),
            ("y6", "(bufif1-strength 0 t15 highz1 strong0)"),
        ],
        expected_delay_ignores: 16,
        expected_warnings: 0,
    },
    FixtureCase {
        name: "supply_tie",
        source: "sv-cells/dmg_cpu_b/cells/tie.sv",
        inputs: &[],
        outputs: &["gnd", "vdd"],
        source_targets: &["gnd", "vdd"],
        temporary_indices: &[],
        expected_values: &[
            ("gnd", "(drive-strength 0 supply1 supply0)"),
            ("vdd", "(drive-strength 1 supply1 supply0)"),
        ],
        expected_delay_ignores: 0,
        expected_warnings: 0,
    },
    FixtureCase {
        name: "direct_signal_bufif",
        source: "sv-to-sexpr/tests/fixtures/drivers/direct_signal_bufif.sv",
        inputs: &["in0", "in1", "ena0", "ena1", "ena2", "t0"],
        outputs: &["y0", "y1", "y2"],
        source_targets: &["y0", "y1", "y2"],
        temporary_indices: &[1],
        expected_values: &[
            ("y0", "(bufif0 in0 ena0)"),
            ("y1", "(bufif1 in1 ena1)"),
            ("t1", "(and ena2 t0)"),
            ("y2", "(bufif0-strength in0 t1 strong1 highz0)"),
        ],
        expected_delay_ignores: 0,
        expected_warnings: 0,
    },
];

#[test]
fn driver_goldens_are_typed_flat_deterministic_and_source_complete() {
    let mut covered_operators = Vec::new();
    let mut covered_strengths = Vec::new();

    for case in CASES {
        assert_logical_path(case.source);
        let first = lower_repository_file(case.source);
        let second = lower_repository_file(case.source);
        assert_eq!(
            first, second,
            "nondeterministic lowering for {}",
            case.source
        );
        first
            .cell
            .validate()
            .unwrap_or_else(|error| panic!("invalid cell for {}: {error}", case.source));
        assert_eq!(
            first.cell.inputs, case.inputs,
            "input order changed in {}",
            case.source
        );
        assert_eq!(
            first.cell.outputs, case.outputs,
            "output order changed in {}",
            case.source
        );
        assert!(
            first.cell.registers.is_empty(),
            "unexpected register in {}",
            case.source
        );
        assert_eq!(
            first
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind == DiagnosticKind::IntentionalIgnore)
                .count(),
            case.expected_delay_ignores,
            "delay tuple ignore count changed in {}",
            case.source
        );
        assert_eq!(
            first
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Warning)
                .count(),
            case.expected_warnings,
            "specify warning count changed in {}",
            case.source
        );
        for diagnostic in &first.diagnostics {
            match diagnostic.kind {
                DiagnosticKind::IntentionalIgnore => assert!(
                    diagnostic.message
                        == "delay tuple entry 2 is intentionally ignored because the cell model selects only entry 1"
                        || diagnostic.message
                            == "delay tuple entry 3 is intentionally ignored because the cell model selects only entry 1"
                ),
                DiagnosticKind::Warning => assert!(
                    diagnostic
                        .message
                        .starts_with("multiple control-dependent specify paths target `")
                ),
                DiagnosticKind::Error => panic!("unexpected lowering error diagnostic"),
            }
        }

        let source = read_repository_file(case.source);
        let design = parse_file(Path::new(case.source), &source).unwrap();
        let analysis = analyze_design_structural(&design);
        let module = &analysis.modules[0];
        assert_source_driver_topology(case, module, &first.cell);

        let assignments = assignments(&first.cell);
        assert_exact_values(case, &assignments);
        assert_flat_strength_and_dependency_contract(
            case,
            module,
            &assignments,
            &mut covered_operators,
            &mut covered_strengths,
        );

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
                "absolute path leaked for {}",
                case.source
            );
        }
        assert!(
            !serialized.contains("(mux "),
            "driver became a mux in {}",
            case.source
        );
        assert!(
            !serialized.split_whitespace().any(|atom| atom == "z"),
            "ordinary z leaked in {}",
            case.source
        );

        assert_or_update_fixture(case.name, "ir", &typed_ir);
        assert_or_update_fixture(case.name, "cell", &serialized);
        assert_or_update_fixture(case.name, "diagnostics", &diagnostics);
    }

    for operator in [
        ValueOperator::DriveStrength,
        ValueOperator::BufIf0Strength,
        ValueOperator::BufIf1Strength,
        ValueOperator::BufIf0,
        ValueOperator::BufIf1,
    ] {
        assert!(
            covered_operators.contains(&operator),
            "missing reviewed {} fixture",
            operator.as_str()
        );
    }
    for pair in StrengthPair::ALL {
        assert!(
            covered_strengths.contains(&pair),
            "missing reviewed strength pair {pair:?}"
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
        ValueOperator::parse(head).expect("contracted operator"),
        operands,
    ))
}

fn assert_logical_path(source: &str) {
    let path = Path::new(source);
    assert!(
        path.is_relative(),
        "fixture source must be logical: {source}"
    );
    assert!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "fixture source must be normalized: {source}"
    );
}

fn assert_source_driver_topology(
    case: &FixtureCase,
    module: &sv_to_sexpr::analyze::ModuleAnalysis,
    cell: &Cell,
) {
    let source_targets = module
        .drivers
        .iter()
        .filter(|driver| !matches!(driver.source, DriverSource::Initial))
        .map(|driver| driver.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        source_targets, case.source_targets,
        "analyzed source driver order changed in {}",
        case.source
    );

    let emitted_targets = assignments(cell)
        .into_iter()
        .filter(|assignment| module.symbols.contains_key(&assignment.target))
        .map(|assignment| assignment.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        emitted_targets, source_targets,
        "lowering dropped, merged, or reordered a source driver in {}",
        case.source
    );
}

fn assert_exact_values(case: &FixtureCase, assignments: &[&Assignment]) {
    assert_eq!(
        assignments
            .iter()
            .map(|assignment| (assignment.target.as_str(), render_expr(&assignment.expr)))
            .collect::<Vec<_>>(),
        case.expected_values
            .iter()
            .map(|(target, value)| (*target, (*value).to_string()))
            .collect::<Vec<_>>(),
        "reviewed driver value/control/source order changed in {}",
        case.source
    );
}

fn assert_flat_strength_and_dependency_contract(
    case: &FixtureCase,
    module: &sv_to_sexpr::analyze::ModuleAnalysis,
    assignments: &[&Assignment],
    covered_operators: &mut Vec<ValueOperator>,
    covered_strengths: &mut Vec<StrengthPair>,
) {
    let source_names = module.symbols.keys().cloned().collect::<BTreeSet<_>>();
    let generated_indices = assignments
        .iter()
        .filter(|assignment| !source_names.contains(&assignment.target))
        .map(|assignment| {
            temp_index(&assignment.target).expect("generated value must be an SSA tN")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        generated_indices, case.temporary_indices,
        "temporary sequence changed in {}",
        case.source
    );

    let generated_names = assignments
        .iter()
        .filter(|assignment| !source_names.contains(&assignment.target))
        .map(|assignment| assignment.target.clone())
        .collect::<BTreeSet<_>>();
    let mut available = BTreeSet::new();
    for assignment in assignments {
        assignment.validate().unwrap();
        if let Some((operator, operands)) = value_operation(&assignment.expr) {
            assert!(operator.accepts_arity(operands.len()));
            assert!(operands.iter().all(
                |operand| matches!(operand, Expr::Atom(atom) if !atom.is_empty() && atom != "z")
            ));
            if !covered_operators.contains(&operator) {
                covered_operators.push(operator);
            }
            if matches!(
                operator,
                ValueOperator::DriveStrength
                    | ValueOperator::BufIf0Strength
                    | ValueOperator::BufIf1Strength
            ) {
                let [.., Expr::Atom(first), Expr::Atom(second)] = operands else {
                    panic!("validated strength operands must end in atoms");
                };
                let pair =
                    StrengthPair::parse(first, second).expect("validated contracted strength pair");
                if !covered_strengths.contains(&pair) {
                    covered_strengths.push(pair);
                }
            }
            for operand in operands {
                let Expr::Atom(atom) = operand else {
                    unreachable!("flatness asserted above");
                };
                if generated_names.contains(atom) {
                    assert!(
                        available.contains(atom),
                        "{} uses {atom} before definition in {}",
                        assignment.target,
                        case.source
                    );
                }
            }
        }
        assignment
            .delay
            .validate_timing("reviewed driver delay")
            .unwrap();
        if generated_names.contains(&assignment.target) {
            assert!(available.insert(assignment.target.clone()));
        }
    }

    if case.name == "direct_signal_bufif" {
        assert!(source_names.contains("t0"));
        assert!(
            !assignments
                .iter()
                .any(|assignment| assignment.target == "t0")
        );
        assert_eq!(generated_indices, vec![1]);
    }
}

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}
