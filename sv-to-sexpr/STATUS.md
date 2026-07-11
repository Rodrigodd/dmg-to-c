# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-10:

- `cargo test` passes 18 unit tests.
- Lexing succeeds for all 206 curated files.
- Parsing succeeds for all 206 curated files.
- The analysis command returns success for all 206 files, but its output is not
  accepted yet because it can misclassify combinational nets as registers and
  combine mutually exclusive generate branches.
- Lowering returns success for 185 files and fails for 21 files.
- Nine failures are transistor-related. Other failures include generated
  DFF/TFF variants, hierarchical adders, mux keepers, an unsupported timing
  factor, and other non-transistor constructs.
- Examined generated files are valid generic S-expressions and become stable
  after `sexpr-fmt`. Full-corpus formatter validation has not been performed.
- Generated register lists, flat SSA structure, and delays have not completed
  fixture review and are not accepted as correct.
- The reference cell does not match the checked-in target: SSA temporaries and
  specify-derived delays are missing, and its precharge delay tuple is summed
  instead of using only its first entry.
- The current CLI has `lex`, `parse`, `analyze`, `lower`, `convert-file`,
  `survey`, and staged `check`. It does not yet have corpus `convert`, strict
  warning behavior, or the complete diagnostic summary required by the plan.

## Milestone Status

- Milestone 0: pending.
- Milestone 1: partial. Full-corpus lexing works; the capability inventory and
  required snapshots are missing.
- Milestone 2: partial. Full-corpus parsing works; losslessness assertions and
  representative golden ASTs are missing.
- Milestone 3: partial. Analysis structures exist, but register classification
  and generate-branch separation are not accepted.
- Milestone 4: partial. Combinational lowering exists, but flat SSA output,
  complete fixtures, and fixture review are missing.
- Milestone 5: partial. Several flat latch/register cells lower, but complete
  family fixtures and review are missing.
- Milestone 6: partial. Constant-drive tri-state and precharge subsets lower;
  signal-valued drives, strength policy, and reviewed fixtures remain.
- Milestone 7: partial. Timing aliases lower in some files, but tuple handling,
  specify paths, resistance factors, and reviewed timing fixtures remain.
- Milestone 8: pending apart from parser and analyzer scaffolding for generate
  nodes.
- Milestone 9: pending apart from parser and analyzer scaffolding for ordinary
  instantiations.
- Milestone 10: pending apart from parsing keepers as instantiations.
- Milestone 11: pending apart from parser and analyzer scaffolding for transistor
  primitives.
- Milestone 12: pending apart from the existing single-file serializer path.

## Review Policy

- Progress is accepted through deterministic fixture tests and manual comparison
  of fixture output against the corresponding SystemVerilog source and DSL
  contract.
- Every fixture change must be visible in review and intentionally approved.
- Cell simulation, truth-table execution, and automated next-state equivalence
  are outside the project scope.
