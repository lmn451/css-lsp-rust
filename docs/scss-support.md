───────────────────────────────────────────────────────────────────────── │
│ ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓ │
│ ┃               SCSS Variable Support Implementation Plan               ┃ │
│ ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛ │
│                                                                           │
│                                                                           │
│                                 Overview                                  │
│                                                                           │
│ Add support for SCSS $variable syntax alongside existing CSS custom       │
│ properties (--var). This will provide completion, hover,                  │
│ go-to-definition, find-references, rename, and diagnostics for SCSS       │
│ variables.                                                                │
│                                                                           │
│                                                                           │
│                           Architecture Decision                           │
│                                                                           │
│ Extend existing types rather than creating parallel structures:           │
│                                                                           │
│  • Reuses existing manager infrastructure                                 │
│  • LSP handlers work uniformly with both variable types                   │
│  • Less code duplication                                                  │
│  • Backward compatible                                                    │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                  Phase 1: Core Infrastructure (types.rs)                  │
│                                                                           │
│                         1.1 Add VariableKind enum                         │
│                                                                           │
│                                                                           │
│  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,      │
│  Default)]                                                                │
│  pub enum VariableKind {                                                  │
│      #[default]                                                           │
│      Css,   // --variable                                                 │
│      Scss,  // $variable                                                  │
│  }                                                                        │
│                                                                           │
│                                                                           │
│                       1.2 Extend CssVariable struct                       │
│                                                                           │
│ Add new fields:                                                           │
│                                                                           │
│  • kind: VariableKind - Css or Scss                                       │
│  • is_default: bool - SCSS !default flag                                  │
│  • is_global: bool - SCSS !global flag                                    │
│                                                                           │
│                    1.3 Extend CssVariableUsage struct                     │
│                                                                           │
│ Add:                                                                      │
│                                                                           │
│  • kind: VariableKind - Css or Scss                                       │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                Phase 2: SCSS Parser (src/parsers/scss.rs)                 │
│                                                                           │
│                     2.1 Create new SCSS parser module                     │
│                                                                           │
│  • Parse $variable-name: value; definitions                               │
│  • Parse $variable usages in property values                              │
│  • Handle // single-line comments (SCSS-specific)                         │
│  • Handle /* */ block comments (shared with CSS)                          │
│  • Track !default and !global flags                                       │
│                                                                           │
│                   2.2 Scope handling (MVP: global-only)                   │
│                                                                           │
│  • For initial implementation, treat all SCSS variables as global         │
│  • Future enhancement: track block depth for local scope                  │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│               Phase 3: LSP Integration (src/lsp_server.rs)                │
│                                                                           │
│                           3.1 Update completion                           │
│                                                                           │
│  • Add $ to trigger characters                                            │
│  • Detect $ prefix for SCSS variable completions                          │
│                                                                           │
│                         3.2 Update word detection                         │
│                                                                           │
│  • Add detection for $variable-name pattern                               │
│                                                                           │
│              3.3 Update hover, goto-def, references, rename               │
│                                                                           │
│  • Work with both -- and $ prefixed variables                             │
│                                                                           │
│                          3.4 Update diagnostics                           │
│                                                                           │
│  • Warn on undefined $variable usages                                     │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                     Phase 4: Testing & Documentation                      │
│                                                                           │
│  • Add unit tests for SCSS parsing                                        │
│  • Add integration tests for SCSS LSP features                            │
│  • Update README.md and CHANGELOG.md                                      │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                          Files to Create/Modify                           │
│                                                                           │
│                                                                           │
│   File                        Action                                      │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│   src/types.rs                Modify - Add VariableKind, extend structs   │
│   src/parsers/scss.rs         Create - New SCSS parser                    │
│   src/parsers/mod.rs          Modify - Export SCSS parser                 │
│   src/lsp_server.rs           Modify - Update completions, triggers,      │
│                               word detection                              │
│   src/workspace.rs            Modify - Call SCSS parser for .scss/.sass   │
│                               files                                       │
│   tests/integration_test.rs   Modify - Add SCSS tests                     │
│                                                                           │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                              MVP Limitations                              │
│                                                                           │
│  1 Global scope only - All $variables treated as global                   │
│  2 No mixin/function scope - Variables inside mixins not specially        │
│    handled                                                                │
│  3 No @import/@use resolution - Won't follow imports                      │
│                                                                           │
│ ───────────────────────────────────────────────────────────────────────── │
│                                                                           │
│                       Estimated Effort: ~3-4 hours                        │
│                                                                           │
│ Does this plan look good? Would you like me to:                           │
│                                                                           │
│  1 Start implementing as planned?                                         │
│  2 Adjust the scope (e.g., skip certain phases)?                          │
│  3 Add more features (e.g., proper scope tracking)?                       │
╰───────────────────────────────────────────────────────────────────────────╯
