use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sv_to_sexpr::analyze::{
    AnalysisReport, ModuleCatalog, analyze_design_with_catalog_and_generate_mode,
    analyze_design_with_catalog_structural,
};
use sv_to_sexpr::ast::Design;
use sv_to_sexpr::elaborate::GenerateMode;
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::survey::collect_sv_files;

pub struct CorpusAnalysis {
    pub repository_root: PathBuf,
    pub designs: BTreeMap<String, Design>,
    pub catalog: ModuleCatalog,
}

impl CorpusAnalysis {
    pub fn analyze(&self, logical_path: &str) -> AnalysisReport {
        let design = self
            .designs
            .get(logical_path)
            .unwrap_or_else(|| panic!("missing parsed corpus design {logical_path}"));
        analyze_design_with_catalog_and_generate_mode(design, &self.catalog, GenerateMode::Delayful)
            .unwrap()
    }

    pub fn analyze_structural(&self, logical_path: &str) -> AnalysisReport {
        let design = self
            .designs
            .get(logical_path)
            .unwrap_or_else(|| panic!("missing parsed corpus design {logical_path}"));
        analyze_design_with_catalog_structural(design, &self.catalog).unwrap()
    }
}

pub fn corpus() -> &'static CorpusAnalysis {
    static CORPUS: OnceLock<CorpusAnalysis> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repository_root = manifest.parent().unwrap().to_path_buf();
        let corpus_root = repository_root.join("sv-cells");
        let mut designs = BTreeMap::new();
        for source_path in collect_sv_files(&corpus_root).unwrap() {
            let logical_path = source_path
                .strip_prefix(&repository_root)
                .unwrap()
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let input = fs::read_to_string(&source_path).unwrap();
            let design = parse_file(Path::new(&logical_path), &input).unwrap();
            designs.insert(logical_path, design);
        }
        let catalog =
            ModuleCatalog::from_designs(&designs.values().cloned().collect::<Vec<_>>()).unwrap();
        CorpusAnalysis {
            repository_root,
            designs,
            catalog,
        }
    })
}

pub fn assert_or_update_fixture(name: &str, actual: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/analysis")
        .join(name);
    if std::env::var_os("UPDATE_ANALYSIS_GOLDENS").is_some() {
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::write(&fixture, actual).unwrap();
    }
    let expected = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture.display()));
    assert_eq!(
        actual,
        expected,
        "analysis fixture {} changed",
        fixture.display()
    );
}
