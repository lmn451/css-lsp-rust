# Option 34: Definition-change gate + targeted revalidation

## Summary
Combine option 4's "only react when definitions change" with option 3's
"revalidate only affected open docs." This minimizes unnecessary work while
remaining correct and responsive.

## Why combine
- Most edits do not change variable names, so no cross-document work needed.
- When variable names do change, only a subset of open docs reference them.
- The combined approach reduces both frequency and scope of revalidation.

## Key idea
1. Detect whether the edited document's variable *name set* has changed.
2. If it has changed, compute the *changed names*.
3. Revalidate only open docs that reference those names.

## Implementation approach (diagnostics-aligned)
Use a diagnostics-aligned usage index (same regex as diagnostics) to avoid
parser mismatch issues.

### State additions
In `CssVariableLsp`:
- `document_usage_map: Arc<RwLock<HashMap<Url, HashSet<String>>>>`
- `usage_index: Arc<RwLock<HashMap<String, HashSet<Url>>>>` (optional but fast)

### Algorithm
On `did_open`/`did_change`:
1. `old_names = get_document_variable_names(uri)` (from manager).
2. Parse document (clears + re-adds definitions/usages).
3. `new_names = get_document_variable_names(uri)`.
4. If `old_names == new_names`, stop (no cross-doc revalidation).
5. `changed_names = symmetric_diff(old_names, new_names)`.
6. `affected_uris = union(usage_index[name]) for each changed name`.
7. Revalidate those open docs (skip current doc).

On `validate_document_text`:
- Compute diagnostics using the existing regex.
- Compute `usage_set` using the same regex.
- Update `document_usage_map` and `usage_index` by diffing old vs new sets.

On `did_close`:
- Remove `document_usage_map[uri]` and update `usage_index`.

## Complexity and cost
- Effort: medium-high (new state + diff logic + usage indexing).
- Runtime: low for common edits; targeted revalidation only on definition changes.
- Memory: moderate but bounded by open docs and variable names.

## Risks and edge cases
- Must keep usage index in sync with diagnostics regex.
- If parsing fails, treat as definition removal (triggering revalidation).
- Concurrent edits need careful locking (use snapshots to avoid holding locks).

## Testing ideas
- Edit a variable value (no name change): no cross-doc revalidation.
- Add a new variable name in `vars.scss`: only open docs that reference it
  revalidate and clear warnings.
- Remove a variable name: only docs that reference it revalidate and add warnings.
- Add nested fallback usage: ensure diagnostics usage index catches it.

## Notes
This combined approach replaces the "revalidate all open docs on every edit"
behavior (including any debounced version). It provides correctness with better
scalability in large workspaces. 
