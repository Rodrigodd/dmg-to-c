# SystemVerilog to S-Expression Cell Converter Plan

## Goal

Implement `sv-to-sexpr`, a Rust tool that converts the curated SystemVerilog cell corpus into the SSA-like S-expression cell DSL.

Primary paths:

- Input cells: `sv-cells/**/*.sv`
- Output cells: `sexpr-cells/**/*.cell`
- The input corpus currently contains 206 files from copied subsets of:
  - `dmg-sim/dmg_cpu_b/cells`
  - `dmg-sim/sm83/cells`
- Reference pair:
  - `sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv`
  - `sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell`

The converter only needs to parse and lower the cell subset used by this repository. It should not attempt to implement a complete SystemVerilog frontend.

## Target Output Model

Each converted file should serialize one `(cell ...)` form:

```scheme
(cell
  module_name
  (inputs ...)
  (outputs ...)
  (registers ...)
  (assignments
    (target expr delay)
    ...
  )
)
```

Output conventions:

- `inputs` include `input` ports and `inout` ports that are read by logic.
- `outputs` include `output` ports and `inout` ports that are driven by logic.
- `registers` include variables assigned by `initial` and `always_latch`.
- `assignments` are in SSA-like order.
- Temporary names should be deterministic: `t0`, `t1`, `t2`, ...
- Assignment expressions use the DSL operators already implied by the reference:
  - Boolean: `(not x)`, `(and a b ...)`, `(or a b ...)`, `(xor a b)`
  - State hold: `(mux enable data old_value)`
  - Tri-state/precharge: `(bufif0 value control)`, `(bufif1 value control)`
  - Delay arithmetic: `0`, `(+ ...)`, `(elmore (wire L_x) (pmos n))`, `(elmore (wire L_x) (nmos n))`
- Comments in output are optional initially, but the serializer should preserve enough structure to allow readable section comments later.

## Implementation Phases

### 1. Corpus Survey

Build a small scanner command before implementing the parser deeply.

Tasks:

- Enumerate all `sv-cells/**/*.sv` files.
- Collect all statement forms:
  - `assign`
  - `initial`
  - `always_latch`
  - primitive calls such as `bufif0`, `bufif1`, `nmos`, `pmos`, `rnmos`
  - `localparam realtime`
  - `specify` / `specparam`
- Collect all expression operators:
  - unary `!`
  - binary or n-ary `&`, `|`, `^`
  - logical `&&`, `||`
  - equality/inequality if present
  - ternary `?:`
  - constants `0`, `1`, `'0`, `'1`, `'z`
- Emit a report listing unsupported constructs by file.

Acceptance criteria:

- Running the survey over all 206 curated cell files produces a stable construct inventory.
- Every construct is classified as either supported now, deliberately ignored, or blocked.

### 2. Incremental CLI Harness

Implement the command-line interface early, before the full parser and converter are complete. The CLI should accept a single file and run the deepest implemented stage, so each new feature can be validated against every file in `sv-cells`.

Suggested commands:

```text
sv-to-sexpr lex <input.sv>
sv-to-sexpr parse <input.sv>
sv-to-sexpr analyze <input.sv>
sv-to-sexpr lower <input.sv>
sv-to-sexpr convert-file <input.sv> <output.cell>
sv-to-sexpr survey <input-dir>
sv-to-sexpr check <input-dir>
sv-to-sexpr convert <input-dir> <output-dir>
```

Stage behavior:

- `lex` tokenizes one file and reports unexpected characters.
- `parse` lexes and parses one file, optionally dumping the AST.
- `analyze` parses and builds symbol/register/timing summaries.
- `lower` analyzes and emits IR, without requiring final serialization.
- `convert-file` runs the full pipeline for one file.
- `check` recursively runs the deepest supported non-writing stage over all `*.sv` files in a directory.
- `convert` recursively writes `.cell` files for all supported inputs.

Suggested flags:

- `--stage lex|parse|analyze|lower|convert`: lets `check` stop at a specific stage.
- `--strict`: treat warnings and unsupported timing as errors.
- `--overwrite`: allow replacing existing `.cell` files.
- `--dry-run`: parse/lower/serialize without writing.
- `--emit-tokens <dir>`: write token dumps.
- `--emit-ast <dir>`: write AST debug snapshots.
- `--emit-ir <dir>`: write IR debug snapshots.
- `--filter <glob-or-substring>`: process selected cells.

Acceptance criteria:

- `sv-to-sexpr lex sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv` succeeds first.
- `sv-to-sexpr check sv-cells --stage lex` can be used to drive the "lex all files" milestone.
- Later stages reuse the same file-oriented CLI and diagnostics.
- Diagnostics summarize processed, skipped, warned, and failed files.

### 3. Specialized Lexer

Implement a small lexer in `sv-to-sexpr/src/lexer.rs`.

Token classes:

- Identifiers and escaped identifiers if discovered in the corpus.
- Keywords used by the cell subset:
  - `module`, `endmodule`
  - `parameter`, `localparam`, `real`, `realtime`
  - `input`, `output`, `inout`, `logic`, `tri`, `wire`
  - `import`
  - `initial`
  - `always_latch`
  - `assign`
  - `specify`, `endspecify`, `specparam`
- Numeric literals:
  - decimal integers
  - real numbers if needed for parameters
  - SystemVerilog constants `'0`, `'1`, `'z`
- Punctuation and operators:
  - `(`, `)`, `[`, `]`, `{`, `}`, `,`, `;`, `:`, `?`, `#`
  - `=`, `<=`, `*>`
  - `!`, `&`, `|`, `^`, `&&`, `||`, `!==`, `==`, `!=`
  - `+`, `-`, `*`, `/`
- Comments:
  - line comments `// ...`
  - block comments `/* ... */`
- Preprocessor directives:
  - tolerate and ignore `` `default_nettype ... ``

Acceptance criteria:

- Lexer tests cover representative lines from the reference cell.
- Lexer reports file, line, and column for invalid tokens.
- `sv-to-sexpr check sv-cells --stage lex` succeeds for all 206 files.

### 4. Specialized Parser

Implement a recursive-descent parser in `sv-to-sexpr/src/parser.rs`.

Parser scope:

- Module declarations:
  - module name
  - optional `#(...)` parameter list
  - port list with direction/type groups
- Declarations:
  - `logic a, b;`
  - `localparam realtime NAME = expr;`
  - ignore `import sm83_timing::*;`
- Statements:
  - `initial target = literal;`
  - `always_latch if (enable_expr) target = data_expr;`
  - `always_latch if (enable_expr) target <= data_expr;`
  - `assign [strength] [delay] target = expr;`
  - primitive calls: `[primitive] [strength] [delay] (args...);`
  - `specify ... endspecify`
- Expressions:
  - identifiers
  - constants
  - parenthesized expressions
  - unary not
  - binary/n-ary logical and/or/xor
  - arithmetic expressions for timing parameters
  - function calls such as `tpd_elmore(...)`, `R_pmos_ohm(...)`, `tpd_z(...)`
  - ternary expressions for tri-state assigns

Intentionally out of scope unless the corpus proves otherwise:

- Packed/unpacked vectors and arrays.
- Procedural blocks other than single-line `always_latch if`.
- Generate blocks.
- Full strength semantics beyond identifying high-Z/strong drive direction.
- Full specify path semantics beyond extracting delay formulas relevant to output assignments.

Acceptance criteria:

- Parser unit tests parse:
  - simple combinational cells such as `and2.sv`
  - latch cells such as `dff_cc_q.sv`
  - the reference `dffs_cc_ee_pch_d_reg_pc_bit.sv`
  - tri-state cells such as `reg_pc_out_bit012.sv`
- Parse errors include the current file, line, column, and expected construct.
- `sv-to-sexpr check sv-cells --stage parse` succeeds for all 206 files before conversion milestones begin.

### 5. Parsed AST

Define a direct AST that mirrors the supported SystemVerilog subset.

Suggested modules:

- `ast.rs`
  - `Module`
  - `Port`
  - `Direction`
  - `Decl`
  - `Statement`
  - `PrimitiveCall`
  - `SvExpr`
  - `TimingExpr`
- `diagnostic.rs`
  - source spans
  - warnings
  - errors

The AST should preserve:

- Source spans for diagnostics.
- Port directions and whether ports are `tri`/`inout`.
- Assignment kind: blocking, non-blocking, continuous.
- Delay annotations from assigns and primitives.
- Localparam/specparam timing expressions.

Acceptance criteria:

- AST debug snapshots are deterministic and can be used in tests.
- AST contains no stringly typed statement bodies.

### 6. Semantic Analysis

Lower the AST into a normalized cell analysis model before generating the final DSL.

Responsibilities:

- Build symbol tables:
  - parameters
  - ports
  - internal signals
  - localparams/specparams
  - register candidates
- Classify port usage:
  - inputs
  - outputs
  - bidirectional nets that appear in both `inputs` and `outputs`
- Classify registers:
  - any target initialized by `initial`
  - any target assigned by `always_latch`
- Resolve timing aliases:
  - map `T_rise_d = tpd_elmore(L_d, R_pmos_ohm(5*L_unit))`
  - map `T_Z_d = tpd_z(T_rise_d)`
  - preserve unknown timing formulas as symbolic fallback, but warn.
- Extract specify delays where they determine an output assignment delay.
- Detect repeated drivers on a net and model them as separate assignments unless a later DSL merge rule is required.
- Reject unsupported constructs with precise diagnostics instead of silently producing incorrect output.

Acceptance criteria:

- The reference cell analysis identifies:
  - inputs: `clk`, `clk_n`, `ena`, `ena_n`, `s_n`, `pch_n`, `d`
  - outputs: `q`, `q_n`, `d`
  - registers: `ff1`, `ff2`, `q_n`
  - three latch assignments, one inverter assignment, one `bufif0` assignment.

### 7. Expression Lowering to SSA-like IR

Introduce an intermediate representation independent of SystemVerilog syntax.

Suggested modules:

- `ir.rs`
  - `Cell`
  - `Assignment`
  - `Expr`
  - `Delay`
  - `Signal`
- `lower.rs`
  - AST-to-IR lowering

Lowering rules:

- Break compound expressions into deterministic temporaries.
- Preserve source evaluation order where possible.
- Convert `&&` and `&` to `(and ...)` for 1-bit logic.
- Convert `||` and `|` to `(or ...)` for 1-bit logic.
- Convert `!x` to `(not x)`.
- Convert `a ? b : c` to `(mux a b c)` when the result is a value.
- Convert latch statements:
  - `always_latch if (enable) q <= data;`
  - becomes temporary assignments for `enable`, temporary assignments for `data`, then `(q (mux enable data q) delay)`.
- Convert simple continuous assigns:
  - `assign q = !q_n;`
  - becomes `(q (not q_n) delay)`.
- Convert tri-state zero drivers:
  - `assign y = cond ? 0 : 'z;`
  - becomes `(y (bufif1 0 cond) delay)` or an equivalent DSL form.
- Convert precharge primitives:
  - `bufif0 (...)(d, '1, pch_n);`
  - becomes `(d (bufif0 1 pch_n) delay)`.
  - `bufif1 (...)(d, '0, cond);`
  - becomes `(d (bufif1 0 cond) delay)`.
- Convert transistor primitives only after defining a DSL representation:
  - either direct forms such as `(nmos drain source gate)`
  - or normalized `bufif*` equivalents when electrically valid for the corpus.

Acceptance criteria:

- Lowering the reference cell produces the same dependency order and equivalent expression tree as the checked-in `.cell`.
- Temporary numbering is stable across runs and independent of filesystem order.

### 8. Timing Lowering

Implement the timing subset needed by cells.

Initial supported mapping:

- `tpd_elmore(L_x, R_pmos_ohm(N*L_unit))` -> `(elmore (wire L_x) (pmos N))`
- `tpd_elmore(L_x, R_nmos_ohm(N*L_unit))` -> `(elmore (wire L_x) (nmos N))`
- Multiplicative resistance factors:
  - `R_nmos_ohm(8*L_unit) * 2` should become an equivalent normalized form.
  - If the DSL cannot represent factor multiplication yet, preserve it symbolically and record the gap.
- Delay sums:
  - `T_a + T_b + T_c` -> `(+ delay_a delay_b delay_c)`
- `tpd_z(...)`:
  - usually ignored for driven value delays unless it is the only available timing annotation.
- Assign delay tuples:
  - choose rise or fall delay based on the driven value/operator where known.
  - for output inverters, choose the path matching the produced transition convention used by the reference.
- `specify` path delays:
  - parse `specparam` aliases.
  - parse path assignments such as `(clk, clk_n *> q_n) = (...);`.
  - use them to annotate latch/output assignments when local delay annotations are not directly attached.

Acceptance criteria:

- The reference `q_n`, `q`, and `d` delay expressions lower to the same symbolic structure as the checked-in `.cell`, modulo formatting.
- Unsupported timing formulas produce warnings and a non-zero exit only in strict mode.

### 9. Serializer

Implement deterministic S-expression serialization in `sv-to-sexpr/src/serialize.rs`.

Requirements:

- Stable indentation matching the current style closely enough for review.
- One output file per input file.
- Module name from `module sm83_...` or `module dmg_...` is the cell name.
- Output path should mirror the input path below `sv-cells`:
  - `sv-cells/sm83/cells/foo.sv`
  - `sexpr-cells/sm83/cells/foo.cell`
  - `sv-cells/dmg_cpu_b/cells/foo.sv`
  - `sexpr-cells/dmg_cpu_b/cells/foo.cell`
- Preserve deterministic section order:
  - `inputs`
  - `outputs`
  - `registers`
  - `assignments`
- Use multi-line formatting for complex expressions and compact single-line formatting for short expressions.

Acceptance criteria:

- Reformatting a generated file with `sexpr-fmt` produces stable output.
- The generated reference file diff is explainable and ideally empty after formatting.

### 10. Validation Strategy

Use layered validation so parser bugs and lowering bugs are easy to isolate.

Parser validation:

- Unit tests for lexer and expression precedence.
- Fixture tests for representative cells.
- Golden AST snapshots for tricky files.

Lowering validation:

- Golden IR snapshots.
- Golden `.cell` output for the reference file.
- Add goldens incrementally for each construct family:
  - simple gates
  - inverting gates
  - latches/flip-flops
  - precharge cells
  - open-drain/tri-state output cells
  - IRQ priority cells with transistor primitives

Corpus validation:

- `sv-to-sexpr survey sv-cells`
- `sv-to-sexpr check sv-cells --stage lex`
- `sv-to-sexpr check sv-cells --stage parse`
- `sv-to-sexpr check sv-cells --stage analyze`
- `sv-to-sexpr check sv-cells --stage lower`
- `sv-to-sexpr convert sv-cells sexpr-cells --dry-run`

Round-trip/format validation:

- Run generated files through `sexpr-fmt`.
- Parse generated `.cell` files if/when a cell DSL parser exists.

Semantic validation:

- For combinational cells, compare a truth table generated from the SystemVerilog expression AST against the generated IR expression for all input combinations.
- For latches, compare next-state equations:
  - generated `q_next = mux(enable, data, q_old)`
  - source `always_latch if (enable) q = data`
- For tri-state cells, compare driver conditions and driven values.

Acceptance criteria:

- CI can run parser and lowering tests without external tools.
- A full corpus check is deterministic and produces no unexpected unsupported constructs.

## Milestones

### Milestone 1: CLI and Lex All Files

- Implement the CLI harness with `lex`, `survey`, and `check --stage lex`.
- Implement the lexer and token diagnostics.
- Validate `sv-to-sexpr lex sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv`.
- Validate `sv-to-sexpr check sv-cells --stage lex` over all 206 files.
- Add token snapshot tests for representative files.

