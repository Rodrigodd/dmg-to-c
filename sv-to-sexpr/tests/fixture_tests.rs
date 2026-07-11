mod ast_support;

use std::fs;
use std::path::{Path, PathBuf};

use ast_support::{AstCoverage, assert_or_update_fixture};
use sv_to_sexpr::ast::render_design;
use sv_to_sexpr::lexer::{Keyword, TokenKind, lex_file};
use sv_to_sexpr::parser::parse_file;
use sv_to_sexpr::survey::collect_sv_files;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repository_root() -> PathBuf {
    manifest_dir()
        .parent()
        .expect("crate must be inside the repository")
        .to_path_buf()
}

fn fixture_path(name: &str) -> PathBuf {
    manifest_dir()
        .join("tests/fixtures/ast")
        .join(format!("{name}.ast"))
}

fn parse_repository_file(relative: &str) -> sv_to_sexpr::ast::Design {
    let contents = fs::read_to_string(repository_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
    parse_file(Path::new(relative), &contents)
        .unwrap_or_else(|error| panic!("failed to parse {relative}: {error}"))
}

#[test]
fn corpus_family_ast_goldens_are_stable() {
    for (name, source) in [
        ("simple_gate", "sv-cells/dmg_cpu_b/cells/and2.sv"),
        ("latch", "sv-cells/dmg_cpu_b/cells/dlatch.sv"),
        ("generated_dff", "sv-cells/dmg_cpu_b/cells/dffr_cc.sv"),
        (
            "tri_state_strength",
            "sv-cells/dmg_cpu_b/cells/pad_bidir_pu.sv",
        ),
        ("hierarchical_adder", "sv-cells/dmg_cpu_b/cells/full_add.sv"),
        ("keeper", "sv-cells/dmg_cpu_b/cells/mux.sv"),
        ("specify_block", "sv-cells/dmg_cpu_b/cells/not_if0.sv"),
        ("transistor_irq", "sv-cells/sm83/cells/irq_prio_bit0.sv"),
    ] {
        let design = parse_repository_file(source);
        let first = render_design(&design);
        let second = render_design(&design);
        assert_eq!(first, second, "nondeterministic AST rendering for {source}");
        assert!(
            first.contains("Directive("),
            "missing directives in {source}"
        );
        assert!(
            first.contains("arguments: ["),
            "missing directive arguments"
        );
        assert!(
            first.contains(&format!("path: \"{source}\"")),
            "golden did not use its logical source path"
        );
        assert!(
            !first.contains(&repository_root().to_string_lossy().to_string()),
            "golden contains an absolute repository path"
        );
        assert_or_update_fixture(&fixture_path(name), &first);
    }
}

#[test]
fn full_corpus_ast_coverage_is_exhaustive_and_stable() {
    let corpus_root = repository_root().join("sv-cells");
    let files = collect_sv_files(&corpus_root).unwrap();
    assert_eq!(files.len(), 206);

    let mut coverage = AstCoverage::default();
    let mut source_directives = 0;
    let mut source_modules = 0;
    let mut source_endmodules = 0;

    for file in files {
        let relative = file
            .strip_prefix(repository_root())
            .expect("corpus file must be below repository root")
            .to_string_lossy()
            .replace('\\', "/");
        let contents = fs::read_to_string(&file).unwrap();
        let tokens = lex_file(Path::new(&relative), &contents).unwrap();
        source_directives += tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Directive)
            .count();
        source_modules += tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Keyword(Keyword::Module))
            .count();
        source_endmodules += tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Keyword(Keyword::Endmodule))
            .count();

        // A successful parse is the parser's EOF contract: parse_design returns
        // only after consuming every token. The top-level token/AST comparisons
        // below additionally freeze the source-order directive/module boundary.
        let design = parse_file(Path::new(&relative), &contents)
            .unwrap_or_else(|error| panic!("failed to parse {relative}: {error}"));
        coverage.visit_design(&design, &relative);
    }

    assert_eq!(coverage.count("design"), 206);
    assert_eq!(source_directives, 410);
    assert_eq!(coverage.count("design-item.directive"), source_directives);
    assert_eq!(source_modules, 206);
    assert_eq!(source_endmodules, source_modules);
    assert_eq!(coverage.count("design-item.module"), source_modules);
    assert_eq!(coverage.count("module"), source_modules);
    assert!(coverage.count("item.total") > source_modules);

    let rendered = coverage.render();
    assert!(!rendered.contains("unknown"));
    assert!(!rendered.contains("placeholder"));
    assert!(!rendered.contains("raw-text"));
    assert_or_update_fixture(&fixture_path("coverage"), &rendered);
}
