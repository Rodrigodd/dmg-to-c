# SystemVerilog to S-Expression Cell Converter Plan

## Goal

Implement `sv-to-sexpr`, a Rust tool that converts the curated scalar
SystemVerilog cell corpus into the repository's SSA-like S-expression cell DSL.

Primary paths:

- Input cells: `sv-cells/**/*.sv`
- Output cells: `sexpr-cells/**/*.cell`
- Current curated corpus: 206 files
- Reference pair:
  - `sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv`
  - `sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell`

The converter is complete only when its output preserves the modeled logic,
state, drivers, and timing according to reviewed fixtures. Parsing a file or
serializing a syntactically valid S-expression is not sufficient by itself.

## Scope

### In Scope

- The SystemVerilog constructs that occur in the 206-file curated corpus.
- Scalar combinational and stateful logic.
- Continuous, procedural, primitive, and selected hierarchical drivers.
- Tri-state, precharge, keeper, and repeated-driver behavior used by the corpus.
- The timing expressions and `specify` paths required to reproduce cell delays.
- Deterministic SSA-like lowering and deterministic serialization.
- Precise diagnostics for constructs that have not been implemented safely.

### Global Non-Goals

These are not expected from any milestone unless the corpus changes and the
plan is revised:

- A general-purpose or standards-complete SystemVerilog frontend.
- Arbitrary vectors, arrays, interfaces, classes, assertions, or generate loops.
- Synthesis, simulation, truth-table evaluation, or next-state execution of
  either the SystemVerilog source or generated cells.
- Analog-accurate transistor or strength simulation beyond a documented DSL
  representation.
- Runtime event scheduling or selection of a delay-tuple component. The DSL
  preserves source rise, fall, and turn-off entries, while a downstream
  compiler or simulator chooses how to consume them.
- Serialized timing arcs or a timing-constraint table. Source timing paths may
  be retained internally while deriving ordinary per-assignment delays, but
  every emitted delay remains attached to an assignment.
- Guessing semantics for unsupported constructs. The tool must reject them.

## Target Output Contract

Each converted file contains one cell form:

```scheme
(cell
  module_name
  (inputs ...)
  (outputs ...)
  (registers (register initial-value) ...)
  (assignments
    (target expression (delay timing-expression ...))
    ...
  )
)
```

The following rules are part of the contract and must be tested:

- `inputs` contains input ports and inout ports read by the cell.
- `outputs` contains output ports and inout ports driven by the cell.
- `registers` contains only modeled state, such as variables initialized by
  `initial` or assigned by stateful procedural logic. Each entry is a
  `(name initial-value)` pair whose value is one of `0`, `1`, `x`, or `z`.
  Selected scalar literal initializers supply that metadata; a modeled register
  without a selected initializer uses `x`. Continuously driven and
  primitive-driven internal nets are not registers.
- Each source driver becomes an explicit assignment or a documented normalized
  equivalent. Repeated drivers remain distinct and in source order unless the
  DSL explicitly defines a merge operation.
- A value expression is either one variable/literal or one flat operator followed
  only by variables/literals: `(operator operand ...)`. Value expressions cannot
  contain nested expressions.
- Every compound source expression is split into deterministic temporaries named
  `t0`, `t1`, and so on, so each serialized value expression remains flat.
  Dependencies precede their uses.
- The allowed value operators are defined centrally. The initial contract must
  cover `not`, `and`, `or`, `xor`, `mux`, `bufif0`, `bufif1`, and the equality
  forms required by the corpus. Operators such as `nand`, `nor`, and `xnor` may
  be emitted only after they are added to the DSL contract and covered by
  reviewed fixtures; otherwise they are expressed using the contracted
  primitive operators.
- Every assignment carries exactly one tagged delay tuple: `(delay value)`,
  `(delay rise fall)`, or `(delay rise fall turn-off)`. Its arity and entries
  preserve the selected source delay tuple exactly; entries are neither filled,
  summed, nor discarded. A missing source delay becomes `(delay 0)`. An omitted
  tuple entry is rejected until an equally explicit representation is
  contracted.
- Delay expressions may be nested. Timing sums within any tuple entry use
  `(+ ...)`, and timing primitives use forms such as
  `(elmore (wire L_x) (pmos 5))`.
- Selecting only the first tuple component remains a supported downstream
  compatibility policy, but selection does not occur during conversion.
- Unknown, ambiguous, or unrepresentable behavior is an error in strict mode
  and is never silently simplified.

For example, this is valid because each value expression is flat while the delay
expression may be nested:

```scheme
(t0 (not a) (delay 0))
(t1 (and t0 b) (delay 0))
(y (mux select t1 c)
  (delay
    (+ (elmore (wire L_y) (pmos 5)) extra_delay)
    T_fall_y))
```

This is invalid because the `not` value expression is nested inside `and`:

```scheme
(y (and (not a) b) (delay 0))
```

## Validation Rules

Every milestone uses the strongest applicable layers below:

1. Unit tests for local lexer, parser, analyzer, lowering, and serializer rules.
2. Golden AST, analysis, IR, and `.cell` fixtures for representative cells.
3. Corpus checks that report processed, skipped, warned, and failed files.
4. Manual comparison of each new or changed fixture against its SystemVerilog
   source and the DSL contract.
5. Explicit review of fixture diffs whenever lowering behavior changes.
6. Parsing and idempotent formatting of generated output with `sexpr-fmt`.

