# src/ Module Knowledge

**Parent:** ./AGENTS.md (root)

## OVERVIEW
Core LSP server implementation with modular architecture. Recent refactoring split the monolithic `lsp_server.rs` into focused modules.

## STRUCTURE
```
src/
├── lsp_server.rs      # Main LSP server (~2048 lines after refactoring)
├── completion_context.rs # Completion context analysis (~478 lines, NEW)
├── document_kind.rs    # DocumentKind enum + resolution (~103 lines, NEW)
├── text_utils.rs      # Pure text utilities (~64 lines, NEW)
├── manager.rs         # CSS variable storage/management
├── types.rs           # Core data structures
├── parsers/           # CSS/HTML parsing submodules
├── dom_tree.rs        # HTML DOM traversal
├── specificity.rs     # CSS specificity calculation
├── color.rs           # Color parsing + color provider
├── workspace.rs       # Workspace scanning
├── flags.rs           # CLI flag parsing helpers
├── runtime_config.rs  # Runtime configuration
└── path_display.rs    # Path formatting for display
```

## KEY MODULES

### document_kind.rs (NEW)
- `DocumentKind` enum: `Css`, `Html`
- `resolve_document_kind()` - determines how to parse a document
- `build_lookup_extension_map()` - maps file extensions to kinds
- `language_id_kind()` - maps language IDs to kinds
- `ClientConfigPatch` + `apply_config_patch()` - config merging

### completion_context.rs (NEW)
- `CompletionContextSlice` - completion trigger context
- `completion_value_context_slice()` - main entry point
- `find_html_style_*()` - HTML style detection helpers
- `find_js_string_segment()` - JS string literal detection
- `score_variable_relevance()` - relevance scoring for completions

### text_utils.rs (NEW)
- `clamp_to_char_boundary()` - safe character boundary indexing
- `is_word_char()`, `is_word_byte()` - word character detection
- `range_contains_position()`, `range_contains()` - range utilities
- `apply_change_to_text()` - apply incremental text changes
- `find_value_range_in_definition()` - find value position in CSS def

## DEPENDENCY GRAPH
```
text_utils (no deps)
    ↓
document_kind (uses types::Config)
    ↓
completion_context (uses document_kind + text_utils)
    ↓
lsp_server (uses all modules above)
```

## CONVENTIONS (src/ specific)
- LSP handlers return `tower_lsp_server::jsonrpc::Result<T>`
- Internal helpers are `pub(crate)` or private
- Tests use `#[cfg(test)] mod tests` at end of each file
- Large functions (>100 lines) should be candidates for extraction

## LARGE FILES (>500 lines)
| File | Lines | Concern |
|------|-------|---------|
| lsp_server.rs | 2048 | High - previously 2669 lines, refactored |
| completion_context.rs | 478 | Medium - recently extracted |
| dom_tree.rs | 672 | Medium |
| manager.rs | 668 | Medium |
| specificity.rs | 397 | Low |
| color.rs | 313 | Low |
