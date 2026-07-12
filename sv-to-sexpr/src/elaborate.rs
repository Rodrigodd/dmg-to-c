use crate::ast::{Block, Design, DesignItem, ExprKind, Item, ItemKind, Module};
use crate::diagnostic::Diagnostic;
use std::fmt;

/// The configured branch of the corpus-specific `if (nodelay)` generate form.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GenerateMode {
    /// Select the delay-bearing `else` branch. This is the converter default.
    #[default]
    Delayful,
    /// Select the explicitly requested `if (nodelay)` branch.
    Nodelay,
}

impl GenerateMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Delayful => "delayful",
            Self::Nodelay => "nodelay",
        }
    }
}

impl fmt::Display for GenerateMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Selects the configured branch of every supported module-level generate.
///
/// Selected branch children are spliced into the module at the generate item's
/// position. All retained nodes are cloned without rewriting their source spans.
pub fn elaborate_design(design: &Design, mode: GenerateMode) -> Result<Design, Diagnostic> {
    let mut items = Vec::with_capacity(design.items.len());
    for item in &design.items {
        items.push(match item {
            DesignItem::Directive(directive) => DesignItem::Directive(directive.clone()),
            DesignItem::Module(module) => DesignItem::Module(elaborate_module(module, mode)?),
        });
    }
    Ok(Design { items })
}

fn elaborate_module(module: &Module, mode: GenerateMode) -> Result<Module, Diagnostic> {
    let mut items = Vec::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Generate(block) => elaborate_generate(block, mode, &mut items)?,
            _ => items.push(item.clone()),
        }
    }
    Ok(Module {
        span: module.span.clone(),
        name: module.name.clone(),
        parameters: module.parameters.clone(),
        ports: module.ports.clone(),
        items,
    })
}

