# Option 4: Revalidate all open docs only when definitions change

## Summary
Revalidate all open documents only when a document's set of defined variable
names changes. This is a low-complexity optimization that avoids cross-document
revalidation during edits that do not change variable definitions.

## Repo-specific context
The current diagnostics only care about whether a variable is defined anywhere
in the workspace. That means the only changes that can affect diagnostics in
other open docs are:
- A variable name gets added or removed.
- A document that defines a variable is deleted or closed without reloading.

Edits that only change variable values do not affect the "undefined" warning
diagnostics, so we can skip global revalidation in those cases.

## Design
1. Before parsing an edited document, read the previous variable names for that
   document from the manager.
2. Parse the document (which clears and re-adds its variables).
3. Read the new variable names.
4. If `old_names == new_names`, skip revalidating other open documents.
5. If they differ, revalidate all open documents (optionally debounced).

This can be implemented entirely in `did_open` and `did_change` by wrapping the
existing parse/validate flow.

## Implementation sketch
1. Add a helper to collect a `HashSet<String>` of variable names for a document
   using `CssVariableManager::get_document_variables`.
2. In `did_open` and `did_change`:
   - `old_names = get_doc_var_names(uri)`
   - parse the document
   - `new_names = get_doc_var_names(uri)`
   - if `old_names != new_names`, revalidate all open docs (skip current doc)
3. Keep the existing per-document diagnostics for the edited document.
4. Optionally debounce the global revalidate to reduce churn on rapid edits.

## Complexity and cost
- Effort: low. Small diff tracking and conditional revalidation.
- Runtime: avoids revalidating all open docs on every keystroke, but still
  revalidates globally when definitions change.
- Memory: negligible.

## Risks and edge cases
- If parsing fails and the manager removes the document's definitions, it should
  be treated as a definition change (triggering revalidation).
- If future diagnostics depend on variable values (not just names), this
  optimization would need to be revisited.
- This does not reduce cost when a definition changes frequently (e.g., when
  editing the variable name itself), but still helps for most edits.

## Testing ideas
- Edit a variable value only; verify no revalidation of other open docs.
- Add/remove a variable name; verify all open docs revalidate and warnings
  update.
- Rename a variable; verify warnings shift from old name to new name across
  open docs.

## References
- TS implementation that revalidates all open docs with a debounce (baseline to
  compare against): https://github.com/lmn451/css-lsp/commit/d0cd11a3afae4a96c4288db8e97e630b2e341be9