A milestone cannot be marked complete if its stated tests are absent, even when
the implementation appears to work on manually selected files. Automated cell
simulation or behavioral equivalence checking is outside the project scope.

## Current Status

See [STATUS.md](STATUS.md).

## Milestones

### Milestone 0: Freeze the DSL and Diagnostic Contracts

Status: complete as of 2026-07-10. The frozen contract is
[CONTRACT.md](CONTRACT.md).

Define the contracts that later lowering work must implement.

Expected to be working after this milestone:

- A documented list of legal value, driver, state, and timing forms.
- A documented decision for initial values, repeated drivers, keepers,
  transistor primitives, omitted delays, and the then-current selected-first
  policy. Milestone 14 supersedes that timing policy with exact tuple
  preservation.
- A diagnostic classification with `error`, `warning`, and intentional-ignore
  categories.
- `--strict` semantics: warnings become failures; errors always fail.

Expected not to be working yet:

- Conversion of cells that depend on unresolved keeper or transistor choices.
- Full-corpus lowering or conversion.

Not expected to do:

- Implement parsing or lowering merely to exercise the contract.
- Define analog behavior beyond what the cell DSL can represent.

Acceptance conditions:

- The contract includes examples for every currently emitted operator and delay
  form.
- The contract defines flat value expressions and permits nesting only inside
  delay expressions.
- No emitted operator exists only by convention in `lower.rs`.
- Every known corpus construct is classified as supported in a named milestone,
  intentionally ignored with justification, or blocked on a contract decision.

### Milestone 1: Corpus Inventory and Lexer

Status: complete as of 2026-07-11. The deterministic survey inventory covers
all 206 curated files and reports zero unsupported capabilities.

Complete the lexer and turn `survey` into a capability inventory rather than a
token counter only.

Expected to be working after this milestone:

- File, line, and column diagnostics for invalid tokens.
- Stable tokenization of every curated file.
- An inventory of statement, expression, primitive, generate, instantiation,
  timing, strength, and literal forms, with files using each form.
- A supported/deferred/ignored classification tied to later milestones.

Expected not to be working yet:

- Parsing, semantic analysis, or conversion may still reject valid token
  streams.

Not expected to do:

- Validate expression precedence or SystemVerilog semantics.
- Infer support from token frequency alone.

Acceptance conditions:

- `check sv-cells --stage lex` reports `processed=206 failed=0`.
- Survey output is deterministic and identifies unsupported constructs by file.
- Token snapshot tests cover the reference cell, generate syntax, named port
  instantiation, strength/delay tuples, transistor calls, and `'0/'1/'x/'z`.
- A deliberately invalid character produces an exact source location.

### Milestone 2: Lossless Specialized Parser and AST

Status: complete as of 2026-07-11. Full-corpus parsing consumes every token in
all 206 curated files, and deterministic typed AST fixtures and exhaustive
coverage tests preserve all observed source forms with logical source spans.

Parse every curated file into typed AST nodes without silently discarding input.

Expected to be working after this milestone:

- Typed AST nodes for all corpus module, declaration, expression, procedural,
  primitive, instantiation, generate, and specify forms.
- Source spans and deterministic debug rendering for every node.
- Correct expression precedence and grouping.
- Explicit AST representation for constructs that later stages defer.

Expected not to be working yet:

- Generate conditions need not be evaluated.
- Instantiations need not be resolved to module definitions.
- Parsing success does not imply semantic or lowering support.

Not expected to do:

- Parse SystemVerilog constructs absent from the curated corpus.
- Store unparsed statement bodies as raw strings.

Acceptance conditions:

- `check sv-cells --stage parse` reports `processed=206 failed=0`.
- Golden AST fixtures cover a simple gate, latch, generated DFF, tri-state cell,
  hierarchical adder, keeper, specify block, and transistor-heavy IRQ cell.
- AST coverage tests prove that no source item is dropped or converted to an
  untyped placeholder.
- Invalid and truncated constructs report the expected construct and location.

### Milestone 3: Correct Semantic Analysis and Support Classification

Status: complete as of 2026-07-11. Catalog-aware analysis classifies all 206
curated files as 1 supported and 205 explicitly deferred, with zero warnings or
failures; reviewed fixtures cover state, internal nets, primitive drivers,
generate alternatives, timing structure, and resolved hierarchy.

Build a trustworthy normalized analysis before lowering any additional family.

Expected to be working after this milestone:

- Correct symbol tables for ports, parameters, declarations, localparams, and
  specparams.
- Correct input/output classification for inout ports based on actual reads and
  writes.
- Register classification limited to initial/stateful procedural targets.
- Separate classification of continuous nets, state variables, primitive nets,
  and hierarchical connections.
- Generate branches represented as alternatives until a later milestone selects
  one; mutually exclusive branches are never combined as simultaneous drivers.
- Specify paths and timing aliases preserved structurally.
- Per-file support status for each later lowering capability.

Expected not to be working yet:

- Unsupported items may still prevent lowering.
- Generate branches, keepers, hierarchy, and transistors may remain unresolved.
- Timing aliases need not yet be converted to final DSL delay expressions.

Not expected to do:

- Treat analysis success as proof that conversion is supported.
- Guess instantiation port directions without resolving the instantiated module.

Acceptance conditions:

- The reference analysis identifies inputs `clk clk_n ena ena_n s_n pch_n d`,
  outputs `q q_n d`, and registers `ff1 ff2 q_n`.
