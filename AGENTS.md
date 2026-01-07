# Repository Guidelines for CSS Variable LSP

## Project Structure & Module Organization

### Source Code Layout
- **`src/main.rs`**: Application entry point with LSP server initialization
- **`src/lsp_server.rs`**: Core LSP protocol handlers (completion, hover, goto-definition, rename, etc.)
- **`src/manager.rs`**: CSS variable storage and management with thread-safe operations
- **`src/types.rs`**: Core data structures (`CssVariable`, `CssVariableUsage`, `Config`, etc.)
- **`src/parsers/`**: CSS/HTML parsing logic
  - **`css.rs`**: CSS variable extraction and parsing
  - **`html.rs`**: HTML document parsing with style extraction
- **`src/dom_tree.rs`**: HTML DOM tree parsing for selector matching
- **`src/specificity.rs`**: CSS specificity calculation for cascade ordering
- **`src/color.rs`**: Color parsing and LSP color provider implementation
- **`src/workspace.rs`**: Workspace scanning and file discovery
- **`src/runtime_config.rs`**: CLI argument and environment variable parsing
- **`src/path_display.rs`**: Path formatting for hover/completion display
- **`tests/integration_test.rs`**: Integration tests covering end-to-end functionality

### Key Design Patterns
- **Async-first**: All I/O operations use `tokio` async runtime
- **Thread-safe**: Shared state uses `Arc<RwLock<T>>` for concurrent access
- **Immutable data**: Core types implement `Clone` for safe sharing
- **Error handling**: Functions return `Result<T, String>` for recoverable errors
- **LSP compliance**: Uses `tower-lsp` framework with standard LSP types

## Build, Test, and Development Commands

### Core Commands
- **`cargo build --release`**: Production build optimized for performance
- **`cargo run --release`**: Run the LSP server in production mode
- **`cargo check`**: Fast compilation check without building binaries
- **`cargo fmt`**: Format code using `rustfmt` (always run before committing)
- **`cargo clippy`**: Lint code for common mistakes and style issues
- **`cargo test`**: Run all unit and integration tests
- **`RUST_LOG=debug cargo run`**: Run with debug logging enabled

### Testing Specific Tests
- **`cargo test test_name`**: Run a specific test function
- **`cargo test module_name::`**: Run all tests in a specific module
- **`cargo test --lib`**: Run only unit tests (exclude integration tests)
- **`cargo test --test integration_test`**: Run only integration tests
- **`cargo test -- --nocapture`**: Show test output even for passing tests
- **`cargo test -- --test-threads=1`**: Run tests sequentially for debugging

### Development Workflow
```bash
# Quick development cycle
cargo check                    # Fast feedback
cargo fmt                      # Format code
cargo test                     # Run tests
cargo build --release          # Final build

# Debug specific functionality
RUST_LOG=debug cargo run -- --lookup-files="*.css"

# Test specific scenarios
cargo test test_lsp_rename_preserves_fallbacks -- --nocapture
```

### Performance Testing
- **`cargo build --release --timings`**: Show build timing information
- **`time cargo test`**: Measure test execution time
- **`cargo flamegraph`**: Generate flame graphs (requires `cargo-flamegraph`)

## Code Style Guidelines

### Rust Language Features
- **Edition**: Use Rust 2021 edition features
- **Async**: Use `async fn` and `await` for all I/O operations
- **Error handling**: Return `Result<T, String>` for fallible operations
- **Ownership**: Follow standard Rust ownership patterns, minimize `.clone()`
- **Lifetimes**: Use explicit lifetimes where needed for borrowed data

### Imports and Dependencies
```rust
// Group imports by source
use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_lsp::lsp_types::*;

// Local imports (alphabetical)
use crate::manager::CssVariableManager;
use crate::types::{CssVariable, CssVariableUsage};
```

### Naming Conventions
- **Modules/Files**: `snake_case` (e.g., `lsp_server.rs`, `variable_manager.rs`)
- **Functions**: `snake_case` (e.g., `get_variables()`, `parse_css_document()`)
- **Types/Structs**: `CamelCase` (e.g., `CssVariable`, `CssParseContext`)
- **Enums**: `CamelCase` for variants (e.g., `PathDisplayMode::Relative`)
- **Constants**: `SCREAMING_SNAKE_CASE` (e.g., `DEFAULT_TIMEOUT`)
- **Fields**: `snake_case` (e.g., `variable_name`, `source_position`)
- **Test functions**: `snake_case` with descriptive names (e.g., `test_lsp_rename_functionality`)

