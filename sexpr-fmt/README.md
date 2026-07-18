# sexpr-fmt

`sexpr-fmt` is a small Rust command-line formatter for generic S-expression files.
It parses atoms, lists, semicolon line comments, and blank lines, then rewrites the
input into a consistent, deterministic layout.

## Build

From the repository root:

```sh
cargo build --manifest-path sexpr-fmt/Cargo.toml
```

Run the tests with:

```sh
cargo test --manifest-path sexpr-fmt/Cargo.toml
```

## Library API

The CLI and downstream tools share the same formatter implementation. Add a
path or registry dependency and call the default canonical formatter:

```rust
let canonical = sexpr_fmt::format_source_default(source)?;
```

Use `format_source(source, FormatOptions { .. })` when a non-default width or
inline-item limit is required.

## Usage

Format a file and print the result to stdout:

```sh
cargo run --manifest-path sexpr-fmt/Cargo.toml -- path/to/file.cell
```

Rewrite a file in place:

```sh
cargo run --manifest-path sexpr-fmt/Cargo.toml -- --write path/to/file.cell
```

Check whether a file is already formatted:

```sh
cargo run --manifest-path sexpr-fmt/Cargo.toml -- --check path/to/file.cell
```

Set a custom line width:

```sh
cargo run --manifest-path sexpr-fmt/Cargo.toml -- --width 80 path/to/file.cell
```

## Notes

- The formatter uses 2 spaces per indentation level.
- Strings are not parsed specially; quote characters are treated as ordinary atom characters.
- `--check` exits with `0` when the file is already formatted and `1` when formatting would change the file.
