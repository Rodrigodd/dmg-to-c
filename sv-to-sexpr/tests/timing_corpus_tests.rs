use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sv_to_sexpr::analyze::{ModuleCatalog, analyze_design_structural};
use sv_to_sexpr::ast::{
    BinaryOp, ConstKind, Delay, Design, Expr as SvExpr, ExprKind, Item, ItemKind, ParamKind,
    SpecifyItem, UnaryOp,
};
use sv_to_sexpr::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticPolicy, Span};
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::hierarchy::flatten_design_with_catalog_and_generate_mode;
use sv_to_sexpr::ir::{CellItem, Expr, TimingOperator};
use sv_to_sexpr::lower::lower_design_with_catalog_and_generate_mode;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::{render_cell, render_expr};
use sv_to_sexpr::survey::collect_sv_files;

const INITIAL_OMISSION: &str = "literal initial value/event is intentionally omitted because the cell model has no initial event queue";
const DELAY_IGNORE_PREFIX: &str = "delay tuple entry ";
const SPECIFY_WARNING_PREFIX: &str = "multiple control-dependent specify paths target `";
const REFERENCE: &str = "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv";
const PAD_XTAL: &str = "sv-cells/dmg_cpu_b/cells/pad_xtal.sv";

const TRANSISTOR_DEFERRALS: &[&str] = &[
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/idu_bit123456.sv",
    "sv-cells/sm83/cells/irq_prio_bit0.sv",
    "sv-cells/sm83/cells/irq_prio_bit1.sv",
    "sv-cells/sm83/cells/irq_prio_bit2.sv",
    "sv-cells/sm83/cells/irq_prio_bit3.sv",
    "sv-cells/sm83/cells/irq_prio_bit4.sv",
    "sv-cells/sm83/cells/irq_prio_bit5.sv",
    "sv-cells/sm83/cells/irq_prio_bit6.sv",
];
const KEEPER_DEFERRALS: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/mux.sv",
    "sv-cells/dmg_cpu_b/cells/muxi.sv",
    PAD_XTAL,
    "sv-cells/sm83/cells/idu_bit0.sv",
    "sv-cells/sm83/cells/reg_wz_out.sv",
];

#[derive(Default)]
struct TupleInventory {
    arities: [usize; 4],
    other_arities: usize,
    omitted_first: usize,
    omitted_later: usize,
    later_entries: usize,
}

impl TupleInventory {
    fn record(&mut self, values: &[Option<SvExpr>]) {
        if values.len() <= 3 {
            self.arities[values.len()] += 1;
        } else {
            self.other_arities += 1;
        }
        if values.first().is_some_and(Option::is_none) {
            self.omitted_first += 1;
        }
        self.later_entries += values.len().saturating_sub(1);
        self.omitted_later += values
            .iter()
            .skip(1)
            .filter(|value| value.is_none())
            .count();
    }
}

#[derive(Default)]
struct SourceForms {
    aliases: usize,
    tpd_elmore_arities: BTreeMap<usize, usize>,
    tpd_z_arities: BTreeMap<usize, usize>,
    tpd_z_first_present: BTreeMap<usize, usize>,
    resistance_pmos: usize,
    resistance_nmos: usize,
    outer_resistance_integer_mul: usize,
    outer_resistance_real_mul: usize,
    resistance_sums: usize,
    direct_real_resistance: usize,
    real_unit_factors: usize,
    timing_adds: usize,
    timing_gt: usize,
    timing_mux: usize,
    real_device_values: BTreeSet<String>,
}

#[derive(Default)]
struct Witnesses {
    tuple_arity: [BTreeSet<String>; 4],
    precharge_first_rise: BTreeSet<String>,
    first_t_z: BTreeSet<String>,
    multi_tpd_z: BTreeSet<String>,
    outer_mul_2: BTreeSet<String>,
    outer_mul_1_5: BTreeSet<String>,
    resistance_sum: BTreeSet<String>,
    real_device_factor: BTreeSet<String>,
}

#[derive(Default)]
struct SourceInventory {
    files: usize,
    assignment_tuples: TupleInventory,
    primitive_tuples: TupleInventory,
    specify_tuples: TupleInventory,
    forms: SourceForms,
    specify_paths: usize,
    specify_controls: usize,
    specify_targets: BTreeSet<String>,
    specify_first_paths: BTreeSet<String>,
    specify_target_multiplicity: BTreeMap<usize, usize>,
    witnesses: Witnesses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeferralCategory {
    Keeper,
    Transistor,
}

impl DeferralCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Keeper => "M10-keeper",
            Self::Transistor => "M11-transistor",
        }
    }
}

#[derive(Debug)]
struct Deferral {
    path: String,
    category: DeferralCategory,
    diagnostic: Diagnostic,
}

#[derive(Default)]
struct LowerAudit {
    succeeded: usize,
    failed: usize,
    assignments: usize,
    temporaries: usize,
    temp_nonzero_delays: usize,
    source_assignments: usize,
    explicit_delays: usize,
    explicit_nonzero_delays: usize,
    explicit_with_specify_match: usize,
    specify_delays: usize,
    specify_nonzero_delays: usize,
    zero_delays: usize,
    emitted_zero_delays: usize,
    emitted_nonzero_delays: usize,
    emitted_nested_delays: usize,
    warnings: usize,
    later_ignores: usize,
    initial_ignores: usize,
    warning_contract_failures: usize,
    diagnostic_mismatches: usize,
    source_delay_mismatches: usize,
    invalid_cells: usize,
    nondeterministic_results: usize,
    absolute_path_leaks: usize,
    uncontracted_timing: usize,
    operator_counts: BTreeMap<String, usize>,
    expected_outer_resistance_multiplications: usize,
    emitted_outer_resistance_multiplications: usize,
    deferrals: Vec<Deferral>,
}

#[derive(Clone, Copy)]
struct TupleRef<'a> {
    span: &'a Span,
    values: &'a [Option<SvExpr>],
}

struct SourceAssignment<'a> {
    target: String,
    explicit: Option<TupleRef<'a>>,
}