- A combinational cell with internal nets reports no registers.
- A primitive-driven internal tri net reports no register.
- A generated DFF analysis keeps the `nodelay` branches distinct.
- A hierarchical adder records named connections and resolves the referenced
  module's port directions without elaborating its behavior.
- `check --stage analyze` reports supported, deferred, warned, and failed counts;
  it cannot report a deferred construct as fully supported.

### Milestone 4: Flat SSA IR and Combinational Logic

Status: complete as of 2026-07-11. Reviewed operator and compound-expression
goldens use deterministic dependency-first temporaries, and a full-corpus audit
proves all 185 currently successful lowerings contain only flat contracted value
expressions. The remaining 21 files fail explicitly in later-milestone families
with frozen source diagnostics.

Implement the reusable IR and scalar combinational subset without timing.

Expected to be working after this milestone:

- Deterministic SSA temporaries for every nested source operation.
- Every IR and serialized value expression contains at most one operator and
  atom operands. No operand is another operator expression.
- Scalar continuous assignments using `!`, `~`, `&`, `&&`, `|`, `||`, `^`,
  `~^`, `~&`, `~|`, equality, and ordinary value ternaries where present.
- Stable source/dependency order independent of filesystem traversal.
- Zero delay only for source assignments that genuinely have no modeled delay.
- Fixture coverage for every supported operator and representative compound
  expressions.

Expected not to be working yet:

- Delayed assignments, stateful logic, tri-state values, generate blocks,
  hierarchy, keepers, and transistor primitives may fail lowering explicitly.

Not expected to do:

- Flatten hierarchical cells.
- Apply Boolean rewrites that change four-state behavior unless the contract
  explicitly permits a two-state approximation for that cell family.

Acceptance conditions:

- Reviewed fixtures cover representative `and`, `or`, `xor`, inverter, NAND,
  NOR, XNOR, equality, and value-mux cells.
- Each fixture is manually checked against its source expression when created or
  changed.
- Golden IR and output fixtures contain stable `t0`, `t1`, ... numbering.
- A structural validator rejects any nested value expression while accepting
  nested delay expressions.
- Unsupported delayed or driver constructs fail with their source location.
- No uncontracted operator appears in serialized output.

### Milestone 5: Flat Stateful Cells

Status: complete as of 2026-07-12. Reviewed fixtures cover blocking and
nonblocking latches, set/reset behavior, nested same-target priority, and
combinational procedural non-state. A recursive corpus audit identifies 27
stateful files: all 21 currently emitted cells contain exact analyzed registers
and flat source-ordered retained state equations, while five generated-state
files and one later-driver latch remain explicitly assigned to later milestones.

Lower stateful procedural logic that does not depend on unresolved generate or
hierarchical behavior.

Expected to be working after this milestone:

- `initial` targets classified as registers according to the DSL contract.
- Blocking and non-blocking assignments normalized consistently.
- `always_latch` and supported stateful `always` forms lowered to next-state
  equations such as `(mux enable data old_value)`.
- Set/reset and nested source conditions preserved through flat temporaries.
- Multiple procedural assignments maintain source-defined priority.

Expected not to be working yet:

- DFF/TFF files containing unresolved `generate if (nodelay)` may remain
  deferred to Milestone 8.
- Specify-derived timing remains deferred to Milestone 7.

Not expected to do:

- General event scheduling, races, or arbitrary procedural blocks.
- Infer latch semantics from unsupported procedural code.

Acceptance conditions:

- Reviewed fixtures cover simple latch, blocking/non-blocking variants,
  set/reset latch, nested `if`, and multiple assignments with priority.
- Stateful golden outputs contain flat `mux` assignments whose condition and
  data operands are variables/literals, never nested operator expressions.
- All flat supported `dff*` and `dlatch*` cells lower successfully.
- Every deferred stateful file is listed with a specific later milestone; the
  milestone is not called complete merely because selected DFFs pass.
- Combinational procedural targets are not listed as registers.

### Milestone 6: Tri-State, Precharge, Strength, and Multiple Drivers

Status: complete as of 2026-07-12. Reviewed fixtures cover both high-Z
polarities, open drain, precharge, direct signal primitives, bidirectional
pads, supply ties, and repeated bus drivers with all four contracted strength
pairs. A scope-aware corpus audit identifies 67 relevant files: all 53 current
successes preserve flat driver form, strength, and source order, while 14 files
remain explicitly blocked on M10 or M11 behavior.

Implement driver semantics independently from timing semantics.

Expected to be working after this milestone:

- Direct `bufif0` and `bufif1` primitive calls with literal or signal values.
- Continuous ternaries with either branch equal to `'z`, including signal-valued
  drives such as `ena_n ? 'z : in`.
- Precharge/open-drain forms and repeated drivers in source order.
- Strength annotations interpreted according to the Milestone 0 contract or
  rejected if they require unrepresentable behavior.
- Reviewed fixtures for every supported driver normalization.

Expected not to be working yet:

- Delay tuples remain deferred to Milestone 7.
- Keeper instantiations remain deferred to Milestone 10.
- `nmos`, `pmos`, and `rnmos` remain deferred to Milestone 11.

Not expected to do:

- Collapse multiple drivers into Boolean OR/AND unless the DSL contract proves
  that transformation valid.
- Silently discard strength information.

Acceptance conditions:

- `buf_if0`, constant open-drain, precharge, bidirectional pad, and repeated-bus
  driver fixtures match driven value, enabled state, and high-Z state.
- `ena_n ? 'z : in` emits an appropriate `bufif0` form, not `(mux ... z ...)`.
- Compound driver conditions are assigned to temporaries before a flat
  `bufif0` or `bufif1` assignment uses them.
