# Option 5: Pull diagnostics + diagnostic refresh

## Summary
Switch to the LSP pull diagnostics model so the client requests diagnostics
(`textDocument/diagnostic`) and the server can trigger a refresh when inter-file
dependencies change (`workspace/diagnostic/refresh`). This replaces or
supplements the current push-based `publishDiagnostics` flow.

## Why this helps
With pull diagnostics, the server can notify the client that diagnostics might
be stale, and the client re-requests them. This is a natural fit for cases
where a change in one document affects diagnostics in other documents.

## Spec requirements (LSP 3.17)
- The server advertises `diagnosticProvider` capabilities with options such as
  `interFileDependencies` (true if diagnostics can be affected by other files)
  and `workspaceDiagnostics` (true only if you support workspace-wide pull).
- The server implements `textDocument/diagnostic`.
- If the client supports it, the server can request a refresh via
  `workspace/diagnostic/refresh`.

## tower-lsp support
`tower-lsp` exposes the pull diagnostics entry points on `LanguageServer` and
provides a client helper for refresh. This means the Rust server can implement
pull diagnostics without adding a custom JSON-RPC layer.

## Implementation sketch
1. **Capabilities**
   - Add `diagnostic_provider` to `ServerCapabilities`.
   - Set `inter_file_dependencies = true`.
   - Set `workspace_diagnostics = false` unless you want to implement
     workspace-wide diagnostics.
2. **Handlers**
   - Implement `LanguageServer::diagnostic` to return diagnostics for a given
     document. Reuse the existing validation logic and return a
     `DocumentDiagnosticReportResult` (usually a full report).
   - Optionally implement `LanguageServer::workspace_diagnostic` if you want to
     support full workspace diagnostics.
3. **Refresh**
   - When a document's definitions change, call
     `client.workspace_diagnostic_refresh()` (only if the client declares
     refresh support).
4. **Push vs pull**
   - Decide whether to disable push diagnostics entirely when pull is available.
     Some clients will prefer pull when the capability is advertised.
5. **Result caching (optional)**
   - Use `resultId` to return `Unchanged` reports for documents that have not
     changed since the last diagnostic request.

## Complexity and cost
- Effort: high. This is a protocol shift and requires new handlers, capability
  negotiation, and potentially new state (e.g., `resultId` tracking).
- Runtime: potentially more efficient for large workspaces, depending on the
  client and how often it pulls.
- Risk: higher, because client support varies and pull diagnostics are more
  complex to implement correctly.

## Risks and edge cases
- Client support: if the editor does not support pull diagnostics, you must
  keep push diagnostics enabled.
- Inter-file dependencies: failing to set `interFileDependencies` can cause
  stale diagnostics.
- Result IDs and incremental reports: optional but recommended for performance.
- Concurrency: diagnostic requests can arrive while parsing or while the manager
  is updating; guard with appropriate locks and/or snapshots.

## Testing ideas
- Verify `textDocument/diagnostic` returns the same warnings as current
  push-based diagnostics.
- Change a definition in one doc, trigger
  `workspace/diagnostic/refresh`, and confirm the client re-requests diagnostics
  for affected docs.
- Test a client that does not support pull diagnostics to ensure push still
  works.

## References
- LSP 3.17 diagnostics specification (diagnostic requests, options, and refresh):
  https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnostic
  https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_diagnostic_refresh
  https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnosticoptions
- tower-lsp `LanguageServer` pull diagnostics hooks:
  https://docs.rs/tower-lsp/latest/tower_lsp/trait.LanguageServer.html
- tower-lsp client refresh helper:
  https://docs.rs/tower-lsp/latest/tower_lsp/struct.Client.html