struct SpecifyPathRef<'a> {
    span: &'a Span,
    values: &'a [Option<SvExpr>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedDiagnostic {
    kind: DiagnosticKind,
    span: Span,
    message: String,
}

#[test]
fn complete_timing_corpus_is_structurally_accounted_and_deterministic() {
    let root = repository_root();
    let paths = corpus_paths(&root);
    assert_eq!(paths.len(), 206);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let mut source = SourceInventory::default();
    let mut lower = LowerAudit::default();
    let designs = paths
        .iter()
        .map(|path| {
            let input = fs::read_to_string(root.join(path)).unwrap();
            (path.clone(), parse_file(Path::new(path), &input).unwrap())
        })
        .collect::<BTreeMap<_, _>>();
    let catalog =
        ModuleCatalog::from_designs(&designs.values().cloned().collect::<Vec<_>>()).unwrap();
    for path in &paths {
        let design = &designs[path];
        inventory_design(path, design, &mut source);
        let selected =
            flatten_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful)
                .unwrap();
        audit_lower_result(path, design, &selected, &catalog, &mut lower);
    }
    finalize_source_inventory(&paths, &mut source);
    assert_exact_contract(&source, &lower);
    let summary = render_summary(&source, &lower);
    assert!(!summary.contains(&root.to_string_lossy().to_string()));
    assert_or_update_fixture(&summary);
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn corpus_paths(root: &Path) -> Vec<String> {
    let mut paths = collect_sv_files(&root.join("sv-cells"))
        .unwrap()
        .into_iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn inventory_design(path: &str, design: &Design, inventory: &mut SourceInventory) {
    inventory.files += 1;
    let module = design.first_module().unwrap();
    for parameter in &module.parameters {
        if matches!(parameter.kind, ParamKind::Localparam | ParamKind::Specparam) {
            inventory.forms.aliases += 1;
            inventory_expr(
                path,
                &parameter.value,
                &mut inventory.forms,
                &mut inventory.witnesses,
            );
        }
    }
    inventory_items(path, &module.items, inventory);
}

fn inventory_items(path: &str, items: &[Item], inventory: &mut SourceInventory) {
    for item in items {
        match &item.kind {
            ItemKind::Assign(assign) => {
                if let Some(delay) = &assign.delay {
                    inventory.assignment_tuples.record(&delay.values);
                    record_tuple_witness(path, &delay.values, &mut inventory.witnesses);
                    for expr in delay.values.iter().flatten() {
                        inventory_expr(path, expr, &mut inventory.forms, &mut inventory.witnesses);
                    }
                }
            }
            ItemKind::Primitive(call) => {
                if let Some(delay) = &call.delay {
                    inventory.primitive_tuples.record(&delay.values);
                    record_tuple_witness(path, &delay.values, &mut inventory.witnesses);
                    record_precharge_witness(
                        path,
                        call.name.as_str(),
                        &delay.values,
                        &mut inventory.witnesses,
                    );
                    for expr in delay.values.iter().flatten() {
                        inventory_expr(path, expr, &mut inventory.forms, &mut inventory.witnesses);
                    }
                }
            }
            ItemKind::Decl(decl)
                if matches!(
                    decl.kind,
                    sv_to_sexpr::ast::DeclKind::Localparam | sv_to_sexpr::ast::DeclKind::Specparam
                ) =>
            {
                inventory.forms.aliases += decl.names.len();
                if let Some(value) = &decl.value {
                    inventory_expr(path, value, &mut inventory.forms, &mut inventory.witnesses);
                }
            }
            ItemKind::Specify(specify) => {
                let mut targets = BTreeMap::<String, usize>::new();
                for specify_item in &specify.items {
                    match specify_item {
                        SpecifyItem::Specparam(param) => {
                            inventory.forms.aliases += 1;
                            inventory_expr(
                                path,
                                &param.value,
                                &mut inventory.forms,
                                &mut inventory.witnesses,
                            );
                        }
                        SpecifyItem::Path(spec_path) => {
                            inventory.specify_paths += 1;
                            assert!(
                                spec_path
                                    .controls
                                    .iter()
                                    .all(|control| scalar_symbol(control).is_some()),
                                "non-scalar specify control in {path}"
                            );
                            inventory.specify_controls += spec_path.controls.len();
                            inventory.specify_tuples.record(&spec_path.delays);
                            record_tuple_witness(path, &spec_path.delays, &mut inventory.witnesses);
                            let target = scalar_symbol(&spec_path.target)
                                .unwrap_or_else(|| panic!("non-scalar specify target in {path}"));
                            inventory.specify_targets.insert(format!("{path}:{target}"));
                            let count = targets.entry(target.clone()).or_default();
                            if *count == 0 {
                                inventory.specify_first_paths.insert(format!(
                                    "{path}:{}:{}:{target}",
                                    spec_path.span.line, spec_path.span.column
                                ));
                            }
                            *count += 1;
                            for expr in spec_path.delays.iter().flatten() {
                                inventory_expr(
                                    path,
                                    expr,
                                    &mut inventory.forms,
                                    &mut inventory.witnesses,
                                );
                            }
                        }
                    }
                }
                for multiplicity in targets.into_values() {
                    *inventory
                        .specify_target_multiplicity
                        .entry(multiplicity)
                        .or_default() += 1;
                }
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                inventory_items(path, &block.items, inventory)
            }
            ItemKind::If(if_stmt) => {
                inventory_items(
                    path,
                    std::slice::from_ref(if_stmt.then_branch.as_ref()),
                    inventory,
                );
                if let Some(else_branch) = &if_stmt.else_branch {
                    inventory_items(path, std::slice::from_ref(else_branch.as_ref()), inventory);
                }
            }
            ItemKind::Import(_)
            | ItemKind::Initial(_)
            | ItemKind::ProcAssign(_)
            | ItemKind::AlwaysLatch(_)
            | ItemKind::Always(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Decl(_)
            | ItemKind::Empty => {}
        }
    }
}

fn record_tuple_witness(path: &str, values: &[Option<SvExpr>], witnesses: &mut Witnesses) {
    if values.len() <= 3 {
        witnesses.tuple_arity[values.len()].insert(path.to_string());
    }
    if values.first().and_then(Option::as_ref).is_some_and(|expr| {
        scalar_symbol(expr)
            .as_deref()
            .is_some_and(|name| name.starts_with("T_Z"))
    }) {
        witnesses.first_t_z.insert(path.to_string());
    }
}

fn record_precharge_witness(
    path: &str,
    primitive: &str,
    values: &[Option<SvExpr>],
    witnesses: &mut Witnesses,
) {
    if primitive == "bufif0"
        && values.len() == 3
        && values.first().and_then(Option::as_ref).is_some_and(|expr| {
            scalar_symbol(expr)
                .as_deref()
                .is_some_and(|name| name.starts_with("T_rise"))
        })
        && values.iter().skip(1).flatten().all(|expr| {
            scalar_symbol(expr)
                .as_deref()
                .is_some_and(|name| name.starts_with("T_Z"))
        })
    {
        witnesses.precharge_first_rise.insert(path.to_string());
    }
}

fn inventory_expr(path: &str, expr: &SvExpr, forms: &mut SourceForms, witnesses: &mut Witnesses) {
    match &expr.kind {
        ExprKind::Group(inner) | ExprKind::Unary { expr: inner, .. } => {
            inventory_expr(path, inner, forms, witnesses)
        }
        ExprKind::Binary { op, left, right } => {
            if *op == BinaryOp::Add {
                forms.timing_adds += 1;
                if is_resistance_call(left) && is_resistance_call(right) {
                    forms.resistance_sums += 1;
                    witnesses.resistance_sum.insert(path.to_string());
                }
            }
            if *op == BinaryOp::Greater {
                forms.timing_gt += 1;
            }
            if *op == BinaryOp::Mul {
                let (call, factor) = if is_resistance_call(left) {
                    (Some(left.as_ref()), right.as_ref())
                } else if is_resistance_call(right) {
                    (Some(right.as_ref()), left.as_ref())
                } else {
                    (None, right.as_ref())
                };
                if call.is_some() {
                    match &factor.kind {
                        ExprKind::Integer(value) => {
                            forms.outer_resistance_integer_mul += 1;
                            if value == "2" {
                                witnesses.outer_mul_2.insert(path.to_string());
                            }
                        }
                        ExprKind::Real(value) => {
                            forms.outer_resistance_real_mul += 1;
                            if value == "1.5" {
                                witnesses.outer_mul_1_5.insert(path.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            inventory_expr(path, left, forms, witnesses);
            inventory_expr(path, right, forms, witnesses);
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => {
            forms.timing_mux += 1;
            inventory_expr(path, condition, forms, witnesses);
            inventory_expr(path, then_expr, forms, witnesses);
            inventory_expr(path, else_expr, forms, witnesses);
        }
        ExprKind::Call { callee, args } => {
            let name = scalar_symbol(callee).unwrap_or_default();
            match name.as_str() {
                "tpd_elmore" => {
                    *forms.tpd_elmore_arities.entry(args.len()).or_default() += 1;
                }
                "tpd_z" => {
                    *forms.tpd_z_arities.entry(args.len()).or_default() += 1;
                    if let Some(position) = args.iter().position(Option::is_some) {
                        *forms.tpd_z_first_present.entry(position + 1).or_default() += 1;
                    }
                    if args.len() > 1 {
                        witnesses.multi_tpd_z.insert(path.to_string());
                    }
                }
                "R_pmos_ohm" | "R_nmos_ohm" => {
                    if name == "R_pmos_ohm" {
                        forms.resistance_pmos += 1;
                    } else {
                        forms.resistance_nmos += 1;
                    }
                    if let Some(Some(arg)) = args.first() {
                        match &arg.kind {
                            ExprKind::Real(value) => {
                                forms.direct_real_resistance += 1;
                                forms.real_device_values.insert(value.clone());
                                witnesses.real_device_factor.insert(path.to_string());
                            }
                            ExprKind::Binary {
                                op: BinaryOp::Mul,
                                left,
                                right,
                            } => {
                                let factor = if is_l_unit(left) {
                                    right.as_ref()
                                } else if is_l_unit(right) {
                                    left.as_ref()
                                } else {
                                    arg
                                };
                                if let ExprKind::Real(value) = &factor.kind {
                                    forms.real_unit_factors += 1;
                                    forms.real_device_values.insert(value.clone());
                                    witnesses.real_device_factor.insert(path.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            inventory_expr(path, callee, forms, witnesses);
            for arg in args.iter().flatten() {
                inventory_expr(path, arg, forms, witnesses);
            }
        }
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {}
    }
}

fn finalize_source_inventory(paths: &[String], inventory: &mut SourceInventory) {
    assert_eq!(inventory.files, paths.len());
    assert_eq!(inventory.assignment_tuples.arities[1], 0);
    assert_eq!(inventory.primitive_tuples.arities[1], 0);
    assert_eq!(inventory.specify_tuples.arities[1], 0);
    inventory.witnesses.tuple_arity[1].insert(
        "synthetic-unit:lower::tests::delay_tuples_select_exactly_the_first_entry".to_string(),
    );
    assert!(!inventory.witnesses.tuple_arity[1].is_empty());
    assert!(!inventory.witnesses.tuple_arity[2].is_empty());
    assert!(!inventory.witnesses.tuple_arity[3].is_empty());
    assert!(!inventory.witnesses.precharge_first_rise.is_empty());
    assert!(!inventory.witnesses.first_t_z.is_empty());
    assert!(!inventory.witnesses.multi_tpd_z.is_empty());
    assert!(!inventory.witnesses.outer_mul_2.is_empty());
    assert!(!inventory.witnesses.outer_mul_1_5.is_empty());
    assert!(!inventory.witnesses.resistance_sum.is_empty());
    assert!(inventory.forms.real_device_values.contains("13.5"));
    assert!(inventory.forms.real_device_values.contains("10.8"));
    assert!(inventory.forms.real_device_values.contains("16.5"));
    assert_eq!(
        inventory.specify_first_paths.len(),
        inventory.specify_targets.len()
    );
}

fn audit_lower_result(
    path: &str,
    design: &Design,
    selected: &Design,
    catalog: &ModuleCatalog,
    audit: &mut LowerAudit,
) {
    let first =
        lower_design_with_catalog_and_generate_mode(design, catalog, GenerateMode::Delayful);
    let second =
        lower_design_with_catalog_and_generate_mode(design, catalog, GenerateMode::Delayful);
    match (first, second) {
        (Ok(first), Ok(second)) => {
            audit.succeeded += 1;
            if first != second || render_cell(&first.cell) != render_cell(&second.cell) {
                audit.nondeterministic_results += 1;
            }
            if first.cell.validate().is_err() {
                audit.invalid_cells += 1;
            }
            if render_cell(&first.cell).contains(&repository_root().to_string_lossy().to_string())
                || first
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.span.path.is_absolute())
            {
                audit.absolute_path_leaks += 1;
            }
            audit_success(path, selected, &first, audit);
        }
        (Err(first), Err(second)) => {
            audit.failed += 1;
            if first != second {
                audit.nondeterministic_results += 1;
            }
            let category = deferral_category(path)
                .unwrap_or_else(|| panic!("unexpected M7/lowering deferral {path}: {first}"));
            assert_expected_deferral(category, &first);
            audit.deferrals.push(Deferral {
                path: path.to_string(),
                category,
                diagnostic: first,
            });
        }
        other => panic!("nondeterministic lower disposition for {path}: {other:?}"),
    }
}

fn audit_success(
    path: &str,
    design: &Design,
    lowered: &sv_to_sexpr::ir::LoweredModule,
    audit: &mut LowerAudit,
) {
    let module = design.first_module().unwrap();
    let analysis = analyze_design_structural(design);
    let module_analysis = &analysis.modules[0];
    let source_names = module_analysis
        .symbols
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let aliases = timing_aliases(module);
    let specify = specify_paths(module);
    let mut source_assignments = Vec::new();
    collect_source_assignments(&module.items, &mut source_assignments);
    let emitted = lowered
        .cell
        .items
        .iter()
        .filter_map(|item| match item {
            CellItem::Assignment(assignment) => Some(assignment),
            CellItem::Blank | CellItem::Comment(_) => None,
        })
        .collect::<Vec<_>>();
    let source_emitted = emitted
        .iter()
        .copied()
        .filter(|assignment| source_names.contains(&assignment.target))
        .collect::<Vec<_>>();

    audit.assignments += emitted.len();
    audit.temporaries += emitted.len() - source_emitted.len();
    audit.source_assignments += source_assignments.len();
    assert_eq!(source_assignments.len(), source_emitted.len(), "{path}");

    let mut used_ambiguous_targets = BTreeSet::new();
    for (source_assignment, emitted_assignment) in source_assignments.iter().zip(&source_emitted) {
        assert_eq!(
            source_assignment.target, emitted_assignment.target,
            "{path}"
        );
        let matches = specify.get(&source_assignment.target);
        let expected = if let Some(explicit) = source_assignment.explicit {
            audit.explicit_delays += 1;
            if matches.is_some_and(|paths| !paths.is_empty()) {
                audit.explicit_with_specify_match += 1;
            }
            let shape = source_tuple_shape(explicit, &aliases);
            if shape != "0" {
                audit.explicit_nonzero_delays += 1;
            }
            shape
        } else if let Some(paths) = matches.filter(|paths| !paths.is_empty()) {
            audit.specify_delays += 1;
            if paths.len() > 1 {
                used_ambiguous_targets.insert(source_assignment.target.clone());
            }
            let shape = source_tuple_shape(
                TupleRef {
                    span: paths[0].span,
                    values: paths[0].values,
                },
                &aliases,
            );
            if shape != "0" {
                audit.specify_nonzero_delays += 1;
            }
            shape
        } else {
            audit.zero_delays += 1;
            "0".to_string()
        };
        audit.expected_outer_resistance_multiplications +=
            expected.matches("(* (pmos ").count() + expected.matches("(* (nmos ").count();
        if render_expr(&emitted_assignment.delay) != expected {
            audit.source_delay_mismatches += 1;
        }
    }

    for assignment in &emitted {
        assignment
            .delay
            .validate_timing("timing corpus delay")
            .unwrap();
        let is_temp = !source_names.contains(&assignment.target);
        if is_temp && assignment.delay != Expr::atom("0") {
            audit.temp_nonzero_delays += 1;
        }
        if assignment.delay == Expr::atom("0") {
            audit.emitted_zero_delays += 1;
        } else {
            audit.emitted_nonzero_delays += 1;
            if matches!(assignment.delay, Expr::List(_)) {
                audit.emitted_nested_delays += 1;
            }
        }
        inventory_ir_timing(&assignment.delay, audit);
    }

    let expected_diagnostics = expected_diagnostics(module, &specify, &used_ambiguous_targets);
    let actual_diagnostics = lowered
        .diagnostics
        .iter()
        .map(|diagnostic| ExpectedDiagnostic {
            kind: diagnostic.kind,
            span: diagnostic.span.clone(),
            message: diagnostic.message.clone(),
        })
        .collect::<Vec<_>>();
    if expected_diagnostics != actual_diagnostics {
        audit.diagnostic_mismatches += 1;
    }
    for diagnostic in &lowered.diagnostics {
        match diagnostic.kind {
            DiagnosticKind::Warning => {
                audit.warnings += 1;
                if !diagnostic.message.starts_with(SPECIFY_WARNING_PREFIX)
                    || DiagnosticPolicy::new(false).is_failure(diagnostic)
                    || !DiagnosticPolicy::new(true).is_failure(diagnostic)
                {
                    audit.warning_contract_failures += 1;
                }
            }
            DiagnosticKind::IntentionalIgnore => {
                if diagnostic.message == INITIAL_OMISSION {
                    audit.initial_ignores += 1;
                } else if diagnostic.message.starts_with(DELAY_IGNORE_PREFIX) {
                    audit.later_ignores += 1;
                }
            }
            DiagnosticKind::Error => audit.diagnostic_mismatches += 1,
        }
    }
}

fn collect_source_assignments<'a>(items: &'a [Item], output: &mut Vec<SourceAssignment<'a>>) {
    for item in items {
        match &item.kind {
            ItemKind::Assign(assign) => output.push(SourceAssignment {
                target: scalar_symbol(&assign.target).unwrap(),
                explicit: assign.delay.as_ref().map(delay_ref),
            }),
            ItemKind::Primitive(call) => output.push(SourceAssignment {
                target: scalar_symbol(call.args[0].as_ref().unwrap()).unwrap(),
                explicit: call.delay.as_ref().map(delay_ref),
            }),
            ItemKind::AlwaysLatch(always) => {
                collect_procedural_assignments(std::slice::from_ref(always.body.as_ref()), output)
            }
            ItemKind::Always(always) => {
                collect_procedural_assignments(std::slice::from_ref(always.body.as_ref()), output)
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                collect_source_assignments(&block.items, output)
            }
            ItemKind::If(if_stmt) => {
                collect_source_assignments(
                    std::slice::from_ref(if_stmt.then_branch.as_ref()),
                    output,
                );
                if let Some(else_branch) = &if_stmt.else_branch {
                    collect_source_assignments(std::slice::from_ref(else_branch.as_ref()), output);
                }
            }
            ItemKind::Import(_)
            | ItemKind::Decl(_)
            | ItemKind::Initial(_)
            | ItemKind::ProcAssign(_)
            | ItemKind::Instantiation(_)
            | ItemKind::Specify(_)
            | ItemKind::Empty => {}
        }
    }
}

fn collect_procedural_assignments<'a>(items: &'a [Item], output: &mut Vec<SourceAssignment<'a>>) {
    for item in items {
        match &item.kind {
            ItemKind::ProcAssign(assign) => output.push(SourceAssignment {
                target: scalar_symbol(&assign.target).unwrap(),
                explicit: None,
            }),
            ItemKind::Block(block) | ItemKind::Generate(block) => {
                collect_procedural_assignments(&block.items, output)
            }
            ItemKind::If(if_stmt) => {
                collect_procedural_assignments(
                    std::slice::from_ref(if_stmt.then_branch.as_ref()),
                    output,
                );
                if let Some(else_branch) = &if_stmt.else_branch {
                    collect_procedural_assignments(
                        std::slice::from_ref(else_branch.as_ref()),
                        output,
                    );
                }
            }
            _ => {}
        }
    }
}

fn delay_ref(delay: &Delay) -> TupleRef<'_> {
    TupleRef {
        span: &delay.span,
        values: &delay.values,
    }
}

fn specify_paths(module: &sv_to_sexpr::ast::Module) -> BTreeMap<String, Vec<SpecifyPathRef<'_>>> {
    let mut paths = BTreeMap::<String, Vec<SpecifyPathRef<'_>>>::new();
    for item in &module.items {
        let ItemKind::Specify(specify) = &item.kind else {
            continue;
        };
        for specify_item in &specify.items {
            let SpecifyItem::Path(path) = specify_item else {
                continue;
            };
            paths
                .entry(scalar_symbol(&path.target).unwrap())
                .or_default()
                .push(SpecifyPathRef {
                    span: &path.span,
                    values: &path.delays,
                });
        }
    }
    paths
}

fn timing_aliases(module: &sv_to_sexpr::ast::Module) -> BTreeMap<String, &SvExpr> {
    let mut aliases = BTreeMap::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Decl(decl)
                if matches!(
                    decl.kind,
                    sv_to_sexpr::ast::DeclKind::Localparam | sv_to_sexpr::ast::DeclKind::Specparam
                ) =>
            {
                if let Some(value) = &decl.value {
                    for name in &decl.names {
                        aliases.insert(name.clone(), value);
                    }
                }
            }
            ItemKind::Specify(specify) => {
                for specify_item in &specify.items {
                    if let SpecifyItem::Specparam(param) = specify_item {
                        aliases.insert(param.name.clone(), &param.value);
                    }
                }
            }
            _ => {}
        }
    }
    aliases
}

fn source_tuple_shape(tuple: TupleRef<'_>, aliases: &BTreeMap<String, &SvExpr>) -> String {
    let first = tuple
        .values
        .first()
        .unwrap_or_else(|| panic!("missing first tuple at {:?}", tuple.span))
        .as_ref()
        .unwrap_or_else(|| panic!("omitted first tuple at {:?}", tuple.span));
    source_timing_shape(first, aliases, &mut Vec::new())
}

fn source_timing_shape(
    expr: &SvExpr,
    aliases: &BTreeMap<String, &SvExpr>,
    stack: &mut Vec<String>,
) -> String {
    match &expr.kind {
        ExprKind::Path(segments) => {
            let name = segments.join("::");
            if segments.len() == 1 && aliases.contains_key(&name) {
                assert!(!stack.contains(&name));
                stack.push(name.clone());
                let result = source_timing_shape(aliases[&name], aliases, stack);
                stack.pop();
                result
            } else {
                name
            }
        }
        ExprKind::Integer(value) | ExprKind::Real(value) => value.clone(),
        ExprKind::Constant(kind) => match kind {
            ConstKind::Zero => "0".to_string(),
            ConstKind::One => "1".to_string(),
            ConstKind::Z => "z".to_string(),
            ConstKind::X => "x".to_string(),
        },
        ExprKind::Group(inner) => source_timing_shape(inner, aliases, stack),
        ExprKind::Unary { op, expr } => match op {
            UnaryOp::Plus => source_timing_shape(expr, aliases, stack),
            UnaryOp::Minus => format!("(- 0 {})", source_timing_shape(expr, aliases, stack)),
            _ => panic!("uncontracted timing unary"),
        },
        ExprKind::Binary { op, left, right } => {
            let operator = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Greater => "gt",
                _ => panic!("uncontracted timing binary"),
            };
            format!(
                "({operator} {} {})",
                source_timing_shape(left, aliases, stack),
                source_timing_shape(right, aliases, stack)
            )
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => format!(
            "(mux {} {} {})",
            source_timing_shape(condition, aliases, stack),
            source_timing_shape(then_expr, aliases, stack),
            source_timing_shape(else_expr, aliases, stack)
        ),
        ExprKind::Call { callee, args } => {
            let name = scalar_symbol(callee).unwrap();
            match name.as_str() {
                "tpd_elmore" => format!(
                    "(elmore (wire {}) {})",
                    source_timing_shape(args[0].as_ref().unwrap(), aliases, stack),
                    source_timing_shape(args[1].as_ref().unwrap(), aliases, stack)
                ),
                "tpd_z" => source_timing_shape(
                    args.iter().find_map(Option::as_ref).unwrap(),
                    aliases,
                    stack,
                ),
                "R_pmos_ohm" | "R_nmos_ohm" => {
                    let operator = if name == "R_pmos_ohm" { "pmos" } else { "nmos" };
                    let arg = args[0].as_ref().unwrap();
                    format!(
                        "({operator} {})",
                        resistance_factor_shape(arg, aliases, stack)
                    )
                }
                _ => panic!("uncontracted timing call {name}"),
            }
        }
    }
}

fn resistance_factor_shape(
    expr: &SvExpr,
    aliases: &BTreeMap<String, &SvExpr>,
    stack: &mut Vec<String>,
) -> String {
    match &expr.kind {
        ExprKind::Group(inner) => resistance_factor_shape(inner, aliases, stack),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right,
        } if is_l_unit(left) => source_timing_shape(right, aliases, stack),
        ExprKind::Binary {
            op: BinaryOp::Mul,
            left,
            right,
        } if is_l_unit(right) => source_timing_shape(left, aliases, stack),
        ExprKind::Path(segments) if segments.len() == 1 && segments[0] == "L_unit" => {
            "1".to_string()
        }
        _ => source_timing_shape(expr, aliases, stack),
    }
}

fn expected_diagnostics(
    module: &sv_to_sexpr::ast::Module,
    specify: &BTreeMap<String, Vec<SpecifyPathRef<'_>>>,
    used_ambiguous_targets: &BTreeSet<String>,
) -> Vec<ExpectedDiagnostic> {
    let mut expected = Vec::new();
    collect_expected_item_diagnostics(&module.items, &mut expected);
    for (target, paths) in specify {
        if used_ambiguous_targets.contains(target) {
            expected.push(ExpectedDiagnostic {
                kind: DiagnosticKind::Warning,
                span: paths[1].span.clone(),
                message: format!(
                    "multiple control-dependent specify paths target `{target}`; the one-delay cell DSL selects the first source-ordered path"
                ),
            });
        }
    }
    expected.sort_by(|left, right| {
        left.span
            .path
            .cmp(&right.span.path)
            .then_with(|| left.span.line.cmp(&right.span.line))
            .then_with(|| left.span.column.cmp(&right.span.column))
    });
    expected
}

fn collect_expected_item_diagnostics(items: &[Item], expected: &mut Vec<ExpectedDiagnostic>) {
    for item in items {
        match &item.kind {
            ItemKind::Initial(_) => expected.push(ExpectedDiagnostic {
                kind: DiagnosticKind::IntentionalIgnore,
                span: item.span.clone(),
                message: INITIAL_OMISSION.to_string(),
            }),
            ItemKind::Assign(assign) => {
                if let Some(delay) = &assign.delay {
                    expected_tuple_ignores(&delay.span, &delay.values, expected);
                }
            }
            ItemKind::Primitive(call) => {
                if let Some(delay) = &call.delay {
                    expected_tuple_ignores(&delay.span, &delay.values, expected);
                }
            }
            ItemKind::Specify(specify) => {
                for specify_item in &specify.items {
                    if let SpecifyItem::Path(path) = specify_item {
                        expected_tuple_ignores(&path.span, &path.delays, expected);
                    }
                }
            }
            ItemKind::Generate(block) | ItemKind::Block(block) => {
                collect_expected_item_diagnostics(&block.items, expected)
            }
            ItemKind::If(if_stmt) => {
                collect_expected_item_diagnostics(
                    std::slice::from_ref(if_stmt.then_branch.as_ref()),
                    expected,
                );
                if let Some(else_branch) = &if_stmt.else_branch {
                    collect_expected_item_diagnostics(
                        std::slice::from_ref(else_branch.as_ref()),
                        expected,
                    );
                }
            }
            _ => {}
        }
    }
}

fn expected_tuple_ignores(
    span: &Span,
    values: &[Option<SvExpr>],
    expected: &mut Vec<ExpectedDiagnostic>,
) {
    for (index, value) in values.iter().enumerate().skip(1) {
        expected.push(ExpectedDiagnostic {
            kind: DiagnosticKind::IntentionalIgnore,
            span: value
                .as_ref()
                .map(|expr| expr.span.clone())
                .unwrap_or_else(|| span.clone()),
            message: format!(
                "delay tuple entry {} is intentionally ignored because the cell model selects only entry 1",
                index + 1
            ),
        });
    }
}

fn inventory_ir_timing(expr: &Expr, audit: &mut LowerAudit) {
    let Expr::List(items) = expr else {
        return;
    };
    let (head, operands) = items.split_first().unwrap();
    let Expr::Atom(head) = head else {
        audit.uncontracted_timing += 1;
        return;
    };
    if TimingOperator::parse(head).is_none() {
        audit.uncontracted_timing += 1;
        return;
    }
    *audit.operator_counts.entry(head.clone()).or_default() += 1;
    if head == "*"
        && operands.iter().any(|operand| {
            matches!(
                operand,
                Expr::List(items)
                    if matches!(items.first(), Some(Expr::Atom(operator)) if operator == "pmos" || operator == "nmos")
            )
        })
    {
        audit.emitted_outer_resistance_multiplications += 1;
    }
    for operand in operands {
        inventory_ir_timing(operand, audit);
    }
}

fn deferral_category(path: &str) -> Option<DeferralCategory> {
    if KEEPER_DEFERRALS.contains(&path) {
        Some(DeferralCategory::Keeper)
    } else if TRANSISTOR_DEFERRALS.contains(&path) {
        Some(DeferralCategory::Transistor)
    } else {
        None
    }
}

fn assert_expected_deferral(category: DeferralCategory, diagnostic: &Diagnostic) {
    let expected = match category {
        DeferralCategory::Keeper => "unsupported item for lowering",
        DeferralCategory::Transistor => "unsupported primitive",
    };
    assert!(diagnostic.message.starts_with(expected));
}

fn scalar_symbol(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) if segments.len() == 1 => Some(segments[0].clone()),
        ExprKind::Group(inner) => scalar_symbol(inner),
        _ => None,
    }
}

fn is_l_unit(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Path(segments) => segments.len() == 1 && segments[0] == "L_unit",
        ExprKind::Group(inner) => is_l_unit(inner),
        _ => false,
    }
}

fn is_resistance_call(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Group(inner) => is_resistance_call(inner),
        ExprKind::Call { callee, .. } => scalar_symbol(callee)
            .is_some_and(|name| matches!(name.as_str(), "R_pmos_ohm" | "R_nmos_ohm")),
        _ => false,
    }
}

fn assert_exact_contract(source: &SourceInventory, lower: &LowerAudit) {
    assert_eq!(source.files, 206);
    assert_eq!(lower.succeeded, 192);
    assert_eq!(lower.failed, 14);
    assert_eq!(lower.assignments, 1_693);
    assert_eq!(lower.temporaries, 1_046);
    assert_eq!(lower.source_assignments, 647);
    assert_eq!(lower.temp_nonzero_delays, 0);
    assert_eq!(
        lower.explicit_delays + lower.specify_delays + lower.zero_delays,
        647
    );
    assert_eq!(lower.explicit_delays, 403);
    assert_eq!(lower.explicit_nonzero_delays, 389);
    assert_eq!(lower.specify_delays, 199);
    assert_eq!(lower.specify_nonzero_delays, 199);
    assert_eq!(lower.zero_delays, 45);
    assert_eq!(
        lower.explicit_nonzero_delays + lower.specify_nonzero_delays,
        588
    );
    assert_eq!(lower.emitted_nonzero_delays, 588);
    assert_eq!(lower.emitted_zero_delays, 1_105);
    assert_eq!(lower.emitted_nested_delays, 588);
    assert_eq!(lower.warnings, 47);
    assert_eq!(lower.later_ignores, 1_032);
    assert_eq!(lower.initial_ignores, 41);
    assert_eq!(lower.warning_contract_failures, 0);
    assert_eq!(lower.diagnostic_mismatches, 0);
    assert_eq!(lower.source_delay_mismatches, 0);
    assert_eq!(lower.invalid_cells, 0);
    assert_eq!(lower.nondeterministic_results, 0);
    assert_eq!(lower.absolute_path_leaks, 0);
    assert_eq!(lower.uncontracted_timing, 0);
    assert!(lower.expected_outer_resistance_multiplications > 0);
    assert_eq!(
        lower.emitted_outer_resistance_multiplications,
        lower.expected_outer_resistance_multiplications
    );
    assert_eq!(lower.expected_outer_resistance_multiplications, 385);
    assert_eq!(
        lower.operator_counts,
        BTreeMap::from([
            ("+".to_string(), 225),
            ("*".to_string(), 387),
            ("elmore".to_string(), 739),
            ("wire".to_string(), 739),
            ("pmos".to_string(), 381),
            ("nmos".to_string(), 432),
        ])
    );
    assert_eq!(lower.deferrals.len(), 14);
    assert_eq!(
        lower
            .deferrals
            .iter()
            .filter(|deferral| deferral.category == DeferralCategory::Keeper)
            .map(|deferral| deferral.path.as_str())
            .collect::<Vec<_>>(),
        KEEPER_DEFERRALS
    );
    let pad = lower
        .deferrals
        .iter()
        .find(|deferral| deferral.path == PAD_XTAL)
        .unwrap();
    assert_eq!(pad.category, DeferralCategory::Keeper);
    assert_eq!(pad.diagnostic.span.line, 26);
    assert_eq!(pad.diagnostic.span.column, 2);
    assert!(lower.deferrals.iter().all(|deferral| {
        matches!(
            deferral.category,
            DeferralCategory::Keeper | DeferralCategory::Transistor
        )
    }));
}

fn render_summary(source: &SourceInventory, lower: &LowerAudit) -> String {
    let mut output = String::new();
    writeln!(&mut output, "timing corpus audit").unwrap();
    writeln!(&mut output, "corpus-files={}", source.files).unwrap();
    writeln!(&mut output, "source-tuples:").unwrap();
    render_tuple_inventory(&mut output, "assignment", &source.assignment_tuples);
    render_tuple_inventory(&mut output, "primitive", &source.primitive_tuples);
    render_tuple_inventory(&mut output, "specify", &source.specify_tuples);
    writeln!(&mut output, "source-timing-forms:").unwrap();
    writeln!(&mut output, "  aliases={}", source.forms.aliases).unwrap();
    writeln!(
        &mut output,
        "  tpd-elmore-arities={}",
        render_usize_map(&source.forms.tpd_elmore_arities)
    )
    .unwrap();
    writeln!(
        &mut output,
        "  tpd-z-arities={}",
        render_usize_map(&source.forms.tpd_z_arities)
    )
    .unwrap();
    writeln!(
        &mut output,
        "  tpd-z-first-present={}",
        render_usize_map(&source.forms.tpd_z_first_present)
    )
    .unwrap();
    for (name, value) in [
        ("resistance-pmos", source.forms.resistance_pmos),
        ("resistance-nmos", source.forms.resistance_nmos),
        (
            "outer-resistance-integer-mul",
            source.forms.outer_resistance_integer_mul,
        ),
        (
            "outer-resistance-real-mul",
            source.forms.outer_resistance_real_mul,
        ),
        ("resistance-sums", source.forms.resistance_sums),
        (
            "direct-real-resistance",
            source.forms.direct_real_resistance,
        ),
        ("real-unit-factors", source.forms.real_unit_factors),
        ("timing-adds", source.forms.timing_adds),
        ("timing-gt", source.forms.timing_gt),
        ("timing-mux", source.forms.timing_mux),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(
        &mut output,
        "  real-device-values=[{}]",
        source
            .forms
            .real_device_values
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    )
    .unwrap();
    writeln!(&mut output, "source-specify:").unwrap();
    writeln!(&mut output, "  paths={}", source.specify_paths).unwrap();
    writeln!(&mut output, "  scalar-controls={}", source.specify_controls).unwrap();
    writeln!(
        &mut output,
        "  target-groups={}",
        source.specify_targets.len()
    )
    .unwrap();
    writeln!(
        &mut output,
        "  target-multiplicity={}",
        render_usize_map(&source.specify_target_multiplicity)
    )
    .unwrap();
    writeln!(
        &mut output,
        "  first-source-paths={} witnesses=[{}]",
        source.specify_first_paths.len(),
        witness_list(&source.specify_first_paths)
    )
    .unwrap();
    writeln!(&mut output, "lowering:").unwrap();
    writeln!(
        &mut output,
        "  succeeded={} failed={}",
        lower.succeeded, lower.failed
    )
    .unwrap();
    writeln!(
        &mut output,
        "  assignments={} temps={} source-targets={}",
        lower.assignments, lower.temporaries, lower.source_assignments
    )
    .unwrap();
    writeln!(
        &mut output,
        "  source-dispositions explicit={} explicit-nonzero={} explicit-with-specify={} specify={} specify-nonzero={} zero={}",
        lower.explicit_delays,
        lower.explicit_nonzero_delays,
        lower.explicit_with_specify_match,
        lower.specify_delays,
        lower.specify_nonzero_delays,
        lower.zero_delays
    )
    .unwrap();
    writeln!(
        &mut output,
        "  emitted zero={} nonzero={} nested={}",
        lower.emitted_zero_delays, lower.emitted_nonzero_delays, lower.emitted_nested_delays
    )
    .unwrap();
    writeln!(
        &mut output,
        "  outer-resistance-multiplications expected={} emitted={}",
        lower.expected_outer_resistance_multiplications,
        lower.emitted_outer_resistance_multiplications
    )
    .unwrap();
    writeln!(
        &mut output,
        "  warnings={} later-ignores={} initial-ignores={}",
        lower.warnings, lower.later_ignores, lower.initial_ignores
    )
    .unwrap();
    writeln!(&mut output, "emitted-timing-operators:").unwrap();
    for operator in TimingOperator::ALL {
        writeln!(
            &mut output,
            "  {}={}",
            operator.as_str(),
            lower
                .operator_counts
                .get(operator.as_str())
                .copied()
                .unwrap_or(0)
        )
        .unwrap();
    }
    writeln!(&mut output, "invariants:").unwrap();
    for (name, value) in [
        ("temp-nonzero-delays", lower.temp_nonzero_delays),
        ("warning-contract-failures", lower.warning_contract_failures),
        ("diagnostic-mismatches", lower.diagnostic_mismatches),
        ("source-delay-mismatches", lower.source_delay_mismatches),
        ("invalid-cells", lower.invalid_cells),
        ("nondeterministic-results", lower.nondeterministic_results),
        ("absolute-path-leaks", lower.absolute_path_leaks),
        ("uncontracted-timing", lower.uncontracted_timing),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "acceptance-witnesses:").unwrap();
    for arity in 1..=3 {
        writeln!(
            &mut output,
            "  tuple-arity-{arity}=[{}]",
            witness_list(&source.witnesses.tuple_arity[arity])
        )
        .unwrap();
    }
    for (name, values) in [
        (
            "precharge-first-rise",
            &source.witnesses.precharge_first_rise,
        ),
        ("first-t-z", &source.witnesses.first_t_z),
        ("multi-tpd-z", &source.witnesses.multi_tpd_z),
        ("outer-mul-2", &source.witnesses.outer_mul_2),
        ("outer-mul-1.5", &source.witnesses.outer_mul_1_5),
        ("resistance-sum", &source.witnesses.resistance_sum),
        ("real-device-factor", &source.witnesses.real_device_factor),
    ] {
        writeln!(&mut output, "  {name}=[{}]", witness_list(values)).unwrap();
    }
    writeln!(&mut output, "  reference={REFERENCE}").unwrap();
    writeln!(
        &mut output,
        "  explicit-precedence=sv-to-sexpr/tests/fixtures/timing/explicit_precedence.sv"
    )
    .unwrap();
    writeln!(&mut output, "  pad-xtal=M10-keeper:no-M7-failure").unwrap();
    writeln!(&mut output, "deferrals:").unwrap();
    for deferral in &lower.deferrals {
        writeln!(
            &mut output,
            "  {} | {} | {}:{} | {}",
            deferral.path,
            deferral.category.label(),
            deferral.diagnostic.span.line,
            deferral.diagnostic.span.column,
            deferral.diagnostic.message
        )
        .unwrap();
    }
    output
}

fn render_tuple_inventory(output: &mut String, name: &str, tuples: &TupleInventory) {
    writeln!(
        output,
        "  {name} arity0={} arity1={} arity2={} arity3={} other={} omitted-first={} omitted-later={} later-entries={}",
        tuples.arities[0],
        tuples.arities[1],
        tuples.arities[2],
        tuples.arities[3],
        tuples.other_arities,
        tuples.omitted_first,
        tuples.omitted_later,
        tuples.later_entries
    )
    .unwrap();
}

fn render_usize_map(values: &BTreeMap<usize, usize>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}:{value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn witness_list(values: &BTreeSet<String>) -> String {
    values.iter().take(6).cloned().collect::<Vec<_>>().join(",")
}

fn assert_or_update_fixture(actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/timing/corpus_summary.timing");
    if std::env::var_os("UPDATE_TIMING_CORPUS_GOLDEN").is_some() {
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(actual, expected, "timing corpus summary changed");
}