- A strength combination outside the contract fails explicitly.
- Tests assert complete assignments, including target and driver expression,
  rather than discarding unverified fields.

### Milestone 7: First-Entry Timing and Specify Paths

Status: complete as of 2026-07-12. Deterministic alias resolution preserves
resistance sums, real device factors, and outer multipliers; every later tuple
entry is a visible intentional ignore. Reviewed goldens cover explicit-delay
precedence, single and ambiguous specify paths, procedural state timing, and the
reference cell's exact `q_n`, `q`, and `d` assignments. A typed 206-file audit
reconciles 393 explicit-delay, 186 specify-derived, and 32 zero-default source
assignments, with 41 additional control-dependent path distinctions now
recorded as intentional ignores, 999 later-entry ignores, and 372 preserved
emitted resistance multiplications. The corpus has no one-entry delay tuple, so
that required case remains covered by its focused unit test while two- and
three-entry forms are covered by corpus witnesses.

This section records the acceptance contract used for the original release.
Milestone 14 supersedes its selected-first output policy without invalidating
the historical implementation and test results recorded here.

Implement nested delay expressions using the DSL's first-entry-only policy.

Expected to be working after this milestone:

- Localparam/specparam alias resolution without silently dropping factors.
- `tpd_elmore`, multi-argument `tpd_z`, resistance sums, resistance
  multiplication, real factors such as `1.5`, and delay sums used by the corpus.
- Lowering of exactly the first delay tuple entry for every assignment,
  primitive, and specify path.
- Specify path lookup and composition for assignments without attached delays.
- Symbolic diagnostics for contract-approved approximations and strict failures
  for genuinely unsupported or unrepresentable formulas.

Expected not to be working yet:

- Timing through unresolved hierarchy, keepers, or transistor networks may be
  deferred to Milestones 9, 10, and 11 respectively.

Not expected to do:

- Use, combine, or emit the second or third delay tuple entries after parsing
  them as part of the source syntax.
- Add delay tuple entries together.
- Drop outer resistance multipliers from the selected first entry.
- Claim analog accuracy beyond the documented Elmore model.

Acceptance conditions:

- Unit tests cover one-, two-, and three-entry delay tuples and prove that all
  three forms lower to their first entry only.
- Precharge `#(T_rise, T_Z, T_Z)` lowers to `T_rise`, never
  `(+ T_rise T_Z T_Z)`.
- A tuple whose first entry is `T_Z` lowers to `T_Z`, regardless of later fall
  or turn-off entries.
- `R_nmos_ohm(8*L_unit) * 2` and `* 1.5` preserve their full meaning.
- `pad_xtal.sv` timing lowers without an unsupported-factor error.
- The reference cell's `q_n`, `q`, and `d` delays match the symbolic structure
  obtained from the first applicable source/specify tuple entry. The checked-in
  reference output is updated if it encodes a different policy.
- Timing golden tests assert the entire rendered assignment.

### Milestone 8: Generate Branch Selection

Status: complete as of 2026-07-12. Typed elaboration selects exactly one
module-level `if (nodelay)` branch before configured analysis and lowering;
delayful selection is the default and `--nodelay` explicitly selects the true
branch. Reviewed dual-mode fixtures and an exact 206-file audit cover all five
generate cells, prove that no unselected declaration, state, driver, timing
alias, diagnostic, or requirement leaks into output, and leave only the 16
Milestone 9–11 corpus deferrals in either mode.

Resolve the conditional generate forms used by the curated corpus.

Expected to be working after this milestone:

- A documented and configurable treatment of `nodelay` generate branches.
- Exactly one generate branch contributes declarations, state, assignments, and
  timing aliases to analysis and lowering.
- Diagnostics identify an unresolved or unsupported generate condition.

Expected not to be working yet:

- Ordinary hierarchy remains deferred to Milestone 9.
- Keeper instances remain deferred to Milestone 10.
- Direct transistor primitives remain deferred to Milestone 11.

Not expected to do:

- General generate loops, arbitrary constant elaboration, or generate syntax
  absent from the curated corpus.
- Combine both branches to avoid choosing the configured mode.

Acceptance conditions:

- `dffr`, `dffr_cc`, `dffr_cc_q`, `dffsr`, and `tffnl` select exactly one
  branch.
- Reviewed analysis, IR, and output fixtures cover both supported `nodelay`
  configurations where both are intended converter modes.
- Fixture inspection confirms that no declaration or driver from the unselected
  branch appears in output.
- No curated cell fails solely because of a supported `Generate` node.

### Milestone 9: Hierarchical Cell Instantiations

Status: complete as of 2026-07-12. Catalog-aware lowering recursively flattens
ordinary instances with typed parameter/port substitution, deterministic
`<instance>__<child-name>` qualification, parent-global SSA temporary order,
and configured recursion checks. Reviewed `half_add` and `full_add` fixtures
preserve all seven source-ordered instances, bindings, connections, drivers,
and substituted delays; the exact dual-mode corpus audit lowers both adders and
leaves only the 14 Milestone 10–11 deferrals.

Resolve ordinary cell instances used to compose larger cells.

Expected to be working after this milestone:

- Named and positional port connections and parameter overrides are resolved.
- Supported cell instances are either flattened with deterministic names or
  represented directly by a form fixed in the DSL contract.
- Instance output drivers and internal dependencies appear in deterministic
  order.

Expected not to be working yet:

- Keeper instances remain deferred to Milestone 10.
- Direct transistor primitives remain deferred to Milestone 11.

Not expected to do:

- General recursive elaboration, arbitrary external module resolution, or
  support for modules outside the curated cell library.
- Treat an unknown module as an empty instance.

Acceptance conditions:

- Reviewed fixtures for `half_add` and `full_add` show the expected flattened or
  contracted instance connections for every input and output.
- Fixture inspection confirms deterministic temporary and instance-derived
  names across repeated runs.
- Parameter overrides used by the hierarchical corpus are present in analysis
  and reflected in any affected output delay.
- No curated cell fails solely because of an ordinary supported instantiation.

### Milestone 10: Keeper Representation

Status: complete as of 2026-07-12. Typed special-instance analysis records a
distinct `KeeperDriven` role and source-ordered keeper driver, while lowering
emits the contracted arity-zero `(keeper)` value at forced delay zero. Reviewed
fixtures cover all five required cells, the exact six-instance corpus audit
proves keeper targets never become registers or merge with neighboring drivers,
and configured lowering now leaves only the 10 Milestone 11 transistor
deferrals.

Define and lower the keeper instances used by tri-state and mux cells.

Expected to be working after this milestone:

- Keeper behavior has one documented DSL representation or normalization.
- Keeper-driven nets remain distinguishable from ordinary state registers and
  from independent tri-state drivers.
- Keeper placement and source order are deterministic.

Expected not to be working yet:

- Direct `nmos`, `pmos`, and `rnmos` primitives remain deferred to Milestone 11.

Not expected to do:

- Simulate charge storage or analog keeper strength.
- Ignore keeper instances merely to make lowering return success.

Acceptance conditions:

- Reviewed fixtures cover `mux`, `muxi`, `pad_xtal`, `idu_bit0`, and
  `reg_wz_out` keeper usage where the rest of each file is supported.
- Manual fixture inspection confirms that the keeper form is attached to the
  intended net and is not listed as a register.
- No curated cell fails solely because of a `keeper` instance.

### Milestone 11: Transistor-Heavy Cells

Status: complete as of 2026-07-12. All 25 curated transistor calls lower as
direct, flat, typed drivers: 17 `nmos`, 7 `pmos`, and 1 `rnmos` across the 10
transistor-bearing files. Reviewed fixtures and an exact dual-mode corpus audit
preserve topology, polarity, source order, repeated drivers, and first-entry
timing without a `bufif*` normalization or transistor-specific warning.

Implement the transistor contract selected in Milestone 0.

Expected to be working after this milestone:

- `nmos`, `pmos`, and `rnmos` represented directly or normalized only where the
  transformation is electrically valid under the DSL contract.
- Signal propagation, enable polarity, strength behavior, repeated drivers, and
  timing retained to the supported fidelity.
- IRQ priority, IDU, and bus-injection transistor families lowered.

Expected not to be working yet:

- Full-corpus file writing and release-quality CLI behavior remain Milestone 12.

Not expected to do:

- General transistor-network solving or SPICE-equivalent simulation.
- Replace a transistor with `bufif*` without a documented rationale and reviewed
  output fixture.

Acceptance conditions:

- Reviewed fixtures cover every transistor normalization used.
- `dlatch_ee_irq`, `idu_bit0`, `idu_bit123456`, and all IRQ priority cells lower
  successfully.
- No curated cell fails because of `nmos`, `pmos`, or `rnmos`.
- Any fidelity limitation is documented next to the DSL contract and exposed in
  non-strict diagnostics.

### Milestone 12: Full Corpus Conversion and Release Gate

Status: complete as of 2026-07-18. Transactional corpus conversion mirrors all
206 curated sources into canonical checked cells with documented dry-run,
strict, overwrite, filtering, and generate-mode behavior. Strict delayful and
nodelay lowering report zero warnings and zero failures; the approved
first-path specify approximation is visible as an intentional ignore and never
promoted by strict mode. Release tests prove exact path coverage, structural IR
validation, formatter canonicality and idempotence, reference equality,
byte-identical repeated conversion, and precise no-partial-output failures. The
checked 206-file output tree and CI release workflow reproduce those gates.

Complete the CLI, serializer, corpus output, and end-to-end validation.

Expected to be working after this milestone:

- `convert` mirrors input paths below `sv-cells` into `sexpr-cells`.
- `--dry-run`, `--strict`, `--overwrite`, and filtering have documented behavior.
- Diagnostics summarize processed, skipped, warned, written, and failed files.
- Serializer output is deterministic and canonical under `sexpr-fmt`.
- All generated cells pass structural checks and reviewed fixture tests.

Expected not to be working yet:

- Nothing required for the curated corpus. A failure here blocks completion.

Not expected to do:

- Convert arbitrary third-party SystemVerilog outside the declared subset.
- Overwrite checked-in files without explicit `--overwrite`.

Acceptance conditions:

- `check sv-cells --stage lower --strict` reports `processed=206 failed=0` and
  no unexpected warnings.
- `convert sv-cells sexpr-cells --dry-run --strict` succeeds deterministically.
- `convert sv-cells sexpr-cells --strict --overwrite` writes exactly 206 mirrored
  `.cell` files.
- Every generated file parses with `sexpr-fmt`; formatting it twice is
  byte-identical; canonical generated output passes `sexpr-fmt --check`.
- The IR/output structural validator reports zero nested value expressions and
  permits nested delay expressions.
