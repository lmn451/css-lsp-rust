# LSP 3.17 Coverage TODO

Reference: LSP 3.17 specification.

## Missing Features (Not Implemented)
1. Document sync: `willSave`, `willSaveWaitUntil`, `didSave`.
2. Declaration.
3. Type definition.
4. Implementation.
5. Call hierarchy (prepare/incoming/outgoing).
6. Type hierarchy (prepare/supertypes/subtypes).
7. Document highlight.
8. Document link (and resolve).
9. Code lens (and refresh).
10. Folding range.
11. Selection range.
12. Semantic tokens.
13. Inline value (and refresh).
14. Inlay hint (resolve/refresh).
15. Moniker.
16. Pull diagnostics provider (new diagnostic flow).
17. Signature help.
18. Code action (and resolve).
19. Formatting (document, range, on-type).
20. Prepare rename.
21. Linked editing range.
22. Workspace symbol resolve.
23. Workspace configuration (`workspace/configuration`, `didChangeConfiguration`).
24. File operations (`will/didCreate`, `will/didRename`, `will/didDelete`).
25. Execute command.

## Capability / Behavior Gaps
1. Workspace folder change notifications are not advertised (`change_notifications` is `None`), so clients may not send `workspace/didChangeWorkspaceFolders` even though a handler exists.
