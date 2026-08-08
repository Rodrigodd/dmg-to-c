# SystemVerilog to S-Expression Converter Status

This file records implementation progress for [PLAN.md](PLAN.md). Update it when
a milestone acceptance condition changes or is completed.

## Verified Baseline

Last audited on 2026-08-08. The corpus is 189 curated `.sv` sources, plus one
hand-written `sexpr-cells/manual/dlatch_ee_irq.cell` that the converter does not
produce. The 15 `dff*.sv` cells were retired by Milestone 17;
`sm83/cells/dlatch_ee_irq.sv` and `dmg_cpu_b/cells/tffnl.sv` were retired
afterwards because they cannot be decomposed exactly.

### Build and test state

- `cargo nextest run` passes: 363 tests, 234 library unit tests and 129
  integration tests across 27 suites, with no ignored or skipped cases and no
  test exceeding the configured 45-second limit. `sexpr-fmt` passes its 11
  tests. `cargo nextest run` is the runner for both crates and supersedes
  `cargo test`.
- `cargo fmt -- --check` and `cargo clippy --all-targets` are clean for both
  crates.
- The `dev` and `test` profiles build at `opt-level = 2`. The exact-cover search
  in the decomposition audit is roughly twenty times slower unoptimized, which
  by itself exceeded the nextest slow timeout on the `dlatch` and `alu_pggen`
  shards. The whole suite now runs in about 27 seconds.
- There is no CI workflow; it was removed in `9e6263b`. The gates above have to
  be run by hand until one is reinstated.

### Frontend and analysis

- `survey sv-cells` deterministically inventories 57,383 tokens across 189 files
  and 117 typed capabilities: 1 supported, 107 deferred, 9 intentional ignores,
  and zero unsupported.
- Lexing and parsing each report `processed=189 failed=0`.
- Configured catalog-aware analysis reports `processed=189 supported=3
  deferred=186 warned=0 failed=0` in both generate modes. It selects exactly one
  `nodelay` branch before analysis, resolves ordinary module interfaces while
  retaining typed parameter and port bindings, and limits registers to modeled
  state.

### Ordinary (non-decomposed) lowering

This is still the default path for `check`, `convert`, and `convert-file`.

- Both configured modes lower all 189 files with zero warnings and zero
  failures, including under `--strict`. Every cell is deterministic,
  structurally valid, and contains only flat contracted value expressions.
- The delayful corpus audit covers 1,680 assignments, of which 958 are generated
  temporaries, 5 are atom values, 75 are repeated-target drivers, and 698 carry
  a modeled nonzero nested delay. Per-operator and per-tuple censuses now live
  only in the regenerable corpus summary goldens.
- Exactly 10 cells contain 12 modeled registers: 6 with an explicit initial
  value `0` and 6 with implicit `x`. No corpus register uses `1` or `z`; both
  remain covered by focused tests.
- Every assignment carries a validated tagged delay tuple. The source audit
  preserves 35 two-entry and 60 three-entry assignment tuples, 21 two-entry and
  396 three-entry primitive tuples, and 234 two-entry and 3 three-entry specify
  tuples, with no omitted component.
- Delayful and nodelay lowering each report exactly 42 intentional ignores, all
  for additional control-dependent specify paths after the temporarily selected
  first source-ordered path. They stay non-failing under `--strict`. The corpus
  has 179 specify target groups with multiplicity 1:137, 2:33, 3:8, 4:1, which
  is exactly the 42 groups that produce those ignores.
- Driver coverage accounts for 63 relevant files with 63 successes, all four
  contracted strength pairs (`highz1/strong0` 338, `strong1/highz0` 104,
  `pull1/highz0` 3, `supply1/supply0` 4), 25 repeated-driver paths over 57
  targets and 132 source occurrences, and 5 source keepers all emitted
  distinctly.
- The transistor audit accounts for 9 files and 24 direct value drivers: 17
  `nmos` and 7 `pmos`. The corpus no longer contains an `rnmos` call; it left
  with `dlatch_ee_irq`, so `rnmos` support is now covered only by focused tests.
- Hierarchy flattening covers `half_add` and `full_add`. The corpus contains no
  `generate` cell, so Milestone 8 elaboration is covered only by focused unit
  tests; the corpus audit asserts the generate set is empty.

### Timing graph and decomposition

- Timing-aware lowering retains all 231 source-owned specify paths, 426 scalar
  controls, and 179 target groups (137 single-path, 42 multiple-path) with full
  tuples and source provenance in both modes.
- The typed functional graph classifies operand, drive, state-boundary and
  resolved-net-boundary edges; exact sizes live in the regenerable graph
  summary golden. Combinational cycles, unreachable
  controls, additive-rebuild mismatches, nondeterministic reports, ordinary
  compatibility breaks, and absolute-path leaks are all zero.
- Decomposed lowering is opt-in behind `--decompose-timing` on `convert` and
  `convert-file`. It is not reachable from `check`, and it is not the default.