- Running conversion twice produces no diff.
- The checked-in reference output matches modulo approved comments/formatting.
- Parser, analyzer, lowering, timing, serializer, fixture, and corpus tests all
  pass in CI.
- Every changed fixture has been manually compared with its source and reviewed
  as part of the change.
- Unsupported input outside the curated subset fails with a precise diagnostic
  rather than producing partial output.

### Milestone 13: Preserve Register Initial Values

Status: complete as of 2026-07-18. Register state is represented by typed,
source-ordered `(name initial-value)` pairs. Selected scalar contracted literal
initializers are preserved as four-state metadata instead of intentional
ignores, while modeled registers without a selected initializer explicitly use
`x`. All 206 checked cells and their reviewed fixtures have been regenerated;
strict delayful and nodelay lowering now report 1,309 and 1,299 intentional
ignores respectively, with zero warnings or failures.

Expected to be working after this milestone:

- Typed IR register entries preserve exact `0`, `1`, `x`, or `z` initial
  metadata independently from next-state assignments.
- Integer `0`/`1`, unbased `'0`/`'1`/`'x`/`'z`, and arbitrarily grouped forms
  normalize to the corresponding target atom.
- A modeled register without a selected source initializer serializes with
  initial value `x`.
- Generate selection and hierarchy qualification preserve only the selected,
  correctly qualified register metadata.
- Multiple selected initializers for one register fail at the second target
  rather than being merged or ordered as simulator events.

Not expected to do:

- Model an initial event queue, scheduling, races, or arbitrary initial block
  bodies.
- Accept non-scalar targets or non-contracted initializer expressions.

Acceptance conditions:

- `Cell::registers` uses an explicit register type with a typed four-state
  initial value, validates unique nonempty names, and serializes every nonempty
  list uniformly as `(registers (name value) ...)`.
- Focused tests cover all six accepted source literal spellings, grouping,
  implicit `x`, selected generate alternatives, qualified child state, and an
  exact duplicate-initializer diagnostic at the second target.
- Valid selected initializers emit neither assignments nor diagnostics and are
  removed from the authorized intentional-ignore categories.
- The configured corpus contains exactly 27 register-bearing cells and 48
  registers: 42 initialized to `0`, 6 initialized to `x`, and none initialized
  to `1` or `z`. The latter two values remain covered by focused tests.
- Assignment totals remain unchanged at 1,958 delayful and 1,955 nodelay.
- Strict delayful and nodelay lowering process all 206 files with zero warnings,
  zero failures, and exactly 1,309 and 1,299 intentional ignores.
- All 206 mirrored checked cells are formatter-canonical and byte-identical to
  repeated strict conversion; the checked reference remains equal to its
  reviewed timing fixture.

### Milestone 14: Preserve Complete Delay Tuples

Status: complete as of 2026-07-18. Every assignment now carries a validated,
tagged one-, two-, or three-entry delay tuple. The exact corpus audit covers
1,958 delayful assignments with emitted arities 1,223/276/459, preserves every
component of all selected source tuples, and compares all 1,958 first-component
projections with the former selected-first behavior with zero mismatches. Both
configured modes lower all 206 files with zero warnings, zero failures, and
exactly 49 intentional ignores, all for additional specify paths. All 206
checked cells are formatter-canonical and byte-identical to repeated strict
conversion. This milestone preserves the current explicit-delay precedence and
temporary first-source-ordered specify-path selection; it does not yet infer
shared physical paths, redistribute delay between assignments, or emit a
timing-arc representation.

Expected to be working after this milestone:

- Every IR assignment carries a typed one-, two-, or three-entry delay tuple.
- Every present source tuple entry is preserved in source order, including
  nested symbolic timing expressions.
- Missing source timing is represented uniformly as `(delay 0)`.
- Serialized assignments use exactly `(delay value)`, `(delay rise fall)`, or
  `(delay rise fall turn-off)`.
- A downstream compiler can select the first component and recover the prior
  single-delay behavior without regenerating cells.
- Later tuple entries no longer produce intentional-ignore diagnostics.

Expected not to be working yet:

- Multiple control-dependent specify paths for one target still select the
  first path in source order and produce one intentional-ignore diagnostic per
  used target at the second path.
- Assignment delays need not yet reconstruct every overlapping specify path.
- No timing arcs or constraint tables are serialized.

Implementation phases:

1. Freeze this roadmap and revise [CONTRACT.md](CONTRACT.md) to define tagged
   tuple syntax, exact source arity, downstream selection compatibility, and
   the temporary first-path policy. This phase changes no Rust or generated
   output.
2. In `src/ir.rs`, create typed `TimingExpr` and `DelayTuple` representations;
   migrate `Assignment::delay` and `LoweredModule::timing_aliases`; add tuple
   iteration, mapping, validation, and first-component projection helpers. In
   `src/lower.rs`, replace single-expression tuple lowering with
   `lower_delay_tuple`, retain every component for explicit and specify timing,
   use a typed one-entry zero tuple for missing delays, and remove later-entry
   intentional ignores. Update hierarchy substitution and qualification to map
   every tuple component. Compile-focused tests complete this phase; serialized
   syntax and checked goldens may remain temporarily unmigrated within the
   phase branch.
3. In `src/serialize.rs`, render tagged delay tuples. Migrate test builders,
   IR/cell fixtures, corpus audit expectations, the reference cell, and all 206
   checked outputs. Do not change assignment placement or value SSA during this
   mechanical migration.
