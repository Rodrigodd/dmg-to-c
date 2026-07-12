#[allow(dead_code)]
mod driver_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use driver_support::{assert_or_update_fixture, repository_root};
use sv_to_sexpr::analyze::{
    AssignmentAnalysis, DriverAnalysis, DriverSource, GenerateAlternativeAnalysis, ModuleAnalysis,
    ModuleCatalog, PrimitiveAnalysis, ScopeAnalysis, analyze_design_structural,
    analyze_design_with_catalog_and_generate_mode,
};
use sv_to_sexpr::ast::{BinaryOp, ConstKind, Expr as SvExpr, ExprKind, Strength, UnaryOp};
use sv_to_sexpr::diagnostic::Diagnostic;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::ir::{Assignment, Cell, CellItem, Expr, StrengthPair, ValueOperator};
use sv_to_sexpr::lower::{lower_design_with_catalog_and_generate_mode, lower_file_structural};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::serialize::render_cell;
use sv_to_sexpr::survey::collect_sv_files;

const M7_FAILURES: &[&str] = &[];

const EXPECTED_RELEVANT_SUCCESSES: &[&str] = &[
    "sv-cells/dmg_cpu_b/cells/buf_if0.sv",
    "sv-cells/dmg_cpu_b/cells/mux.sv",
    "sv-cells/dmg_cpu_b/cells/muxi.sv",
    "sv-cells/dmg_cpu_b/cells/not_if0.sv",
    "sv-cells/dmg_cpu_b/cells/not_if1.sv",
    "sv-cells/dmg_cpu_b/cells/pad_bidir.sv",
    "sv-cells/dmg_cpu_b/cells/pad_bidir_pu.sv",
    "sv-cells/dmg_cpu_b/cells/pad_bidir_pu_latch.sv",
    "sv-cells/dmg_cpu_b/cells/pad_in_pu.sv",
    "sv-cells/dmg_cpu_b/cells/pad_out_diff.sv",
    "sv-cells/dmg_cpu_b/cells/pad_xtal.sv",
    "sv-cells/dmg_cpu_b/cells/tie.sv",
    "sv-cells/sm83/cells/alu_decoder.sv",
    "sv-cells/sm83/cells/alu_pggen.sv",
    "sv-cells/sm83/cells/alu_shifter.sv",
    "sv-cells/sm83/cells/b2b_wand_inj_a.sv",
    "sv-cells/sm83/cells/decoder1.sv",
    "sv-cells/sm83/cells/decoder2.sv",
    "sv-cells/sm83/cells/decoder3.sv",
    "sv-cells/sm83/cells/dff_cc_ee_pch_d_reg_sp_bit.sv",
    "sv-cells/sm83/cells/dffn_ee_pch_d_alu_flag.sv",
    "sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
    "sv-cells/sm83/cells/dlatch_ee_irq.sv",
    "sv-cells/sm83/cells/idu_bit0.sv",
    "sv-cells/sm83/cells/idu_bit123456.sv",
    "sv-cells/sm83/cells/idu_bit7.sv",
    "sv-cells/sm83/cells/irq_prio_bit0.sv",
    "sv-cells/sm83/cells/irq_prio_bit1.sv",
    "sv-cells/sm83/cells/irq_prio_bit2.sv",
    "sv-cells/sm83/cells/irq_prio_bit3.sv",
    "sv-cells/sm83/cells/irq_prio_bit4.sv",
    "sv-cells/sm83/cells/irq_prio_bit5.sv",
    "sv-cells/sm83/cells/irq_prio_bit6.sv",
    "sv-cells/sm83/cells/irq_prio_bit7.sv",
    "sv-cells/sm83/cells/nand2_nand3_od_irq.sv",
    "sv-cells/sm83/cells/nand2_od_a_dbus.sv",
    "sv-cells/sm83/cells/nand2_od_b_dbus.sv",
    "sv-cells/sm83/cells/nor2_pch_in1_dec3.sv",
    "sv-cells/sm83/cells/not2_pch_dec1.sv",
    "sv-cells/sm83/cells/not_p2_pch_dec3.sv",
    "sv-cells/sm83/cells/not_pch_dec1.sv",
    "sv-cells/sm83/cells/not_pch_dec3_a.sv",
    "sv-cells/sm83/cells/not_pch_dec3_a2.sv",
    "sv-cells/sm83/cells/not_pch_dec3_b.sv",
    "sv-cells/sm83/cells/not_pch_dec3_b2.sv",
    "sv-cells/sm83/cells/not_pch_x1_alu.sv",
    "sv-cells/sm83/cells/not_pch_x2_alu.sv",
    "sv-cells/sm83/cells/not_x1_pch_dec2.sv",
    "sv-cells/sm83/cells/pch_dec2_a.sv",
    "sv-cells/sm83/cells/pch_dec2_b.sv",
    "sv-cells/sm83/cells/pch_dec2_c.sv",
    "sv-cells/sm83/cells/reg_a_out.sv",
    "sv-cells/sm83/cells/reg_bc_out.sv",
    "sv-cells/sm83/cells/reg_bus_pch_a_bit0123.sv",
    "sv-cells/sm83/cells/reg_bus_pch_a_bit4.sv",
    "sv-cells/sm83/cells/reg_bus_pch_a_bit5.sv",
    "sv-cells/sm83/cells/reg_bus_pch_a_bit6.sv",
    "sv-cells/sm83/cells/reg_bus_pch_a_bit7.sv",
    "sv-cells/sm83/cells/reg_bus_pch_b.sv",
    "sv-cells/sm83/cells/reg_de_out.sv",
    "sv-cells/sm83/cells/reg_hl_out.sv",
    "sv-cells/sm83/cells/reg_pc_out_bit012.sv",
    "sv-cells/sm83/cells/reg_pc_out_bit345.sv",
    "sv-cells/sm83/cells/reg_pc_out_bit67.sv",
    "sv-cells/sm83/cells/reg_sp_out.sv",
    "sv-cells/sm83/cells/reg_wz_out.sv",
    "sv-cells/sm83/cells/tie.sv",
];

const EXPECTED_RELEVANT_DEFERRALS: &[&str] = &[];

