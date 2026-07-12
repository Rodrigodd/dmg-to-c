# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-12:

- `cargo test` passes 129 unit tests and 58 integration/corpus tests.
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
- Both configured modes lower all 206 files with zero failures. Every cell is
  deterministic, structurally valid, and contains only flat contracted value
  expressions. The default delayful corpus audit covers 1,958 assignments,
  including 1,168 generated temporaries and 721 modeled nonzero delays; nodelay
  contains 1,955 assignments and the same 1,168 temporaries.
- Default delayful lowering reports 1,302 visible intentional ignores: 42
  literal initial events and 1,260 delay tuple entries after the first. Nodelay
  reports 1,292. They remain non-failing under `--strict`; initial events
  classify their targets as modeled registers without serializing an initial
  event queue.
- Target-only selection among multiple control-dependent specify paths emits
  49 documented warnings in the configured delayful corpus. Ordinary lowering
  succeeds, while `--strict` promotes those warnings to failures.
- No curated lowering failure remains. The exact transistor audit accounts for
  10 files and 25 direct value drivers: 17 `nmos`, 7 `pmos`, and 1 `rnmos`.
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
  The configured full-corpus audit proves all 206 files are structurally valid
  and freezes exact operator, assignment, delay, and diagnostic totals.
- Milestone 5: complete. Reviewed stateful goldens cover simple, set/reset,
  blocking/nonblocking, nested-priority, and block-body latches. The configured
  recursive audit finds 27 stateful files with 48 exact modeled registers and
  48 flat retained equations, including the `dlatch_ee_irq` transistor/keeper
  topology.
- Milestone 6: complete. Reviewed fixtures cover signal- and literal-valued
  tri-state polarity, open drain, precharge, bidirectional pull-up pads, supply
  ties, direct primitives, and repeated buses. Exact strength metadata uses
  only the four contracted ordered pairs. The scope-aware audit accounts for
  67 relevant files and proves all 67 preserve flat driver dependencies and
  source order while keeping genuine repeated targets separate without
  combining mutually exclusive generate alternatives. The audit also accounts
  for all six keeper drivers, all of which are emitted distinctly.
- Milestone 7: complete. Timing aliases resolve deterministically without
  dropping resistance sums, real factors, or outer multipliers; assignments and
  primitives use exactly tuple entry zero, and every later entry is tracked as
  an intentional ignore. Source-level writes without an explicit delay use the
  first source-ordered specify path for their scalar target, with one strict-mode
  warning for each used ambiguous target. Reviewed timing goldens include
  explicit precedence, procedural state, ambiguity diagnostics, and the exact
  reference `q_n`, `q`, and `d` assignments. The configured corpus audit
  accounts for all 206 files, 266 structural specify paths, 790 selected source
  targets, 421 preserved outer resistance multiplications, 49 warnings, and
  zero M7 deferrals.
- Milestone 8: complete. `GenerateMode::Delayful` is the default and
  `GenerateMode::Nodelay` is explicitly selectable through the configured APIs
  and CLI. Exact fixtures and a dual-mode 206-file audit prove that `dffr`,
  `dffr_cc`, `dffr_cc_q`, `dffsr`, and `tffnl` each select one branch with no
  unselected declarations, state, drivers, timing aliases, diagnostics, or
  requirements. Both modes lower all 206 files with zero failures.
- Milestone 9: complete. Catalog-owned typed module definitions and the
  hierarchy transformer recursively substitutes named, positional, omitted,
  and default parameter bindings plus named/positional port connections,
  qualifies child-local names and timing aliases, preserves instance/child
  driver order, and rejects collisions, unknown modules, and recursion.
  Reviewed `half_add`/`full_add` fixtures cover all seven actual instances, and
  the exact dual-mode corpus audit reports 206 successes with no configured M9
  requirement or hierarchy-only failure.
- Milestone 10: complete. Validated special keeper instances carry typed target
  connections, a distinct `KeeperDriven` signal role, and a source-ordered
  keeper driver. Lowering emits exactly `(target (keeper) 0)`, bypasses specify
  delays, preserves independent neighboring drivers, and never adds the target
  to registers. Reviewed fixtures cover the five required cells; the exact
  audit accounts for all six source keepers as distinct emitted drivers.
- Milestone 11: complete. Direct `nmos`, `pmos`, and `rnmos` value operators
  preserve primitive identity, source/gate topology and polarity, source order,
  repeated drivers, and first-entry timing. Compound operands flatten
  dependency-first into atom-only roots; `rnmos` is never weakened and no
  transistor is normalized to `bufif*`. Reviewed fixtures cover `dlatch_ee_irq`,
  `idu_bit0`, `idu_bit123456`, and the IRQ forms. The exact dual-mode audit
  matches all 25 source calls to 25 emitted roots across the required 10 files,
  proves no transistor target becomes state merely due to its driver, and
  reports 206 successful lowerings with no M11 requirement or diagnostic.
- Milestone 12: pending apart from the existing single-file serializer path.

## Review Policy

- Progress is accepted through deterministic fixture tests and manual comparison
  of fixture output against the corresponding SystemVerilog source and DSL
  contract.
- Every fixture change must be visible in review and intentionally approved.
- Cell simulation, truth-table execution, and automated next-state equivalence
  are outside the project scope.
