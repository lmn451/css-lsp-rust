# Quick Start Guide

## Prerequisites

- Rust 1.70+ (2021 edition)
- Cargo

## Build

```bash
cargo build --release
```

The binary will be at `target/release/css-variable-lsp`.

## Test LSP Communication

The LSP communicates via JSON-RPC over stdin/stdout. You can test it manually:

```bash
# Start the server
./target/release/css-variable-lsp

# Send an initialize request (paste this and press Enter):
Content-Length: 246

{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":"file:///tmp","capabilities":{},"initializationOptions":{},"trace":"off","workspaceFolders":[{"uri":"file:///tmp","name":"tmp"}]}}
```

You should see an initialize response with server capabilities.

## Use with Zed

The Zed extension should download or bundle `css-variable-lsp`, launch it as a language server, and route CSS variable LSP requests to the binary.

## Development

### Run checks

```bash
cargo check
```

### Run with debug logging

```bash
RUST_LOG=debug cargo run
```

### Run tests

```bash
cargo test
```

## Project Structure

```
rust-css-lsp/
├── Cargo.toml           # Dependencies and metadata
├── src/
│   ├── main.rs          # Entry point
│   ├── lsp_server.rs    # LSP protocol handlers
│   ├── manager.rs       # Variable storage and management
│   ├── types.rs         # Core data types
│   ├── specificity.rs   # Cascade and specificity
│   ├── workspace.rs     # Workspace scanning
│   ├── runtime_config.rs# CLI/env config
│   ├── path_display.rs  # Path formatting
│   ├── dom_tree.rs      # HTML DOM scanner
│   ├── color.rs         # Color provider
│   └── parsers/         # CSS/HTML parsing modules
├── README.md
```