const EXPECTED_REPEATED_TARGETS: &[(&str, &str, usize, Option<usize>)] = &[
    ("sv-cells/dmg_cpu_b/cells/mux.sv", "mux", 5, Some(5)),
    ("sv-cells/dmg_cpu_b/cells/muxi.sv", "mux", 5, Some(5)),
    ("sv-cells/dmg_cpu_b/cells/pad_bidir.sv", "pad", 2, Some(2)),
    (
        "sv-cells/dmg_cpu_b/cells/pad_bidir_pu.sv",
        "pad",
        3,
        Some(3),
    ),
    (
        "sv-cells/dmg_cpu_b/cells/pad_bidir_pu_latch.sv",
        "pad",
        3,
        Some(3),
    ),
    (
        "sv-cells/dmg_cpu_b/cells/pad_out_diff.sv",
        "pad",
        2,
        Some(2),
    ),
    ("sv-cells/dmg_cpu_b/cells/pad_xtal.sv", "clk", 4, Some(4)),
    ("sv-cells/sm83/cells/b2b_wand_inj_a.sv", "a", 3, Some(3)),
    ("sv-cells/sm83/cells/b2b_wand_inj_a.sv", "b", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y1", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y12", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y13", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y14", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y15", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y16", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y18", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y19", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y2", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y21", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y22", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y23", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y24", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y25", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y26", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y27", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y28", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y29", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y3", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y30", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y4", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y5", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y7", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y8", 2, Some(2)),
    ("sv-cells/sm83/cells/decoder2.sv", "y9", 2, Some(2)),
    (
        "sv-cells/sm83/cells/dlatch_ee_irq.sv",
        "gated_q",
        3,
        Some(3),
    ),
    ("sv-cells/sm83/cells/idu_bit0.sv", "aoi_y", 4, Some(4)),
    ("sv-cells/sm83/cells/idu_bit0.sv", "buf_a_y", 2, Some(2)),
    ("sv-cells/sm83/cells/idu_bit0.sv", "buf_b_y", 2, Some(2)),
    (
        "sv-cells/sm83/cells/idu_bit123456.sv",
        "buf_a_y",
        2,
        Some(2),
    ),
    (
        "sv-cells/sm83/cells/idu_bit123456.sv",
        "buf_b_y",
        2,
        Some(2),
    ),
    ("sv-cells/sm83/cells/idu_bit7.sv", "buf_a_y", 2, Some(2)),
    ("sv-cells/sm83/cells/idu_bit7.sv", "buf_b_y", 2, Some(2)),
    (
        "sv-cells/sm83/cells/irq_prio_bit0.sv",
        "nand_a_y",
        2,
        Some(2),
    ),
    (
        "sv-cells/sm83/cells/irq_prio_bit3.sv",
        "nand_d_y",
        2,
        Some(2),
    ),
    (
        "sv-cells/sm83/cells/irq_prio_bit5.sv",
        "nand_c_y",
        2,
        Some(2),
    ),
    (
        "sv-cells/sm83/cells/reg_bus_pch_a_bit0123.sv",
        "c_y",
        2,
        Some(2),
    ),
    (
        "sv-cells/sm83/cells/reg_bus_pch_a_bit4.sv",
        "c_y",
        3,
        Some(3),
    ),
    (
        "sv-cells/sm83/cells/reg_bus_pch_a_bit5.sv",
        "c_y",
        3,
        Some(3),
    ),
    (
        "sv-cells/sm83/cells/reg_bus_pch_a_bit6.sv",
        "c_y",
        3,
        Some(3),
    ),
    (
        "sv-cells/sm83/cells/reg_bus_pch_a_bit7.sv",
        "c_y",
        3,
        Some(3),
    ),
    ("sv-cells/sm83/cells/reg_de_out.sv", "d_y1", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_pc_out_bit012.sv", "y4", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_pc_out_bit345.sv", "y4", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_pc_out_bit345.sv", "y5", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_pc_out_bit67.sv", "y4", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_pc_out_bit67.sv", "y5", 2, Some(2)),
    ("sv-cells/sm83/cells/reg_wz_out.sv", "aoi_a_y", 3, Some(3)),
    ("sv-cells/sm83/cells/reg_wz_out.sv", "aoi_b_y", 2, Some(2)),
];

#[derive(Debug, Clone)]
struct SourceDriver {
    source_order: usize,
    target: String,
    source: DriverSource,
}

#[derive(Debug, Clone)]
struct DriverScope {
    label: String,
    drivers: Vec<SourceDriver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedRepeat {
    scope: String,
    target: String,
    occurrences: usize,
}

#[derive(Debug, Clone)]
struct SourceStrength {
    source_order: usize,
    pair: StrengthPair,
}

#[derive(Debug, Clone)]
struct M6Construct {
    source_order: usize,
    target: String,
    kind: M6ConstructKind,
}

#[derive(Debug, Clone)]
enum M6ConstructKind {
    ContinuousHighZ {
        operator: ValueOperator,
        drive: SvExpr,
        control: SvExpr,
        strength: Option<StrengthPair>,
    },
    ContinuousStrength {
        value: SvExpr,
        strength: StrengthPair,
    },
    DirectBufIf {
        operator: ValueOperator,
        drive: SvExpr,
        control: SvExpr,
        strength: Option<StrengthPair>,
    },
}

#[derive(Debug, Default)]
struct SourceInventory {
    source_names: BTreeSet<String>,
    drivers: Vec<SourceDriver>,
    driver_scopes: Vec<DriverScope>,
    strengths: Vec<SourceStrength>,
    constructs: Vec<M6Construct>,
    contains_z_continuous: usize,
    high_z_bufif0: usize,
    high_z_bufif1: usize,
    continuous_strength: usize,
    direct_bufif0: usize,
    direct_bufif1: usize,
    primitive_strength: usize,
}

#[derive(Debug, Default, Clone)]
struct EmittedForms {
    bufif0: usize,
    bufif1: usize,
    drive_strength: usize,
    bufif0_strength: usize,
    bufif1_strength: usize,
}

#[derive(Debug)]
struct RelevantSuccess {
    path: String,
    inventory: SourceInventory,
    emitted: EmittedForms,
    repeated: Vec<ScopedRepeat>,
}

#[derive(Debug)]
struct RelevantDeferral {
    path: String,
    inventory: SourceInventory,
    repeated: Vec<ScopedRepeat>,
    category: &'static str,
    rationale: &'static str,
    diagnostic: Diagnostic,
}

#[derive(Debug)]
struct RepeatedEntry {
    path: String,
    scope: String,
    target: String,
    source_occurrences: usize,
    emitted_occurrences: Option<usize>,
}

#[derive(Debug, Default)]
struct Invariants {
    invalid_cells: usize,
    nested_values: usize,
    ordinary_z_values: usize,
    target_order_mismatches: usize,
    unaccounted_m6_constructs: usize,
    strength_lost: usize,
    strength_mismatch: usize,
    strength_invented: usize,
    polarity_form_mismatches: usize,
    repeated_collapse_or_reorder: usize,
    temp_dependency_failures: usize,
    temp_collisions: usize,
    driver_register_leaks: usize,
    nondeterministic_results: usize,
    absolute_path_leaks: usize,
}

#[derive(Debug, Default)]
struct Audit {
    processed: usize,
    succeeded: usize,
    failed: usize,
    relevant_successes: Vec<RelevantSuccess>,
    relevant_deferrals: Vec<RelevantDeferral>,
    repeated_entries: Vec<RepeatedEntry>,
    source_pair_counts: BTreeMap<String, usize>,
    emitted_forms: EmittedForms,
    source_keepers: usize,
    emitted_keepers: usize,
    deferred_keepers: usize,
    invariants: Invariants,
}

#[test]
fn structural_m6_relevance_and_disposition_sets_are_exact() {
    let root = repository_root();
    let paths = corpus_paths(&root);
    assert_eq!(paths.len(), 206);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut global_successes = 0;
    let mut global_failures = 0;
    for path in paths {
        let input = fs::read_to_string(root.join(&path)).unwrap();
        let design = parse_file(Path::new(&path), &input).unwrap();
        let analysis = analyze_design_structural(&design);
        let inventory = source_inventory(&analysis.modules[0]);
        let repeated = repeated_targets(&inventory);
        let relevant = inventory.contains_z_continuous > 0
            || inventory.continuous_strength > 0
            || inventory.direct_bufif0 > 0
            || inventory.direct_bufif1 > 0
            || !repeated.is_empty();
        match lower_file_structural(Path::new(&path), &input) {
            Ok(_) => {
                global_successes += 1;
                if relevant {
                    successes.push(path);
                }
            }
            Err(_) => {
                global_failures += 1;
                if relevant {
                    failures.push(path);
                }
            }
        }
    }

    assert_eq!(global_successes, 199);
    assert_eq!(global_failures, 7);
    assert_eq!(successes, EXPECTED_RELEVANT_SUCCESSES);
    assert_eq!(failures, EXPECTED_RELEVANT_DEFERRALS);
    assert_eq!(successes.len(), 67);
    assert_eq!(failures.len(), 0);
    assert_eq!(successes.len() + failures.len(), 67);
}

#[test]
fn generate_alternatives_keep_repeated_driver_scopes_isolated() {
    let path = Path::new("generate_scope_repeat.sv");
    let input = "module sample(input logic sel, a, b, output logic y, z, w);\n\
                 generate\n\
                   if (sel) begin\n\
                     assign y = a;\n\
                     assign z = a;\n\
                     assign z = b;\n\
                   end else begin\n\
                     assign y = b;\n\
                     bufif0 (strong1, highz0) (w, a, sel);\n\
                   end\n\
                 endgenerate\n\
                 endmodule\n";
    let design = parse_file(path, input).unwrap();
    let analysis = analyze_design_structural(&design);
    let inventory = source_inventory(&analysis.modules[0]);

    assert_eq!(
        inventory
            .drivers
            .iter()
            .filter(|driver| driver.target == "y")
            .count(),
        2,
        "the full structural inventory must retain both mutually exclusive alternatives"
    );
    assert_eq!(
        repeated_targets(&inventory),
        vec![ScopedRepeat {
            scope: "root/generate[0]/then".to_string(),
            target: "z".to_string(),
            occurrences: 2,
        }]
    );
    assert_eq!(inventory.direct_bufif0, 1);
    assert_eq!(inventory.constructs.len(), 1);
}

#[test]
fn complete_driver_corpus_is_accounted_flat_and_source_ordered() {
    let root = repository_root();
    let paths = corpus_paths(&root);
    assert_eq!(paths.len(), 206);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));

    let mut audit = Audit::default();
    let absolute_root = root.to_string_lossy().to_string();
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
        audit.processed += 1;
        let design = &designs[path];
        let analysis =
            analyze_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful)
                .unwrap();
        let module = &analysis.modules[0];
        let inventory = source_inventory(module);
        let source_keepers = inventory
            .drivers
            .iter()
            .filter(|driver| matches!(driver.source, DriverSource::Keeper { .. }))
            .count();
        audit.source_keepers += source_keepers;
        let repeated = repeated_targets(&inventory);
        let relevant = inventory.contains_z_continuous > 0
            || inventory.continuous_strength > 0
            || inventory.direct_bufif0 > 0
            || inventory.direct_bufif1 > 0
            || !repeated.is_empty();

        if relevant {
            for strength in &inventory.strengths {
                *audit
                    .source_pair_counts
                    .entry(pair_label(strength.pair))
                    .or_default() += 1;
            }
        }

        let first =
            lower_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful);
        let second =
            lower_design_with_catalog_and_generate_mode(design, &catalog, GenerateMode::Delayful);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                audit.succeeded += 1;
                if first != second || render_cell(&first.cell) != render_cell(&second.cell) {
                    audit.invariants.nondeterministic_results += 1;
                }
                if render_cell(&first.cell).contains(&absolute_root) {
                    audit.invariants.absolute_path_leaks += 1;
                }
                audit_success_cell(&first.cell, &inventory, &mut audit.invariants);
                let emitted_keepers = assignments(&first.cell)
                    .iter()
                    .filter(|assignment| {
                        matches!(
                            operation(&assignment.expr),
                            Some((ValueOperator::Keeper, []))
                        )
                    })
                    .inspect(|assignment| assert_eq!(assignment.delay, Expr::atom("0"), "{path}"))
                    .count();
                assert_eq!(emitted_keepers, source_keepers, "{path}");
                audit.emitted_keepers += emitted_keepers;
                if relevant {
                    let emitted = audit_relevant_success(
                        path,
                        &first.cell,
                        &inventory,
                        &repeated,
                        &mut audit.invariants,
                    );
                    audit.emitted_forms.add(&emitted);
                    for repeat in &repeated {
                        let emitted_occurrences = assignments(&first.cell)
                            .iter()
                            .filter(|assignment| assignment.target == repeat.target)
                            .count();
                        if emitted_occurrences != repeat.occurrences {
                            audit.invariants.repeated_collapse_or_reorder += 1;
                        }
                        audit.repeated_entries.push(RepeatedEntry {
                            path: path.clone(),
                            scope: repeat.scope.clone(),
                            target: repeat.target.clone(),
                            source_occurrences: repeat.occurrences,
                            emitted_occurrences: Some(emitted_occurrences),
                        });
                    }
                    audit.relevant_successes.push(RelevantSuccess {
                        path: path.clone(),
                        inventory,
                        emitted,
                        repeated,
                    });
                }
            }
            (Err(first), Err(second)) => {
                audit.failed += 1;
                audit.deferred_keepers += source_keepers;
                if first != second {
                    audit.invariants.nondeterministic_results += 1;
                }
                if first.span.path.is_absolute() || first.to_string().contains(&absolute_root) {
                    audit.invariants.absolute_path_leaks += 1;
                }
                if relevant {
                    let (category, rationale) = later_blocker(path);
                    for repeat in &repeated {
                        audit.repeated_entries.push(RepeatedEntry {
                            path: path.clone(),
                            scope: repeat.scope.clone(),
                            target: repeat.target.clone(),
                            source_occurrences: repeat.occurrences,
                            emitted_occurrences: None,
                        });
                    }
                    audit.relevant_deferrals.push(RelevantDeferral {
                        path: path.clone(),
                        inventory,
                        repeated,
                        category,
                        rationale,
                        diagnostic: first,
                    });
                }
            }
            (first, second) => panic!(
                "nondeterministic success/failure disposition for {path}: first={first:?}, second={second:?}"
            ),
        }
    }

    audit
        .relevant_successes
        .sort_by(|left, right| left.path.cmp(&right.path));
    audit
        .relevant_deferrals
        .sort_by(|left, right| left.path.cmp(&right.path));
    audit.repeated_entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.target.cmp(&right.target))
    });

    assert_exact_audit(&audit);
    let summary = render_summary(&audit);
    assert!(!summary.contains(&absolute_root));
    assert_or_update_fixture("corpus_summary", "drivers", &summary);
}

