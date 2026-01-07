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

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release
```

The LSP server communicates via stdin/stdout using the Language Server Protocol.

## Architecture

- `main.rs` - Entry point, sets up async runtime and LSP server
- `lsp_server.rs` - LSP protocol handlers (implements `tower_lsp::LanguageServer`)
- `manager.rs` - CSS variable manager (stores definitions/usages, DOM trees)
- `types.rs` - Core data types (CssVariable, CssVariableUsage, Config, etc.)
- `parsers/` - CSS and HTML parsing (definitions + var() usages)
- `dom_tree.rs` - Lightweight HTML scanner for selector matching
- `specificity.rs` - Specificity calculation and cascade ordering
- `workspace.rs` - Workspace scanning and file discovery
- `runtime_config.rs` - CLI/env configuration parsing
- `path_display.rs` - Path formatting for hover/completion
- `color.rs` - Color parsing and color provider helpers

## Dependencies

- `tower-lsp` - LSP server framework
- `tokio` - Async runtime
- `globset` / `walkdir` - Workspace scanning
- `csscolorparser` - Color value parsing
- `regex` / `pathdiff` - Parsing helpers and path formatting

## License

GPL-3.0
