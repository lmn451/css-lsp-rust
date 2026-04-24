# src/parsers/ Module Knowledge

**Parent:** ./AGENTS.md (root)

## OVERVIEW
CSS and HTML parsing submodules. Extract variable definitions, `var()` usages, and literal colors from documents.

## STRUCTURE
```
src/parsers/
├── mod.rs      # Module exports (css, html)
├── css.rs      # CSS variable extraction
└── html.rs     # HTML style extraction
```

## FILES

### css.rs
- `parse_css_document()` - main entry point
- `parse_css_snippet()` - parses CSS content with context
- Handles: variable definitions, `var()` usages, `!important`, fallback values
- Returns: `CssVariable` definitions + `CssVariableUsage` occurrences

### html.rs
- `parse_html_document()` - main entry point
- Extracts from: `<style>` blocks, `style=""` attributes
- Uses: `DomTree` for selector matching

## CONVENTIONS
- Functions return `Result<(), String>` for fallible operations
- All parsing functions are `async`
- Manager is passed as reference: `manager: &CssVariableManager`
- URI used for document identification

## USAGE FROM LSP
```rust
// Called from lsp_server.rs parse_document_text()
Some(DocumentKind::Html) => parse_html_document(text, uri, &self.manager).await,
Some(DocumentKind::Css) => parse_css_document(text, uri, &self.manager).await,
```