fn corpus_paths(root: &Path) -> Vec<String> {
    let mut paths = collect_sv_files(&root.join("sv-cells"))
        .unwrap()
        .into_iter()
        .map(|path| logical_path(root, &path))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn logical_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn source_inventory(module: &ModuleAnalysis) -> SourceInventory {
    let mut inventory = SourceInventory::default();
    inventory
        .source_names
        .extend(module.symbols.keys().cloned());
    collect_analysis_parts(
        &module.continuous_assignments,
        &module.primitive_calls,
        &module.drivers,
        &module.generate_alternatives,
        "root",
        &mut inventory,
    );
    inventory.drivers.sort_by_key(|driver| driver.source_order);
    inventory
        .strengths
        .sort_by_key(|strength| strength.source_order);
    inventory
        .constructs
        .sort_by_key(|construct| construct.source_order);
    inventory
}

fn collect_scope(scope: &ScopeAnalysis, label: &str, inventory: &mut SourceInventory) {
    inventory.source_names.extend(scope.symbols.keys().cloned());
    collect_analysis_parts(
        &scope.continuous_assignments,
        &scope.primitive_calls,
        &scope.drivers,
        &scope.generate_alternatives,
        label,
        inventory,
    );
}

fn collect_analysis_parts(
    continuous: &[AssignmentAnalysis],
    primitives: &[PrimitiveAnalysis],
    drivers: &[DriverAnalysis],
    alternatives: &[GenerateAlternativeAnalysis],
    scope_label: &str,
    inventory: &mut SourceInventory,
) {
    let scope_drivers = drivers
        .iter()
        .map(|driver| SourceDriver {
            source_order: driver.source_order,
            target: driver.target.clone(),
            source: driver.source.clone(),
        })
        .collect::<Vec<_>>();
    inventory.drivers.extend(scope_drivers.iter().cloned());
    inventory.driver_scopes.push(DriverScope {
        label: scope_label.to_string(),
        drivers: scope_drivers,
    });

    for assignment in continuous {
        let contains_z = expr_contains_z(&assignment.value_expression);
        inventory.contains_z_continuous += usize::from(contains_z);
        let strength = assignment.strength.as_ref().map(parse_strength);
        if let Some(pair) = strength {
            inventory.continuous_strength += 1;
            inventory.strengths.push(SourceStrength {
                source_order: assignment.source_order,
                pair,
            });
        }
        if contains_z {
            let (operator, drive, control) = root_high_z_driver(&assignment.value_expression)
                .unwrap_or_else(|| {
                    panic!(
                        "corpus high-Z continuous expression is not a root driver at {:?}",
                        assignment.span
                    )
                });
            match operator {
                ValueOperator::BufIf0 => inventory.high_z_bufif0 += 1,
                ValueOperator::BufIf1 => inventory.high_z_bufif1 += 1,
                _ => unreachable!(),
            }
            inventory.constructs.push(M6Construct {
                source_order: assignment.source_order,
                target: assignment.target.clone(),
                kind: M6ConstructKind::ContinuousHighZ {
                    operator,
                    drive: drive.clone(),
                    control: control.clone(),
                    strength,
                },
            });
        } else if let Some(strength) = strength {
            inventory.constructs.push(M6Construct {
                source_order: assignment.source_order,
                target: assignment.target.clone(),
                kind: M6ConstructKind::ContinuousStrength {
                    value: assignment.value_expression.clone(),
                    strength,
                },
            });
        }
    }

    for primitive in primitives {
        if let Some(values) = &primitive.strength {
            inventory.primitive_strength += 1;
            inventory.strengths.push(SourceStrength {
                source_order: primitive.source_order,
                pair: parse_strength_values(values, &primitive.span),
            });
        }
        let operator = match primitive.name.as_str() {
            "bufif0" => {
                inventory.direct_bufif0 += 1;
                ValueOperator::BufIf0
            }
            "bufif1" => {
                inventory.direct_bufif1 += 1;
                ValueOperator::BufIf1
            }
            _ => continue,
        };
        let target = primitive.args[0]
            .clone()
            .expect("corpus bufif target must be present");
        let drive = primitive.argument_expressions[1]
            .clone()
            .expect("corpus bufif drive must be present");
        let control = primitive.argument_expressions[2]
            .clone()
            .expect("corpus bufif control must be present");
        let strength = primitive
            .strength
            .as_ref()
            .map(|values| parse_strength_values(values, &primitive.span));
        inventory.constructs.push(M6Construct {
            source_order: primitive.source_order,
            target,
            kind: M6ConstructKind::DirectBufIf {
                operator,
                drive,
                control,
                strength,
            },
        });
    }

    for (index, alternative) in alternatives.iter().enumerate() {
        let prefix = format!("{scope_label}/generate[{index}]");
        collect_scope(
            &alternative.then_branch,
            &format!("{prefix}/then"),
            inventory,
        );
        if let Some(else_branch) = &alternative.else_branch {
            collect_scope(else_branch, &format!("{prefix}/else"), inventory);
        }
    }
}

fn expr_contains_z(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Constant(ConstKind::Z) => true,
        ExprKind::Group(inner) | ExprKind::Unary { expr: inner, .. } => expr_contains_z(inner),
        ExprKind::Binary { left, right, .. } => expr_contains_z(left) || expr_contains_z(right),
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } => expr_contains_z(condition) || expr_contains_z(then_expr) || expr_contains_z(else_expr),
        ExprKind::Call { callee, args } => {
            expr_contains_z(callee) || args.iter().flatten().any(expr_contains_z)
        }
        ExprKind::Path(_) | ExprKind::Integer(_) | ExprKind::Real(_) | ExprKind::Constant(_) => {
            false
        }
    }
}

