# Cell DSL and Diagnostic Contract

This document defines the cell format the converter emits and the diagnostics
it may report. `ir::ValueOperator` and `ir::TimingOperator` are the
machine-readable authorities for operator spellings and arities. A construct
not described here is unsupported and must produce a source-spanned error.

## Cell and expression shape

Each input file produces exactly one form:

```scheme
(cell
  module_name
  (inputs input_port inout_port_if_read ...)
  (outputs output_port inout_port_if_driven ...)
  (registers (modeled_state initial_value) ...)
  (parameters (name value) ...)
  (assignments
    (target value-expression (delay timing-expression ...))
    ...
  )
)
```

Names and literals are atoms. A value expression is either one atom or one
legal value operator whose operands are all non-empty atoms. Operator-valued
operands are forbidden. Compound source expressions are split in dependency
order into assignments to deterministic `t0`, `t1`, ... temporaries. Thus
`(t0 (not a) (delay 0))` followed by
`(y (and t0 b) (delay 0))` is legal, while
`(y (and (not a) b) (delay 0))` is not. Timing expressions have their own
operator set and may nest recursively inside each delay-tuple component.

`inputs` contains input ports plus inouts that are read. `outputs` contains
output ports plus inouts that are driven. `registers` contains only modeled
state, never a continuous, primitive, hierarchical, or keeper-driven net. Each
register entry is a `(name initial-value)` pair whose value is exactly one of
the four atoms `0`, `1`, `x`, or `z`.

`parameters` records each module parameter as a `(name value)` pair in source
order, with the value normalized to a numeric atom. The section is always
emitted and is empty when the module declares no parameter. Parameter names are
reserved and cannot collide with a generated temporary.

## Value operators

The table is exhaustive. NAND, NOR, and XNOR are direct contracted operators;
this avoids Boolean rewrites that could change four-state behavior.

| Operator | Arity | Example |
| --- | ---: | --- |
| `not` | 1 | `(not a)` |
| `and` | 2 or more | `(and a b c)` |
| `or` | 2 or more | `(or a b c)` |
| `xor` | 2 or more | `(xor a b)` |
| `nand` | 2 or more | `(nand a b)` |
| `nor` | 2 or more | `(nor a b)` |
| `xnor` | 2 or more | `(xnor a b)` |
| `mux` | 3 | `(mux select when_true when_false)` |
| `bufif0` | 2 | `(bufif0 value enable)` |
| `bufif1` | 2 | `(bufif1 value enable)` |
| `drive-strength` | 3 | `(drive-strength value first_strength second_strength)` |
| `bufif0-strength` | 4 | `(bufif0-strength value enable first_strength second_strength)` |
| `bufif1-strength` | 4 | `(bufif1-strength value enable first_strength second_strength)` |
| `eq` (`==`) | 2 | `(eq a b)` |
| `caseeq` (`===`) | 2 | `(caseeq a b)` |
| `neq` (`!=`) | 2 | `(neq a b)` |
| `caseneq` (`!==`) | 2 | `(caseneq a b)` |
| `keeper` | 0 | `(keeper)` |
| `nmos` | 2 | `(nmos source gate)` |
| `pmos` | 2 | `(pmos source gate)` |
| `rnmos` | 2 | `(rnmos source gate)` |

Arithmetic, ordering comparisons, and arbitrary function calls are not legal
value operators. They must be evaluated during parameter elaboration or
rejected. `keeper`, `nmos`, `pmos`, and `rnmos` are driver forms, not ordinary
Boolean functions. The three strength forms are driver forms too. Their last two operands preserve the exact first and
second strength tokens from the SystemVerilog source; lowering must not reorder
them by strength kind or driven value. Strength names are atoms from the typed
`Strength` node, not nested expressions.

## Drivers, state, hierarchy, and strength

An ordinary continuous or procedural driver is
`(target value (delay timing-expression ...))`. Every source driver is retained
as a separate assignment in source order. Multiple drivers are never collapsed
into `and` or `or`. A high-impedance branch is represented only as the disabled
side of `bufif0`/`bufif1`; it is not emitted as an ordinary `(mux ... z ...)`
value. Signal-valued and literal-valued drives use the same form.

