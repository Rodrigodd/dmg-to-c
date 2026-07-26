use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{CellItem, DelayTuple, Expr, LogicValue, TimingExpr, TimingOperator};
use sv_to_sexpr::lower::{
    DecomposedTimingStrategy, LoweredDecomposedTimingModel,
    lower_design_with_decomposed_timing_and_generate_mode,
};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::timing_graph::{AssignmentDelayOrigin, AssignmentOrigin, Transition};
use sv_to_sexpr::topology_hint::ResolvedPathStepKind;

struct Case {
    name: &'static str,
    source: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "ao21.delayful",
        source: "sv-cells/dmg_cpu_b/cells/ao21.sv",
    },
    Case {
        name: "dffsr.delayful",
        source: "sv-cells/dmg_cpu_b/cells/dffsr.sv",
    },
];

#[test]
fn reviewed_decomposed_timing_goldens_are_deterministic_and_structural() {
    for case in CASES {
        let design = parse_repository_source(case.source);
        let first =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap_or_else(|error| panic!("failed to lower {}: {error}", case.source));
        let second =
            lower_design_with_decomposed_timing_and_generate_mode(&design, GenerateMode::Delayful)
                .unwrap_or_else(|error| panic!("failed to lower {} again: {error}", case.source));

        let first_rendered = render_cell(&first.lowered().cell);
        let second_rendered = render_cell(&second.lowered().cell);
        assert_eq!(
            first_rendered, second_rendered,
            "nondeterministic cell for {}",
            case.name
        );
        assert_eq!(
            first.strategy(),
            second.strategy(),
            "nondeterministic strategy for {}",
            case.name
        );
        assert_or_update_golden(case.name, &first_rendered);

        match case.name {
            "ao21.delayful" => {
                assert!(matches!(
                    first.strategy(),
                    DecomposedTimingStrategy::ExactCover { .. }
                ));
                assert!(!first.is_physical_topology());
                assert_ao21_distributes_its_shared_output_suffix(&first);
            }
            "dffsr.delayful" => assert_dffsr_physical_topology(&first_rendered, &first),
            _ => unreachable!(),
        }
    }
}