fn root_high_z_driver(expr: &SvExpr) -> Option<(ValueOperator, &SvExpr, &SvExpr)> {
    match &expr.kind {
        ExprKind::Group(inner) => root_high_z_driver(inner),
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } if is_z(else_expr) => Some((ValueOperator::BufIf1, then_expr, condition)),
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } if is_z(then_expr) => Some((ValueOperator::BufIf0, else_expr, condition)),
        _ => None,
    }
}

fn is_z(expr: &SvExpr) -> bool {
    match &expr.kind {
        ExprKind::Constant(ConstKind::Z) => true,
        ExprKind::Group(inner) => is_z(inner),
        _ => false,
    }
}

fn parse_strength(strength: &Strength) -> StrengthPair {
    parse_strength_values(&strength.values, &strength.span)
}

fn parse_strength_values(values: &[String], span: &sv_to_sexpr::diagnostic::Span) -> StrengthPair {
    let [first, second] = values else {
        panic!("corpus strength at {span:?} must contain exactly two values: {values:?}");
    };
    StrengthPair::parse(first, second)
        .unwrap_or_else(|| panic!("unknown corpus strength pair at {span:?}: ({first}, {second})"))
}

fn repeated_targets(inventory: &SourceInventory) -> Vec<ScopedRepeat> {
    let mut repeated = Vec::new();
    for scope in &inventory.driver_scopes {
        let mut counts = BTreeMap::new();
        for driver in scope
            .drivers
            .iter()
            .filter(|driver| !matches!(driver.source, DriverSource::Initial))
        {
            *counts.entry(driver.target.clone()).or_insert(0usize) += 1;
        }
        repeated.extend(counts.into_iter().filter(|(_, count)| *count > 1).map(
            |(target, occurrences)| ScopedRepeat {
                scope: scope.label.clone(),
                target,
                occurrences,
            },
        ));
    }
    repeated
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

fn operation(expr: &Expr) -> Option<(ValueOperator, &[Expr])> {
    let Expr::List(items) = expr else {
        return None;
    };
    let (head, operands) = items.split_first().expect("validated value operation");
    let Expr::Atom(head) = head else {
        panic!("validated value head must be an atom");
    };
    Some((
        ValueOperator::parse(head).expect("validated contracted value operator"),
        operands,
    ))
}

fn audit_success_cell(cell: &Cell, inventory: &SourceInventory, invariants: &mut Invariants) {
    if cell.validate().is_err() {
        invariants.invalid_cells += 1;
    }
    let assignments = assignments(cell);
    let generated = assignments
        .iter()
        .filter(|assignment| !inventory.source_names.contains(&assignment.target))
        .map(|assignment| assignment.target.clone())
        .collect::<BTreeSet<_>>();
    let mut available = BTreeSet::new();
    for assignment in assignments {
        if generated.contains(&assignment.target) {
            if inventory.source_names.contains(&assignment.target) {
                invariants.temp_collisions += 1;
            }
            if temp_index(&assignment.target).is_none() {
                invariants.temp_collisions += 1;
            }
        }
        match &assignment.expr {
            Expr::Atom(atom) => {
                if atom == "z" {
                    invariants.ordinary_z_values += 1;
                }
            }
            Expr::List(items) => {
                let Some((_, operands)) = operation(&assignment.expr) else {
                    invariants.nested_values += 1;
                    continue;
                };
                if items.is_empty() {
                    invariants.nested_values += 1;
                }
                for operand in operands {
                    match operand {
                        Expr::List(_) => invariants.nested_values += 1,
                        Expr::Atom(atom) => {
                            if atom == "z" {
                                invariants.ordinary_z_values += 1;
                            }
                            if generated.contains(atom) && !available.contains(atom) {
                                invariants.temp_dependency_failures += 1;
                            }
                        }
                    }
                }
            }
        }
        if generated.contains(&assignment.target) && !available.insert(assignment.target.clone()) {
            invariants.temp_collisions += 1;
        }
    }
}

fn audit_relevant_success(
    path: &str,
    cell: &Cell,
    inventory: &SourceInventory,
    repeated: &[ScopedRepeat],
    invariants: &mut Invariants,
) -> EmittedForms {
    let assignments = assignments(cell);
    let emitted_source = assignments
        .iter()
        .copied()
        .filter(|assignment| inventory.source_names.contains(&assignment.target))
        .collect::<Vec<_>>();
    let source_drivers = inventory
        .drivers
        .iter()
        .filter(|driver| !matches!(driver.source, DriverSource::Initial))
        .collect::<Vec<_>>();
    let source_targets = source_drivers
        .iter()
        .map(|driver| driver.target.as_str())
        .collect::<Vec<_>>();
    let emitted_targets = emitted_source
        .iter()
        .map(|assignment| assignment.target.as_str())
        .collect::<Vec<_>>();
    if source_targets != emitted_targets {
        invariants.target_order_mismatches += 1;
        if !repeated.is_empty() {
            invariants.repeated_collapse_or_reorder += 1;
        }
    }

    let emitted = count_emitted_forms(&assignments);
    let source_pairs = inventory
        .strengths
        .iter()
        .map(|entry| entry.pair)
        .collect::<Vec<_>>();
    let emitted_pairs = assignments
        .iter()
        .filter_map(|assignment| strength_pair(&assignment.expr))
        .collect::<Vec<_>>();
    if source_pairs.len() > emitted_pairs.len() {
        invariants.strength_lost += source_pairs.len() - emitted_pairs.len();
    } else if emitted_pairs.len() > source_pairs.len() {
        invariants.strength_invented += emitted_pairs.len() - source_pairs.len();
    }
    invariants.strength_mismatch += source_pairs
        .iter()
        .zip(&emitted_pairs)
        .filter(|(source, emitted)| source != emitted)
        .count();

    for construct in &inventory.constructs {
        let Some(driver_index) = source_drivers.iter().position(|driver| {
            driver.source_order == construct.source_order && driver.target == construct.target
        }) else {
            invariants.unaccounted_m6_constructs += 1;
            continue;
        };
        let Some(assignment) = emitted_source.get(driver_index) else {
            invariants.unaccounted_m6_constructs += 1;
            continue;
        };
        let assignment_index = assignments
            .iter()
            .position(|candidate| std::ptr::eq(*candidate, *assignment))
            .expect("source assignment must be in the cell");
        audit_construct(
            path,
            construct,
            assignment,
            assignment_index,
            &assignments,
            inventory,
            invariants,
        );
    }

    let registers = cell.registers.iter().cloned().collect::<BTreeSet<_>>();
    let mut target_sources: BTreeMap<&str, Vec<&DriverSource>> = BTreeMap::new();
    for driver in source_drivers {
        target_sources
            .entry(driver.target.as_str())
            .or_default()
            .push(&driver.source);
    }
    for (target, sources) in target_sources {
        if sources.iter().all(|source| {
            matches!(
                source,
                DriverSource::Continuous
                    | DriverSource::Primitive { .. }
                    | DriverSource::Keeper { .. }
            )
        }) && registers.contains(target)
        {
            invariants.driver_register_leaks += 1;
        }
    }
    emitted
}

fn audit_construct(
    path: &str,
    construct: &M6Construct,
    assignment: &Assignment,
    assignment_index: usize,
    assignments: &[&Assignment],
    inventory: &SourceInventory,
    invariants: &mut Invariants,
) {
    let Some((actual_operator, operands)) = operation(&assignment.expr) else {
        invariants.polarity_form_mismatches += 1;
        return;
    };
    let (base_operator, drive, control, strength) = match &construct.kind {
        M6ConstructKind::ContinuousHighZ {
            operator,
            drive,
            control,
            strength,
        }
        | M6ConstructKind::DirectBufIf {
            operator,
            drive,
            control,
            strength,
        } => (*operator, drive, Some(control), *strength),
        M6ConstructKind::ContinuousStrength { value, strength } => {
            (ValueOperator::DriveStrength, value, None, Some(*strength))
        }
    };
    let expected_operator = match (base_operator, strength) {
        (ValueOperator::BufIf0, Some(_)) => ValueOperator::BufIf0Strength,
        (ValueOperator::BufIf1, Some(_)) => ValueOperator::BufIf1Strength,
        (ValueOperator::DriveStrength, Some(_)) => ValueOperator::DriveStrength,
        (operator, None) => operator,
        _ => unreachable!("contracted M6 source operator"),
    };
    if actual_operator != expected_operator || actual_operator == ValueOperator::Mux {
        invariants.polarity_form_mismatches += 1;
        return;
    }
    let ordinary_count = if control.is_some() { 2 } else { 1 };
    if operands.len() != ordinary_count + usize::from(strength.is_some()) * 2 {
        invariants.polarity_form_mismatches += 1;
        return;
    }
    audit_source_operand(
        path,
        drive,
        &operands[0],
        assignment_index,
        assignments,
        inventory,
        invariants,
    );
    if let Some(control) = control {
        audit_source_operand(
            path,
            control,
            &operands[1],
            assignment_index,
            assignments,
            inventory,
            invariants,
        );
    }
    if let Some(expected_pair) = strength {
        let actual_pair = strength_pair(&assignment.expr);
        if actual_pair != Some(expected_pair) {
            invariants.strength_mismatch += 1;
        }
    }
}

fn audit_source_operand(
    path: &str,
    source: &SvExpr,
    emitted: &Expr,
    assignment_index: usize,
    assignments: &[&Assignment],
    inventory: &SourceInventory,
    invariants: &mut Invariants,
) {
    let Expr::Atom(emitted) = emitted else {
        invariants.nested_values += 1;
        return;
    };
    if let Some(expected) = simple_atom(source) {
        if emitted != &expected {
            invariants.polarity_form_mismatches += 1;
        }
        return;
    }
    if inventory.source_names.contains(emitted) {
        invariants.polarity_form_mismatches += 1;
        return;
    }
    let Some(dependency_index) = assignments
        .iter()
        .position(|assignment| assignment.target == *emitted)
    else {
        invariants.unaccounted_m6_constructs += 1;
        return;
    };
    if dependency_index >= assignment_index {
        invariants.temp_dependency_failures += 1;
        return;
    }
    audit_source_expression(
        path,
        source,
        &assignments[dependency_index].expr,
        dependency_index,
        assignments,
        inventory,
        invariants,
    );
}

fn audit_source_expression(
    path: &str,
    source: &SvExpr,
    emitted: &Expr,
    assignment_index: usize,
    assignments: &[&Assignment],
    inventory: &SourceInventory,
    invariants: &mut Invariants,
) {
    let Some((expected_operator, source_operands)) = source_operation(source) else {
        invariants.polarity_form_mismatches += 1;
        return;
    };
    let Some((emitted_operator, emitted_operands)) = operation(emitted) else {
        invariants.polarity_form_mismatches += 1;
        return;
    };
    if emitted_operator != expected_operator || emitted_operands.len() != source_operands.len() {
        invariants.polarity_form_mismatches += 1;
        return;
    }
    for (source_operand, emitted_operand) in source_operands.iter().zip(emitted_operands) {
        audit_source_operand(
            path,
            source_operand,
            emitted_operand,
            assignment_index,
            assignments,
            inventory,
            invariants,
        );
    }
}

fn source_operation(expr: &SvExpr) -> Option<(ValueOperator, Vec<&SvExpr>)> {
    match &expr.kind {
        ExprKind::Group(inner) => source_operation(inner),
        ExprKind::Unary {
            op: UnaryOp::Not | UnaryOp::BitNot,
            expr: operand,
        } => match &operand.kind {
            ExprKind::Group(inner) => source_not_operation(inner),
            _ => source_not_operation(operand),
        },
        ExprKind::Unary {
            op: UnaryOp::Plus | UnaryOp::Minus,
            ..
        } => None,
        ExprKind::Binary { op, left, right } => {
            let operator = match op {
                BinaryOp::BitAnd | BinaryOp::LogicalAnd => {
                    let mut operands = Vec::new();
                    collect_source_and_operands(left, &mut operands);
                    collect_source_and_operands(right, &mut operands);
                    return Some((ValueOperator::And, operands));
                }
                BinaryOp::BitOr | BinaryOp::LogicalOr => {
                    let mut operands = Vec::new();
                    collect_source_or_operands(left, &mut operands);
                    collect_source_or_operands(right, &mut operands);
                    return Some((ValueOperator::Or, operands));
                }
                BinaryOp::BitXor => ValueOperator::Xor,
                BinaryOp::BitNand => ValueOperator::Nand,
                BinaryOp::BitNor => ValueOperator::Nor,
                BinaryOp::BitXnor => ValueOperator::Xnor,
                BinaryOp::Eq => ValueOperator::Eq,
                BinaryOp::CaseEq => ValueOperator::CaseEq,
                BinaryOp::Neq => ValueOperator::Neq,
                BinaryOp::CaseNeq => ValueOperator::CaseNeq,
                BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Less
                | BinaryOp::Greater => return None,
            };
            Some((operator, vec![left, right]))
        }
        ExprKind::Ternary {
            condition,
            then_expr,
            else_expr,
        } if !expr_contains_z(expr) => {
            Some((ValueOperator::Mux, vec![condition, then_expr, else_expr]))
        }
        ExprKind::Path(_)
        | ExprKind::Integer(_)
        | ExprKind::Real(_)
        | ExprKind::Constant(_)
        | ExprKind::Ternary { .. }
        | ExprKind::Call { .. } => None,
    }
}

fn source_not_operation(expr: &SvExpr) -> Option<(ValueOperator, Vec<&SvExpr>)> {
    match &expr.kind {
        ExprKind::Group(inner) => source_not_operation(inner),
        ExprKind::Binary {
            op: BinaryOp::BitAnd | BinaryOp::LogicalAnd,
            left,
            right,
        } => {
            let mut operands = Vec::new();
            collect_source_and_operands(left, &mut operands);
            collect_source_and_operands(right, &mut operands);
            Some((ValueOperator::Nand, operands))
        }
        ExprKind::Binary {
            op: BinaryOp::BitOr | BinaryOp::LogicalOr,
            left,
            right,
        } => {
            let mut operands = Vec::new();
            collect_source_or_operands(left, &mut operands);
            collect_source_or_operands(right, &mut operands);
            Some((ValueOperator::Nor, operands))
        }
        ExprKind::Binary {
            op: BinaryOp::BitXor,
            left,
            right,
        } => Some((ValueOperator::Xnor, vec![left, right])),
        _ => Some((ValueOperator::Not, vec![expr])),
    }
}

fn collect_source_and_operands<'a>(expr: &'a SvExpr, operands: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary {
            op: BinaryOp::BitAnd | BinaryOp::LogicalAnd,
            left,
            right,
        } => {
            collect_source_and_operands(left, operands);
            collect_source_and_operands(right, operands);
        }
        _ => operands.push(expr),
    }
}

