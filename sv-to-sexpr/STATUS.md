# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-12:

- `cargo test` passes 98 unit tests and 32 integration tests.
- Lexing succeeds for all 206 curated files.
- Parsing succeeds for all 206 curated files.
- `survey sv-cells` deterministically inventories 63,240 tokens and 138 typed
  capabilities: 128 deferred, 10 intentional ignores, and zero unsupported.
- Catalog-aware semantic analysis succeeds for all 206 files and reports 1
  supported, 205 deferred, zero warned, and zero failed. It preserves distinct
  generate alternatives, resolves ordinary module interfaces without
  elaborating child behavior, and limits registers to modeled state.
- Lowering returns success for 185 files and fails explicitly for 21 files. All
  185 successful cells are deterministic, structurally valid, and contain only
  flat contracted value expressions; the corpus audit covers 1,610 assignments
  including 999 generated temporaries.
- Successful lowering reports 32 literal initial events as visible
  intentional-ignore diagnostics. They remain non-failing under `--strict`,
  classify their targets as modeled registers, and do not serialize an initial
  event queue.
- Nine failures are transistor-related. The other failures are five generated
  DFF/TFF variants, two hierarchical adders, four keeper users, and one
  unsupported timing factor.
- Examined generated files are valid generic S-expressions and become stable
  after `sexpr-fmt`. Full-corpus formatter validation has not been performed.
- Flat SSA, combinational operators, register lists, supported stateful
  behavior, and driver/strength normalizations have completed fixture review.
  Timing semantics remain subject to their later milestone fixture review.
- The reference cell now lowers with flat SSA values but does not yet match the
  checked-in target because specify-derived delays have not completed fixture
  review. Delay tuples select only their first entry, but reference output under
  that policy is not yet accepted.
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
- Milestone 3: complete. Typed symbol, signal-role, source-ordered driver,
  timing, generate-alternative, and hierarchy analyses are covered by reviewed
  fixtures. The reference inputs, outputs, and registers match the contract;
  continuous and primitive nets remain non-state; full-adder connections
  resolve against child port directions; and analyze checks distinguish
  supported, deferred, warned, and failed files.
- Milestone 4: complete. Reviewed goldens cover the required combinational
  operator families and compound equality/mux expressions; deterministic
  dependency-first `t0`, `t1`, ... assignments keep every value operation flat.
  A full-corpus audit proves all 185 current successes are structurally valid
  and freezes exact diagnostics for the 21 later-milestone failures.
- Milestone 5: complete. Reviewed stateful goldens cover simple, set/reset,
  blocking/nonblocking, nested-priority, and block-body latches. The recursive
  audit finds 27 stateful files: 21 emitted cells have 38 exact modeled
  registers and 38 flat retained mux equations; five generated DFF/TFF files
  remain M8 deferrals, and `dlatch_ee_irq` remains explicitly assigned to its
  M7/M10/M11 timing, keeper, and transistor work. All 13 combinational
  procedural writes found in generated alternatives remain non-state.
- Milestone 6: complete. Reviewed fixtures cover signal- and literal-valued
  tri-state polarity, open drain, precharge, bidirectional pull-up pads, supply
  ties, direct primitives, and repeated buses. Exact strength metadata uses
  only the four contracted ordered pairs. The scope-aware audit accounts for
  67 relevant files, proves all 53 current successes preserve flat driver
  dependencies and source order, and keeps all 58 genuine repeated targets
  separate without combining mutually exclusive generate alternatives.
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
