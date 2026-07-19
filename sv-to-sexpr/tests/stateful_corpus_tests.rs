#[allow(dead_code)]
mod stateful_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use stateful_support::{assert_or_update_fixture, repository_root};
use sv_to_sexpr::analyze::{
    AnalysisReport, AssignmentAnalysis, AssignmentKind, GenerateAlternativeAnalysis,
    ModuleAnalysis, ScopeAnalysis, TargetMilestone, analyze_design_structural,
};
use sv_to_sexpr::ast::{AssignOp, ExprKind, Item, ItemKind};
use sv_to_sexpr::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticPolicy, Span};
use sv_to_sexpr::elaborate::{GenerateMode, elaborate_design};
use sv_to_sexpr::ir::{
    Assignment, Cell, CellItem, DelayTuple, Expr, LogicValue, LoweredModule, ValueOperator,
};
use sv_to_sexpr::lower::lower_file;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::collect_sv_files;

const STATEFUL_PATHS: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/dffr.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc_q.sv",
    "sv-cells/dmg_cpu_b/cells/dffsr.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee_q.sv",
    "sv-cells/dmg_cpu_b/cells/drlatch_ee.sv",
    "sv-cells/dmg_cpu_b/cells/nand_latch.sv",
    "sv-cells/dmg_cpu_b/cells/nor_latch.sv",
    "sv-cells/dmg_cpu_b/cells/pad_bidir_pu_latch.sv",
    "sv-cells/dmg_cpu_b/cells/tffnl.sv",
    "sv-cells/sm83/cells/dff_cc_ee_pch_d_reg_sp_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_n_reg_wz_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x1_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x2_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_q.sv",
    "sv-cells/sm83/cells/dff_cc_q_alt.sv",
    "sv-cells/sm83/cells/dffn_ee_pch_d_alu_flag.sv",
    "sv-cells/sm83/cells/dffn_ee_q_alu_sign.sv",
    "sv-cells/sm83/cells/dffr_cc_ee_reg_ie_bit.sv",
    "sv-cells/sm83/cells/dffre_cc_q.sv",
    "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/dlatch_ee_q_n.sv",
    "sv-cells/sm83/cells/srlatch_r_n.sv",
    "sv-cells/sm83/cells/srlatch_r_n_alt.sv",
];

const SUCCESSFUL_STATEFUL_PATHS: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/dffr.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc_q.sv",
    "sv-cells/dmg_cpu_b/cells/dffsr.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee_q.sv",
    "sv-cells/dmg_cpu_b/cells/drlatch_ee.sv",
    "sv-cells/dmg_cpu_b/cells/nand_latch.sv",
    "sv-cells/dmg_cpu_b/cells/nor_latch.sv",
    "sv-cells/dmg_cpu_b/cells/pad_bidir_pu_latch.sv",
    "sv-cells/dmg_cpu_b/cells/tffnl.sv",
    "sv-cells/sm83/cells/dff_cc_ee_pch_d_reg_sp_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_n_reg_wz_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x1_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x2_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_q.sv",
    "sv-cells/sm83/cells/dff_cc_q_alt.sv",
    "sv-cells/sm83/cells/dffn_ee_pch_d_alu_flag.sv",
    "sv-cells/sm83/cells/dffn_ee_q_alu_sign.sv",
    "sv-cells/sm83/cells/dffr_cc_ee_reg_ie_bit.sv",
    "sv-cells/sm83/cells/dffre_cc_q.sv",
    "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/dlatch_ee_q_n.sv",
    "sv-cells/sm83/cells/srlatch_r_n.sv",
    "sv-cells/sm83/cells/srlatch_r_n_alt.sv",
];

const SUCCESSFUL_FLAT_DFF_DLATCH_PATHS: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/dffr.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
    "sv-cells/dmg_cpu_b/cells/dffr_cc_q.sv",
    "sv-cells/dmg_cpu_b/cells/dffsr.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee.sv",
    "sv-cells/dmg_cpu_b/cells/dlatch_ee_q.sv",
    "sv-cells/sm83/cells/dff_cc_ee_pch_d_reg_sp_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_n_reg_wz_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x1_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_ee_q_x2_reg_bit.sv",
    "sv-cells/sm83/cells/dff_cc_q.sv",
    "sv-cells/sm83/cells/dff_cc_q_alt.sv",
    "sv-cells/sm83/cells/dffn_ee_pch_d_alu_flag.sv",
    "sv-cells/sm83/cells/dffn_ee_q_alu_sign.sv",
    "sv-cells/sm83/cells/dffr_cc_ee_reg_ie_bit.sv",
    "sv-cells/sm83/cells/dffre_cc_q.sv",
    "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/dlatch_ee_q_n.sv",
];

