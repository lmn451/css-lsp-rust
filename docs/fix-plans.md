# Fix Plans for CSS Variable LSP Issues

---

## Issue 1: `remove_document()` doesn't rebuild color index

### Problem
When `remove_document()` is called, the `color_variables` index is not updated. Stale entries remain pointing to variable names that no longer exist.

### Fix Location
`src/manager.rs` - `remove_document()` method (~line 168)

### Implementation
```rust
pub async fn remove_document(&self, uri: &Uri) {
    // ... existing code to remove from variables, usages, etc. ...

    // ADD: Rebuild color index to remove stale entries
    // Need to drop the locks first since rebuild_color_index needs read access
    drop(vars);
    drop(usages);
    drop(literal_colors);
    drop(dom_trees);
    self.rebuild_color_index().await;
}
```

### Test Update
After fix, update `tests/issues_proof_test.rs`:
```rust
#[tokio::test]
async fn issue_1_fixed_color_index_after_remove() {
    // ... setup with 2 docs, one with white color ...
    
    manager.remove_document(&uri1).await;
    
    // After fix: The color index should be automatically rebuilt
    // No stale entries should remain
    assert_eq!(
        manager.get_variables_by_color_key(&white_key).await.len(),
        0,
        "After fix: color index is automatically rebuilt"
    );
}
```

### Verification
1. Run `cargo test issue_1`
2. Verify all existing tests still pass

---

## Issue 2: `did_change` should ensure color index is rebuilt

### Problem
When a document changes, its color variables may change. The color index needs to be rebuilt to reflect the new state.

### Current Behavior
`parse_document_text()` in `lsp_server.rs` already calls `rebuild_color_index()` at line ~302.

### Fix Location
`src/lsp_server.rs` - Verify `parse_document_text()` calls rebuild

### Implementation
Verify this code path is correct:
```rust
async fn parse_document_text(&self, uri: &Uri, text: &str, language_id: Option<&str>) {
    self.manager.remove_document(uri).await;  // Already calls remove_document
    
    // ... parse document ...
    
    self.manager.rebuild_color_index().await;  // Line ~302 - rebuild after parse
}
```

### Note
After fixing Issue 1, `remove_document()` will also rebuild. This means `parse_document_text` may do a redundant rebuild. Consider:
- Option A: Keep both (safe, slight performance hit)
- Option B: Remove rebuild from `parse_document_text` since `remove_document` now rebuilds
- Option C: Add a `rebuild_if_needed()` method that's smarter

### Verification
1. Run `cargo test issue_2`
2. Verify color changes are reflected after `did_change`

---

## Issue 3 & 4: Refactor `extract_is_arg` and `extract_not_arg`

### Problem
Two nearly identical functions with ~40 lines of duplicated logic, only differing in the prefix bytes (`:is(` vs `:not(`).

### Fix Location
`src/specificity.rs` - `extract_is_arg()` (~line 36) and `extract_not_arg()` (~line 83)

### Implementation
Extract a shared helper:

```rust
/// Extract the argument from a pseudo-class with parentheses.
/// e.g., `:is(.foo, #bar)` returns `.foo, #bar`
/// e.g., `:not(.foo)` returns `.foo`
fn extract_pseudo_arg(selector: &str, prefix: &[u8]) -> Option<&str> {
    let bytes = selector.as_bytes();
    let len = bytes.len();
    let prefix_len = prefix.len();

    for i in 0..len {
        if bytes[i] != b':' || i + prefix_len > len {
            continue;
        }
        let mut match_prefix = true;
        for j in 0..prefix_len {
            if bytes[i + j] != prefix[j] {
                match_prefix = false;
                break;
            }
        }
        if !match_prefix {
            continue;
        }

        // Found prefix at position i. Now find matching ) at depth 1.
        let mut depth = 1;
        let mut pos = i + prefix_len;
        while pos < len {
            match bytes[pos] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&selector[i + prefix_len..pos]);
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        return None;
    }
    None
}

// Update existing functions to use the helper
fn extract_is_arg(selector: &str) -> Option<&str> {
    extract_pseudo_arg(selector, b":is(")
}

fn extract_not_arg(selector: &str) -> Option<&str> {
    extract_pseudo_arg(selector, b":not(")
}
```

### Also rename `specificity_of_not_arg`:
```rust
/// Calculate the specificity of a pseudo-class argument (:not, :is, :where)
/// Per CSS spec: takes max specificity across comma-separated arguments.
fn specificity_of_pseudo_argument(arg: &str) -> Specificity {
    // ... existing implementation ...
}
```

### Test Update
After refactor, verify behavior is unchanged:
```rust
#[test]
fn issue_3_4_refactor_preserves_behavior() {
    // These should still produce same results after refactor
    assert_eq!(calculate_specificity(":not(.foo)"), calculate_specificity(".foo"));
    assert_eq!(calculate_specificity(":is(.foo)"), calculate_specificity(".foo"));
}
```

### Verification
1. Run `cargo test specificity`
2. All specificity tests should still pass

---

## Issue 5: Hex color parsing missing lengths 3 and 6

### Problem
`extract_literal_colors_from_value()` accepts hex lengths 4, 5, 7, 9 but NOT 3 (`#abc`) or 6 (`#aabbcc`), which are the most common formats.

### Fix Location
`src/parsers/css.rs` - `extract_literal_colors_from_value()` (~line 556)

### Current Code
```rust
let len = end - i;
if matches!(len, 4 | 5 | 7 | 9) {
    if let Some(color) = normalized_color_key(&value[i..end]) {
```

### Fix
```rust
let len = end - i;
// Include all valid hex lengths: 3 (#abc), 4 (#argb), 6 (#aabbcc), 8 (#aabbccdd)
// Also accept extended formats: 5 (#abcx), 7 (#aabbccx), 9 (#aabbccddxx)
if matches!(len, 3 | 4 | 5 | 6 | 7 | 8 | 9) {
    if let Some(color) = normalized_color_key(&value[i..end]) {
```

### Verification
1. Run `cargo test` - all tests should pass
2. Verify short hex colors (`#abc`, `#aabbcc`) are now extracted as literal colors

---

## Issue 6: Double extensions lose inner part

### Problem
`normalize_extension()` only returns the last extension. `.module.css` becomes just `.css`, losing the `module` part.

### Fix Location
`src/document_kind.rs` - `extract_extensions()` (~line 56)

### Current Code
```rust
fn extract_extensions(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    // ... glob handling ...

    let ext = std::path::Path::new(pattern)
        .extension()
        .and_then(|ext| ext.to_str());
    ext.and_then(normalize_extension).into_iter().collect()
}
```

### Options

**Option A: Return all extensions (breaking change)**
```rust
fn extract_extensions(pattern: &str) -> Vec<String> {
    // Handle double extensions: "foo.module.css" -> [".module", ".css"]
    let path = std::path::Path::new(pattern);
    let mut extensions = Vec::new();
    
    let mut current = path;
    while let Some(ext) = current.extension() {
        if let Some(ext_str) = ext.to_str() {
            extensions.push(format!(".{}", ext_str.to_lowercase()));
        }
        current = current.with_extension("");
        // Stop if no more extensions or we hit the stem
        if current.extension().is_none() {
            break;
        }
    }
    extensions.reverse(); // [.module, .css]
    extensions
}
```

**Option B: Handle specific known patterns (safer)**
```rust
fn extract_extensions(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    // ... glob handling ...

    let path = std::path::Path::new(pattern);
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or(pattern);
    
    // Check for known double-extension patterns
    let known_double = [
        ".module.css", ".module.scss", ".module.sass",
        ".d.ts", ".test.ts", ".spec.ts",
    ];
    
    for double in &known_double {
        if filename.ends_with(double) {
            let inner = &filename[filename.len() - double.len()..];
            if let Some(dot_pos) = inner.rfind('.') {
                return vec![
                    format!(".{}", &inner[dot_pos + 1..].to_lowercase()),
                    double.to_lowercase(),
                ];
            }
        }
    }
    
    // Fallback to single extension
    let ext = path.extension().and_then(|ext| ext.to_str());
    ext.and_then(normalize_extension).into_iter().collect()
}
```

### Recommendation
Option B is safer - it handles specific known patterns without breaking existing behavior.

### Verification
1. Run `cargo test` - all tests should pass
2. Verify `.module.css` is handled correctly

---

## Issue 7: NOT AN ISSUE

### Status
The function `is_var_function_context_slice` is actively used at `lsp_server.rs:905`. Not dead code.

### Action
None needed - close this issue as inaccurate.

---

## Summary of Fixes

| Issue | Fix Required | Complexity |
|-------|--------------|-------------|
| 1 | Add `rebuild_color_index()` to `remove_document()` | Low |
| 2 | Verify rebuild in `parse_document_text` | Low (already correct) |
| 3/4 | Extract shared helper function | Medium |
| 5 | Add hex lengths 3, 6 to extraction filter | Low |
| 6 | Handle double extensions in `extract_extensions` | Medium |
| 7 | None - not an issue | N/A |
