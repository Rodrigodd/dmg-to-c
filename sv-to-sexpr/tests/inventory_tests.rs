use std::path::{Path, PathBuf};

use sv_to_sexpr::inventory::ClassificationKind;
use sv_to_sexpr::survey::{SurveyReport, survey_dir};

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate must be inside the repository")
        .join("sv-cells")
}

fn assert_file_mapping(report: &SurveyReport, id: &str, path: &str) {
    let record = report
        .inventory
        .record_by_id(id)
        .unwrap_or_else(|| panic!("missing capability `{id}`"));
    assert!(
        record.files.contains(path),
        "capability `{id}` does not contain `{path}`; files={:?}",
        record.files
    );
}

#[test]
fn full_corpus_inventory_is_complete_normalized_and_deterministic() {
    let root = corpus_root();
    let first = survey_dir(&root).unwrap();
    let second = survey_dir(&root).unwrap();

    assert_eq!(first.files, 206);
    assert_eq!(first.failed_files, 0);
    assert!(first.failures.is_empty());
    assert!(first.inventory.all_classified());
    assert_eq!(first.inventory.unsupported_count(), 0);
    assert_eq!(
        first
            .inventory
            .classification_count(ClassificationKind::Unsupported),
        0
    );
    assert_eq!(first, second);

    let rendered = first.render();
    assert_eq!(rendered, second.render());
    assert!(!rendered.contains(&root.to_string_lossy().to_string()));
    assert!(!rendered.contains(env!("CARGO_MANIFEST_DIR")));
    assert!(rendered.contains("unsupported capabilities: 0\n"));

    for (id, file) in [
        ("statement.item.module.generate", "dmg_cpu_b/cells/dffr.sv"),
        (
            "hierarchy.instantiation.ordinary-known.dmg_and2",
            "dmg_cpu_b/cells/half_add.sv",
        ),
        (
            "hierarchy.instantiation.parameter.named",
            "dmg_cpu_b/cells/half_add.sv",
        ),
        (
            "hierarchy.instantiation.connection.named",
            "dmg_cpu_b/cells/full_add.sv",
        ),
        (
            "hierarchy.instantiation.keeper.keeper",
            "sm83/cells/dlatch_ee_irq.sv",
        ),
        (
            "strength.strong1-highz0",
            "sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        ),
        ("strength.highz1-strong0", "dmg_cpu_b/cells/pad_bidir_pu.sv"),
        ("strength.pull1-highz0", "dmg_cpu_b/cells/pad_bidir_pu.sv"),
        ("strength.supply1-supply0", "dmg_cpu_b/cells/tie.sv"),
        (
            "timing.delay.primitive.arity-3",
            "sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        ),
        (
            "statement.item.module.specify-path",
            "sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        ),
        ("primitive.nmos", "sm83/cells/irq_prio_bit0.sv"),
        ("primitive.pmos", "sm83/cells/irq_prio_bit0.sv"),
        ("primitive.rnmos", "sm83/cells/dlatch_ee_irq.sv"),
        (
            "expression.value.constant-zero",
            "dmg_cpu_b/cells/pad_bidir_pu.sv",
        ),
        (
            "expression.value.constant-one",
            "sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv",
        ),
        ("expression.value.constant-x", "sm83/cells/alu_cgen.sv"),
        ("expression.value.constant-z", "sm83/cells/reg_a_out.sv"),
    ] {
        assert_file_mapping(&first, id, file);
    }
}

#[test]
fn single_file_inventory_uses_only_its_file_name() {
    let file = corpus_root().join("dmg_cpu_b/cells/half_add.sv");
    let report = survey_dir(Path::new(&file)).unwrap();
    assert_eq!(report.files, 1);
    assert_eq!(report.failed_files, 0);
    assert!(report.inventory.records().values().all(
        |record| record.files == std::collections::BTreeSet::from(["half_add.sv".to_string()])
    ));
}
