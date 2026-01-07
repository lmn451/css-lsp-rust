# Repository Guidelines

## Project Structure & Module Organization
The Rust source lives under `src/`. Entry point: `main.rs`. Core LSP handlers live in `lsp_server.rs`, variable storage in `manager.rs`, and shared types in `types.rs`. Parsing is under `src/parsers/` (`css.rs`, `html.rs`) with DOM support in `dom_tree.rs`. Other key modules: `specificity.rs` (cascade), `color.rs` (color parsing/presentations), `workspace.rs` (workspace scan), `runtime_config.rs` (CLI/env flags), and `path_display.rs` (path formatting for hover/completion). Top-level docs include `README.md`, `QUICKSTART.md`, and status notes. Build outputs land in `target/`. Example fixtures are `TEST_EXAMPLE.css` and `TEST_EXAMPLE.html`.

## Build, Test, and Development Commands
- `cargo build --release` - builds `target/release/css-variable-lsp`.
- `cargo run --release` - runs the LSP over stdin/stdout.
- `cargo check` - fast compile check during development.
- `cargo fmt` - format with rustfmt.
- `RUST_LOG=debug cargo run` - enables verbose logging via `tracing`.
- `cargo test` - runs unit tests.
For manual LSP request testing, follow the JSON-RPC steps in `QUICKSTART.md`.

## Coding Style & Naming Conventions
Use Rust 2021 defaults (4-space indentation) and standard `rustfmt` output. Follow Rust naming: `snake_case` for modules/functions/files, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants. When adding parsers, place them in `src/parsers/` and update `src/parsers/mod.rs`.

## Testing Guidelines
Unit tests live next to the code (e.g., `src/specificity.rs`, `src/parsers/css.rs`, `src/runtime_config.rs`). Prefer `#[cfg(test)]` modules with descriptive `snake_case` test names. Integration tests can be added under `tests/` when needed.

## Commit & Pull Request Guidelines
This checkout does not include a `.git` directory, so commit conventions cannot be inferred. Use short, imperative subjects (e.g., "Add hover cascade ordering") and keep commits focused. For PRs, include: a clear description, how you tested (`cargo check`/`cargo test`), and any LSP behavior changes; update `README.md` or `QUICKSTART.md` if commands or usage change.

## Configuration & Runtime Notes
The server communicates via LSP over stdio. Common runtime flags/env:
- `--no-color-preview` / `CSS_LSP_COLOR_ONLY_VARIABLES=1`
- `--lookup-files`, `--ignore-globs`
- `--path-display=relative|absolute|abbreviated[:N]`
- `--path-display-length=N`
Use `RUST_LOG=debug` to turn on debug output when troubleshooting.
