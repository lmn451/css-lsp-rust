# Plan: Literal Color Index Optimization

## Problem
The `rebuild_color_index()` function rebuilds the entire color index on every document change, causing O(n * m) performance where n = total variables, m = chain depth. This is wasteful for incremental changes.

---

## 1. Incremental Color Index Updates

### Approach
Replace full rebuilds with targeted updates when variables change.

### Changes to `src/manager.rs`

```rust
// New field in CssVariableManager
color_variables_dirty: Arc<AtomicBool>,

// New method: mark index as needing rebuild
pub fn mark_color_index_dirty(&self) {
    self.color_variables_dirty.store(true);
}

// New method: rebuild if dirty, no-op if clean
pub async fn ensure_color_index_valid(&self) {
    if self.color_variables_dirty.load() {
        self.rebuild_color_index().await;
        self.color_variables_dirty.store(false);
    }
}
```

### Changes to `src/lsp_server.rs`

```rust
// In parse_document_text: mark dirty instead of immediate rebuild
self.manager.mark_color_index_dirty().await;

// Before validation/completion, ensure index is valid
self.manager.ensure_color_index_valid().await;
validate_document_text_with(...)

// In remove_document:
self.manager.remove_document(uri).await;
self.manager.mark_color_index_dirty().await;  // or remove variable from index
```

### Benefits
- O(1) per document change instead of O(n)
- Index rebuilt only when actually needed (lazy evaluation)
- Batch multiple changes together

---

## 2. Remove Document Index Cleanup

### Changes to `src/manager.rs`

```rust
pub async fn remove_document(&self, uri: &Uri) {
    // ... existing cleanup ...

    // Get variable names that were removed
    let removed_names: Vec<String> = vars.iter()
        .filter(|(_, list)| list.iter().all(|v| &v.uri == uri))
        .map(|(name, _)| name.clone())
        .collect();

    // Remove from color index
    {
        let mut color_vars = self.color_variables.write().await;
        for name in removed_names {
            // Need to find and remove this variable's entries
            // We'll add a helper for this
            color_vars.remove_variable_from_index(&name);
        }
    }
}

// New method in color index map
impl HashMap<NormalizedColorKey, HashSet<String>> {
    pub fn remove_variable(&mut self, var_name: &str) {
        self.retain(|_, names| {
            names.remove(var_name);
            !names.is_empty()
        });
    }
}
```

### Alternative: Lazy cleanup
Just mark dirty and let `ensure_color_index_valid()` handle it. Simpler but may show stale suggestions briefly.

---

## 3. Add Variable API Consistency

### Option A: Document the pattern
Add a comment in `add_variable()`:
```rust
/// Note: Does not update color index. Call rebuild_color_index()
/// or ensure_color_index_valid() after batch operations.
pub async fn add_variable(&self, variable: CssVariable) { ... }
```

### Option B: Auto-update (not recommended)
Have `add_variable()` call `update_color_index_for_variable()`. But this is inefficient for bulk operations.

**Recommendation**: Option A with documentation + ensure index is marked dirty.

---

## 4. Stricter Env Var Parsing

### Changes to `src/flags.rs`

```rust
pub fn flag_bool(...) -> bool {
    if args.iter().any(|arg| arg == cli_disable) {
        return false;
    }
    if let Some(v) = env.get(env_key) {
        // Stricter: only accept "0" as false, "1" as true
        match v.as_str() {
            "1" | "true" | "yes" | "on" => return true,
            "0" | "false" | "no" | "off" => return false,
            _ => {} // Fall through to default
        }
    }
    default
}
```

Or stricter version:
```rust
if let Some(v) = env.get(env_key) {
    return v == "1";  // Only "1" is truthy
}
```

---

## 5. Test Coverage for Incremental Updates

### New integration tests

```rust
#[tokio::test]
async fn test_color_index_updates_incrementally_on_variable_change() {
    // Setup with multiple variables
    // Change one variable's color
    // Verify diagnostics update
    // Verify index only has the one change
}

#[tokio::test]
async fn test_color_index_removes_variable_on_document_close() {
    // Define color variable in file A
    // Use literal in file B
    // Close file A
    // Verify diagnostic disappears from file B
}

#[tokio::test]
async fn test_color_index_batches_multiple_changes() {
    // Change 5 variables in one file
    // Verify single rebuild, not 5
}
```

---

## Implementation Order

1. **Phase 1: Incremental updates** (biggest perf win) ✅ DONE
   - [x] Add `color_variables_dirty` flag
   - [x] Add `mark_dirty()` and `ensure_valid()` methods
   - [x] Update lsp_server.rs to use lazy rebuild
   - [x] Update `remove_document` to invalidate index

2. **Phase 2: Remove document cleanup** ✅ DONE (lazy cleanup via mark_dirty)
   - [x] mark_color_index_dirty on document removal

3. **Phase 3: Env var parsing fix** ⏭️ Skipped (low priority)

4. **Phase 4: Test coverage** ✅ DONE
   - [x] Add performance benchmark test (`test_color_index_rebuild_performance_100k_vars`)
   - [x] Add lazy rebuild batching test (`test_lazy_rebuild_batches_multiple_changes`)

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/manager.rs` | +dirty flag, +mark_dirty, +ensure_valid, +remove_variable_from_index |
| `src/lsp_server.rs` | Use lazy rebuild pattern |
| `src/flags.rs` | Stricter boolean parsing |
| `tests/diagnostics_integration_test.rs` | New test cases |

---

## Estimated Effort

- Phase 1: 2-3 hours
- Phase 2: 30 minutes  
- Phase 3: 15 minutes
- Phase 4: 1-2 hours

**Total**: ~4-5 hours

---

## Verification

### Before Fix: Measure Performance Problem
Add a benchmark test to quantify the issue:

```rust
#[tokio::test]
async fn test_color_index_rebuild_performance_100k_vars() {
    use std::time::Instant;
    
    let manager = CssVariableManager::new(Config::default());
    
    // Add 100_000 color variables with chains
    for i in 0..100_000 {
        manager.add_variable(create_test_variable(
            &format!("--color-{}", i),
            &format!("var(--color-{})", (i + 1) % 100_000),  // chain to next
            ":root",
            "file:///test.css",
        )).await;
    }
    
    // Measure rebuild time
    let start = Instant::now();
    manager.rebuild_color_index().await;
    let duration = start.elapsed();
    
    println!("Rebuild time for 100_000 vars: {:?}", duration);
    
    // Should complete in reasonable time (adjust threshold after baseline)
    assert!(duration.as_secs() < 10, "Rebuild took {:?}", duration);
}
```

Run: `cargo test test_color_index_rebuild_performance -- --nocapture`

Record the time, then run again after fix to confirm improvement.

### After Fix: Functional Tests
- [ ] Run existing tests: `cargo test`
- [ ] Test color diagnostics appear for matching literal colors
- [ ] Test quick fixes replace with correct variable names
- [ ] Test completion filters by exact color match
- [ ] Test incremental updates work (change one var → correct behavior)
- [ ] Test `remove_document` clears stale color suggestions