fn collect_source_or_operands<'a>(expr: &'a SvExpr, operands: &mut Vec<&'a SvExpr>) {
    match &expr.kind {
        ExprKind::Binary {
            op: BinaryOp::BitOr | BinaryOp::LogicalOr,
            left,
            right,
        } => {
            collect_source_or_operands(left, operands);
            collect_source_or_operands(right, operands);
        }
        _ => operands.push(expr),
    }
}

fn simple_atom(expr: &SvExpr) -> Option<String> {
    match &expr.kind {
        ExprKind::Path(segments) => Some(segments.join("::")),
        ExprKind::Integer(value) | ExprKind::Real(value) => Some(value.clone()),
        ExprKind::Constant(ConstKind::Zero) => Some("0".to_string()),
        ExprKind::Constant(ConstKind::One) => Some("1".to_string()),
        ExprKind::Constant(ConstKind::X) => Some("x".to_string()),
        ExprKind::Constant(ConstKind::Z) => Some("z".to_string()),
        ExprKind::Group(inner) => simple_atom(inner),
        ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Ternary { .. }
        | ExprKind::Call { .. } => None,
    }
}

fn count_emitted_forms(assignments: &[&Assignment]) -> EmittedForms {
    let mut forms = EmittedForms::default();
    for assignment in assignments {
        let Some((operator, _)) = operation(&assignment.expr) else {
            continue;
        };
        match operator {
            ValueOperator::BufIf0 => forms.bufif0 += 1,
            ValueOperator::BufIf1 => forms.bufif1 += 1,
            ValueOperator::DriveStrength => forms.drive_strength += 1,
            ValueOperator::BufIf0Strength => forms.bufif0_strength += 1,
            ValueOperator::BufIf1Strength => forms.bufif1_strength += 1,
            _ => {}
        }
    }
    forms
}

