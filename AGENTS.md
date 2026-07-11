# Repository Guide

## Project overview

This repository contains a curated library of scalar SystemVerilog cells and
tools for converting and formatting the repository's SSA-like S-expression cell
DSL.

- `sv-cells/`: the 206-file curated SystemVerilog input corpus.
- `sexpr-cells/`: checked-in `.cell` outputs and reference fixtures.
- `sv-to-sexpr/`: the Rust converter. `PLAN.md` is its authoritative roadmap
  and acceptance contract; `STATUS.md` records the verified implementation
  baseline.
- `sexpr-fmt/`: the Rust parser/formatter used to validate generated cell files.
- `dmg-sim/`: simulation-related source and support files; it is separate from
  the converter implementation.

The converter is intentionally specialized to constructs present in
`sv-cells/`. It lexes and parses SystemVerilog into a typed AST, analyzes symbols
and driver/state roles, lowers supported behavior into a flat SSA-like IR, and
serializes one `(cell ...)` form per source file. Do not silently approximate
unsupported SystemVerilog; preserve source locations and return a diagnostic.

## Converter structure

- `sv-to-sexpr/src/lexer.rs`: tokens, source locations, and lexical errors.
- `sv-to-sexpr/src/ast.rs`: typed, spanned SystemVerilog subset AST.
- `sv-to-sexpr/src/parser.rs`: corpus-specialized recursive-descent parser.
- `sv-to-sexpr/src/analyze.rs`: symbols, port direction usage, state/driver
  classification, generate alternatives, hierarchy, and timing metadata.
- `sv-to-sexpr/src/ir.rs`: target cell IR and expressions.
- `sv-to-sexpr/src/lower.rs`: analysis/AST to cell IR lowering.
- `sv-to-sexpr/src/serialize.rs`: deterministic `.cell` rendering.
- `sv-to-sexpr/src/survey.rs`: deterministic corpus inventory and staged checks.
- `sv-to-sexpr/src/cli.rs`: command-line parsing and command dispatch.
- `sv-to-sexpr/tests/`: corpus, fixture, timing, and CLI integration tests; keep
  golden inputs and outputs below `tests/fixtures/` as described in `PLAN.md`.

## Coding conventions

- Use Rust 2024 edition and standard `rustfmt` formatting.
- Keep AST, analysis, and IR types explicit and typed. Do not store recognized
  syntax as raw text or silently discard parsed items.
- Attach a `Span` to diagnostics and preserve the most specific source location
  available.
- Prefer deterministic data structures and ordering (`BTreeMap`, sorted paths,
  and source order) because output and golden fixtures must be reproducible.
- Value expressions in the target IR are flat: an operator may contain only
  atoms. Split compound expressions into deterministic `t0`, `t1`, ...
  assignments in dependency order. Delay expressions may be nested.
- Preserve every source driver in source order unless the DSL contract in
  `PLAN.md` explicitly defines a normalization. Registers contain modeled state
  only, never merely internal or primitive-driven nets.
- Lower only the first entry of a SystemVerilog delay tuple. Do not sum later
  rise/fall/turn-off entries.
- Add focused unit tests near small units and use first-class integration/golden
  tests for corpus behavior. Manually compare changed golden cells with their
  SystemVerilog sources and the DSL contract.
- Treat `PLAN.md` as authoritative. Update its milestone progress only after the
  milestone's acceptance checks pass, and keep `STATUS.md` consistent with the
  verified state.

## Common commands

Run these from the repository root.

```sh
# Build and run a converter command.
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lex
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- parse path/to/cell.sv
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- analyze path/to/cell.sv
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- lower path/to/cell.sv
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- convert-file input.sv output.cell --dry-run

# Run converter tests and required lint validation.
cargo test --manifest-path sv-to-sexpr/Cargo.toml
cargo clippy --manifest-path sv-to-sexpr/Cargo.toml --all-targets
cargo fmt --manifest-path sv-to-sexpr/Cargo.toml -- --check

# Run a staged full-corpus check (stages currently exposed by the CLI).
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lex
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage parse
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage analyze
cargo run --manifest-path sv-to-sexpr/Cargo.toml -- check sv-cells --stage lower

# Build or run the sibling formatter crate.
cargo test --manifest-path sexpr-fmt/Cargo.toml
cargo run --manifest-path sexpr-fmt/Cargo.toml -- path/to/file.cell
cargo run --manifest-path sexpr-fmt/Cargo.toml -- --check path/to/file.cell
```

Before committing converter changes, at minimum run its full tests, formatting
check, and `cargo clippy --manifest-path sv-to-sexpr/Cargo.toml --all-targets`.
Also run the strongest applicable staged corpus and formatter checks required by
the current milestone in `PLAN.md`.