An `initial` target and a target with supported stateful procedural retention
is listed in `registers`. A selected scalar `initial` assignment is accepted
only when its value is `0`, `1`, `'0`, `'1`, `'x`, or `'z`, with arbitrary
grouping. The value is normalized to the corresponding `0`, `1`, `x`, or `z`
register atom and is metadata, never an ordinary assignment or diagnostic. A
modeled register without a selected source initializer receives `x`. Multiple
selected initializers for one register are an error at the second target
because one register entry cannot represent multiple initial events. Later
procedural next-state assignments remain separate and ordered. Blocking and
non-blocking syntax does not change this representation; source-defined
priority must be preserved.

A keeper instance becomes a distinct zero-delay driver
`(held_net (keeper) (delay 0))`. It is not a register and is not merged with
tri-state drivers. This records retention intent without claiming analog charge
or keeper strength simulation. The accepted source shape is exactly one
positional scalar visible target and no parameter overrides; named, missing,
extra, compound, or unknown connections are source-spanned errors.

Transistors are direct drivers:

```scheme
(drain (nmos source gate) (delay timing-expression ...))
(drain (pmos source gate) (delay timing-expression ...))
(drain (rnmos source gate) (delay timing-expression ...))
```

The forms preserve primitive kind, source, gate polarity semantics, driver
order, topology, and timing. `rnmos` remains distinct because weakening it to
`nmos` would discard fidelity. The DSL does not perform analog network solving
or numeric multi-driver strength resolution; that is a scope boundary rather
than a lossy transistor normalization, so direct corpus transistor forms do not
produce a generic fidelity warning. Any demonstrated information loss must
still block conversion instead of being silently approximated.

Source strength is never silently erased. A strength-qualified continuous
assignment uses `(drive-strength value first_strength second_strength)`. A
strength-qualified tri-state primitive uses `bufif0-strength` or
`bufif1-strength`, preserving the exact pair as the last two operands. Thus the
corpus forms include:

```scheme
(pad (bufif0-strength 1 pdrv_n strong1 highz0) (delay T_rise T_fall T_z))
(pad (bufif1-strength 0 ndrv highz1 strong0) (delay T_rise T_fall T_z))
(pad (bufif0-strength 1 ena_n_pu pull1 highz0) (delay T_rise T_fall T_z))
(vdd (drive-strength 1 supply1 supply0) (delay 0))
```

The complete corpus set is `(strong1, highz0)`, `(highz1, strong0)`,
`(pull1, highz0)`, and `(supply1, supply0)`. Those four exact pairs/forms are
supported; any unknown combination is rejected. The representation
preserves strength metadata but the
DSL does not define multi-driver strength resolution or analog drive behavior.
If a cell requires such resolution for fidelity, conversion is blocked with an
error; a functional-only conversion is not silently substituted. If future
transistor syntax carries a source strength, conversion rejects it at the
strength span because the direct transistor operators cannot carry that
metadata. Supporting such syntax requires adding an equally explicit
contracted operator before emitting it.

The only contracted generate form is a module-level `generate if (nodelay)`
with `begin ... end else begin ... end endgenerate`. Configured analysis and
lowering select exactly one branch before symbol, driver, state, timing, or
requirement analysis; branches are never combined and no content from the
unselected branch may contribute a declaration, port use, driver, register,
timing alias, diagnostic, or requirement. `GenerateMode::Delayful` selects the
false/`else` branch and is the API and CLI default. `GenerateMode::Nodelay`,
exposed by the explicit `--nodelay` option on `analyze`, `lower`, `convert`,
`convert-file`, and `check --stage analyze|lower`, is the sole true-branch
selection.

