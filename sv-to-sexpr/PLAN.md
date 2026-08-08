# SystemVerilog to S-Expression Cell Converter Plan

## Goal

Implement `sv-to-sexpr`, a Rust tool that converts the curated scalar
SystemVerilog cell corpus into the repository's SSA-like S-expression cell DSL.

Primary paths:

- Input cells: `sv-cells/**/*.sv`
- Output cells: `sexpr-cells/**/*.cell`
- Current curated corpus: 190 files. The 15 former `dff*.sv` cells are
  intentionally out of scope as of Milestone 17 and will be translated
  manually in separate work. `sm83/cells/dlatch_ee_irq.sv` was additionally
  retired from the converter corpus because it does not decompose; its
  hand-written translation lives in `sexpr-cells/manual/dlatch_ee_irq.cell`
  and is not produced, checked, or formatted by the converter.

The converter is complete only when its output preserves the modeled logic,
state, drivers, and timing according to reviewed fixtures. Parsing a file or
serializing a syntactically valid S-expression is not sufficient by itself.

## Scope

### In Scope

- The SystemVerilog constructs that occur in the retained 190-file curated
  corpus.
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
  (parameters (parameter value) ...)
  (assignments
    (target expression (delay timing-expression ...))
    ...
  )
)
```

The `(parameters ...)` section is currently emitted by the serializer but is
not yet part of the frozen contract in [CONTRACT.md](CONTRACT.md); see
[Future Work](#future-work).

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

Status: complete as of 2026-07-24. Timing-aware lowering retains all 266
source-owned specify paths, 480 scalar controls, 203 target groups, and 49
multiple-path groups with exact tuples and source provenance. Hierarchy
expansion deterministically produces 273 constraints, 494 controls, and 210
target groups. The typed functional graph distinguishes operand, drive,
state-control, state-boundary, and multiply-driven resolved-net-boundary edges;
its separately cut DAG supports reachability, dominance, shared-prefix,
shared-suffix, reconvergence, and public-output-split analysis while preserving
the full graph for inspection. Exact additive timing-term reconstruction passes
for every constraint in both generate modes. All 206 cells pass the
timing-aware corpus audit under forward and reversed traversal, while ordinary
Milestone 14 output, diagnostics, and the aggregate checked-cell digest remain
byte-identical. Timing constraints remain internal and no timing arc or table is
serialized.

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

Status: complete as of 2026-07-26. Generic exact-cover decomposition now
distributes ordered timing terms across ordinary assignments, maximizes
compatible shared prefixes and suffixes, and introduces deterministic `dN`
identities only for source-specific edges. A checked-in, typed physical-topology
hint handles the hidden internal regions of delayful `dmg_dffsr`; all twelve
rise/fall recipes independently reconstruct the six source constraints without
routing `q_n` through `q`'s public output delay. Timing arcs remain an internal
verification oracle and are never serialized.

The `dmg_dffsr` physical-topology overlay was part of the verified Milestone 16
baseline. Milestone 17 supersedes that scope decision: all `dff*.sv` cells are
retired from the converter corpus, and the now-unused overlay implementation is
removed rather than retained as a dormant extension point.

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
  `DecompositionError`, `decompose_timing`, and `verify_decomposition`.
- Add `src/timing_apply.rs` with typed transformation facts, independent
  verification against the rebuilt graph, deterministic application of edge
  identities through `insert_edge_delay`, raw/public transformations through
  `split_public_output`, and exact erasure back to the baseline equations,
  driver provenance, registers, and signal metadata.
- Add deterministic `d0`, `d1`, ... names for timing-only identity assignments
  without perturbing logical `t0`, `t1`, ... numbering, skipping any index whose
  name is already reserved by the cell.
- Extend `src/timing_terms.rs` with exact ordered term containment,
  factor/recompose operations, and structural equality for each tuple
  component. Non-additive subexpressions such as `elmore`, multiplication,
  clamps, and aliases remain indivisible terms.
- Rewrite all tuple components jointly with edge timing sense so inversion maps
  rise and fall constraints correctly and turn-off remains distinct.
- Use deterministic exact-cover/backtracking over the small per-cell candidate
  placement set. Prefer existing assignments, then shared post-dominator
  placement with maximal compatible path breadth, then sites nearer the target,
  with source order as the final tie breaker. Do not introduce floating-point
  linear programming, negative coefficients, or synthesized timing
  subtraction.
- Add `src/topology_hint.rs`, `src/topology_apply.rs`, and
  `src/topology_verify.rs` for source-spanned typed physical-topology overlays,
  exact materialization and erasure, and independent reconstruction from the
  actual transformed graph. Keep the reviewed delayful `dmg_dffsr` topology in
  `topology-hints/dmg_dffsr.toml`; its knownness guards select the exact
  baseline equations whenever a four-state input or state value is not binary.
- Use `serde` derives and `toml` for the checked-in topology schema. Keep
  source-ordered `Vec` and `BTreeMap` as the deterministic external boundary;
  do not add a second graph or ordered-map crate.
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

Status: in progress after the completed Milestone 16. Retire the 15 DFF-family
cells that will be translated manually, remove their one-off physical-topology
machinery, close every former multiple-specify-path approximation in the
retained corpus, and make exact per-assignment timing a bounded release gate.

Step progress as audited on 2026-08-08:

- Step 1 (retire DFF corpus and topology machinery): complete. Additionally,
  `sm83/cells/dlatch_ee_irq.sv` was retired for the same reason, leaving 190
  curated sources.
- Step 2 (rebase tests and fixtures): complete. The planned repository reference
  pair was dropped instead of being switched to `dlatch_ee_irq`, so the plan no
  longer names a reference pair.
- Step 3 (bounded sharded decomposition audit): complete. The sharded suite
  covers each of the 190 files once per mode and records the current outcome
  per case, within the configured 45-second limit.
- Step 4 (classify and fix retained decomposition failures): not started. Ten
  files still fail decomposition in both modes and are recorded in the shard
  goldens as `failure unclassified`.
- Step 5 (make decomposition the ordinary lowering path): not started.
  Decomposition is opt-in behind `--decompose-timing`; the legacy
  first-source-ordered specify-path selection and its 44 intentional ignores
  remain the default. The checked `sexpr-cells` tree is a mix: it was written
  with `--decompose-timing`, so the 180 decomposable cells carry decomposed
  delays while the 10 failing cells retain the first-path approximation.

The retained decomposition failures fall into two error classes:

- Incompatible placement (one site required to carry different values for two
  constraints): `buf_if0`, `not_if0`, `not_if1`.
- Transition-ambiguous placement (an edge whose rise/fall sense cannot be
  assigned a single tuple rewrite): `dlatch`, `drlatch_ee`, `nand_latch`,
  `nor_latch`, `tffnl`, `mux_idu_h`, `mux_idu_l`.

Implementation plan:

1. Retire the DFF-family corpus and its special topology implementation.
   Delete every source whose basename matches `dff*.sv`: the four
   `sv-cells/dmg_cpu_b/cells/dff*.sv` files and eleven
   `sv-cells/sm83/cells/dff*.sv` files. Delete their mirrored generated
   `sexpr-cells/**/*.cell` outputs; TFF and latch cells remain in scope. The
   separately versioned `dmg-sim` submodule and its upstream source manifest
   are not modified by this converter milestone. Delete
   `topology-hints/dmg_dffsr.toml`, `src/topology_hint.rs`,
   `src/topology_apply.rs`, and `src/topology_verify.rs`. In `src/lower.rs`,
   collapse `LoweredDecomposedTimingModel`, `DecomposedTimingStrategy`, and
   `DecomposedTimingErasure` to the single generic exact-cover path and expose
   its `Decomposition`, `AppliedTimingFacts`,
   `ActualDecompositionVerification`, and `TimingErasure` directly. Remove the
   topology modules from `src/lib.rs`; remove `TopologyTemporary`,
   `GeneratedTopology`, `TopologyPlacement`, and their topology-only helpers
   from `src/timing_graph.rs`. Remove the direct `serde` and `toml`
   dependencies from `Cargo.toml`; remove the TOML packages and direct root
   dependency edges from `Cargo.lock`. Cargo may retain inactive optional
   `serde` package metadata declared by `petgraph`, but `cargo tree -i serde`
   must show no active dependency. Keep `petgraph` for the functional graph and
   `proptest` for exact symbolic decomposition properties; add no solver or
   numeric crate.

2. Rebase tests and fixtures on the retained corpus without weakening generic
   language coverage. Replace corpus references to removed DFF files in
   `src/analyze.rs`, `src/lower.rs`, `sexpr-fmt/src/format.rs`, and the
   integration suites under `tests/` with retained latch, TFF, generate,
   driver, and timing cells where the tested construct still exists. Remove
   DFF-only fixtures under `tests/fixtures/{analysis,ast,generate,stateful,
   timing_decomposition,timing_graph}` and regenerate all aggregate corpus
   snapshots for exactly 190 sorted paths. The repository no longer designates
   a single reference pair. Do not keep a copied DFF as a hidden corpus fixture
   merely to preserve a historical golden; focused inline unit syntax may
   remain only when it tests a generic parser or analyzer invariant.

3. Replace the monolithic timing-closure audit with bounded independent test
   cases in `tests/timing_decomposition_corpus_tests.rs`. Partition the sorted
   190-file corpus deterministically into fixed shards for both delayful and
   nodelay modes. Each retained file is lowered twice, compared byte-for-byte,
   checked for canonical assignment-only output, and independently verified
   against every retained full-tuple timing constraint. A cheap inventory test
   proves that the shards cover each of the 191 files exactly once per mode.
   Run these and all other converter tests through `cargo nextest run`; the
   checked-in `.config/nextest.toml` slow timeout terminates a test after three
   15-second periods (45 seconds total). No test-only timeout, ignored case, or
   silent skip may mask an expensive or failing cell.

4. Use the bounded audit to classify each retained failure by exact source path
   and mode as a conflicting constraint, reconvergent/non-unate analysis gap,
   or generic exact-cover search defect. Improve `src/timing_graph.rs`,
   `src/timing_decompose.rs`, `src/timing_apply.rs`, and
   `src/timing_terms.rs` generically, one reviewed failure class at a time.
   Hidden physical topology has no permitted hint fallback in the retained
   design: if a retained cell cannot be represented exactly as assignment
   delays, stop for a new scope or source-structure decision. Floating-point
   solving, negative coefficients, arbitrary subtraction, name heuristics, and
   first-path selection remain prohibited.

   The audit is in place and the ten current failures are listed above, but no
   failure class has been classified or fixed yet. Every shard golden still
   records them as `failure unclassified`. Retiring a failing cell to
   `sexpr-cells/manual/` is a scope decision, not a classification, and each
   such retirement must be recorded in this plan.

5. Make exact decomposition the ordinary lowering path used by CLI checks,
   `convert-file`, and corpus conversion; remove the legacy first-path warning
   and intentional-ignore path and the now-redundant `--decompose-timing`
   opt-in. Regenerate
   all 190 checked `sexpr-cells` outputs, manually compare every changed timing
   placement with its retained SystemVerilog source and the independent
   reconstruction evidence, then run the complete release gate and update
   `CONTRACT.md`, `STATUS.md`, and this plan.

Acceptance conditions:

- Exactly 15 `sv-cells/**/dff*.sv` files and their 15 generated
  `sexpr-cells/**/dff*.cell` outputs are removed; no in-scope converter test or
  fixture retains a dead reference to them. Exactly 190 source/output pairs
  remain, while TFF and latch cells remain covered. The external `dmg-sim`
  submodule remains clean and pinned to its existing revision. Files retired
  beyond the DFF family are named in this plan with their reason.
- The physical-topology hint files, public variants, provenance roles, tests,
  direct `serde`/`toml` dependencies, and TOML lock packages are absent, and
  the decomposed lowering API has one generic exact-cover strategy.
- Every former additional-path intentional ignore belonging to a retained cell
  is gone in delayful and nodelay modes; all 190 files lower in both modes with
  zero warnings, zero intentional ignores, and zero failures. Removed cases are
  reported as explicit scope removals, never counted as successful lowers.
- Every emitted delay placement passes independent full-tuple path
  reconstruction against all retained source constraints.
- All output remains assignment-only, formatter-canonical, idempotent, and
  byte-identical across repeated strict conversion. A corpus conversion with
  any failing cell writes nothing.
- `cargo nextest run --manifest-path sv-to-sexpr/Cargo.toml` passes with no
  test exceeding the configured 45-second limit. Formatter tests, formatting,
  clippy `--all-targets`, staged corpus checks, and CI also pass; every changed
  golden is manually compared with its retained SystemVerilog source and this
  contract.
- `PLAN.md` and `STATUS.md` record completion only after all preceding checks
  and the DFF/topology retirement are reviewed.

## Future Work

Work that earlier milestones assumed, or that later changes introduced, and
that is not implemented as of 2026-08-08. Each item is a prerequisite for
accepting Milestone 17 unless it is explicitly marked otherwise.

### Reinstate CI

The repository has no CI workflow since it was removed in `9e6263b`. The release
gate that Milestone 12 and Milestone 17 both cite has to run somewhere before
either can be re-accepted. The gate is `cargo nextest run`, `cargo test`,
`cargo fmt --check`, and `cargo clippy --all-targets` for both crates, plus the
four staged corpus checks.

### Contract the emitted `(parameters ...)` section

Lowering records every module parameter as a `(name value)` pair with an `f64`
value, and the serializer always emits a `(parameters ...)` section: 157 of the
190 checked cells carry at least one entry and 33 carry an empty section. This
form is not described in [CONTRACT.md](CONTRACT.md), has no reviewed fixture
coverage of its own, and has no stated rule for value normalization, duplicate
names, or hierarchy qualification. Freeze it in the contract, or remove it.

### Close the retained timing decomposition failures

This is Milestone 17 steps 4 and 5, restated as concrete outstanding work:

- Classify and fix the incompatible-placement class (`buf_if0`, `not_if0`,
  `not_if1`) and the transition-ambiguous-placement class (`dlatch`,
  `drlatch_ee`, `nand_latch`, `nor_latch`, `tffnl`, `mux_idu_h`, `mux_idu_l`).
- Replace the default first-source-ordered specify-path selection and its 44
  intentional ignores with exact decomposition, and drop the
  `--decompose-timing` opt-in.
- Regenerate the checked tree from one lowering path. It is currently a mix of
  180 decomposed cells and 10 cells that retain the first-path approximation,
  so no single command reproduces it. With the transactional guard restored, a
  decomposed corpus conversion writes nothing until the ten failures are fixed.

### Decide the status of `sexpr-cells/manual/`

`sexpr-cells/manual/dlatch_ee_irq.cell` is hand-written, is not produced by the
converter, is not formatter-canonical, and carries no `(parameters ...)`
section. Decide whether hand-written cells are part of the checked output
contract at all, and if so which checks apply to them.

### Remove stray working files

`sv-to-sexpr/output.txt`, `sv-to-sexpr/profile.json`, `sv-to-sexpr/slow.txt`,
and `sv-to-sexpr/snippets.rs` are checked-in artifacts of the decomposition
performance work. They are not referenced by the build and are stale.

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

- All 190 retained files produce deterministic, structurally valid, manually
  reviewed fixture output for every supported construct family.
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