impl EmittedForms {
    fn add(&mut self, other: &Self) {
        self.bufif0 += other.bufif0;
        self.bufif1 += other.bufif1;
        self.drive_strength += other.drive_strength;
        self.bufif0_strength += other.bufif0_strength;
        self.bufif1_strength += other.bufif1_strength;
    }
}

fn strength_pair(expr: &Expr) -> Option<StrengthPair> {
    let (operator, operands) = operation(expr)?;
    if !matches!(
        operator,
        ValueOperator::DriveStrength
            | ValueOperator::BufIf0Strength
            | ValueOperator::BufIf1Strength
    ) {
        return None;
    }
    let [.., Expr::Atom(first), Expr::Atom(second)] = operands else {
        return None;
    };
    StrengthPair::parse(first, second)
}

fn temp_index(name: &str) -> Option<usize> {
    name.strip_prefix('t')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

fn pair_label(pair: StrengthPair) -> String {
    let (first, second) = pair.atoms();
    format!("{first}/{second}")
}

fn later_blocker(path: &str) -> (&'static str, &'static str) {
    assert!(
        M7_FAILURES.contains(&path),
        "M6-relevant lower failure must have one later blocker: {path}"
    );
    (
        "M7",
        "M7 timing-factor lowering remains; M6 bufif polarity and strength are structurally inventoried",
    )
}

fn assert_exact_audit(audit: &Audit) {
    assert_eq!(audit.processed, 206);
    assert_eq!(audit.succeeded, 206);
    assert_eq!(audit.failed, 0);
    assert_eq!(audit.relevant_successes.len(), 67);
    assert_eq!(audit.relevant_deferrals.len(), 0);
    assert_eq!(audit.source_keepers, 6);
    assert_eq!(audit.emitted_keepers, 6);
    assert_eq!(audit.deferred_keepers, 0);
    assert_eq!(
        audit
            .relevant_successes
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_RELEVANT_SUCCESSES
    );
    assert_eq!(
        audit
            .relevant_deferrals
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_RELEVANT_DEFERRALS
    );

    let inventories = audit
        .relevant_successes
        .iter()
        .map(|file| &file.inventory)
        .chain(audit.relevant_deferrals.iter().map(|file| &file.inventory))
        .collect::<Vec<_>>();
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.contains_z_continuous)
            .sum::<usize>(),
        63
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.high_z_bufif0)
            .sum::<usize>(),
        2
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.high_z_bufif1)
            .sum::<usize>(),
        61
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.direct_bufif0)
            .sum::<usize>(),
        104
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.direct_bufif1)
            .sum::<usize>(),
        293
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.continuous_strength)
            .sum::<usize>(),
        56
    );
    assert_eq!(
        inventories
            .iter()
            .map(|inventory| inventory.primitive_strength)
            .sum::<usize>(),
        397
    );
    assert_eq!(
        audit.source_pair_counts,
        BTreeMap::from([
            ("highz1/strong0".to_string(), 338),
            ("pull1/highz0".to_string(), 3),
            ("strong1/highz0".to_string(), 108),
            ("supply1/supply0".to_string(), 4),
        ])
    );
    assert_eq!(audit.emitted_forms.bufif0, 2);
    assert_eq!(audit.emitted_forms.bufif1, 10);
    assert_eq!(audit.emitted_forms.drive_strength, 5);
    assert_eq!(audit.emitted_forms.bufif0_strength, 104);
    assert_eq!(audit.emitted_forms.bufif1_strength, 344);

    assert_eq!(audit.repeated_entries.len(), 58);
    assert_eq!(
        audit
            .repeated_entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        26
    );
    assert_eq!(
        audit
            .repeated_entries
            .iter()
            .map(|entry| entry.source_occurrences)
            .sum::<usize>(),
        135
    );
    assert_eq!(
        audit
            .repeated_entries
            .iter()
            .filter_map(|entry| entry.emitted_occurrences)
            .sum::<usize>(),
        135
    );
    assert_eq!(
        audit
            .repeated_entries
            .iter()
            .filter(|entry| entry.emitted_occurrences.is_none())
            .map(|entry| entry.source_occurrences)
            .sum::<usize>(),
        0
    );
    assert!(
        audit
            .repeated_entries
            .iter()
            .all(|entry| entry.scope == "root"),
        "all current corpus repeats must be local to the module root scope"
    );
    assert_eq!(
        audit
            .repeated_entries
            .iter()
            .map(|entry| {
                (
                    entry.path.as_str(),
                    entry.target.as_str(),
                    entry.source_occurrences,
                    entry.emitted_occurrences,
                )
            })
            .collect::<Vec<_>>(),
        EXPECTED_REPEATED_TARGETS
    );

    assert!(audit.relevant_deferrals.is_empty());

    let invariants = &audit.invariants;
    assert_eq!(invariants.invalid_cells, 0);
    assert_eq!(invariants.nested_values, 0);
    assert_eq!(invariants.ordinary_z_values, 0);
    assert_eq!(invariants.target_order_mismatches, 0);
    assert_eq!(invariants.unaccounted_m6_constructs, 0);
    assert_eq!(invariants.strength_lost, 0);
    assert_eq!(invariants.strength_mismatch, 0);
    assert_eq!(invariants.strength_invented, 0);
    assert_eq!(invariants.polarity_form_mismatches, 0);
    assert_eq!(invariants.repeated_collapse_or_reorder, 0);
    assert_eq!(invariants.temp_dependency_failures, 0);
    assert_eq!(invariants.temp_collisions, 0);
    assert_eq!(invariants.driver_register_leaks, 0);
    assert_eq!(invariants.nondeterministic_results, 0);
    assert_eq!(invariants.absolute_path_leaks, 0);
}