4. Add exact inventory tests and a compatibility oracle that projects every
   tuple through its first component and compares it with the pre-Milestone 14
   semantics. Update release documentation and run the complete acceptance
   gate before marking the milestone complete.

Files, types, and functions:

- `src/ir.rs`: make `TimingExpr` a validated newtype over the existing
  S-expression tree, constructible only through timing atoms/operators, so a
  value operator cannot enter a delay accidentally. Define
  `DelayTuple::One(TimingExpr)`, `DelayTuple::Two { rise, fall }`, and
  `DelayTuple::Three { rise, fall, turn_off }`; add
  `DelayTuple::{first, components, map, try_map, validate}`. Migrate
  `Assignment::delay`, `LoweredModule::timing_aliases`, and structural
  validation.
- `src/lower.rs`: `lower_delay_tuple`, `zero_delay_tuple`, and tuple-aware
  explicit/specify lookup; remove the later-entry-ignore path while preserving
  first-source-ordered path selection.
- `src/hierarchy.rs`: parameter substitution and qualified-name rewriting over
  every timing component.
- `src/serialize.rs`: deterministic `(delay ...)` rendering.
- `src/convert.rs`, `src/survey.rs`, test helpers, fixtures, and checked cells:
  migrate tuple-aware counts, comparison, and output.
- `CONTRACT.md`, `README.md`, `PLAN.md`, `STATUS.md`, and CI expectations:
  document and verify the accepted representation. `PLAN.md` and `STATUS.md`
  completion updates occur only after all acceptance checks pass.

No new production dependency is required. `sexpr-fmt` remains the canonical
output parser and formatter.

Acceptance conditions:

- Source inventory is preserved exactly: assignment delays contain 45
  two-entry and 60 three-entry tuples; primitive delays contain 21 two-entry
  and 399 three-entry tuples; specify paths contain 263 two-entry and 3
  three-entry tuples. The curated corpus contains no omitted tuple component.
- Tuple arity, expression structure, factors, and source ordering are unchanged;
  no entry is copied, filled, summed with another entry, or discarded.
- First-component projection reproduces the pre-Milestone 14 delay expression
  for every assignment, including the temporary selected first specify path.
- Assignment totals remain exactly 1,958 delayful and 1,955 nodelay.
- Strict delayful and nodelay lowering process all 206 files with zero warnings
  and zero failures. Intentional ignores are exactly 49 in each mode, all for
  additional specify paths; no tuple-entry ignore remains.
- All focused and full tests, `cargo fmt --check`, and clippy pass. All 206
  checked outputs parse and are canonical/idempotent under `sexpr-fmt`, and a
  repeated strict conversion is byte-identical.

### Milestone 15: Build the Functional Timing Graph

Status: planned after Milestone 14. Preserve every specify path as an internal
constraint and relate it to the flat functional IR without changing serialized
output or the temporary first-path assignment placement.

Expected to be working after this milestone:

- Every specify control, target, transition tuple, and source span is retained
  in a deterministic timing-constraint graph.
- Functional dependencies distinguish combinational edges, register/state
  boundaries, operand position, timing polarity, and reconvergence.
- Reachability, dominator, and post-dominator information identifies candidate
  shared prefixes, shared suffixes, and public-output splits.
- The 49 ambiguous target groups have a deterministic structural
  classification instead of being visible only as lowering diagnostics.

Implementation plan:

- Add `src/timing_graph.rs` with `TimingGraph`, `TimingNodeId`,
  `TimingNodeKind`, `DependencyEdge`, `TimingSense`, `Transition`, and
  `TimingConstraint`.
- Implement `build_timing_graph`, `cut_register_cycles`,
  `collect_timing_constraints`, `classify_timing_sense`,
  `validate_constraint_reachability`, and deterministic graph reporting.
- Add `src/timing_terms.rs` with `DelayTerm`, `AdditiveDelay`, exact flattening
  of associative timing `+`, opaque structural terms for all other timing
  expressions, and deterministic reconstruction. No algebraic simplification
  or symbolic subtraction is permitted.
- Add `petgraph = "0.8"` as the only new production dependency for stable
  directed graphs, strongly connected components, topological traversal,
  reachability, and dominance algorithms. Continue to use source-ordered
  `Vec` and `BTreeMap` for stable external ordering; do not add `indexmap`.
- Keep timing constraints internal to analysis/lowering. The serializer must
  continue to emit only ordinary assignments with tagged delay tuples.

Acceptance conditions:

- All 266 structural specify paths and 480 scalar controls are retained exactly
  once with their full tuples and source provenance; the deterministic report
  contains 203 target groups and identifies the known 49 multiple-path groups.
- Register feedback is cut only at modeled state boundaries; ordinary
  combinational cycles and unreachable controls are source-spanned errors.
- Positive-unate, negative-unate, non-unate/conditional, and state-control
  dependencies have focused tests, including inversion of rise/fall sense.
- Graph construction is deterministic across repeated runs and filesystem
  traversal order.
- Milestone 14 cell output and diagnostics remain byte-identical.

### Milestone 16: Decompose Timing Paths into Assignment Delays

Status: planned after Milestone 15. Convert internal control-to-target timing
constraints into exact delay tuples on ordinary assignments. Timing arcs remain
an implementation input and verification oracle; they are never serialized.

Expected to be working after this milestone:

- Shared path suffixes are placed on shared assignments, while source-specific
  prefixes are represented by deterministic delay-only identity assignments.
- Outputs that are read internally can be split into an internal raw value and
  a public identity assignment, preventing an output-local delay from being
  counted again on a derived output.
