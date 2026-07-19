# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-07-18:

- `cargo test` passes 153 unit tests and 70 integration/corpus tests; the sibling
  formatter passes 7 unit and 4 integration tests.
- Lexing succeeds for all 206 curated files.
- Parsing succeeds for all 206 curated files.
- `survey sv-cells` deterministically inventories 63,240 tokens and 138 typed
  capabilities: 1 supported, 128 deferred, 9 intentional ignores, and zero
  unsupported. Contracted scalar initial events are the supported Milestone 13
  capability rather than an intentional ignore.
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
  including 1,168 generated temporaries and 735 modeled nonzero delays; nodelay
  contains 1,955 assignments and the same 1,168 temporaries. Exactly 27 cells
  contain 48 modeled registers: 42 with explicit initial value `0` and 6 with
  implicit initial value `x`.
- Every assignment carries a tagged delay tuple. The delayful corpus emits
  1,223 one-entry, 276 two-entry, and 459 three-entry tuples. The source audit
  preserves exactly 45/60 assignment, 21/399 primitive, and 263/3 specify
  two-/three-entry tuples. An independent compatibility oracle compares the
  first component of all 1,958 assignments with the former selected-first
  semantics and reports zero mismatches; the full-component audit reports no
  discarded, filled, reordered, or uncontracted timing expression.
- Delayful and nodelay lowering each report exactly 49 visible intentional
  ignores, all for additional control-dependent specify paths after the
  temporary selected first path for each used target. Later tuple entries are
  preserved and produce no diagnostic. The remaining ignores stay non-failing
  under `--strict`. Valid selected initializers are typed register metadata,
  emit no assignment or diagnostic, and are no longer an ignore category. Both
  configured modes report zero warnings and zero failures under strict policy.
- No curated lowering failure remains. The exact transistor audit accounts for
  10 files and 25 direct value drivers: 17 `nmos`, 7 `pmos`, and 1 `rnmos`.
- All 206 checked generated cells are valid generic S-expressions, canonical
  and idempotent under `sexpr-fmt`, and exact path mirrors of the 206 curated
  sources. The checked reference byte-matches the reviewed timing fixture.
- Flat SSA, combinational operators, register lists, supported stateful
  behavior, driver/strength normalizations, and complete selected symbolic
  delay tuples have completed fixture and corpus review.
- The reference cell's `q_n`, `q`, and `d` assignments preserve the complete
  selected source/specify tuples, including all resistance multipliers and
  transition components; their first components still match the accepted
  temporary first-applicable-path policy.
- The current CLI has `lex`, `parse`, `analyze`, `lower`, transactional corpus
  `convert`, `convert-file`, `survey`, and staged `check`. Configured analysis,
  catalog-aware lowering, conversion, and analyze/lower checks accept
  `--nodelay`; delayful selection is the default. Corpus conversion supports
  documented strict, dry-run, overwrite, and normalized relative-path filter
  policies, validates the complete catalog before selected lowering, refuses
  preflight-detectable partial output, and reports deterministic processed,
  selected, skipped, warned, intentional-ignore, written, would-write, and
  failed totals.

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
  dropping resistance sums, real factors, or outer multipliers. Its historical
  selected-first tuple policy was superseded by Milestone 14, which preserves
  every selected tuple component. Source-level writes without an explicit delay
  still use the first source-ordered specify path for their scalar target, with
  one strict-mode intentional ignore for each used target having additional
  control-dependent paths. Reviewed timing goldens include
  explicit precedence, procedural state, ambiguity diagnostics, and the exact
  reference `q_n`, `q`, and `d` assignments. The configured corpus audit
  accounts for all 206 files, 266 structural specify paths, 790 selected source
  targets, 421 preserved outer resistance multiplications, 49 additional-path
  intentional ignores, zero warnings, and zero M7 deferrals.
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
  keeper driver. Lowering emits exactly `(target (keeper) (delay 0))`, bypasses specify
  delays, preserves independent neighboring drivers, and never adds the target
  to registers. Reviewed fixtures cover the five required cells; the exact
  audit accounts for all six source keepers as distinct emitted drivers.
- Milestone 11: complete. Direct `nmos`, `pmos`, and `rnmos` value operators
  preserve primitive identity, source/gate topology and polarity, source order,
  repeated drivers, and complete selected timing tuples. Compound operands flatten
  dependency-first into atom-only roots; `rnmos` is never weakened and no
  transistor is normalized to `bufif*`. Reviewed fixtures cover `dlatch_ee_irq`,
  `idu_bit0`, `idu_bit123456`, and the IRQ forms. The exact dual-mode audit
  matches all 25 source calls to 25 emitted roots across the required 10 files,
  proves no transistor target becomes state merely due to its driver, and
  reports 206 successful lowerings with no M11 requirement or diagnostic.
- Milestone 12: complete. The typed conversion API and `convert` CLI perform a
  complete-catalog, globally preflighted, deterministic conversion with strict,
  dry-run, overwrite, filter, and dual-generate-mode policies. The serializer
  shares `sexpr-fmt`'s canonical implementation. Release tests and CI prove the
  exact 206-path mirror, canonical/idempotent formatting, flat structural IR,
  byte-identical repeated overwrite conversion, precise unsupported-input
  diagnostics with no preflight partial writes, and exact reference equality.
  The authoritative `sexpr-cells` tree contains all 206 generated cells; strict
  delayful and nodelay corpus gates have zero warnings and zero failures.
- Milestone 13: complete. Typed `LogicValue` and `Register` IR entries preserve
  selected scalar literal initialization as uniform `(name value)` register
  metadata, default uninitialized modeled state to `x`, survive configured
  generate selection and hierarchy qualification, and reject duplicate selected
  initializers at the second target. Focused tests cover all contracted values;
  the exact corpus audit proves 42 zero-initialized and 6 unknown-initialized
  registers, unchanged assignment totals, no initializer diagnostics, canonical
  regenerated outputs, and the then-current strict ignore totals of
  1,309/1,299, subsequently reduced by Milestone 14.
- Milestone 14: complete. Validated `TimingExpr` and exact-arity `DelayTuple`
  types preserve every component of selected explicit, primitive, specify, and
  hierarchy-substituted delays. Serialization emits only `(delay value)`,
  `(delay rise fall)`, or `(delay rise fall turn-off)` on ordinary assignments;
  missing and generated timing uses canonical `(delay 0)`. The full audit
  freezes 1,958 emitted tuples with arities 1,223/276/459 and proves zero
  mismatches across all first-component compatibility projections. Repeated
  strict conversion is byte-identical and all 206 checked cells remain
  formatter-canonical. The only remaining diagnostics are 49 intentional
  ignores for additional specify paths in either generate mode.

## Review Policy

- Progress is accepted through deterministic fixture tests and manual comparison
  of fixture output against the corresponding SystemVerilog source and DSL
  contract.
- Every fixture change must be visible in review and intentionally approved.
- Cell simulation, truth-table execution, and automated next-state equivalence
  are outside the project scope.