- **Decomposition is closed: all 189 cells decompose exactly in both generate
  modes, with zero failures and zero intentional ignores.** Three defects
  accounted for the ten former failures. Synthetic `dN` identity names skip
  reserved indices, so a cell with `d0`/`d1` ports no longer aborts. Mux data
  operands are positive-unate rather than conditional; only the select is
  non-unate. And a placement's orientation now follows the modeled path's
  inversion parity rather than collapsing to an unusable "ambiguous" state,
  with a placement permitted to be wider than the constraint it serves so a
  tri-state output keeps its turn-off component while serving a two-entry
  control. `PathOrientation::Ambiguous`, `UnrepresentableSense`, and
  `ReservedDelayName` are gone as unreachable.
- The nine cells fixed were `buf_if0`, `not_if0`, `not_if1`, `dlatch`,
  `drlatch_ee`, `nand_latch`, `nor_latch`, `mux_idu_h`, and `mux_idu_l`. The
  180 cells that already decomposed are byte-identical across the change, so
  the fix added capability without perturbing any existing placement.
- `tffnl` was retired rather than fixed. Its RTL models `q_n = !q` while the
  source timing gives `q` and `q_n` independent chains on opposite clock
  phases, so a single shared edge would have to begin with both `T_rise_not1`
  and `T_fall_not1`. No exact cover exists; this is hidden physical topology,
  the same class as `dmg_dffsr`.
- The `dmg_dffsr` physical-topology overlay, the `topology-hints/` directory,
  the three topology modules, and the direct `serde`/`toml` dependencies are all
  gone. `petgraph` and `proptest` remain.

### Checked output

- The checked `sexpr-cells` tree still predates this change: it was written with
  `--decompose-timing` while the transactional guard was disabled, so it mixes
  decomposed cells with cells that retain the first-specify-path approximation,
  and it still contains the retired `tffnl.cell`. It has not been regenerated;
  that is Milestone 17 step 5.
- The mirrored cells are valid generic S-expressions and canonical under
  `sexpr-fmt --check`. `sexpr-cells/manual/dlatch_ee_irq.cell` is not.
- Most mirrored cells emit a non-empty `(parameters ...)` section. This section is not described in `CONTRACT.md` and has
  no reviewed fixture of its own.

### CLI

`lex`, `parse`, `analyze`, `lower`, transactional corpus `convert`,
`convert-file`, `survey`, and staged `check` are available. Configured analysis,
catalog-aware lowering, conversion, and analyze/lower checks accept `--nodelay`;
delayful selection is the default. `convert` and `convert-file` additionally
accept `--decompose-timing`. Corpus conversion supports documented strict,
dry-run, overwrite, and normalized relative-path filter policies, validates the
complete catalog before selected lowering, and reports deterministic processed,
selected, skipped, warned, intentional-ignore, written, would-write, and failed
totals. Its refusal to emit partial output is currently disabled; see the build
and test state above.

## Milestone Status

- Milestone 0: complete. `CONTRACT.md` freezes value, driver, state, strength,
  hierarchy, keeper, transistor, timing, and diagnostic behavior. Typed IR
  operators and structural validation enforce the expression boundary, and the
  CLI applies the shared strict diagnostic policy. The later `(parameters ...)`
  output section is not covered by this freeze.
- Milestone 1: complete. Full-corpus lexing reports `processed=189 failed=0`;
  deterministic snapshots cover the required syntax families and the typed
  survey inventory attributes every observed capability to sorted source files.
- Milestone 2: complete. Full-corpus parsing reports `processed=189 failed=0`;
  typed AST goldens cover the required families, an exhaustive visitor accounts
  for every AST variant and source item, and exact diagnostics cover malformed
  and truncated constructs at logical source locations.
- Milestone 3: complete. Typed symbol, signal-role, source-ordered driver,
  timing, generate-alternative, and hierarchy analyses are covered by reviewed
  fixtures. Continuous and primitive nets remain non-state, full-adder
  connections resolve against child port directions, and analyze checks
  distinguish supported, deferred, warned, and failed files.
- Milestone 4: complete. Reviewed goldens cover the combinational operator
  families and compound equality/mux expressions; deterministic dependency-first
  `t0`, `t1`, ... assignments keep every value operation flat. The configured
  full-corpus audit proves all 189 files are structurally valid and freezes
  exact operator, assignment, delay, and diagnostic totals.
- Milestone 5: complete. Reviewed stateful goldens cover simple, set/reset,
  blocking/nonblocking, nested-priority, and block-body latches. The configured
  recursive audit finds 11 stateful files with 14 exact modeled registers and 14
  flat retained equations.
- Milestone 6: complete. Reviewed fixtures cover signal- and literal-valued
  tri-state polarity, open drain, precharge, bidirectional pull-up pads, supply
  ties, direct primitives, and repeated buses. Exact strength metadata uses only
  the four contracted ordered pairs. The scope-aware audit accounts for 63
  relevant files and proves all 63 preserve flat driver dependencies and source
  order while keeping genuine repeated targets separate.