The explicitly named structural analysis/lowering APIs retain both branches or
the unresolved Generate node only for milestone inventory and compatibility
fixtures. They are not conversion entrypoints and do not claim configured
generate support. A missing `else`, a condition other than the scalar symbol
`nodelay`, nested generate syntax, or another generate shape is a source-spanned
error. Ordinary instances are flattened after configured analysis has retained
their typed parameter bindings and named/positional port connections. Child
ports and parameters are substituted as typed expressions, child-local signals
and timing aliases use the exact `<instance>__<child-name>` form, and nested
instances extend that prefix recursively. Child drivers are spliced at the
parent instance position in child source order; SSA temporaries remain one
parent-wide deterministic `t0`, `t1`, ... sequence. A qualified-name collision,
unknown module, or recursive reference is an error. Keepers use the direct form
above rather than ordinary flattening.

## Timing

Every assignment has exactly one tagged delay tuple with one of these forms:

```scheme
(delay value)
(delay rise fall)
(delay rise fall turn-off)
```

The tag keeps timing metadata distinct from the value expression. A selected
one-, two-, or three-entry SystemVerilog delay tuple preserves the same arity,
entry order, and expression structure. Entries are never added to one another,
copied to fill another transition, or discarded. A missing source delay is the
one-entry tuple `(delay 0)`. The supported corpus has no omitted tuple
component; an explicitly omitted component, including `#(, T_fall)`, is an
error until the DSL contracts an equally explicit representation for the hole.

Downstream compilation or simulation may select the first component as a
compatibility policy, select a named transition component, or use the tuple by
output transition. That selection is outside conversion: the converter always
retains and serializes the complete tuple. A first-component projection helper
exists only as a compatibility and regression oracle.

The timing operator table is exhaustive. Unlike value expressions, timing
operands may recursively be timing expressions.

| Operator | Arity | Example |
| --- | ---: | --- |
| `+` | 2 or more | `(+ T_wire T_gate)` |
| `-` | 2 | `(- total offset)` |
| `*` | 2 or more | `(* resistance 1.5)` |
| `/` | 2 | `(/ total 2)` |
| `wire` | 1 | `(wire L_y)` |
| `pmos` | 1 | `(pmos 5)` |
| `nmos` | 1 | `(nmos 8)` |
| `elmore` | 2 | `(elmore (wire L_y) (pmos 5))` |
| `gt` | 2 | `(gt (* 0.2 T_fall_y1) T_Z_min)` |
| `mux` | 3 | `(mux (gt (* 0.2 T_fall_y1) T_Z_min) (* 0.2 T_fall_y1) T_Z_min)` |

For example,
`(delay (+ (elmore (wire L_y) (pmos 5)) extra_delay) T_fall_y)` is legal.
The nested `mux` example is the exact timing clamp shape used by
`alu_decoder.sv`. Only greater-than is contracted for timing comparison;
less-than remains an error because no curated timing form requires it.
`tpd_elmore(L_y, R_pmos_ohm(5*L_unit))` maps to the nested `elmore` example;
`tpd_z` selects its first present source argument according to the parser's
typed representation. Localparam/specparam aliases may remain atoms or resolve
to these forms. Resistance factors such as `* 2` and `* 1.5` must remain in the
selected expression and may not be dropped. Arbitrary timing calls are errors.

An explicit continuous-assignment or primitive delay takes precedence over
specify timing. When a source-level continuous, primitive, or procedural
assignment has no explicit delay, specify lookup uses only its scalar target
symbol; generated value-SSA temporaries retain `(delay 0)`. A single matching
specify path contributes its complete tuple.

Without decomposition, specify lookup is target-only: if multiple
control-dependent paths target the same symbol, the first path in source order
supplies the assignment's complete delay tuple, and lowering emits one
intentional-ignore diagnostic per used target at the second matching path.
Every path and every tuple component is still parsed, validated, and retained
by analysis; tuple components themselves are not intentional ignores.

Timing-aware lowering retains all control-to-target paths as internal
constraints and relates them to the flat functional IR. Signal and assignment
nodes use durable IDs; dependency edges retain operand position and timing
sense. Reachability, dominance/post-dominance, reconvergence, and
public-output split classifications are deterministic verification oracles for
assignment-delay decomposition.