fn assert_dffsr_physical_topology(rendered: &str, model: &LoweredDecomposedTimingModel) {
    let DecomposedTimingStrategy::PhysicalTopology {
        module,
        applied_facts,
        actual_verification,
    } = model.strategy()
    else {
        panic!("dffsr must select the checked-in physical topology");
    };
    assert_eq!(module, "dmg_dffsr");
    assert_eq!(actual_verification.paths().len(), 12);

    let components = actual_verification
        .paths()
        .iter()
        .map(|path| {
            (
                path.constraint().ordinal(),
                path.control().ordinal(),
                path.target_transition(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(components.len(), 12);
    assert_eq!(
        components,
        (0..6)
            .flat_map(|path| {
                [Transition::Rise, Transition::Fall]
                    .into_iter()
                    .map(move |transition| (path, path, transition))
            })
            .collect()
    );

    for path in actual_verification
        .paths()
        .iter()
        .filter(|path| path.recipe().contains("q_n"))
    {
        assert!(!path.steps().iter().any(|step| matches!(
            step.kind(),
            ResolvedPathStepKind::Generated(id)
                if id.as_str() == "physical_q" || id.as_str() == "q_replacement"
        )));
        assert!(!path.steps().iter().any(|step| matches!(
            step.kind(),
            ResolvedPathStepKind::Rewrite(id) if id.as_str() == "q_state"
        )));
    }

    let zero = DelayTuple::One(sv_to_sexpr::ir::TimingExpr::atom("0").unwrap());
    for target in ["q", "q_n"] {
        let assignment = assignments(model)
            .into_iter()
            .find(|assignment| assignment.target == target)
            .unwrap_or_else(|| panic!("missing public terminal assignment {target}"));
        assert_eq!(assignment.delay, zero, "{target} must remain zero-delay");
    }
    assert_eq!(
        model
            .lowered()
            .cell
            .registers
            .iter()
            .map(|register| (register.name.as_str(), register.initial))
            .collect::<Vec<_>>(),
        vec![("ff", LogicValue::Zero), ("q", LogicValue::Zero)]
    );

    let physical_delay_assignments = applied_facts
        .assignments
        .values()
        .filter(|fact| fact.assignment.delay != zero)
        .collect::<Vec<_>>();
    assert_eq!(physical_delay_assignments.len(), 12);
    assert!(
        physical_delay_assignments
            .iter()
            .all(|fact| matches!(fact.assignment.delay, DelayTuple::Two { .. }))
    );
    assert_eq!(
        model
            .assignment_provenance()
            .iter()
            .filter(|provenance| provenance.origin().is_topology_generated())
            .count(),
        applied_facts.assignments.len()
    );
    assert!(model.assignment_provenance().iter().all(|provenance| {
        !matches!(
            provenance.origin(),
            AssignmentOrigin::GeneratedTimingIdentity { .. }
        ) && !matches!(
            provenance.delay_origin(),
            AssignmentDelayOrigin::DecompositionPlacement
                | AssignmentDelayOrigin::LegacySelectedSpecifyFallback
        )
    }));

    assert!(
        model
            .lowered()
            .cell
            .items
            .iter()
            .all(|item| matches!(item, CellItem::Assignment(_)))
    );
    assert!(!rendered.contains("(timing"));
    assert!(!rendered.contains("(arc"));
    assert!(!rendered.contains("(table"));
    assert!(
        !assignments(model)
            .iter()
            .flat_map(|assignment| assignment.delay.components())
            .any(|component| contains_subtract(component.as_expr()))
    );
}

fn assert_ao21_distributes_its_shared_output_suffix(model: &LoweredDecomposedTimingModel) {
    let elmore = |length: &str, device, multiplier: Option<&str>| {
        let drive = TimingExpr::operation(device, vec![TimingExpr::atom("35").unwrap()]).unwrap();
        let drive = multiplier.map_or(drive.clone(), |multiplier| {
            TimingExpr::operation(
                TimingOperator::Multiply,
                vec![drive, TimingExpr::atom(multiplier).unwrap()],
            )
            .unwrap()
        });
        TimingExpr::operation(
            TimingOperator::Elmore,
            vec![
                TimingExpr::operation(
                    TimingOperator::Wire,
                    vec![TimingExpr::atom(length).unwrap()],
                )
                .unwrap(),
                drive,
            ],
        )
        .unwrap()
    };
    let zero = TimingExpr::atom("0").unwrap();
    let expected = [
        (
            "t0",
            DelayTuple::Two {
                rise: elmore("112", TimingOperator::Nmos, Some("2")),
                fall: zero.clone(),
            },
        ),
        (
            "d0",
            DelayTuple::Two {
                rise: elmore("112", TimingOperator::Nmos, None),
                fall: zero.clone(),
            },
        ),
        (
            "y",
            DelayTuple::Two {
                rise: elmore("L_y", TimingOperator::Pmos, None),
                fall: TimingExpr::operation(
                    TimingOperator::Add,
                    vec![
                        elmore("112", TimingOperator::Pmos, Some("2")),
                        elmore("L_y", TimingOperator::Nmos, None),
                    ],
                )
                .unwrap(),
            },
        ),
    ];
    for (target, delay) in expected {
        let assignment = assignments(model)
            .into_iter()
            .find(|assignment| assignment.target == target)
            .unwrap_or_else(|| panic!("missing ao21 assignment {target}"));
        assert_eq!(
            assignment.delay, delay,
            "{target} must retain only its source prefix before the shared output suffix"
        );
    }
}

fn assignments(model: &LoweredDecomposedTimingModel) -> Vec<&sv_to_sexpr::ir::Assignment> {
    model
        .lowered()
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect()
}

fn contains_subtract(expr: &Expr) -> bool {
    match expr {
        Expr::Atom(_) => false,
        Expr::List(items) => {
            matches!(items.first(), Some(Expr::Atom(operator)) if operator == TimingOperator::Subtract.as_str())
                || items.iter().any(contains_subtract)
        }
    }
}

fn parse_repository_source(logical_path: &str) -> sv_to_sexpr::ast::Design {
    let path = repository_root().join(logical_path);
    let input = fs::read_to_string(&path).unwrap();
    parse_file(Path::new(logical_path), &input)
        .unwrap_or_else(|error| panic!("failed to parse {logical_path}: {error}"))
}

fn assert_or_update_golden(name: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing_decomposition")
        .join(format!("{name}.cell"));
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual, expected,
        "timing decomposition fixture changed: {name}"
    );
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
