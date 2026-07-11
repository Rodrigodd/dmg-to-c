# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-11:

- `cargo test` passes 56 unit tests and 18 integration tests.
- Lexing succeeds for all 206 curated files.
- Parsing succeeds for all 206 curated files.
- `survey sv-cells` deterministically inventories 63,240 tokens and 138 typed
  capabilities: 128 deferred, 10 intentional ignores, and zero unsupported.
- The analysis command returns success for all 206 files, but its output is not
  accepted yet because it can misclassify combinational nets as registers and
  combine mutually exclusive generate branches.
- Lowering returns success for 182 files and fails explicitly for 24 files.
- Nine failures are transistor-related. The other failures are five generated
  DFF/TFF variants, two hierarchical adders, four keeper users, three
  signal-valued high-Z drivers, and one unsupported timing factor.
- Examined generated files are valid generic S-expressions and become stable
  after `sexpr-fmt`. Full-corpus formatter validation has not been performed.
- Generated register lists, flat SSA structure, and delays have not completed
  fixture review and are not accepted as correct.
- The reference cell does not match the checked-in target: SSA temporaries and
  specify-derived delays are missing. Delay tuples now select only their first
  entry, but reference output under that policy has not completed fixture
  review.
- The current CLI has `lex`, `parse`, `analyze`, `lower`, `convert-file`,
  `survey`, and staged `check`. These diagnostic-capable commands accept the
  shared `--strict` warning policy, although current stages do not produce
  warnings yet. It does not yet have corpus `convert` or the complete release
  diagnostic summary required by the plan.

## Milestone Status

- Milestone 0: complete. `CONTRACT.md` freezes value, driver, state, strength,
  hierarchy, keeper, transistor, timing, and diagnostic behavior. Typed IR
  operators and structural validation enforce the expression boundary, and the
  CLI applies the shared strict diagnostic policy.
- Milestone 1: complete. Full-corpus lexing reports `processed=206 failed=0`;
  deterministic snapshots cover the required syntax families and the typed
  survey inventory attributes every observed capability to sorted source files.
- Milestone 2: complete. Full-corpus parsing reports `processed=206 failed=0`;
  typed AST goldens cover all eight required families, an exhaustive visitor
  accounts for every AST variant and source item, and exact diagnostics cover
  malformed and truncated constructs at logical source locations.
- Milestone 3: partial. Analysis structures exist, but register classification
  and generate-branch separation are not accepted.
- Milestone 4: partial. Combinational lowering exists, but flat SSA output,
  complete fixtures, and fixture review are missing.
- Milestone 5: partial. Several flat latch/register cells lower, but complete
  family fixtures and review are missing.
- Milestone 6: partial. Constant-drive tri-state and precharge subsets lower;
  the strength representation is contracted, while signal-valued drives,
  strength lowering, and reviewed fixtures remain.
- Milestone 7: partial. First-entry tuple selection and the corpus timing clamp
  are enforced, but specify paths, resistance factors, complete alias handling,
  and reviewed timing fixtures remain.
- Milestone 8: pending apart from parser and analyzer scaffolding for generate
  nodes.
- Milestone 9: pending apart from parser and analyzer scaffolding for ordinary
  instantiations.
- Milestone 10: pending apart from parsing keepers as instantiations and the
  contracted direct keeper driver form.
- Milestone 11: pending apart from parser/analyzer scaffolding and contracted
  direct `nmos`, `pmos`, and `rnmos` driver forms.
- Milestone 12: pending apart from the existing single-file serializer path.

## Review Policy

- Progress is accepted through deterministic fixture tests and manual comparison
  of fixture output against the corresponding SystemVerilog source and DSL
  contract.
- Every fixture change must be visible in review and intentionally approved.
- Cell simulation, truth-table execution, and automated next-state equivalence
  are outside the project scope.