const LATER_DRIVER_DEFERRALS: &[&str] = &[];

struct ExpectedSuccess {
    path: &'static str,
    registers: &'static [&'static str],
    state_targets: &'static [&'static str],
    initial: LogicValue,
}

const EXPECTED_SUCCESSES: &[ExpectedSuccess] = &[
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dffr.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dffr_cc.sv",
        registers: &["mux1", "mux2"],
        state_targets: &["mux1", "mux2"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dffr_cc_q.sv",
        registers: &["mux1", "mux2"],
        state_targets: &["mux1", "mux2"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dffsr.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dlatch.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dlatch_ee.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/dlatch_ee_q.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/drlatch_ee.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/nand_latch.sv",
        registers: &["q", "q_n"],
        state_targets: &["q", "q_n"],
        initial: LogicValue::X,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/nor_latch.sv",
        registers: &["q", "q_n"],
        state_targets: &["q", "q_n"],
        initial: LogicValue::X,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/pad_bidir_pu_latch.sv",
        registers: &["ff"],
        state_targets: &["ff"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/dmg_cpu_b/cells/tffnl.sv",
        registers: &["ff", "q"],
        state_targets: &["q", "ff"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_ee_pch_d_reg_sp_bit.sv",
        registers: &["ff1", "ff2", "q_n"],
        state_targets: &["ff1", "ff2", "q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_ee_q_n_reg_wz_bit.sv",
        registers: &["ff", "q_n"],
        state_targets: &["ff", "q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_ee_q_x1_reg_bit.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_ee_q_x2_reg_bit.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_q.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dff_cc_q_alt.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dffn_ee_pch_d_alu_flag.sv",
        registers: &["ff1", "ff2", "q_n"],
        state_targets: &["ff1", "ff2", "q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dffn_ee_q_alu_sign.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dffr_cc_ee_reg_ie_bit.sv",
        registers: &["ff1", "ff2", "q_n"],
        state_targets: &["ff1", "ff2", "q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dffre_cc_q.sv",
        registers: &["ff", "q"],
        state_targets: &["ff", "q"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        registers: &["ff1", "ff2", "q_n"],
        state_targets: &["ff1", "ff2", "q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dlatch_ee_irq.sv",
        registers: &["q_n"],
        state_targets: &["q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/dlatch_ee_q_n.sv",
        registers: &["q_n"],
        state_targets: &["q_n"],
        initial: LogicValue::Zero,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/srlatch_r_n.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::X,
    },
    ExpectedSuccess {
        path: "sv-cells/sm83/cells/srlatch_r_n_alt.sv",
        registers: &["q"],
        state_targets: &["q"],
        initial: LogicValue::X,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeferralCategory {
    LaterDriver,
}

impl DeferralCategory {
    fn label(self) -> &'static str {
        match self {
            Self::LaterDriver => "later-driver-state",
        }
    }
}

struct DeferredExpectation {
    path: &'static str,
    line: usize,
    column: usize,
    message: &'static str,
    category: DeferralCategory,
    rationale: &'static str,
}

const DEFERRED_EXPECTATIONS: &[DeferredExpectation] = &[];

#[derive(Default)]
struct AuditTotals {
    corpus_files: usize,
    stateful_files: usize,
    succeeded: usize,
    deferred: usize,
    recursive_modeled_registers: usize,
    recursive_state_assignments: usize,
    blocking_state_assignments: usize,
    nonblocking_state_assignments: usize,
    successful_modeled_registers: usize,
    successful_state_assignments: usize,
    retained_muxes: usize,
    direct_state_assignments: usize,
    successful_explicit_initializers: usize,
    initial_zero: usize,
    initial_one: usize,
    initial_x: usize,
    initial_z: usize,
    successful_delay_tuple_omissions: usize,
    successful_specify_ignores: usize,
    combinational_procedural_nonregisters: usize,
    invalid_cells: usize,
    nested_state_values: usize,
    register_mismatches: usize,
    state_target_order_mismatches: usize,
    wrong_retention_operands: usize,
    nonzero_state_delays: usize,
    initializer_metadata_mismatches: usize,
    delay_diagnostic_mismatches: usize,
    temp_dependency_or_collision_failures: usize,
    combinational_procedural_register_leaks: usize,
    nondeterministic_results: usize,
    absolute_path_leaks: usize,
}

#[derive(Default)]
struct RecursiveFileAudit {
    modeled_registers: usize,
    modeled_register_names: BTreeSet<String>,
    state_assignment_spans: BTreeSet<(usize, usize)>,
    state_assignments: usize,
    initial_assignments: usize,
    combinational_procedural_assignments: usize,
    combinational_procedural_targets: Vec<String>,
    combinational_register_leaks: usize,
    total_procedural_assignments: usize,
}

#[derive(Clone, Copy)]
struct SourceProcedure {
    op: AssignOp,
    conditional: bool,
}

struct SuccessRecord {
    path: String,
    registers: Vec<(String, LogicValue)>,
    state_targets: Vec<String>,
}

struct DeferralRecord {
    path: String,
    diagnostic: Diagnostic,
    category: DeferralCategory,
    rationale: &'static str,
}

#[test]
fn complete_stateful_corpus_is_flat_or_explicitly_deferred() {
    assert_sorted_unique(STATEFUL_PATHS);
    assert_sorted_unique(SUCCESSFUL_STATEFUL_PATHS);
    assert_sorted_unique(SUCCESSFUL_FLAT_DFF_DLATCH_PATHS);

    let root = repository_root();
    let absolute_root = root.to_string_lossy().to_string();
    let paths = collect_sv_files(&root.join("sv-cells"))
        .unwrap()
        .into_iter()
        .map(|path| logical_path(&root, &path))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 206);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let mut totals = AuditTotals {
        corpus_files: paths.len(),
        ..AuditTotals::default()
    };
    let mut derived_stateful_paths = Vec::new();
    let mut success_records = Vec::new();
    let mut deferral_records = Vec::new();

    for path in &paths {
        let input = fs::read_to_string(root.join(path)).unwrap();
        let design = parse_file(Path::new(path), &input).unwrap();
        let selected = elaborate_design(&design, GenerateMode::Delayful).unwrap();
        let analysis = analyze_design_structural(&selected);
        assert_eq!(analysis.modules.len(), 1, "{path} must contain one module");
        let module = &analysis.modules[0];

        let mut recursive = RecursiveFileAudit::default();
        collect_module_audit(module, &mut recursive);
        recursive.combinational_register_leaks = recursive
            .combinational_procedural_targets
            .iter()
            .filter(|target| recursive.modeled_register_names.contains(*target))
            .count();
        totals.combinational_procedural_nonregisters +=
            recursive.combinational_procedural_assignments;
        totals.combinational_procedural_register_leaks += recursive.combinational_register_leaks;

        let mut source_procedures = BTreeMap::new();
        collect_source_procedures(
            &selected.first_module().unwrap().items,
            false,
            &mut source_procedures,
        );
        assert_eq!(
            source_procedures.len(),
            recursive.total_procedural_assignments,
            "analysis/source procedural count differs in {path}"
        );

        let is_stateful = recursive.modeled_registers > 0 || recursive.state_assignments > 0;
        if !is_stateful {
            continue;
        }
        derived_stateful_paths.push(path.clone());
        totals.stateful_files += 1;
        totals.recursive_modeled_registers += recursive.modeled_registers;
        totals.recursive_state_assignments += recursive.state_assignments;
        for span in &recursive.state_assignment_spans {
            let source = source_procedures.get(span).unwrap_or_else(|| {
                panic!("missing source procedure at {path}:{}:{}", span.0, span.1)
            });
            match source.op {
                AssignOp::Blocking => totals.blocking_state_assignments += 1,
                AssignOp::NonBlocking => totals.nonblocking_state_assignments += 1,
            }
        }

        let first = lower_file(Path::new(path), &input);
        let second = lower_file(Path::new(path), &input);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                totals.succeeded += 1;
                if first != second || render_cell(&first.cell) != render_cell(&second.cell) {
                    totals.nondeterministic_results += 1;
                }
                let record = audit_success(
                    path,
                    &selected.first_module().unwrap().items,
                    module,
                    &recursive,
                    &source_procedures,
                    &first,
                    &absolute_root,
                    &mut totals,
                );
                success_records.push(record);
            }
            (Err(first), Err(second)) => {
                totals.deferred += 1;
                if first != second {
                    totals.nondeterministic_results += 1;
                }
                let expectation = deferred_expectation(path, &first);
                assert_deferral_requirements(&analysis, expectation);
                deferral_records.push(DeferralRecord {
                    path: path.clone(),
                    diagnostic: first,
                    category: expectation.category,
                    rationale: expectation.rationale,
                });
            }
            (first, second) => panic!(
                "nondeterministic success/failure for {path}: first={first:?} second={second:?}"
            ),
        }
    }

    assert_eq!(derived_stateful_paths, STATEFUL_PATHS);
    assert_eq!(
        success_records
            .iter()
            .map(|record| record.path.as_str())
            .collect::<Vec<_>>(),
        SUCCESSFUL_STATEFUL_PATHS
    );
    assert_success_records(&success_records);
    assert_eq!(
        deferral_records
            .iter()
            .map(|record| record.path.as_str())
            .collect::<Vec<_>>(),
        DEFERRED_EXPECTATIONS
            .iter()
            .map(|expectation| expectation.path)
            .collect::<Vec<_>>()
    );
    assert_exact_filename_families(&paths, &success_records, &deferral_records);
    assert_zero_invariant_failures(&totals);

    let summary = render_summary(&totals, &success_records, &deferral_records);
    assert!(!summary.contains(&absolute_root));
    assert_or_update_fixture("corpus_summary", "stateful", &summary);
}

fn collect_module_audit(module: &ModuleAnalysis, audit: &mut RecursiveFileAudit) {
    collect_scope_parts(
        &module.registers,
        &module.initial_assignments,
        &module.procedural_assignments,
        &module.generate_alternatives,
        audit,
    );
}

fn collect_scope_audit(scope: &ScopeAnalysis, audit: &mut RecursiveFileAudit) {
    collect_scope_parts(
        &scope.registers,
        &scope.initial_assignments,
        &scope.procedural_assignments,
        &scope.generate_alternatives,
        audit,
    );
}

fn collect_scope_parts(
    registers: &[String],
    initials: &[AssignmentAnalysis],
    procedures: &[AssignmentAnalysis],
    alternatives: &[GenerateAlternativeAnalysis],
    audit: &mut RecursiveFileAudit,
) {
    audit.modeled_registers += registers.len();
    audit
        .modeled_register_names
        .extend(registers.iter().cloned());
    audit.initial_assignments += initials.len();
    for assignment in procedures {
        audit.total_procedural_assignments += 1;
        match assignment.kind {
            AssignmentKind::Procedural { state: true, .. } => {
                audit.state_assignments += 1;
                assert!(
                    audit
                        .state_assignment_spans
                        .insert((assignment.span.line, assignment.span.column))
                );
            }
            AssignmentKind::Procedural { state: false, .. } => {
                audit.combinational_procedural_assignments += 1;
                audit
                    .combinational_procedural_targets
                    .push(assignment.target.clone());
            }
            AssignmentKind::Continuous | AssignmentKind::Initial => {
                panic!("procedural analysis list contained a non-procedural assignment")
            }
        }
    }
    for alternative in alternatives {
        collect_scope_audit(&alternative.then_branch, audit);
        if let Some(else_branch) = &alternative.else_branch {
            collect_scope_audit(else_branch, audit);
        }
    }
}

fn collect_source_procedures(
    items: &[Item],
    conditional: bool,
    output: &mut BTreeMap<(usize, usize), SourceProcedure>,
) {
    for item in items {
        match &item.kind {
            ItemKind::ProcAssign(statement) => {
                assert!(
                    output
                        .insert(
                            (statement.span.line, statement.span.column),
                            SourceProcedure {
                                op: statement.op,
                                conditional,
                            },
                        )
                        .is_none()
                );
            }
            ItemKind::AlwaysLatch(always) => collect_source_procedures(
                std::slice::from_ref(always.body.as_ref()),
                always.condition.is_some(),
                output,
            ),
            ItemKind::Always(always) => {
                collect_source_procedures(std::slice::from_ref(always.body.as_ref()), false, output)
            }
            ItemKind::Block(block) | ItemKind::Generate(block) => {
                collect_source_procedures(&block.items, conditional, output)
            }
            ItemKind::If(statement) => {
                collect_source_procedures(
                    std::slice::from_ref(statement.then_branch.as_ref()),
                    true,
                    output,
                );
                if let Some(else_branch) = &statement.else_branch {
                    collect_source_procedures(
                        std::slice::from_ref(else_branch.as_ref()),
                        true,
                        output,
                    );
                }
            }
            ItemKind::Import(_)
            | ItemKind::Decl(_)
            | ItemKind::Initial(_)
            | ItemKind::Assign(_)
            | ItemKind::Primitive(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Specify(_)
            | ItemKind::Empty => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_success(
    path: &str,
    source_items: &[Item],
    analysis: &ModuleAnalysis,
    recursive: &RecursiveFileAudit,
    source_procedures: &BTreeMap<(usize, usize), SourceProcedure>,
    lowered: &LoweredModule,
    absolute_root: &str,
    totals: &mut AuditTotals,
) -> SuccessRecord {
    if lowered.cell.validate().is_err() {
        totals.invalid_cells += 1;
    }
    let lowered_register_names = lowered
        .cell
        .registers
        .iter()
        .map(|register| register.name.as_str())
        .collect::<Vec<_>>();
    if lowered_register_names != analysis.registers {
        totals.register_mismatches += 1;
    }
    totals.successful_modeled_registers += lowered.cell.registers.len();

    let assignments = assignments(&lowered.cell);
    audit_flat_values(&assignments, totals);
    audit_temporary_dependencies(&assignments, analysis, totals);

    let source_state = analysis
        .procedural_assignments
        .iter()
        .filter(|assignment| {
            matches!(
                assignment.kind,
                AssignmentKind::Procedural { state: true, .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recursive.state_assignments,
        source_state.len(),
        "successful stateful file unexpectedly retained branch-local state: {path}"
    );
    let registers = lowered
        .cell
        .registers
        .iter()
        .map(|register| register.name.as_str())
        .collect::<BTreeSet<_>>();
    let emitted_state = assignments
        .iter()
        .copied()
        .filter(|assignment| registers.contains(assignment.target.as_str()))
        .collect::<Vec<_>>();
    let source_targets = source_state
        .iter()
        .map(|assignment| assignment.target.as_str())
        .collect::<Vec<_>>();
    let emitted_targets = emitted_state
        .iter()
        .map(|assignment| assignment.target.as_str())
        .collect::<Vec<_>>();
    if source_targets != emitted_targets {
        totals.state_target_order_mismatches += 1;
    }
    totals.successful_state_assignments += emitted_state.len();

    for (source, emitted) in source_state.iter().zip(emitted_state.iter()) {
        let info = source_procedures
            .get(&(source.span.line, source.span.column))
            .unwrap();
        if info.conditional {
            match value_operation(&emitted.expr) {
                Some((ValueOperator::Mux, [Expr::Atom(_), Expr::Atom(_), Expr::Atom(old)]))
                    if old == &emitted.target =>
                {
                    totals.retained_muxes += 1;
                }
                _ => totals.wrong_retention_operands += 1,
            }
        } else {
            totals.direct_state_assignments += 1;
        }
        if !is_zero_delay(&emitted.delay) {
            totals.nonzero_state_delays += 1;
        }
    }

    let initial_items = collect_initial_items(source_items);
    let initial_targets = initial_items.iter().cloned().collect::<BTreeSet<_>>();
    if initial_items.len() != recursive.initial_assignments
        || initial_items.len() != initial_targets.len()
        || lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("literal initial value/event"))
    {
        totals.initializer_metadata_mismatches += 1;
    }
    totals.successful_explicit_initializers += initial_items.len();
    for register in &lowered.cell.registers {
        match register.initial {
            LogicValue::Zero => totals.initial_zero += 1,
            LogicValue::One => totals.initial_one += 1,
            LogicValue::X => totals.initial_x += 1,
            LogicValue::Z => totals.initial_z += 1,
        }
        let expected = if initial_targets.contains(&register.name) {
            LogicValue::Zero
        } else {
            LogicValue::X
        };
        if register.initial != expected {
            totals.initializer_metadata_mismatches += 1;
        }
    }

    let delay_diagnostics = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.starts_with("delay tuple entry "))
        .collect::<Vec<_>>();
    if !delay_diagnostics.is_empty() {
        totals.delay_diagnostic_mismatches += 1;
    }
    totals.successful_delay_tuple_omissions += delay_diagnostics.len();
    let specify_ignores = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .message
                .starts_with("additional control-dependent specify path for target `")
        })
        .collect::<Vec<_>>();
    if specify_ignores.iter().any(|diagnostic| {
        diagnostic.kind != DiagnosticKind::IntentionalIgnore
            || DiagnosticPolicy::new(true).is_failure(diagnostic)
    }) || delay_diagnostics.len() + specify_ignores.len() != lowered.diagnostics.len()
    {
        totals.delay_diagnostic_mismatches += 1;
    }
    totals.successful_specify_ignores += specify_ignores.len();

    let serialized = render_cell(&lowered.cell);
    if serialized.contains(absolute_root)
        || lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.span.path.is_absolute())
    {
        totals.absolute_path_leaks += 1;
    }

    SuccessRecord {
        path: path.to_string(),
        registers: lowered
            .cell
            .registers
            .iter()
            .map(|register| (register.name.clone(), register.initial))
            .collect(),
        state_targets: emitted_targets.into_iter().map(str::to_string).collect(),
    }
}

fn is_zero_delay(delay: &DelayTuple) -> bool {
    delay.len() == 1 && delay.first().as_expr() == &Expr::atom("0")
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
    let (Expr::Atom(head), operands) = items.split_first()? else {
        return None;
    };
    ValueOperator::parse(head).map(|operator| (operator, operands))
}

fn audit_flat_values(assignments: &[&Assignment], totals: &mut AuditTotals) {
    for assignment in assignments {
        if let Expr::List(items) = &assignment.expr {
            let Some((Expr::Atom(head), operands)) = items.split_first() else {
                totals.nested_state_values += 1;
                continue;
            };
            let Some(operator) = ValueOperator::parse(head) else {
                totals.nested_state_values += 1;
                continue;
            };
            if (operands.is_empty() && operator != ValueOperator::Keeper)
                || operands
                    .iter()
                    .any(|operand| !matches!(operand, Expr::Atom(atom) if !atom.is_empty()))
                || (operator == ValueOperator::Mux && operands.len() != 3)
            {
                totals.nested_state_values += 1;
            }
        }
    }
}

fn audit_temporary_dependencies(
    assignments: &[&Assignment],
    analysis: &ModuleAnalysis,
    totals: &mut AuditTotals,
) {
    let source_symbols = analysis.symbols.keys().cloned().collect::<BTreeSet<_>>();
    for source_name in source_symbols
        .iter()
        .filter(|name| temp_index(name).is_some())
    {
        let emitted_assignments = assignments
            .iter()
            .filter(|assignment| assignment.target == *source_name)
            .count();
        let source_drivers = analysis
            .drivers
            .iter()
            .filter(|driver| driver.target == *source_name)
            .count();
        if emitted_assignments != source_drivers {
            totals.temp_dependency_or_collision_failures += 1;
        }
    }
    let generated = assignments
        .iter()
        .filter(|assignment| {
            temp_index(&assignment.target).is_some() && !source_symbols.contains(&assignment.target)
        })
        .map(|assignment| assignment.target.clone())
        .collect::<BTreeSet<_>>();
    let mut available = BTreeSet::new();
    let mut expected_index = 0;
    for assignment in assignments {
        if let Some((_, operands)) = value_operation(&assignment.expr) {
            for operand in operands {
                let Expr::Atom(atom) = operand else {
                    continue;
                };
                if generated.contains(atom) && !available.contains(atom) {
                    totals.temp_dependency_or_collision_failures += 1;
                }
            }
        }
        if generated.contains(&assignment.target) {
            let actual_index = temp_index(&assignment.target).unwrap();
            while expected_index < actual_index {
                if !source_symbols.contains(&format!("t{expected_index}")) {
                    totals.temp_dependency_or_collision_failures += 1;
                }
                expected_index += 1;
            }
            if actual_index != expected_index || !available.insert(assignment.target.clone()) {
                totals.temp_dependency_or_collision_failures += 1;
            }
            expected_index += 1;
        }
    }
}

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

fn collect_initial_items(items: &[Item]) -> Vec<String> {
    fn collect(items: &[Item], output: &mut Vec<String>) {
        for item in items {
            match &item.kind {
                ItemKind::Initial(statement) => {
                    let ExprKind::Path(segments) = &statement.target.kind else {
                        panic!("successful initial target must be scalar")
                    };
                    assert_eq!(segments.len(), 1);
                    output.push(segments[0].clone());
                }
                ItemKind::Block(block) | ItemKind::Generate(block) => collect(&block.items, output),
                ItemKind::If(statement) => {
                    collect(std::slice::from_ref(statement.then_branch.as_ref()), output);
                    if let Some(else_branch) = &statement.else_branch {
                        collect(std::slice::from_ref(else_branch.as_ref()), output);
                    }
                }
                ItemKind::AlwaysLatch(always) => {
                    collect(std::slice::from_ref(always.body.as_ref()), output)
                }
                ItemKind::Always(always) => {
                    collect(std::slice::from_ref(always.body.as_ref()), output)
                }
                ItemKind::Import(_)
                | ItemKind::Decl(_)
                | ItemKind::ProcAssign(_)
                | ItemKind::Assign(_)
                | ItemKind::Primitive(_)
                | ItemKind::Instantiation(_)
                | ItemKind::Specify(_)
                | ItemKind::Empty => {}
            }
        }
    }
    let mut output = Vec::new();
    collect(items, &mut output);
    output
}

fn deferred_expectation(path: &str, diagnostic: &Diagnostic) -> &'static DeferredExpectation {
    let matches = DEFERRED_EXPECTATIONS
        .iter()
        .filter(|expectation| expectation.path == path)
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "stateful whole-file failure must have exactly one explicit category: {path}: {diagnostic}"
    );
    let expectation = matches[0];
    assert_eq!(
        diagnostic.span,
        Span::new(path, expectation.line, expectation.column)
    );
    assert_eq!(diagnostic.message, expectation.message);
    expectation
}

fn assert_deferral_requirements(report: &AnalysisReport, expectation: &DeferredExpectation) {
    let has = |capability: &str, milestone: TargetMilestone| {
        report.requirements.iter().any(|requirement| {
            requirement.capability_id == capability && requirement.milestone == milestone
        })
    };
    match expectation.category {
        DeferralCategory::LaterDriver => {
            for (capability, milestone) in [
                (
                    "driver.primitive-tristate",
                    TargetMilestone::M6DriversAndStrength,
                ),
                ("driver.repeated", TargetMilestone::M6DriversAndStrength),
                ("timing.alias", TargetMilestone::M7SymbolicTiming),
                ("timing.specify-path", TargetMilestone::M7SymbolicTiming),
            ] {
                assert!(
                    has(capability, milestone),
                    "{} is missing {capability} {}",
                    expectation.path,
                    milestone.label()
                );
            }
        }
    }
}

fn assert_exact_filename_families(
    all_paths: &[String],
    successes: &[SuccessRecord],
    deferrals: &[DeferralRecord],
) {
    let flat_successes = successes
        .iter()
        .filter(|record| {
            let name = Path::new(&record.path)
                .file_name()
                .unwrap()
                .to_string_lossy();
            name.starts_with("dff") || name.starts_with("dlatch")
        })
        .map(|record| record.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(flat_successes, SUCCESSFUL_FLAT_DFF_DLATCH_PATHS);

    let later_driver = deferrals
        .iter()
        .filter(|record| record.category == DeferralCategory::LaterDriver)
        .map(|record| record.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(later_driver, LATER_DRIVER_DEFERRALS);

    let all_dff_dlatch = all_paths
        .iter()
        .filter(|path| {
            let name = Path::new(path).file_name().unwrap().to_string_lossy();
            name.starts_with("dff") || name.starts_with("dlatch")
        })
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let accounted = SUCCESSFUL_FLAT_DFF_DLATCH_PATHS
        .iter()
        .chain(LATER_DRIVER_DEFERRALS)
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(all_dff_dlatch, accounted);
}

fn assert_success_records(records: &[SuccessRecord]) {
    assert_eq!(records.len(), EXPECTED_SUCCESSES.len());
    for (actual, expected) in records.iter().zip(EXPECTED_SUCCESSES) {
        assert_eq!(actual.path, expected.path);
        assert_eq!(
            actual
                .registers
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            expected.registers
        );
        assert!(
            actual
                .registers
                .iter()
                .all(|(_, initial)| *initial == expected.initial),
            "{}",
            actual.path
        );
        assert_eq!(actual.state_targets, expected.state_targets);
    }
}

fn assert_zero_invariant_failures(totals: &AuditTotals) {
    assert_eq!(totals.corpus_files, 206);
    assert_eq!(totals.stateful_files, 27);
    assert_eq!(totals.succeeded, 27);
    assert_eq!(totals.deferred, 0);
    assert_eq!(totals.recursive_modeled_registers, 48);
    assert_eq!(totals.recursive_state_assignments, 48);
    assert_eq!(totals.blocking_state_assignments, 17);
    assert_eq!(totals.nonblocking_state_assignments, 31);
    assert_eq!(totals.successful_modeled_registers, 48);
    assert_eq!(totals.successful_state_assignments, 48);
    assert_eq!(totals.retained_muxes, 47);
    assert_eq!(totals.direct_state_assignments, 1);
    assert_eq!(totals.successful_explicit_initializers, 42);
    assert_eq!(totals.initial_zero, 42);
    assert_eq!(totals.initial_one, 0);
    assert_eq!(totals.initial_x, 6);
    assert_eq!(totals.initial_z, 0);
    assert_eq!(totals.successful_delay_tuple_omissions, 0);
    assert_eq!(totals.successful_specify_ignores, 15);
    assert_eq!(totals.nonzero_state_delays, 26);
    assert_eq!(totals.combinational_procedural_nonregisters, 0);
    for (name, value) in invariant_failures(totals) {
        assert_eq!(value, 0, "stateful invariant failed: {name}");
    }
}

fn invariant_failures(totals: &AuditTotals) -> [(&'static str, usize); 11] {
    [
        ("invalid_cells", totals.invalid_cells),
        ("nested_state_values", totals.nested_state_values),
        ("register_mismatches", totals.register_mismatches),
        (
            "state_target_order_mismatches",
            totals.state_target_order_mismatches,
        ),
        ("wrong_retention_operands", totals.wrong_retention_operands),
        (
            "initializer_metadata_mismatches",
            totals.initializer_metadata_mismatches,
        ),
        (
            "delay_diagnostic_mismatches",
            totals.delay_diagnostic_mismatches,
        ),
        (
            "temp_dependency_or_collision_failures",
            totals.temp_dependency_or_collision_failures,
        ),
        (
            "combinational_procedural_register_leaks",
            totals.combinational_procedural_register_leaks,
        ),
        ("nondeterministic_results", totals.nondeterministic_results),
        ("absolute_path_leaks", totals.absolute_path_leaks),
    ]
}

fn render_summary(
    totals: &AuditTotals,
    successes: &[SuccessRecord],
    deferrals: &[DeferralRecord],
) -> String {
    let mut output = String::new();
    writeln!(&mut output, "stateful corpus audit").unwrap();
    writeln!(&mut output, "corpus-files={}", totals.corpus_files).unwrap();
    writeln!(&mut output, "stateful-files={}", totals.stateful_files).unwrap();
    writeln!(
        &mut output,
        "stateful-lowering: succeeded={} deferred={}",
        totals.succeeded, totals.deferred
    )
    .unwrap();
    writeln!(&mut output, "recursive-analysis:").unwrap();
    for (name, value) in [
        ("modeled-registers", totals.recursive_modeled_registers),
        ("state-assignments", totals.recursive_state_assignments),
        (
            "combinational-procedural-nonregisters",
            totals.combinational_procedural_nonregisters,
        ),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "source-state-syntax:").unwrap();
    writeln!(
        &mut output,
        "  blocking={}",
        totals.blocking_state_assignments
    )
    .unwrap();
    writeln!(
        &mut output,
        "  nonblocking={}",
        totals.nonblocking_state_assignments
    )
    .unwrap();
    writeln!(&mut output, "successful-topology:").unwrap();
    for (name, value) in [
        ("modeled-registers", totals.successful_modeled_registers),
        ("state-assignments", totals.successful_state_assignments),
        ("retained-muxes", totals.retained_muxes),
        ("direct-state-assignments", totals.direct_state_assignments),
        (
            "explicit-register-initializers",
            totals.successful_explicit_initializers,
        ),
        ("initial-zero", totals.initial_zero),
        ("initial-one", totals.initial_one),
        ("initial-x", totals.initial_x),
        ("initial-z", totals.initial_z),
        (
            "delay-tuple-intentional-ignores",
            totals.successful_delay_tuple_omissions,
        ),
        (
            "specify-intentional-ignores",
            totals.successful_specify_ignores,
        ),
        ("modeled-state-delays", totals.nonzero_state_delays),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "invariant-failures:").unwrap();
    for (name, value) in invariant_failures(totals) {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "successful-stateful-files:").unwrap();
    for success in successes {
        writeln!(
            &mut output,
            "  {} | registers=[{}] | state-targets=[{}]",
            success.path,
            success
                .registers
                .iter()
                .map(|(name, initial)| format!("{name}={}", initial.as_str()))
                .collect::<Vec<_>>()
                .join(","),
            success.state_targets.join(","),
        )
        .unwrap();
    }
    writeln!(&mut output, "successful-flat-dff-dlatch-files:").unwrap();
    for path in SUCCESSFUL_FLAT_DFF_DLATCH_PATHS {
        writeln!(&mut output, "  {path}").unwrap();
    }
    writeln!(&mut output, "whole-file-deferrals:").unwrap();
    for deferral in deferrals {
        writeln!(
            &mut output,
            "  {}:{}:{} | {} | category={} | rationale={}",
            deferral.path,
            deferral.diagnostic.span.line,
            deferral.diagnostic.span.column,
            deferral.diagnostic.message,
            deferral.category.label(),
            deferral.rationale
        )
        .unwrap();
    }
    output
}

fn assert_sorted_unique(values: &[&str]) {
    assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
}

fn logical_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
