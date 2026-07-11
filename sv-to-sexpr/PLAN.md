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
- Per-transition rise, fall, turn-off, or high-Z delays. The DSL stores one
  delay per assignment and uses only the first SystemVerilog delay tuple entry.
- Guessing semantics for unsupported constructs. The tool must reject them.

## Target Output Contract

Each converted file contains one cell form:

```scheme
(cell
  module_name
  (inputs ...)
  (outputs ...)
  (registers ...)
  (assignments
    (target expression delay)
    ...
  )
)
```

The following rules are part of the contract and must be tested:

- `inputs` contains input ports and inout ports read by the cell.
- `outputs` contains output ports and inout ports driven by the cell.
- `registers` contains only modeled state, such as variables initialized by
  `initial` or assigned by stateful procedural logic. Continuously driven and
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
- Only the first entry of a SystemVerilog delay tuple is lowered. All later tuple
  entries are intentionally ignored because the DSL does not model separate
  transition delays. A missing delay is `0`; an explicitly omitted first tuple
  entry is rejected unless Milestone 0 defines an equivalent single delay.
- Delay expressions may be nested. Timing sums within the selected first tuple
  entry use `(+ ...)`, and timing primitives use forms such as
  `(elmore (wire L_x) (pmos 5))`.
- Unknown, ambiguous, or unrepresentable behavior is an error in strict mode
  and is never silently simplified.

For example, this is valid because each value expression is flat while the delay
expression may be nested:

```scheme
(t0 (not a) 0)
(t1 (and t0 b) 0)
(y (mux select t1 c) (+ (elmore (wire L_y) (pmos 5)) extra_delay))
```

This is invalid because the `not` value expression is nested inside `and`:

```scheme
(y (and (not a) b) 0)
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

Define the contracts that later lowering work must implement.

Expected to be working after this milestone:

- A documented list of legal value, driver, state, and timing forms.
- A documented decision for initial values, repeated drivers, keepers,
  transistor primitives, omitted first delays, and the single-delay policy.
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

Implement nested delay expressions using the DSL's first-entry-only policy.

Expected to be working after this milestone:

- Localparam/specparam alias resolution without silently dropping factors.
- `tpd_elmore`, multi-argument `tpd_z`, resistance sums, resistance
  multiplication, real factors such as `1.5`, and delay sums used by the corpus.
- Lowering of exactly the first delay tuple entry for every assignment,
  primitive, and specify path.
- Specify path lookup and composition for assignments without attached delays.
- Symbolic warnings for contract-approved approximations and strict failures for
  ambiguous/unrepresentable formulas.

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

The project is done only when Milestone 12 is accepted. In particular:

- All 206 files produce deterministic, structurally valid, manually reviewed
  fixture output for every supported construct family.
- Register lists contain state only.
- Generate branches are not combined accidentally.
- Tri-state drivers never encode high-Z as an ordinary mux value unless that is
  explicitly part of the DSL contract.
- Each source delay uses only its first tuple entry; later transition entries are
  intentionally ignored and are never summed.
- Specify timing required by the reference and corpus is present.
- Every value expression is flat, and SSA temporary order is deterministic.
- No in-scope construct, strength, multiplier, driver, or first-entry timing path
  is silently lost. Later delay tuple entries are excluded by explicit policy.
- Strict conversion succeeds with zero unsupported constructs.