fn render_summary(audit: &Audit) -> String {
    let source_high_z_bufif0 = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.high_z_bufif0)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.high_z_bufif0)
            .sum::<usize>();
    let source_high_z_bufif1 = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.high_z_bufif1)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.high_z_bufif1)
            .sum::<usize>();
    let source_continuous_strength = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.continuous_strength)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.continuous_strength)
            .sum::<usize>();
    let source_direct_bufif0 = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.direct_bufif0)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.direct_bufif0)
            .sum::<usize>();
    let source_direct_bufif1 = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.direct_bufif1)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.direct_bufif1)
            .sum::<usize>();
    let source_primitive_strength = audit
        .relevant_successes
        .iter()
        .map(|file| file.inventory.primitive_strength)
        .sum::<usize>()
        + audit
            .relevant_deferrals
            .iter()
            .map(|file| file.inventory.primitive_strength)
            .sum::<usize>();
    let repeated_paths = audit
        .repeated_entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let repeated_source_occurrences = audit
        .repeated_entries
        .iter()
        .map(|entry| entry.source_occurrences)
        .sum::<usize>();
    let repeated_emitted_occurrences = audit
        .repeated_entries
        .iter()
        .filter_map(|entry| entry.emitted_occurrences)
        .sum::<usize>();
    let repeated_deferred_occurrences = audit
        .repeated_entries
        .iter()
        .filter(|entry| entry.emitted_occurrences.is_none())
        .map(|entry| entry.source_occurrences)
        .sum::<usize>();

    let mut output = String::new();
    writeln!(&mut output, "driver corpus audit").unwrap();
    writeln!(
        &mut output,
        "corpus-files={} relevant-files={} relevant-successes={} relevant-later-deferrals={}",
        audit.processed,
        audit.relevant_successes.len() + audit.relevant_deferrals.len(),
        audit.relevant_successes.len(),
        audit.relevant_deferrals.len()
    )
    .unwrap();
    writeln!(
        &mut output,
        "global-lowering: succeeded={} failed={}",
        audit.succeeded, audit.failed
    )
    .unwrap();
    writeln!(
        &mut output,
        "keeper-drivers: source={} emitted={} deferred={}",
        audit.source_keepers, audit.emitted_keepers, audit.deferred_keepers
    )
    .unwrap();
    writeln!(&mut output, "source-forms:").unwrap();
    writeln!(&mut output, "  high-z-bufif0={source_high_z_bufif0}").unwrap();
    writeln!(&mut output, "  high-z-bufif1={source_high_z_bufif1}").unwrap();
    writeln!(&mut output, "  direct-bufif0={source_direct_bufif0}").unwrap();
    writeln!(&mut output, "  direct-bufif1={source_direct_bufif1}").unwrap();
    writeln!(
        &mut output,
        "  continuous-strength={source_continuous_strength}"
    )
    .unwrap();
    writeln!(
        &mut output,
        "  primitive-strength={source_primitive_strength}"
    )
    .unwrap();
    writeln!(&mut output, "source-strength-pairs:").unwrap();
    for (pair, count) in &audit.source_pair_counts {
        writeln!(&mut output, "  {pair}={count}").unwrap();
    }
    writeln!(&mut output, "emitted-success-forms:").unwrap();
    writeln!(&mut output, "  bufif0={}", audit.emitted_forms.bufif0).unwrap();
    writeln!(&mut output, "  bufif1={}", audit.emitted_forms.bufif1).unwrap();
    writeln!(
        &mut output,
        "  drive-strength={}",
        audit.emitted_forms.drive_strength
    )
    .unwrap();
    writeln!(
        &mut output,
        "  bufif0-strength={}",
        audit.emitted_forms.bufif0_strength
    )
    .unwrap();
    writeln!(
        &mut output,
        "  bufif1-strength={}",
        audit.emitted_forms.bufif1_strength
    )
    .unwrap();
    writeln!(&mut output, "repeated-drivers:").unwrap();
    writeln!(&mut output, "  paths={repeated_paths}").unwrap();
    writeln!(&mut output, "  targets={}", audit.repeated_entries.len()).unwrap();
    writeln!(
        &mut output,
        "  source-occurrences={repeated_source_occurrences}"
    )
    .unwrap();
    writeln!(
        &mut output,
        "  emitted-success-occurrences={repeated_emitted_occurrences}"
    )
    .unwrap();
    writeln!(
        &mut output,
        "  deferred-source-occurrences={repeated_deferred_occurrences}"
    )
    .unwrap();
    writeln!(&mut output, "invariants:").unwrap();
    for (name, value) in [
        ("invalid-cells", audit.invariants.invalid_cells),
        ("nested-values", audit.invariants.nested_values),
        ("ordinary-z-values", audit.invariants.ordinary_z_values),
        (
            "target-order-mismatches",
            audit.invariants.target_order_mismatches,
        ),
        (
            "unaccounted-m6-constructs",
            audit.invariants.unaccounted_m6_constructs,
        ),
        ("strength-lost", audit.invariants.strength_lost),
        ("strength-mismatch", audit.invariants.strength_mismatch),
        ("strength-invented", audit.invariants.strength_invented),
        (
            "polarity-form-mismatches",
            audit.invariants.polarity_form_mismatches,
        ),
        (
            "repeated-collapse-or-reorder",
            audit.invariants.repeated_collapse_or_reorder,
        ),
        (
            "temp-dependency-failures",
            audit.invariants.temp_dependency_failures,
        ),
        ("temp-collisions", audit.invariants.temp_collisions),
        (
            "driver-register-leaks",
            audit.invariants.driver_register_leaks,
        ),
        (
            "nondeterministic-results",
            audit.invariants.nondeterministic_results,
        ),
        ("absolute-path-leaks", audit.invariants.absolute_path_leaks),
    ] {
        writeln!(&mut output, "  {name}={value}").unwrap();
    }
    writeln!(&mut output, "relevant-successes:").unwrap();
    for file in &audit.relevant_successes {
        writeln!(
            &mut output,
            "  {} source[hiz0={},hiz1={},bufif0={},bufif1={},continuous-strength={},primitive-strength={},repeated-targets={}] emitted[bufif0={},bufif1={},drive-strength={},bufif0-strength={},bufif1-strength={}]",
            file.path,
            file.inventory.high_z_bufif0,
            file.inventory.high_z_bufif1,
            file.inventory.direct_bufif0,
            file.inventory.direct_bufif1,
            file.inventory.continuous_strength,
            file.inventory.primitive_strength,
            file.repeated.len(),
            file.emitted.bufif0,
            file.emitted.bufif1,
            file.emitted.drive_strength,
            file.emitted.bufif0_strength,
            file.emitted.bufif1_strength,
        )
        .unwrap();
    }
    writeln!(&mut output, "repeated-target-inventory:").unwrap();
    for entry in &audit.repeated_entries {
        let emitted = entry
            .emitted_occurrences
            .map(|count| count.to_string())
            .unwrap_or_else(|| "later".to_string());
        writeln!(
            &mut output,
            "  {} [{}] {} source={} emitted={emitted}",
            entry.path, entry.scope, entry.target, entry.source_occurrences
        )
        .unwrap();
    }
    writeln!(&mut output, "relevant-later-deferrals:").unwrap();
    for file in &audit.relevant_deferrals {
        writeln!(
            &mut output,
            "  {}:{}:{}: {} | {} | {} | source[hiz0={},hiz1={},bufif0={},bufif1={},continuous-strength={},primitive-strength={},repeated-targets={}]",
            file.diagnostic.span.path.display(),
            file.diagnostic.span.line,
            file.diagnostic.span.column,
            file.diagnostic.message,
            file.category,
            file.rationale,
            file.inventory.high_z_bufif0,
            file.inventory.high_z_bufif1,
            file.inventory.direct_bufif0,
            file.inventory.direct_bufif1,
            file.inventory.continuous_strength,
            file.inventory.primitive_strength,
            file.repeated.len(),
        )
        .unwrap();
    }
    output
}