### Documentation Standards
```rust
/// Main function description with examples
/// # Arguments
/// * `param` - Description of parameter
/// # Returns
/// Description of return value
/// # Examples
/// ```
/// let result = function_call(args);
/// assert!(result.is_ok());
/// ```
pub async fn function_name(param: Type) -> Result<ReturnType, String> {
    // Implementation
}
```

### Code Formatting
- **Indentation**: 4 spaces (enforced by `rustfmt`)
- **Line length**: 100 characters maximum
- **Braces**: Same line for functions, structs; new line for `impl` blocks
- **Trailing commas**: Always include in multi-line structures
- **Empty lines**: Use to separate logical sections

### Function Structure
```rust
pub async fn parse_css_document(
    text: &str,
    uri: &Url,
    manager: &CssVariableManager,
) -> Result<(), String> {
    // Input validation
    if text.is_empty() {
        return Ok(());
    }

    // Core logic
    let context = CssParseContext {
        css_text: text,
        full_text: text,
        uri,
        manager,
        base_offset: 0,
        inline: false,
        usage_context_override: None,
        dom_node: None,
    };

    // Error handling
    parse_css_snippet(context).await.map_err(|e| {
        format!("Failed to parse CSS document {}: {}", uri, e)
    })
}
```

### Error Handling Patterns
```rust
// For recoverable errors, use Result<T, String>
pub fn parse_value(value: &str) -> Result<ParsedValue, String> {
    value.parse().map_err(|_| format!("Invalid value: {}", value))
}

// For programming errors, use panic! or unwrap() with context
let config = runtime_config.as_ref()
    .expect("RuntimeConfig should be set during initialization");

// For LSP protocol errors, return tower_lsp::jsonrpc::Result
async fn hover(&self, params: HoverParams)
    -> tower_lsp::jsonrpc::Result<Option<Hover>>
{
    // Implementation
    Ok(Some(hover_info))
}
```

### Async Patterns
```rust
// Prefer async fn for public APIs
pub async fn get_variables(&self, name: &str) -> Vec<CssVariable> {
    let variables = self.variables.read().await;
    variables.get(name)
        .cloned()
        .unwrap_or_default()
}

// Use tokio::spawn for background tasks
tokio::spawn(async move {
    workspace::scan_workspace(folders, manager).await?;
    Ok(())
});
```

### Testing Guidelines

#### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_specificity() {
        let spec = calculate_specificity(":root");
        assert_eq!(spec.ids, 0);
        assert_eq!(spec.classes, 1);
        assert_eq!(spec.elements, 0);
    }

    #[tokio::test]
    async fn test_manager_operations() {
        let manager = CssVariableManager::new(Config::default());
        // Test async operations
    }
}
```

#### Integration Tests
- Place in `tests/` directory
- Test end-to-end functionality
- Use descriptive names: `test_feature_scenario_expected_result`
- Cover edge cases and error conditions

#### Test Organization
- **Unit tests**: Test individual functions in isolation
- **Integration tests**: Test component interactions
- **Edge cases**: Empty inputs, malformed data, boundary conditions
- **Async testing**: Use `#[tokio::test]` for async test functions

### Performance Considerations
- **Minimize allocations**: Reuse buffers where possible
- **Async efficiency**: Avoid blocking operations in async contexts
- **Memory usage**: Use `Arc` for shared immutable data
- **Lock granularity**: Hold locks for minimal time
- **Regex compilation**: Compile regexes once and reuse

### Security Guidelines
- **Input validation**: Validate all user inputs before processing
- **Path safety**: Use `Url` types for file paths, avoid raw string manipulation
- **Memory limits**: Be mindful of memory usage with large files
- **Error messages**: Don't expose internal implementation details in errors

### Commit Message Guidelines
- **Format**: `type(scope): description`
- **Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
- **Examples**:
  - `feat(parser): add support for CSS calc() expressions`
  - `fix(rename): preserve fallbacks in var() calls`
  - `test: add integration tests for complex selectors`
  - `refactor: improve error handling in workspace scanning`

### LSP Protocol Compliance
- **Message format**: Use standard LSP JSON-RPC 2.0 format
- **Type safety**: Leverage `tower-lsp` types for all protocol messages
- **Error responses**: Return appropriate LSP error codes
- **Capabilities**: Advertise supported features in `initialize` response
- **Incremental updates**: Support incremental document changes

### Configuration Management
- **Environment variables**: Use `CSS_LSP_*` prefix
- **CLI flags**: Follow `--kebab-case` convention
- **Default values**: Provide sensible defaults in `Default` implementations
- **Validation**: Validate configuration at startup, not runtime
