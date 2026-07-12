# Cell DSL and Diagnostic Contract

This document freezes the representation decisions required by Milestone 0 of
[PLAN.md](PLAN.md). `ir::ValueOperator` and `ir::TimingOperator` are the
machine-readable authorities for operator spellings and arities. A construct
not described here is unsupported and must produce a source-spanned error.

## Cell and expression shape

Each input file produces exactly one form:

```scheme
(cell
  module_name
  (inputs input_port inout_port_if_read ...)
  (outputs output_port inout_port_if_driven ...)
  (registers modeled_state_only ...)
  (assignments
    (target value-expression delay-expression)
    ...
  )
)
```

Names and literals are atoms. A value expression is either one atom or one
legal value operator whose operands are all non-empty atoms. Operator-valued
operands are forbidden. Compound source expressions are split in dependency
order into assignments to deterministic `t0`, `t1`, ... temporaries. Thus
`(t0 (not a) 0)` followed by `(y (and t0 b) 0)` is legal, while
`(y (and (not a) b) 0)` is not. Timing expressions have their own operator set
and may nest recursively.

`inputs` contains input ports plus inouts that are read. `outputs` contains
output ports plus inouts that are driven. `registers` contains only modeled
state, never a continuous, primitive, hierarchical, or keeper-driven net.

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
rejected. `keeper`, `nmos`, `pmos`, and `rnmos` are driver forms reserved for
Milestones 10 and 11, not ordinary Boolean functions. The three strength forms
are driver forms too. Their last two operands preserve the exact first and
second strength tokens from the SystemVerilog source; lowering must not reorder
them by strength kind or driven value. Strength names are atoms from the typed
`Strength` node, not nested expressions.

## Drivers, state, hierarchy, and strength

An ordinary continuous or procedural driver is `(target value delay)`. Every
source driver is retained as a separate assignment in source order. Multiple
drivers are never collapsed into `and` or `or`. A high-impedance branch is
represented only as the disabled side of `bufif0`/`bufif1`; it is not emitted
as an ordinary `(mux ... z ...)` value. Signal-valued and literal-valued drives
use the same form.

An `initial` target and a target with supported stateful procedural retention
is listed in `registers`. Corpus initialization is accepted only when it assigns
a contracted literal to a scalar target. The initial value/event is not
serialized: the target describes logic/state topology, not the simulator's
initial event queue. This omission is an explicit intentional-ignore diagnostic
at the `initial` item. Later procedural next-state assignments remain separate
and ordered. Blocking and non-blocking syntax does not change this
representation; source-defined priority must be preserved.

A keeper instance becomes a distinct zero-delay driver
`(held_net (keeper) 0)`. It is not a register and is not merged with tri-state
drivers. This records retention intent without claiming analog charge or keeper
strength simulation. The accepted source shape is exactly one positional scalar
visible target and no parameter overrides; named, missing, extra, compound, or
unknown connections are source-spanned errors.

Transistors are direct drivers:

```scheme
(drain (nmos source gate) delay)
(drain (pmos source gate) delay)
(drain (rnmos source gate) delay)
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
(pad (bufif0-strength 1 pdrv_n strong1 highz0) delay)
(pad (bufif1-strength 0 ndrv highz1 strong0) delay)
(pad (bufif0-strength 1 ena_n_pu pull1 highz0) delay)
(vdd (drive-strength 1 supply1 supply0) 0)
```

The complete corpus set is `(strong1, highz0)`, `(highz1, strong0)`,
`(pull1, highz0)`, and `(supply1, supply0)`. Milestone 6 must support those four
exact corpus pairs/forms and reject any unknown combination. The representation
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
exposed by the explicit `--nodelay` option on `analyze`, `lower`, `convert-file`,
and `check --stage analyze|lower`, is the sole true-branch selection.

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

A missing source delay is the atom `0`. For every one-, two-, or three-entry
SystemVerilog delay tuple, exactly the first entry is selected. Later entries
are parsed and recorded as intentional ignores because the DSL has no separate
rise/fall/turn-off timing. They are never summed. An explicitly omitted first
entry, such as `#(, T_fall)`, is an error; it has no unambiguous single-delay
meaning.

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

For example, `(+ (elmore (wire L_y) (pmos 5)) extra_delay)` is legal.
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
symbol; generated SSA temporaries always retain delay `0`. A single matching
specify path contributes its selected first tuple entry. If multiple
control-dependent paths target the same symbol, the one-delay DSL selects the
first path in source order and emits one warning for that target at the second
matching path. This target-only selection is a documented approximation:
ordinary lowering succeeds with the warning, while strict mode promotes it to
a failure. Every specify tuple is still validated and every entry after the
first is recorded as an intentional ignore, even when that path is not selected
by an assignment.

## Diagnostics and strict mode

- **Error:** unsupported, ambiguous, malformed, or unrepresentable behavior.
  Errors always fail the file and command.