fn elaborate_generate(
    block: &Block,
    mode: GenerateMode,
    output: &mut Vec<Item>,
) -> Result<(), Diagnostic> {
    let alternative = match block.items.as_slice() {
        [] => {
            return Err(Diagnostic::new(
                block.span.clone(),
                "generate block must contain exactly one `if (nodelay)` alternative",
            ));
        }
        [item] => item,
        [_, extra, ..] => {
            return Err(Diagnostic::new(
                extra.span.clone(),
                "generate block must contain exactly one `if (nodelay)` alternative",
            ));
        }
    };
    let statement = match &alternative.kind {
        ItemKind::If(statement) => statement,
        _ => {
            return Err(Diagnostic::new(
                alternative.span.clone(),
                "generate block must contain an `if (nodelay)` alternative",
            ));
        }
    };

    match &statement.condition.kind {
        ExprKind::Path(path) if path.len() == 1 && path[0] == "nodelay" => {}
        _ => {
            return Err(Diagnostic::new(
                statement.condition.span.clone(),
                "unsupported generate condition; expected scalar `nodelay`",
            ));
        }
    }

    let selected = match mode {
        GenerateMode::Nodelay => statement.then_branch.as_ref(),
        GenerateMode::Delayful => statement.else_branch.as_deref().ok_or_else(|| {
            Diagnostic::new(
                statement.span.clone(),
                "`if (nodelay)` generate must have an `else` branch",
            )
        })?,
    };
    // Missing `else` is an unsupported generate shape in either mode, even
    // though the true branch could otherwise be selected.
    if statement.else_branch.is_none() {
        return Err(Diagnostic::new(
            statement.span.clone(),
            "`if (nodelay)` generate must have an `else` branch",
        ));
    }

    let selected_block = match &selected.kind {
        ItemKind::Block(block) => block,
        _ => {
            return Err(Diagnostic::new(
                selected.span.clone(),
                format!(
                    "selected {} generate branch must be a begin/end block",
                    mode.label()
                ),
            ));
        }
    };
    for child in &selected_block.items {
        if matches!(child.kind, ItemKind::Generate(_)) {
            return Err(Diagnostic::new(
                child.span.clone(),
                "nested generate blocks are unsupported",
            ));
        }
        output.push(child.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{analyze_design_structural, analyze_file_with_generate_mode};
    use crate::ir::{CellItem, Expr as IrExpr};
    use crate::lower::{lower_file, lower_file_with_generate_mode};
    use crate::parser::parse_file;
    use std::path::Path;

    const SELECTABLE: &str = r#"module selectable(input logic a, output logic y);
  logic outside;
  generate
    if (nodelay) begin
      logic fast;
      localparam realtime T_sel = 1;
      assign fast = a;
      assign #(T_sel) y = fast;
    end else begin
      logic slow;
      localparam realtime T_sel = 2;
      assign slow = ~a;
      assign #(T_sel) y = slow;
    end
  endgenerate
endmodule
"#;

    fn parse(source: &str) -> Design {
        parse_file(Path::new("generate_test.sv"), source).unwrap()
    }

    fn assignment_targets(lowered: &crate::ir::LoweredModule) -> Vec<&str> {
        lowered
            .cell
            .items
            .iter()
            .filter_map(|item| match item {
                CellItem::Assignment(assignment) => Some(assignment.target.as_str()),
                CellItem::Blank | CellItem::Comment(_) => None,
            })
            .collect()
    }

    #[test]
    fn default_selects_else_and_explicit_nodelay_selects_then() {
        let delayful = lower_file(Path::new("generate_test.sv"), SELECTABLE).unwrap();
        assert_eq!(assignment_targets(&delayful), ["slow", "y"]);
        assert_eq!(delayful.timing_aliases["T_sel"], IrExpr::Atom("2".into()));
        assert!(!delayful.cell.registers.iter().any(|name| name == "fast"));

        let nodelay = lower_file_with_generate_mode(
            Path::new("generate_test.sv"),
            SELECTABLE,
            GenerateMode::Nodelay,
        )
        .unwrap();
        assert_eq!(assignment_targets(&nodelay), ["fast", "y"]);
        assert_eq!(nodelay.timing_aliases["T_sel"], IrExpr::Atom("1".into()));
        assert!(!nodelay.cell.registers.iter().any(|name| name == "slow"));
    }

    #[test]
    fn selected_children_keep_source_order_and_spans() {
        let design = parse(SELECTABLE);
        let elaborated = elaborate_design(&design, GenerateMode::Delayful).unwrap();
        let original_generate = match &design.first_module().unwrap().items[1].kind {
            ItemKind::Generate(block) => block,
            _ => panic!("expected generate"),
        };
        let original_else = match &original_generate.items[0].kind {
            ItemKind::If(statement) => match &statement.else_branch.as_ref().unwrap().kind {
                ItemKind::Block(block) => block,
                _ => panic!("expected block"),
            },
            _ => panic!("expected if"),
        };
        let module = elaborated.first_module().unwrap();
        assert_eq!(module.items.len(), 5);
        assert_eq!(module.items[0].span.line, 2);
        assert_eq!(&module.items[1..], original_else.items.as_slice());
        assert_eq!(module.items[1].span.line, 10);
        assert_eq!(module.items[4].span.line, 13);
    }

    #[test]
    fn configured_analysis_contains_only_selected_declarations_and_aliases() {
        let report = analyze_file_with_generate_mode(
            Path::new("generate_test.sv"),
            SELECTABLE,
            GenerateMode::Nodelay,
        )
        .unwrap();
        let module = &report.modules[0];
        assert_eq!(
            module.declarations.keys().cloned().collect::<Vec<_>>(),
            ["fast", "outside"]
        );
        assert_eq!(
            module.localparams.keys().cloned().collect::<Vec<_>>(),
            ["T_sel"]
        );
        assert!(module.generate_alternatives.is_empty());
        assert_eq!(
            module
                .continuous_assignments
                .iter()
                .map(|assignment| assignment.target.as_str())
                .collect::<Vec<_>>(),
            ["fast", "y"]
        );

        let structural = analyze_design_structural(&parse(SELECTABLE));
        assert_eq!(structural.modules[0].generate_alternatives.len(), 1);
    }

    #[test]
    fn rejects_unsupported_generate_shapes_at_exact_spans() {
        let cases = [
            (
                "module m; generate if (other) begin end else begin end endgenerate endmodule\n",
                GenerateMode::Delayful,
                1,
                24,
                "unsupported generate condition; expected scalar `nodelay`",
            ),
            (
                "module m; generate wire x; endgenerate endmodule\n",
                GenerateMode::Delayful,
                1,
                20,
                "generate block must contain an `if (nodelay)` alternative",
            ),
            (
                "module m; generate if (nodelay) begin end endgenerate endmodule\n",
                GenerateMode::Nodelay,
                1,
                20,
                "`if (nodelay)` generate must have an `else` branch",
            ),
            (
                "module m; generate if (nodelay) begin generate if (nodelay) begin end else begin end endgenerate end else begin end endgenerate endmodule\n",
                GenerateMode::Nodelay,
                1,
                39,
                "nested generate blocks are unsupported",
            ),
            (
                "module m(output logic y); generate if (nodelay) assign y = 1; else begin end endgenerate endmodule\n",
                GenerateMode::Nodelay,
                1,
                49,
                "selected nodelay generate branch must be a begin/end block",
            ),
        ];
        for (source, mode, line, column, message) in cases {
            let error = elaborate_design(&parse(source), mode).unwrap_err();
            assert_eq!(error.span.line, line, "{source}");
            assert_eq!(error.span.column, column, "{source}");
            assert_eq!(error.message, message, "{source}");
        }
    }
}
