# sv-to-sexpr

`sv-to-sexpr` converts the repository's curated scalar SystemVerilog cell
subset into the SSA-like S-expression cell format described in
[`CONTRACT.md`](CONTRACT.md). It is intentionally specialized to `sv-cells`;
it is not a general SystemVerilog frontend or an event-level simulator. A
malformed, unsupported, or unrepresentable construct produces a source-spanned
error instead of an approximate or partial cell.

Each curated `.sv` source has one mirrored `.cell` output under `sexpr-cells`.
`sexpr-cells/manual/` holds hand-written cells for sources the converter cannot
represent exactly; those are not produced, checked, or formatted by this tool.

## Output shape

```scheme
(cell
  module_name
  (inputs ...)
  (outputs ...)
  (registers (name initial-value) ...)
  (parameters (name value) ...)
  (assignments
    (target value-expression (delay ...))
    ...
  )
)
```

Value expressions are flat: one atom, or one operator over atoms only.
Compound source expressions are split into deterministic `t0`, `t1`, ...
temporaries in dependency order. Registers hold modeled state only, each with a
four-state initial value. `CONTRACT.md` defines the legal operators, delay
forms, strength pairs, and diagnostic classes.

## Commands

Run from the repository root:

```sh
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- lower sv-cells/sm83/cells/and3.sv
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert sv-cells sexpr-cells --dry-run --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert-file input.sv output.cell --dry-run
```

```text
convert      <input-dir> <output-dir> [--dry-run] [--strict] [--overwrite]
             [--filter <relative-path-substring>] [--nodelay] [--decompose-timing]
convert-file <input.sv> <output.cell> [--dry-run] [--strict] [--overwrite]
             [--nodelay] [--decompose-timing]
check        <input-dir> --stage lex|parse|analyze|lower [--strict] [--nodelay]
```

Options may appear in any order. Only recursively discovered regular files with
the exact `.sv` extension are processed. Output paths mirror the input-relative
structure, replacing the final `.sv` with `.cell`.

Behaviour worth knowing, because it is not obvious from the flag names:

- **Conversion never writes partial output.** Source reads, parsing, the
  complete module catalog, lowering, IR validation, canonical serialization,
  and output path conflicts are all preflighted before any file is written.
- **`--filter` selects what is emitted, not what is read.** All discovered
  sources are still parsed and the complete catalog is still built, so an error
  in an excluded source still fails the command.
- **`--nodelay` selects the true branch** of the contracted
  `generate if (nodelay)` form; the delayful branch is the default.
- **`--strict`** promotes warnings to failures. It does not promote the
  intentional ignores the contract approves.
- **`--overwrite`** permits replacing existing outputs. Without it, existing
  outputs are skipped only after their source has been fully lowered and
  serialized.

`convert-file --dry-run` prints the canonical cell to stdout.

## Timing

Every assignment carries one tagged delay tuple:

```text
(delay value)
(delay rise fall)
(delay rise fall turn-off)
```

The exact arity and source order of the selected SystemVerilog delay are
preserved; components are never filled, copied, summed, or discarded.
Assignments without source timing use `(delay 0)`.

Lowering retains every scalar specify control, target, tuple, and source span,
and relates them to the flat IR through a deterministic functional timing
graph. Dependency edges carry operand position and positive, negative,
non-unate, conditional, or state-control sense. Register updates are cut at
typed state boundaries for cycle and topological analysis; assignment results
entering a resolved net (`inout`, `wire`, `tri`) are cut only when that net has
multiple drivers. Single-driver resolved loops and multiply-driven `logic`
loops remain combinational-cycle errors. The graph is an analysis input: no
timing arcs or constraint tables are serialized.

`--decompose-timing` distributes each specify path across the assignments along
it, so the emitted delays reconstruct the source constraints exactly. It places
delay-only identity assignments named `d0`, `d1`, ... on individual edges, and
may split an output into an internal value plus a public identity assignment so
a derived output does not inherit the output-local delay. Placement orientation
follows the modeled path's inversion parity, and a placement may be wider than
a constraint it serves. Without the flag, an assignment without an explicit
delay takes the first source-ordered specify path for its target, and each
additional path for that target reports one intentional ignore.

`petgraph` is the only direct production dependency, used for stable directed
graphs, SCCs, topological traversal, reachability, and dominance. Externally
visible ordering is source-ordered `Vec` plus `BTreeMap`; raw petgraph indices
and hash traversal order never enter reports.

## Diagnostics

Errors always fail. Warnings describe a supported conversion with a documented
fidelity limitation and fail only in strict mode. Intentional ignores are
excluded by the contract and never fail. Register initial values and later
delay-tuple entries are preserved metadata and produce no diagnostics.

`convert` prints diagnostics in deterministic source order and one summary:

- `processed`: all discovered regular `.sv` files
- `selected`: files matching the filter, or all files when none is supplied
- `skipped`: filter-excluded files plus existing outputs skipped without
  `--overwrite`
- `warned`: selected files containing at least one warning
- `intentional-ignored`: intentional-ignore diagnostics from selected files
- `written` / `would-write`: files written, or that a dry run would write
- `failed`: failed sources plus global conversion failures

## Validation

The serializer calls the `sexpr-fmt` library directly, so emitted cells are
already in the formatter's canonical form, and `Cell::validate` rejects nested
value expressions before serialization.

```sh
cargo nextest run --manifest-path sv-to-sexpr/Cargo.toml
cargo fmt --manifest-path sv-to-sexpr/Cargo.toml -- --check
cargo clippy --manifest-path sv-to-sexpr/Cargo.toml --all-targets
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert sv-cells sexpr-cells --strict --overwrite --decompose-timing
```

Repeating a strict overwrite conversion must produce byte-identical files.
Corpus-wide counts are recorded in the regenerable golden fixtures under
`tests/fixtures/`, not in this document; regenerate them with `UPDATE_FIXTURES=1`.