- **Warning:** a supported conversion with an explicitly documented fidelity
  limitation or approximation. Warnings are printed and counted. They fail the
  command only with `--strict`.
- **Intentional ignore:** source information the contract deliberately excludes
  from the model. It is printed/tracked separately and never masquerades as
  support or as a warning. Strict mode does not promote it.

The only intentional ignores currently authorized are: supported literal
initial values/events after they classify the target as state; delay tuple
entries after the first; comments and formatting; and directives/imports proven
by analysis to affect neither elaborated values nor behavior (the corpus
license/timescale and package imports whose referenced parameters are
resolved). Strengths are preserved metadata, not intentional ignores. Unknown
directives/import effects are errors.

All diagnostic-capable commands accept `--strict`. Because current stage APIs
return errors but do not yet produce warnings, accepting the option does not
fabricate warnings; the shared policy and report APIs enforce the behavior as
warning-producing stages are implemented.

## Typed AST classification

This table is exhaustive over the node and variant categories in `src/ast.rs`.
“Parsed M2” means that the node is retained with a span; it does not claim
later semantic or lowering support. Milestone 1 inventory must amend this
contract before any newly discovered corpus category can be emitted.

| Typed category and variants | Contract classification |
| --- | --- |
| `Design`, `Module` | One scalar module per output cell; parsed M2, checked M3, serialized M12. Extra/absent modules are errors. |
| `ParamDecl`; `ParamKind::{Parameter, Localparam, Specparam}` | Parsed M2; symbol/constant analysis M3; timing use M7. Unevaluated value-context arithmetic is an error. |
| `PortDecl`; `Direction::{Input, Output, Inout}` | Parsed M2; read/write port classification M3. Non-scalar port modifiers are errors. |
| `ItemKind::Import(ImportDecl)` | Parsed M2. A resolved behavior-free corpus package import is intentional-ignore; unresolved names or side effects are errors in M3. |
| `ItemKind::Decl(Decl)`; `DeclKind::{Logic, Tri, Wire, Parameter, Localparam, Specparam}` | Parsed M2; symbol/role analysis M3; parameter and timing aliases M3/M7. Scalar forms are supported; vector/array modifiers are errors. |
| `ItemKind::Initial(AssignStmt)` | Literal scalar initialization classifies modeled state in M3/M5; its value/event is then an explicit intentional-ignore because no initial event is serialized. Other initial bodies are errors. |
| `ItemKind::ProcAssign(AssignStmt)` | Blocking/nonblocking assignment in a supported procedural block is M5. At module scope or in an unsupported context it is an error. |
| `ItemKind::AlwaysLatch(AlwaysLatch)` | Stateful condition/body analysis M3 and source-ordered next-state lowering M5. |
| `ItemKind::Always(AlwaysBlock)`; `AlwaysKind::{Plain, Comb, Ff}` | Sensitivity/driver analysis M3; supported scalar procedural lowering M5. Ambiguous state or unsupported combinational procedures are errors. |
| `Sensitivity::{Any, List}`; `EventControl` with optional edge/expression | Parsed M2; stateful event classification M3; supported procedural lowering M5. Unknown edge names or omitted required event expressions are errors. |
| `ItemKind::Assign(AssignDecl)` | Continuous scalar driver analysis M3; flat SSA M4; repeated/tri-state drivers M6; selected-first delay M7. |
| `ItemKind::Primitive(PrimitiveCall)` | `bufif0`/`bufif1` lower in M6; `nmos`/`pmos`/`rnmos` in M11. Unknown names, omitted required arguments, and wrong arity are errors. |
| `ItemKind::Instantiation(Instantiation)` | Named/positional hierarchy and overrides flatten in M9; recognized `keeper` instances use M10. Unknown/recursive modules are errors. |
| `ParamOverride::{Named, Positional}`; `Connection::{Named, Positional}` | Parsed M2, resolved and substituted M9. Omitted positional parameter entries remain explicit; invalid/unknown ports or parameters are errors. |
| `Strength` | Exact pair metadata lowers with contracted strength driver forms in M6. Known pairs are `strong1/highz0`, `highz1/strong0`, `pull1/highz0`, and `supply1/supply0`; unknown combinations or resolution-dependent behavior are errors. |
| `Delay` (`Vec<Option<Expr>>`) | M7 selects exactly entry zero. Entries after zero are intentional-ignore; absent delay becomes `0`; omitted entry zero is an error. |
| `ItemKind::Specify(SpecifyBlock)`; `SpecifyItem::{Specparam, Path}`; `SpecPath` | Parsed M2, preserved/analyzed M3, alias/path timing lowered M7. Unsupported conditional/path structure is an error. |
| `ItemKind::Generate(Block)` | Alternatives remain separate in M3; exactly one configured `nodelay` branch is selected in M8. Unresolved generate conditions are errors. |
| `ItemKind::Block(Block)`, `ItemKind::If(IfStmt)` | Typed nesting M2; procedural priority M5 or generate selection M8 according to context. Unsupported `else`/context is an error, never dropped. |
| `ItemKind::Empty` | Supported structural no-op from an explicit semicolon; no driver is emitted. |
| `ExprKind::Path` | Scalar symbol/package path; resolution M3, atom emission in supported value/timing contexts M4-M7. Unknown paths are errors. |
| `ExprKind::{Integer, Real}`, `ConstKind::{Zero, One, X, Z}` | Parsed M2. Four-state scalar atoms are preserved in legal contexts; real values are timing/parameter-only; ordinary `z` is legal only where high impedance is contracted. |
| `ExprKind::Group` | Preserves source grouping in M2; lowers according to its contained expression without adding a DSL operator. |
| `ExprKind::Unary`; `UnaryOp::{Not, BitNot, Plus, Minus}` | Boolean not/bit-not lower flat in M4. Unary plus/minus are M3 constant or M7 timing operations; runtime value use is an error. |
| `ExprKind::Binary`; `BinaryOp::{BitAnd, BitOr, BitXor, BitNand, BitNor, BitXnor, LogicalAnd, LogicalOr}` | Flat four-state value operators M4; no unsafe Boolean rewriting. |
| `BinaryOp::{Eq, CaseEq, Neq, CaseNeq}` | Four distinct contracted equality operators lower in M4. |
| `BinaryOp::{Mul, Div, Add, Sub}` | Constant elaboration M3 or timing arithmetic M7 as applicable. Runtime arithmetic is an error. |
| `BinaryOp::Greater` | Constant elaboration M3 or the contracted timing `gt` comparison used by the M7 timing clamp. Runtime ordering is an error. |
| `BinaryOp::Less` | Constant elaboration M3 where applicable. Timing and runtime uses are uncontracted errors. |
| `ExprKind::Ternary` | Flat value `mux` M4; high-Z normalization M6; nested timing `mux` for the corpus timing clamp in M7. |
| `ExprKind::Call` | Only contracted timing calls (`tpd_elmore`, `tpd_z`, `R_pmos_ohm`, `R_nmos_ohm`) lower in M7. Arbitrary value/timing calls are errors. |
| `AssignOp::{Blocking, NonBlocking}` | Retained M2 and used to preserve procedural scheduling/source priority M5; never silently conflated where behavior differs. |

