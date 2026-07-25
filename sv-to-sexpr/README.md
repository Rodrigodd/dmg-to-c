# sv-to-sexpr

`sv-to-sexpr` converts the repository's curated scalar SystemVerilog cell
subset into the SSA-like S-expression cell format described in
[`CONTRACT.md`](CONTRACT.md). It is intentionally specialized to `sv-cells`;
it is not a general SystemVerilog frontend or an event-level simulator. A
malformed, unsupported, or unrepresentable construct produces a
source-spanned error instead of an approximate or partial cell.

## Commands

Run commands from the repository root:

```sh
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- lower sv-cells/sm83/cells/and3.sv
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert sv-cells sexpr-cells --dry-run --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert-file input.sv output.cell --dry-run
```

The corpus conversion syntax is:

```text
convert <input-dir> <output-dir> [--dry-run] [--strict] [--overwrite]
        [--filter <relative-path-substring>] [--nodelay]
```

Options may appear in any order. By default, conversion selects the delayful
generate branch, selects every discovered source, writes missing outputs,
skips existing regular output files, and does not promote warnings to errors.
Only recursively discovered regular files with the exact `.sv` extension are
processed; input symlinks and other file types are ignored. Output paths mirror
the input-relative directory structure and replace only the final `.sv`
extension with `.cell`.

`--filter` performs a case-sensitive substring match against each normalized
input-relative path, whose separator is `/`. Filtering controls which cells
are lowered and emitted, but it does not make an isolated partial catalog: all
discovered sources are still read and parsed, and the complete module catalog
is built before selected files are lowered. A parse or catalog error in an
excluded source therefore still fails the command. A filter matching no files
is a successful run after that complete parse/catalog check.

- `--dry-run` performs the complete preflight without mutating the output tree.
  It counts only prepared, non-skipped outputs as `would-write`.
- `--overwrite` permits replacement of existing regular output files. Without
  it, those files are skipped only after their source has been lowered,
  structurally validated, and canonically serialized. A directory, symlink, or
  other non-regular output target is an error.
- `--strict` promotes warnings to failures. It does not promote the approved
  intentional ignores defined by the contract.
- `--nodelay` selects the true branch of the contracted
  `generate if (nodelay)` form. Delayful/else selection is the default.

Conversion preflights source reads, parsing, the complete module catalog,
lowering, IR validation, canonical serialization, output mapping, and existing
path conflicts before writing any file. A failure found during that preflight
causes no output writes. This guarantee covers unsupported input and every
other error discoverable before filesystem mutation; an unexpected I/O failure
during the write phase is reported at the affected source/output path.

Single-file conversion accepts the corresponding options:

```text
convert-file <input.sv> <output.cell> [--dry-run] [--strict] [--overwrite]
             [--nodelay]
```

Its dry run prints the canonical cell to stdout. A real conversion refuses to
replace an existing file unless `--overwrite` is present.

## Delay tuples

Every serialized assignment carries one tagged delay tuple. The only emitted
forms are:

```text
(delay value)
(delay rise fall)
(delay rise fall turn-off)
```

The converter preserves the exact one-, two-, or three-entry arity and source
order of the selected SystemVerilog delay. It does not fill, copy, sum, or
discard tuple components. Assignments without source timing, including
generated SSA temporaries, use the canonical one-entry `(delay 0)` form.

A downstream compiler can project the first component to reproduce the
converter's pre-tuple single-delay behavior. A simulator may instead select
the rise, fall, or turn-off component according to its transition policy. The
cell format remains assignment-only: explicit delays retain precedence and the
first source-ordered specify path is temporarily attached to the assignment,
but no timing arcs or timing-constraint tables are serialized.

## Internal timing graph

The library exposes timing-aware lowering separately from the ordinary CLI
conversion path. It retains every scalar specify control, target, complete
delay tuple, and source span, then relates those constraints to the flat
assignment IR through a deterministic functional timing graph. This graph and
its constraints are analysis inputs only: neither durable graph IDs nor timing
arcs/tables are part of the `.cell` syntax.

