# CSS Variable LSP (Rust Implementation)

A Language Server Protocol implementation for CSS Variables, written in Rust.

## Status: Complete

This is a ground-up Rust rewrite of the TypeScript/Node-based `css-variable-lsp`, eliminating the Node/npm dependency for the Zed extension.

### Features

- CSS parsing for variable definitions and `var()` usage tracking
- HTML parsing for `<style>` blocks and inline styles (custom DOM scanner)
- Cascade sorting and specificity calculation
- LSP features: completion, hover, definition, references, rename, diagnostics, document/workspace symbols
- Workspace scanning and color provider (hex/rgb/hsl + named colors)

## Autocomplete Contexts

TypeScript LSP (`css-variable-lsp`):
- Triggers on `-`, `(`, and `:`.
- Returns items only in CSS value contexts (after a `:` and before the next `;`).
- CSS-like files: inside rules/declarations (requires `{}` context).
- HTML-like files: inside `style="..."` attributes or `<style>...</style>` blocks.
- JS/TS/JSX/TSX: only inside string literals or template literal text (not inside `${...}`).
- Insert text: `--name` if already inside `var(`, otherwise `var(--name)`.

Rust LSP (this repo):
- Matches the TypeScript LSP behavior above.
- Document kind detection uses language ID when available, otherwise extensions derived from `lookupFiles`.

## Building

```bash
cargo build --release
```

## Release Assets (Local)

Build and package release assets into `dist/` (tar.gz on Unix, zip on Windows):

```bash
./scripts/build-release-assets.sh
```

To build a subset of targets:

```bash
./scripts/build-release-assets.sh x86_64-apple-darwin aarch64-apple-darwin
```

## Running

```bash
cargo run --release
```

The LSP server communicates via stdin/stdout using the Language Server Protocol.

## Architecture

- `main.rs` - Entry point, sets up async runtime and LSP server
- `lsp_server.rs` - LSP protocol handlers (implements `tower_lsp_server::LanguageServer`)
- `manager.rs` - CSS variable manager (stores definitions/usages, DOM trees)
- `types.rs` - Core data types (CssVariable, CssVariableUsage, Config, etc.)
- `parsers/` - CSS and HTML parsing (definitions + var() usages)
- `dom_tree.rs` - Lightweight HTML scanner for selector matching
- `specificity.rs` - Specificity calculation and cascade ordering
- `workspace.rs` - Workspace scanning and file discovery
- `flags.rs` - Reusable flag parsing helper functions
- `runtime_config.rs` - CLI/env configuration parsing
- `path_display.rs` - Path formatting for hover/completion
- `color.rs` - Color parsing and color provider helpers

## Configuration

The LSP server accepts configuration via CLI flags and environment variables.

### Feature Flags

| Flag | CLI (disable) | Env var | Default | Description |
|------|--------------|---------|---------|-------------|
| Color preview | `--no-color-preview` | `CSS_LSP_COLOR_PREVIEW=0` | true | Enable color picker |
| Color only variables | `--color-only-variables` | `CSS_LSP_COLOR_ONLY_VARIABLES=1` | false | Colors only on var() |
| Lookup files | `--lookup-files` | `CSS_LSP_LOOKUP_FILES` | None | File extensions to scan |
| Ignore globs | `--ignore-globs` | `CSS_LSP_IGNORE_GLOBS` | None | Patterns to exclude |
| Path display | `--path-display` | `CSS_LSP_PATH_DISPLAY` | relative | Path format mode |
| Path length | `--path-display-length` | `CSS_LSP_PATH_DISPLAY_LENGTH` | 1 | Abbreviation length |
| Undefined fallback | `--undefined-var-fallback` | `CSS_LSP_UNDEFINED_VAR_FALLBACK` | warning | Fallback diagnostic level |
| Suggest add fallback | `--no-suggest-add-fallback` | `CSS_LSP_SUGGEST_ADD_FALLBACK=0` | true | Add fallback quickfix |
| Suggest color vars | `--no-suggest-exact-color-variables` | `CSS_LSP_SUGGEST_EXACT_COLOR_VARIABLES=0` | true | Color replacement suggestions |

### Examples

```bash
# Disable color preview
./css-variable-lsp --no-color-preview

# Set lookup files via env
CSS_LSP_LOOKUP_FILES="*.css,*.scss" ./css-variable-lsp

# Combine options
./css-variable-lsp --path-display=abbreviated:2 --no-suggest-add-fallback
```

## Dependencies

- `tower-lsp-server` - LSP server framework
- `tokio` - Async runtime
- `globset` / `walkdir` - Workspace scanning
- `csscolorparser` - Color value parsing
- `regex` / `pathdiff` - Parsing helpers and path formatting

## License

GPL-3.0