Decomposition distributes each retained constraint across the assignments on
its path so the emitted delays reconstruct it exactly. It may add delay-only
identity assignments named `d0`, `d1`, ... on individual dependency edges, and
may split an output into an internal value plus a public identity assignment so
a derived output does not inherit the output-local delay. Those names skip any
index already reserved by the cell. A placement's orientation follows the
modeled path's inversion parity, and a placement may carry more components than
a constraint it serves; the extra components belong to controls that can drive
transitions this one cannot. No negative delay, subtraction, or algebraic
simplification is introduced.

The graph cuts modeled-register update edges only as typed state boundaries.
It separately recognizes source-declared resolved nets from `inout`, `wire`,
and `tri` syntax. An assignment-result edge into such a net is a
`ResolvedNetBoundary` only when the emitted IR has multiple drivers for that
target. Resolution boundaries remain in the full reachability/dominance graph
and are excluded separately from state boundaries only for SCC/topological
analysis. This models resolution iteration; it neither creates serialized
state nor collapses source drivers. A single-driver resolved self-loop and a
multiply-driven ordinary `logic` loop remain ordinary combinational-cycle
errors.

Each complete delay tuple also has an exact additive-term view. Only
associative timing `+` at the current expression level is flattened, in source
order; every other timing expression remains an opaque term. Reconstruction is
a checked invariant and must reproduce every tuple component exactly. No
deduplication, reordering, simplification, or symbolic subtraction is
permitted.

The timing graph and its constraints are internal lowering inputs and
verification oracles only. The cell DSL does not serialize durable graph IDs,
timing arcs, or a timing table. `petgraph 0.8` supplies stable directed graphs,
SCC/topological traversal, reachability, and dominance; public ordering remains
source-ordered `Vec` and `BTreeMap`. Ordinary CLI serialization retains the
temporary first-path fallback and its 49 intentional ignores until a later
milestone distributes exact tuples over ordinary assignments. An
unrepresentable decomposition is an error rather than an implicit
approximation.

## Diagnostics and strict mode

- **Error:** unsupported, ambiguous, malformed, or unrepresentable behavior.
  Errors always fail the file and command.
- **Warning:** a supported conversion with an explicitly documented fidelity
  limitation or approximation. Warnings are printed and counted. They fail the
  command only with `--strict`.
- **Intentional ignore:** source information the contract deliberately excludes
  from the model. It is printed/tracked separately and never masquerades as
  support or as a warning. Strict mode does not promote it.

The only intentional ignores currently authorized are: additional
control-dependent specify paths after the first source-ordered path selected for
each used scalar target, which decomposition eliminates; comments and
formatting; and directives/imports proven by analysis to affect neither
elaborated values nor behavior (the corpus license/timescale and package
imports whose referenced parameters are resolved). Delay tuple entries,
register initial values, and strengths are preserved metadata, not intentional
ignores. Unknown directives/import effects are errors.

All diagnostic-capable commands accept `--strict`. Because current stage APIs
return errors but do not yet produce warnings, accepting the option does not
fabricate warnings; the shared policy and report APIs enforce the behavior as
warning-producing stages are implemented.

## Typed AST classification

This table is exhaustive over the node and variant categories in `src/ast.rs`.
Stage names refer to the pipeline: *lex*, *parse*, *analyze*, *lower*, and
*convert*. “Parsed” means the node is retained with a span; it does not claim
later semantic or lowering support. A newly discovered corpus category must be
added here before it can be emitted.

