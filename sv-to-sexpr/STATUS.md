# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-12:

- `cargo test` passes 118 unit tests and 49 integration/corpus tests.
- Lexing succeeds for all 206 curated files.
- Parsing succeeds for all 206 curated files.
- `survey sv-cells` deterministically inventories 63,240 tokens and 138 typed
  capabilities: 128 deferred, 10 intentional ignores, and zero unsupported.
- Configured catalog-aware semantic analysis succeeds for all 206 files in both
  generate modes and reports 3 supported, 203 deferred, zero warned, and zero
  failed. It selects exactly one `nodelay` branch before analysis, resolves
  ordinary module interfaces while retaining their typed parameter and port
  bindings, and limits registers to modeled state. Explicit structural APIs
  retain both generate alternatives or unresolved hierarchy only for earlier
  source-inventory fixtures.
- Default delayful lowering returns success for 192 files and fails explicitly
  for 14 files. All 192 successful cells are deterministic, structurally valid,
  and contain only flat contracted value expressions; the configured corpus
  audit covers 1,693 assignments including 1,046 generated temporaries and 588
  modeled nonzero delays. Nodelay mode has the same exact success/failure sets.
- Default delayful lowering reports 1,073 visible intentional ignores: 41
  literal initial events and 1,032 delay tuple entries after the first. Nodelay
  reports 1,063. They remain non-failing under `--strict`; initial events
  classify their targets as modeled registers without serializing an initial
  event queue.
- Target-only selection among multiple control-dependent specify paths emits
  47 documented warnings in the configured delayful corpus. Ordinary lowering
  succeeds, while `--strict` promotes those warnings to failures.
- Nine failures are transistor-related and five are keeper users. No failure
  remains assigned to Milestone 7 timing, Milestone 8 generate selection, or
  Milestone 9 ordinary hierarchy.
- Timing goldens are valid generic S-expressions, and the checked reference is
  canonical under `sexpr-fmt --check`. Full-corpus formatter validation has not
  been performed.
- Flat SSA, combinational operators, register lists, supported stateful
  behavior, driver/strength normalizations, and first-entry symbolic timing
  have completed fixture and corpus review.
- The reference cell's `q_n`, `q`, and `d` assignments now match the accepted
  first applicable source/specify entry policy, including all resistance
  multipliers, and the checked-in reference has been updated accordingly.
- The current CLI has `lex`, `parse`, `analyze`, `lower`, `convert-file`,
  `survey`, and staged `check`. Configured analysis, catalog-aware lowering,
  single-file conversion, and analyze/lower checks accept `--nodelay`; delayful
  selection is the default. Single-file and directory lowering build a sibling
  or shared catalog for ordinary hierarchy. Diagnostic-capable commands accept
  the shared `--strict` warning policy, and lower/convert surface timing
  approximation warnings and intentional ignores. The CLI does not yet have
  corpus `convert` or the complete release diagnostic summary required by the
  plan.

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
  The configured full-corpus audit proves all 192 current successes are
  structurally valid and freezes exact diagnostics for the 14 later-milestone
  failures.
- Milestone 5: complete. Reviewed stateful goldens cover simple, set/reset,
  blocking/nonblocking, nested-priority, and block-body latches. The configured
  recursive audit finds 27 stateful files: 26 emitted cells have 47 exact
  modeled registers and 47 flat retained equations; only `dlatch_ee_irq`
  remains explicitly assigned to its M10/M11 keeper and transistor work.
- Milestone 6: complete. Reviewed fixtures cover signal- and literal-valued
  tri-state polarity, open drain, precharge, bidirectional pull-up pads, supply
  ties, direct primitives, and repeated buses. Exact strength metadata uses
  only the four contracted ordered pairs. The scope-aware audit accounts for
  67 relevant files, proves all 53 current successes preserve flat driver
  dependencies and source order, and keeps all 58 genuine repeated targets
  separate without combining mutually exclusive generate alternatives.
- Milestone 7: complete. Timing aliases resolve deterministically without
  dropping resistance sums, real factors, or outer multipliers; assignments and
  primitives use exactly tuple entry zero, and every later entry is tracked as
  an intentional ignore. Source-level writes without an explicit delay use the
  first source-ordered specify path for their scalar target, with one strict-mode
  warning for each used ambiguous target. Reviewed timing goldens include
  explicit precedence, procedural state, ambiguity diagnostics, and the exact
  reference `q_n`, `q`, and `d` assignments. The configured corpus audit
  accounts for all 206 files, 266 structural specify paths, 647 selected source
  targets, 385 preserved outer resistance multiplications, 47 warnings, and
  zero M7 deferrals.
- Milestone 8: complete. `GenerateMode::Delayful` is the default and
  `GenerateMode::Nodelay` is explicitly selectable through the configured APIs
  and CLI. Exact fixtures and a dual-mode 206-file audit prove that `dffr`,
  `dffr_cc`, `dffr_cc_q`, `dffsr`, and `tffnl` each select one branch with no
  unselected declarations, state, drivers, timing aliases, diagnostics, or
  requirements. Both modes lower 192 files and leave exactly the 5 keeper and 9
  transistor deferrals.
- Milestone 9: complete. Catalog-owned typed module definitions and the
  hierarchy transformer recursively substitutes named, positional, omitted,
  and default parameter bindings plus named/positional port connections,
  qualifies child-local names and timing aliases, preserves instance/child
  driver order, and rejects collisions, unknown modules, and recursion.
  Reviewed `half_add`/`full_add` fixtures cover all seven actual instances, and
  the exact dual-mode corpus audit reports 192 successes with no configured M9
  requirement or hierarchy-only failure.
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
