# Option 3: Targeted revalidate only affected open documents

## Summary
Revalidate only the open documents that actually reference variables whose
definitions changed, instead of revalidating every open document.

This aims to preserve correctness while reducing the cost of cross-file
diagnostic updates when a single file changes.

## Repo-specific context
- Diagnostics are computed by scanning the document text with a regex and
  checking whether each var() name is defined in the workspace.
- The manager already tracks definitions and usages, but usage extraction in
  the parser does not fully mirror the diagnostics regex (for example nested
  fallback var() calls are skipped by the parser but would still be matched by
  the diagnostics regex). That mismatch matters if we build a usage index from
  manager data.

## Design options
### 3A) Use manager usages to target
1. Before re-parse, capture the old set of variable names defined by the
   changed document (from the manager).
2. After re-parse, capture the new set.
3. Compute `changed_names = (old - new) U (new - old)`.
4. For each name in `changed_names`, fetch usages from the manager and collect
   their URIs.
5. Revalidate only those open documents.

Pros:
- Minimal new state in the LSP layer.
- Reuses existing manager indexes.

Cons:
- The parser usage index does not match diagnostic semantics today, so this
  could miss documents that still show warnings (especially for nested fallback
  var() usage).
- Fixing that mismatch likely means changing the parser to record those usages,
  or switching diagnostics to rely on the manager usage data.

### 3B) Maintain a diagnostics-aligned usage index (recommended)
Build a per-open-document usage map using the same regex used by diagnostics,
and keep it in sync as documents open/close/change. Use that map to target
revalidation.

Algorithm:
1. On each validate of a document, compute `usage_set` using the diagnostics
   regex.
2. Store `doc_usage_map[uri] = usage_set`.
3. Also maintain `usage_index[name] = {uris...}` for quick reverse lookups
   (optional but helpful).
4. When a document's definitions change, compute `changed_names`.
5. Target revalidation to `usage_index[name]` for those names.

Pros:
- Exact match to diagnostic semantics, no mismatch with parser rules.
- Fast revalidate for the common case (one file defines a variable used by a
  small subset of open docs).

Cons:
- New state to maintain (doc usage map and optional reverse index).
- Slight extra CPU on each validate to build the usage set.

## Implementation sketch (3B)
1. Add new state to `CssVariableLsp`:
   - `document_usage_map: Arc<RwLock<HashMap<Url, HashSet<String>>>>`
   - `usage_index: Arc<RwLock<HashMap<String, HashSet<Url>>>>` (optional)
2. Add a helper to compute usage sets from a string using the diagnostics regex.
3. Update `validate_document_text` to:
   - compute diagnostics (existing logic)
   - compute and store usage sets
   - update the reverse index by diffing old vs new sets for that document
4. In `did_open`/`did_change`:
   - capture `old_var_names` before parse
   - parse the document
   - capture `new_var_names`
   - compute `changed_names`
   - look up affected URIs via usage_index
   - revalidate those open docs (skip the currently edited one)
5. In `did_close`:
   - remove the document's usage set and update the reverse index

## Complexity and cost
- Effort: medium (new state + diff logic + extra validation path).
- Runtime: avoids full O(open_docs) revalidate on every change; worst case
  still O(open_docs) if many docs use the same variable, but much cheaper in
  typical scenarios.
- Memory: small extra maps keyed by open documents and variable names.

## Risks and edge cases
- Need to keep usage indexes consistent in the presence of async operations and
  debouncing.
- Must handle parse errors gracefully: if parsing fails, treat as a full
  definition removal (triggering revalidation for old names).
- HTML inline styles should be covered as long as diagnostics are applied to the
  raw document text (same as today).

## Testing ideas
- Open `index.scss` with `var(--dark)` warning.
- Open `vars.scss`, define `--dark`, verify warning clears in `index.scss`.
- Remove `--dark`, verify warning returns.
- Add nested fallback usage to ensure targeted revalidate covers it when using
  diagnostics-based usage sets.

## References
- TS implementation that revalidates all open docs with a debounce (baseline to
  compare against): https://github.com/lmn451/css-lsp/commit/d0cd11a3afae4a96c4288db8e97e630b2e341be9