| Typed category and variants | Contract classification |
| --- | --- |
| `Design`, `Module` | One scalar module per output cell; parsed, checked in analysis, serialized in conversion. Extra/absent modules are errors. |
| `ParamDecl`; `ParamKind::{Parameter, Localparam, Specparam}` | Parsed; symbol and constant resolution in analysis; timing use in lowering. Unevaluated value-context arithmetic is an error. |
| `PortDecl`; `Direction::{Input, Output, Inout}` | Parsed; read/write port classification in analysis. Non-scalar port modifiers are errors. |
| `ItemKind::Import(ImportDecl)` | Parsed. A resolved behavior-free corpus package import is intentional-ignore; unresolved names or side effects are errors in analysis. |
| `ItemKind::Decl(Decl)`; `DeclKind::{Logic, Tri, Wire, Parameter, Localparam, Specparam}` | Parsed; symbol and role resolution in analysis; parameter and timing aliases in analysis and lowering. Scalar forms are supported; vector/array modifiers are errors. |
| `ItemKind::Initial(AssignStmt)` | A selected contracted scalar literal becomes the register's four-state initial metadata in lowering and emits no assignment. Duplicate selected initializers and other initial bodies are errors. |
| `ItemKind::ProcAssign(AssignStmt)` | Blocking/nonblocking assignment in a supported procedural block is lowered. At module scope or in an unsupported context it is an error. |
| `ItemKind::AlwaysLatch(AlwaysLatch)` | Stateful condition/body in analysis and source-ordered next-state lowering. |
| `ItemKind::Always(AlwaysBlock)`; `AlwaysKind::{Plain, Comb, Ff}` | Sensitivity and driver classification in analysis; supported scalar procedural lowering. Ambiguous state or unsupported combinational procedures are errors. |
| `Sensitivity::{Any, List}`; `EventControl` with optional edge/expression | Parsed; stateful event classification in analysis; supported procedural lowering. Unknown edge names or omitted required event expressions are errors. |
| `ItemKind::Assign(AssignDecl)` | Continuous scalar driver in analysis; flat SSA; repeated/tri-state drivers; symbolic timing; complete delay tuples in lowering. |
| `ItemKind::Primitive(PrimitiveCall)` | `bufif0`/`bufif1` lower in; `nmos`/`pmos`/`rnmos` in lowering. Unknown names, omitted required arguments, and wrong arity are errors. |
| `ItemKind::Instantiation(Instantiation)` | Named/positional hierarchy and overrides flatten in; recognized `keeper` instances use in lowering. Unknown/recursive modules are errors. |
| `ParamOverride::{Named, Positional}`; `Connection::{Named, Positional}` | Parsed, resolved and substituted in lowering. Omitted positional parameter entries remain explicit; invalid/unknown ports or parameters are errors. |
| `Strength` | Exact pair metadata lowers with contracted strength driver forms in lowering. Known pairs are `strong1/highz0`, `highz1/strong0`, `pull1/highz0`, and `supply1/supply0`; unknown combinations or resolution-dependent behavior are errors. |
| `Delay` (`Vec<Option<Expr>>`) | lowering preserves every present entry as `DelayTuple::{One, Two, Three}`; absent delay becomes `(delay 0)`; any omitted component is an error under the current contract. |
| `ItemKind::Specify(SpecifyBlock)`; `SpecifyItem::{Specparam, Path}`; `SpecPath` | Parsed, preserved/analyzed in analysis, alias/path timing lowered in lowering. Unsupported conditional/path structure is an error. |
| `ItemKind::Generate(Block)` | Alternatives remain separate in analysis; exactly one configured `nodelay` branch is selected in lowering. Unresolved generate conditions are errors. |
| `ItemKind::Block(Block)`, `ItemKind::If(IfStmt)` | Typed nesting in parsing; procedural priority in lowering or generate selection in lowering according to context. Unsupported `else`/context is an error, never dropped. |
| `ItemKind::Empty` | Supported structural no-op from an explicit semicolon; no driver is emitted. |
| `ExprKind::Path` | Scalar symbol/package path; resolution in analysis, atom emission in supported value/timing contexts in lowering-lowering. Unknown paths are errors. |
| `ExprKind::{Integer, Real}`, `ConstKind::{Zero, One, X, Z}` | Parsed. Four-state scalar atoms are preserved in legal contexts; real values are timing/parameter-only; ordinary `z` is legal only where high impedance is contracted. |
| `ExprKind::Group` | Preserves source grouping in parsing; lowers according to its contained expression without adding a DSL operator. |
| `ExprKind::Unary`; `UnaryOp::{Not, BitNot, Plus, Minus}` | Boolean not/bit-not lower flat in lowering. Unary plus/minus are in analysis constant or lowering timing operations; runtime value use is an error. |
| `ExprKind::Binary`; `BinaryOp::{BitAnd, BitOr, BitXor, BitNand, BitNor, BitXnor, LogicalAnd, LogicalOr}` | Flat four-state value operators; no unsafe Boolean rewriting. |
| `BinaryOp::{Eq, CaseEq, Neq, CaseNeq}` | Four distinct contracted equality operators lower in lowering. |
| `BinaryOp::{Mul, Div, Add, Sub}` | Constant elaboration in analysis or timing arithmetic in lowering as applicable. Runtime arithmetic is an error. |
| `BinaryOp::Greater` | Constant elaboration in analysis or the contracted timing `gt` comparison used by the lowering timing clamp. Runtime ordering is an error. |
| `BinaryOp::Less` | Constant elaboration in analysis where applicable. Timing and runtime uses are uncontracted errors. |
| `ExprKind::Ternary` | Flat value `mux`; high-Z normalization; nested timing `mux` for the corpus timing clamp in lowering. |
| `ExprKind::Call` | Only contracted timing calls (`tpd_elmore`, `tpd_z`, `R_pmos_ohm`, `R_nmos_ohm`) lower in lowering. Arbitrary value/timing calls are errors. |
| `AssignOp::{Blocking, NonBlocking}` | Retained in parsing and used to preserve procedural scheduling/source priority; never silently conflated where behavior differs. |