## Known corpus-specific forms

| Corpus form/family | Contract classification |
| --- | --- |
| Simple gates, compound scalar logic, equality, and value ternaries | Deterministic flat SSA M4 using `t0`, `t1`, ... in dependency order. |
| Latches, generated DFF/TFF variants, nested priority `if`, reset/enable logic | State analysis M3 and next-state lowering M5; initialization omission is the explicit intentional-ignore above. |
| High-Z ternaries, direct `bufif*`, precharge/open-drain, and repeated drivers | Source-ordered driver lowering M6; high-Z ternaries normalize only to polarity-equivalent `bufif0`/`bufif1`. |
| `(strong1, highz0)`, `(highz1, strong0)`, `(pull1, highz0)`, `(supply1, supply0)` | Exact strength-bearing driver forms M6, with no claim of strength-resolution simulation. |
| One/two/three-entry tuples; timing aliases and paths; `tpd_elmore`, `tpd_z`; resistance factors including real factors | Selected-first symbolic timing M7. Later tuple entries are intentional-ignore and never summed. |
| `(0.2 * T_fall_yN) > T_Z_min ? (0.2 * T_fall_yN) : T_Z_min` in `alu_decoder.sv` | Nested timing `(mux (gt ...) ... ...)` clamp M7; this does not contract general runtime ternaries or less-than comparisons. |
| `if (nodelay)` generate alternatives | Exactly one M8 branch: delayful/false by default, nodelay/true only by explicit configuration; the unselected branch is not a driver. |
| Named/positional half-adder and full-adder instances with parameter overrides | Deterministic instance-qualified flattening, substitution, and dependency/source order M9. |
| `keeper` instances in mux, pad, IDU, and register-bus cells | Direct source-ordered `(held_net (keeper) 0)` driver M10, never a register. |
| `nmos`, `pmos`, `rnmos` in IRQ priority, IDU, and bus-injection cells | Direct typed transistor drivers M11; polarity and `rnmos` distinction preserved, compound inputs first flattened to SSA atoms. |
| License/timescale directives, comments, and formatting | Intentional-ignore only after M1/M3 proves no elaboration/behavior effect. Unknown directives are errors. |
| Curated package imports | Intentional-ignore only after referenced parameters resolve; unresolved import effects are errors. |
| Corpus mirroring, filtering, overwrite/dry-run, formatter validation | Release CLI M12. |
| Vectors, arrays, interfaces, classes, assertions, generate loops, other third-party syntax | Global non-goal and blocked/unsupported; reject with a precise diagnostic. |