Signal and assignment nodes have durable source/build-ordered IDs. Dependency
edges retain operand position and positive, negative, non-unate, conditional,
or state-control sense. Modeled register updates are cut only at typed state
boundaries for combinational SCC/topological analysis. Separately, assignment
results entering a source-declared resolved net (`inout`, `wire`, or `tri`) are
resolution boundaries only when the net has multiple emitted drivers. These
edges remain present in the full graph for reachability and dominance; the cut
models resolution iteration and does not create register state or collapse
drivers. Single-driver resolved loops and multiply-driven ordinary `logic`
loops remain combinational-cycle errors.

Every specify control must reach its target in the full graph. Reports expose
per-control path senses, cross-target shared-prefix candidates,
cross-control shared-suffix candidates, reconvergence, and public-output split
candidates using durable IDs only. Complete delay tuples also have an exact
Add-only term view: top-level associative timing `+` is flattened in source
order, every other expression is opaque, and reconstruction must reproduce the
original tuple exactly. No symbolic simplification or subtraction occurs.

`petgraph 0.8` is the only direct production dependency added for stable
directed graphs, SCCs, topological traversal, reachability, and dominance.
Externally visible ordering remains source-ordered `Vec` plus `BTreeMap`; raw
petgraph indices and hash traversal order never enter reports. Ordinary
serialization still uses the Milestone 14 first-path assignment placement and
49 intentional ignores until assignment-delay decomposition is enabled in a
later milestone.

## Diagnostics and summaries

Errors always fail. Warnings describe a supported conversion with a documented
fidelity limitation and fail only in strict mode. Intentional ignores are
explicitly excluded by the cell contract and never fail, even in strict mode.
Later delay-tuple entries are preserved and do not produce diagnostics. The
remaining temporary intentional ignores are exactly the 49 additional
control-dependent specify paths after the first selected source-ordered path in
either generate mode. The internal timing graph retains and analyzes all of
those paths, while ordinary serialization continues to defer redistribution.
Register initial values are preserved metadata and do not produce diagnostics.

Each modeled register is serialized as `(name initial-value)`, where the value
is one of `0`, `1`, `x`, or `z`. A selected scalar contracted literal `initial`
assignment supplies that metadata and does not become an ordinary assignment.
Registers without a selected initializer use `x`; duplicate selected
initializers for one register are errors.

`convert` prints diagnostics in deterministic source/location order and one
summary whose counters mean:

- `processed`: all discovered regular `.sv` files;
- `selected`: files whose normalized relative paths match the filter, or all
  files when no filter is supplied;
- `skipped`: filter-excluded files plus selected existing outputs skipped
  because `--overwrite` was absent;
- `warned`: selected files containing at least one warning;
- `intentional-ignored`: intentional-ignore diagnostics from selected files;
- `written`: files actually written (`0` in a dry run);
- `would-write`: prepared files a dry run would write (`0` in a real run);
- `failed`: failed source files plus deterministic global conversion failures.

## Canonical output and release checks

The serializer calls the `sexpr-fmt` library directly, so emitted cells are in
the formatter's default canonical form. `Cell::validate` rejects nested value
expressions before serialization while permitting contracted nested timing
expressions. The release tests parse and format every generated cell twice and
require byte-identical output.

Standard validation commands are:

```sh
cargo fmt --manifest-path sv-to-sexpr/Cargo.toml -- --check
cargo clippy --manifest-path sv-to-sexpr/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path sv-to-sexpr/Cargo.toml
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower --strict
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower --strict --nodelay
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert sv-cells sexpr-cells --strict --overwrite
cargo run --manifest-path sexpr-fmt/Cargo.toml -- --check path/to/generated.cell
```

The authoritative release corpus contains exactly one mirrored `.cell` for
each of the 206 curated `.sv` sources. Repeating strict overwrite conversion
must produce byte-identical files.