## Known corpus-specific forms

| Corpus form/family | Contract classification |
| --- | --- |
| Simple gates, compound scalar logic, equality, and value ternaries | Deterministic flat SSA in lowering using `t0`, `t1`, ... in dependency order. |
| Latches, generated DFF/TFF variants, nested priority `if`, reset/enable logic | State analysis, next-state lowering, and exact four-state register initialization metadata; absent initialization is `x`. |
| High-Z ternaries, direct `bufif*`, precharge/open-drain, and repeated drivers | Source-ordered driver in lowering; high-Z ternaries normalize only to polarity-equivalent `bufif0`/`bufif1`. |
| `(strong1, highz0)`, `(highz1, strong0)`, `(pull1, highz0)`, `(supply1, supply0)` | Exact strength-bearing driver forms in lowering, with no claim of strength-resolution simulation. |
| One/two/three-entry tuples; timing aliases and paths; `tpd_elmore`, `tpd_z`; resistance factors including real factors | Symbolic timing expressions; exact tuple arity and every component preserved; internal functional timing graph; assignment-delay decomposition in later milestones. Tuple entries are never summed together. |
| `(0.2 * T_fall_yN) > T_Z_min ? (0.2 * T_fall_yN) : T_Z_min` in `alu_decoder.sv` | Nested timing `(mux (gt ...) ... ...)` clamp; this does not contract general runtime ternaries or less-than comparisons. |
| `if (nodelay)` generate alternatives | Exactly one in lowering branch: delayful/false by default, nodelay/true only by explicit configuration; the unselected branch is not a driver. |
| Named/positional half-adder and full-adder instances with parameter overrides | Deterministic instance-qualified flattening, substitution, and dependency/source order in lowering. |
| `keeper` instances in mux, pad, IDU, and register-bus cells | Direct source-ordered `(held_net (keeper) (delay 0))` driver in lowering, never a register. |
| `nmos`, `pmos`, `rnmos` in IRQ priority, IDU, and bus-injection cells | Direct typed transistor drivers; polarity and `rnmos` distinction preserved, compound inputs first flattened to SSA atoms. |
| License/timescale directives, comments, and formatting | Intentional-ignore only after lexing/analysis proves no elaboration/behavior effect. Unknown directives are errors. |
| Curated package imports | Intentional-ignore only after referenced parameters resolve; unresolved import effects are errors. |
| Corpus mirroring, filtering, overwrite/dry-run, formatter validation | Release CLI in conversion. |
| Contracted scalar register initial values | Uniform `(name initial-value)` register metadata; duplicate selected initializers are errors. |
| Vectors, arrays, interfaces, classes, assertions, generate loops, other third-party syntax | Global non-goal and blocked/unsupported; reject with a precise diagnostic. |