- Every accepted control-to-target transition reconstructs the original timing
  expression exactly from the delays along its emitted functional path.
- Unrepresentable hidden topology or conflicting reconvergent paths produce a
  source-spanned error rather than a heuristic approximation.

Files, types, and functions:

- Add `src/timing_decompose.rs` with `DelayPlacement`, `Decomposition`,
  `DecompositionError`, `decompose_timing`, `insert_edge_delay`,
  `split_public_output`, and `verify_decomposition`.
- Add deterministic `d0`, `d1`, ... names for timing-only identity assignments
  without perturbing logical `t0`, `t1`, ... numbering; reject collisions.
- Extend `src/timing_terms.rs` with exact ordered term containment,
  factor/recompose operations, and structural equality for each tuple
  component. Non-additive subexpressions such as `elmore`, multiplication,
  clamps, and aliases remain indivisible terms.
- Rewrite all tuple components jointly with edge timing sense so inversion maps
  rise and fall constraints correctly and turn-off remains distinct.
- Use deterministic exact-cover/backtracking over the small per-cell candidate
  placement set. Prefer existing assignments, then shared post-dominator
  placement, then sites nearer the target, with source order as the final tie
  breaker. Do not introduce floating-point linear programming, negative
  coefficients, or synthesized timing subtraction.
- Add `proptest = "1"` as a development dependency for generated acyclic graph
  properties, especially decompose/reconstruct equality. Do not add `good_lp`
  or `microlp`; their floating-point models are not authoritative for exact
  symbolic golden output. Exact rational crates remain deferred unless corpus
  evidence proves whole-term placement insufficient.

Acceptance conditions:

- Reviewed fixtures cover `ao21`, `dffsr`, complementary outputs, shared
  suffixes, source-specific prefixes, inversion, state boundaries,
  reconvergence, and a precise unrepresentable case.
- The `dffsr` public `q_n` equation does not accumulate `q`'s public output
  delay; both outputs derive from the appropriate raw state/path region.
- Every accepted original path and transition is independently reconstructed
  and structurally equals its source constraint.
- Removing `dN` identities and collapsing raw/public output splits preserves
  the original value equations, driver order, registers, and initial metadata.
- No serialized timing arc, timing table, negative delay, arbitrary subtraction,
  or first-path fallback is produced.
- Output, synthetic naming, and diagnostics are deterministic and all unchanged
  non-timing fixtures remain stable.

### Milestone 17: Full-Corpus Timing Closure

Status: planned after Milestone 16. Close every former multiple-specify-path
approximation across the 206-cell corpus and make exact per-assignment timing a
release gate.

Implementation plan:

- Run decomposition and independent reconstruction across both delayful and
  nodelay modes. Classify every failure as a conflicting constraint,
  reconvergent/non-unate analysis gap, or hidden physical topology absent from
  the functional RTL.
- Improve the generic graph/decomposition implementation before introducing
  cell-specific knowledge. If exact hidden topology cannot be recovered, stop
  for a reviewed contract decision. A permitted fallback must be a typed,
  checked-in topology hint with validated module/signal/term references, never
  a name-string heuristic or arbitrary output override.
- Only if reviewed hints are required, add a schema and `serde` with derive plus
  the compatible `toml` crate. Only if exact coefficient splitting is proven
  necessary, add `num-rational`, `num-bigint`, and `num-traits`; floating-point
  solving remains prohibited.
- Regenerate and manually review affected fixtures and checked cells, then run
  the full release gate and update documentation/status.

Acceptance conditions:

- All 49 former additional-path intentional ignores are gone in delayful and
  nodelay modes; all 206 files lower with zero warnings, zero intentional
  ignores, and zero failures.
- Every emitted delay placement passes independent full-tuple path
  reconstruction against all retained source constraints.
- All output remains assignment-only, formatter-canonical, idempotent, and
  byte-identical across repeated strict conversion.
- Full converter tests, formatter tests, formatting, clippy, staged corpus
  checks, and CI pass; every changed golden is manually compared with its
  SystemVerilog source and this contract.
- `PLAN.md` and `STATUS.md` record completion only after the preceding checks
  and the chosen treatment of any hidden-topology case are reviewed.

## Required Test Layout

Tests may remain next to small units, but corpus and golden coverage should be
visible as first-class test suites:

```text
sv-to-sexpr/tests/
  fixtures/
    sv/
    ast/
    analysis/
    ir/
    cell/
  corpus_tests.rs
  fixture_tests.rs
  timing_tests.rs
  cli_tests.rs
```

## Definition of Done

The original full-corpus release baseline was accepted at Milestone 13. The
revised timing work is done only when Milestone 17 is accepted. In particular:

- All 206 files produce deterministic, structurally valid, manually reviewed
  fixture output for every supported construct family.
- Register lists contain state only and preserve exact four-state initial
  metadata, using `x` when no selected initializer exists.
- Generate branches are not combined accidentally.
- Tri-state drivers never encode high-Z as an ordinary mux value unless that is
  explicitly part of the DSL contract.
- Every source delay tuple preserves its exact arity and every present entry;
  transition entries are never filled, summed together, or discarded.
- Specify timing required by the reference and corpus is decomposed into
  ordinary assignment delays and independently reconstructs every retained
  control-to-target transition constraint.
- Every value expression is flat, and SSA temporary order is deterministic.
- No in-scope construct, strength, multiplier, driver, tuple entry, or timing
  path is silently lost.
- Strict conversion succeeds with zero unsupported constructs.