- Milestone 7: complete. Timing aliases resolve deterministically without
  dropping resistance sums, real factors, or outer multipliers. Its historical
  selected-first tuple policy was superseded by Milestone 14. Source-level
  writes without an explicit delay still use the first source-ordered specify
  path for their scalar target, with one strict-mode intentional ignore per used
  target that has additional control-dependent paths; Milestone 17 step 5 is
  meant to remove that fallback and has not started.
- Milestone 8: complete. `GenerateMode::Delayful` is the default and
  `GenerateMode::Nodelay` is explicitly selectable through the configured APIs
  and CLI. The corpus retains no `generate` cell after the DFF and `tffnl`
  retirements, so branch selection is covered by focused unit tests and the
  corpus audit asserts the generate set is empty. Both modes lower all 189 files
  with zero failures.
- Milestone 9: complete. The hierarchy transformer recursively substitutes
  named, positional, omitted, and default parameter bindings plus named and
  positional port connections, qualifies child-local names and timing aliases,
  preserves instance and child driver order, and rejects collisions, unknown
  modules, and recursion. Reviewed `half_add`/`full_add` fixtures cover all
  seven actual instances.
- Milestone 10: complete. Validated special keeper instances carry typed target
  connections, a distinct `KeeperDriven` signal role, and a source-ordered
  keeper driver. Lowering emits exactly `(target (keeper) (delay 0))`, bypasses
  specify delays, preserves independent neighboring drivers, and never adds the
  target to registers. The exact audit accounts for all 5 remaining source
  keepers as distinct emitted drivers.
- Milestone 11: complete for `nmos` and `pmos`, with a coverage caveat. The
  exact dual-mode audit matches all 24 source calls to 24 emitted roots across
  9 files and proves no transistor target becomes state merely due to its
  driver. `rnmos` left the corpus with `dlatch_ee_irq` and is now covered only
  by focused tests, not by a corpus witness.
- Milestone 12: complete, with one carry-over. The typed conversion API and
  `convert` CLI perform a complete-catalog, globally preflighted, deterministic
  conversion with strict, dry-run, overwrite, filter, and dual-generate-mode
  policies, the serializer shares `sexpr-fmt`'s canonical implementation, and
  the no-partial-output guard in `execute_prepared` is back in force with its
  transactional tests passing. The carry-over is that the checked tree no longer
  reproduces from a single lowering path; Milestone 17 step 5 resolves that.
- Milestone 13: complete. Typed `LogicValue` and `Register` IR entries preserve
  selected scalar literal initialization as uniform `(name value)` register
  metadata, default uninitialized modeled state to `x`, survive configured
  generate selection and hierarchy qualification, and reject duplicate selected
  initializers at the second target. Focused tests cover all contracted values;
  the corpus now holds 8 zero-initialized and 6 unknown-initialized registers.
- Milestone 14: complete. Validated `TimingExpr` and exact-arity `DelayTuple`
  types preserve every component of selected explicit, primitive, specify, and
  hierarchy-substituted delays. Serialization emits only `(delay value)`,
  `(delay rise fall)`, or `(delay rise fall turn-off)` on ordinary assignments;
  missing and generated timing uses canonical `(delay 0)`.
- Milestone 15: complete. Internal timing-aware lowering preserves every specify
  path and scalar control with full tuple and source provenance, relates them to
  a deterministic typed functional graph, and separately cuts modeled state and
  multiply-driven resolved-net boundaries for DAG analysis. All retained paths
  reconstruct exactly and no timing arc or table is serialized.
- Milestone 16: complete, with its topology extension removed by Milestone 17.
  Exact ordered timing terms are factored across existing assignments, typed
  `dN` edge identities, and raw/public output splits by deterministic
  exact-cover search. Application is verified from the actual rebuilt graph and
  is exactly erasable. The `serde`/`toml` physical-topology hints and the
  `dmg_dffsr` overlay they served are gone with the DFF corpus. The reviewed
  `ao21` output fixture remains canonical and deterministic, and no arc, timing
  table, negative delay, subtraction, or selected-first fallback is emitted by
  decomposed lowering.
- Milestone 17: steps 1-4 complete, step 5 outstanding. The DFF family and the
  topology machinery are retired, the fixtures are rebased on the 189-file
  corpus, and the bounded sharded audit covers each file once per mode. Step 4
  closed all ten decomposition failures: nine by fixing the `dN` naming, mux
  data-pin sense, and placement-orientation defects, and `tffnl` by retiring it
  as unrepresentable. Step 5 remains: make decomposition the ordinary lowering
  path, drop the `--decompose-timing` opt-in and the 42 first-path intentional
  ignores, and regenerate the checked tree. See [PLAN.md](PLAN.md).

## Review Policy

- Progress is accepted through deterministic fixture tests and manual comparison
  of fixture output against the corresponding SystemVerilog source and DSL
  contract.
- Every fixture change must be visible in review and intentionally approved.
- Cell simulation, truth-table execution, and automated next-state equivalence
  are outside the project scope.
