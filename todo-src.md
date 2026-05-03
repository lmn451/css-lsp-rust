# LSP 3.17 Coverage TODO

Reference: LSP 3.17 specification.

## Implemented (since initial audit)

1. ~~Document sync: `willSave`, `willSaveWaitUntil`, `didSave`.~~ — stubbed in v0.2.1.
2. ~~Code action (and resolve).~~ — `code_action()` handler with quickfixes for undefined variables and literal color replacements.
3. ~~Prepare rename.~~ — `prepare_rename()` handler validates rename targets.
4. ~~File operations (`will/didCreate`, `will/didRename`, `will/didDelete`).~~ — `did_create_files`, `did_rename_files`, `did_delete_files` handlers.
5. ~~Workspace configuration (`workspace/configuration`, `didChangeConfiguration`).~~ — `did_change_configuration()` handler for runtime config updates.

## Missing Features (Not Implemented)

1. Declaration.
2. Type definition.
3. Implementation.
4. Call hierarchy (prepare/incoming/outgoing).
5. Type hierarchy (prepare/supertypes/subtypes).
6. Document highlight.
7. Document link (and resolve).
8. Code lens (and refresh).
9. Folding range.
10. Selection range.
11. Semantic tokens.
12. Inline value (and refresh).
13. Inlay hint (resolve/refresh).
14. Moniker.
15. Pull diagnostics provider (new diagnostic flow).
16. Signature help.
17. Formatting (document, range, on-type).
18. Linked editing range.
19. Workspace symbol resolve.
20. Execute command.

## Capability / Behavior Gaps

1. ~~Workspace folder change notifications are not advertised~~ — Fixed: `change_notifications` is now set, and `did_change_workspace_folders` handler is active.