### Milestone 2: Parse All Files

- Implement parser coverage for the full curated corpus.
- Implement AST data structures and AST debug output.
- Validate `sv-to-sexpr parse sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv`.
- Validate `sv-to-sexpr check sv-cells --stage parse` over all 206 files.
- Add golden AST snapshots for representative combinational, latch, tri-state, and transistor-heavy cells.

### Milestone 3: Analyze All Files

- Implement symbol tables, port classification, register classification, localparam/specparam collection, and unsupported-construct diagnostics.
- Validate `sv-to-sexpr analyze sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv`.
- Validate `sv-to-sexpr check sv-cells --stage analyze` over all 206 files.
- Confirm the reference cell analysis identifies the expected inputs, outputs, registers, and primitive assignments.

### Milestone 4: Reference Cell Conversion

- Implement enough lowering, timing, and serialization to regenerate `dffs_cc_ee_pch_d_reg_pc_bit.cell`.
- Cover boolean expressions, latch-to-`mux` lowering, output inverter lowering, `bufif0`, localparams, and relevant specify delays.
- Validate `sv-to-sexpr convert-file sv-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.sv sexpr-cells/sm83/cells/dffs_cc_ee_pch_d_reg_pc_bit.cell --dry-run`.
- Add a golden output test for the reference pair.

### Milestone 5: Simple Combinational Cells

- Support common continuous assignments with `!`, `&`, `|`, `^`.
- Convert simple gates such as `and2`, `and3`, `or3_b`, `nor8_alu`, `xor_idu_l`.
- Add truth-table comparison tests.

### Milestone 6: Register and Latch Families

- Cover all `dff*` and `dlatch*` cells.
- Normalize blocking and non-blocking latch assignments the same way.
- Support set/reset variants such as source expressions containing `!s_n` or reset conditions.

### Milestone 7: Tri-state and Precharge Families

- Support continuous tri-state assigns with strengths and delay tuples.
- Support repeated drivers on the same net.
- Cover register output bus cells and precharge decoder cells.

### Milestone 8: Transistor-heavy Cells

- Define the DSL representation for `nmos`, `pmos`, and `rnmos`.
- Lower IRQ priority and bus-injection cells.
- Add explicit tests for any electrical simplification used to map transistors to existing DSL operators.

### Milestone 9: Full Corpus Conversion

- Convert all files in `sv-cells`.
- Write outputs to mirrored paths under `sexpr-cells`.
- Require zero unsupported constructs in strict mode.
- Document any intentional semantic approximations.

## File Layout

Proposed Rust source layout:

```text
sv-to-sexpr/src/
  main.rs
  cli.rs
  diagnostic.rs
  lexer.rs
  parser.rs
  ast.rs
  analyze.rs
  ir.rs
  lower.rs
  timing.rs
  serialize.rs
  survey.rs
```

Suggested test layout:

```text
sv-to-sexpr/tests/
  fixtures/
    sv/
    cell/
  lexer_tests.rs
  parser_tests.rs
  lower_tests.rs
  corpus_tests.rs
```

## Key Risks

- Timing semantics may not be fully inferable from local `assign` delays; some cells rely on `specify` paths.
- Multiple drivers and transistor primitives need a precise DSL representation before conversion can be considered complete.
- SystemVerilog strength annotations are probably only a clue for high-Z behavior, not something the initial DSL can represent completely.
- Delay tuple selection can become inconsistent unless the lowering rules are documented and tested per construct family.
- A parser that silently skips unsupported statements would be dangerous; unsupported constructs must be hard diagnostics in strict mode.

## Definition of Done

- `sv-to-sexpr convert sv-cells sexpr-cells --strict` succeeds.
- All 206 curated cells produce deterministic `.cell` files under mirrored `sexpr-cells` paths.
- The reference cell output matches the checked-in target modulo formatting.
- Parser, lowering, timing, serializer, and corpus tests pass.
- Unsupported SystemVerilog outside the known cell subset fails with clear diagnostics instead of producing partial output.
