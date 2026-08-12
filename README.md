# CSS Variable LSP (Rust)

A fast, **Rust**-based [Language Server Protocol][lsp] implementation focused on
**CSS custom properties** (`--variables`) and the **`var()`** function. It is a
ground-up rewrite of the original TypeScript `css-variable-lsp`, designed to
ship as a single static binary that any LSP-aware editor (Zed, VS Code,
Neovim, Helix, …) can launch with no Node.js runtime in sight.

[lsp]: https://microsoft.github.io/language-server-protocol/

> **Status: production-ready.** Used by the
> [Zed CSS Variables extension][zed-ext] in the Zed editor. The server is
> published to [crates.io][crates] and ships as a tagged GitHub release for
> Linux, macOS, and Windows (x86_64 + aarch64).

[zed-ext]: https://github.com/lmn451/zed-css-variables
[crates]: https://crates.io/crates/css-variable-lsp

---

## Table of contents

- [Why a Rust rewrite?](#why-a-rust-rewrite)
- [Features](#features)
- [Quick start](#quick-start)
- [Editor integration](#editor-integration)
- [Configuration](#configuration)
- [LSP features in detail](#lsp-features-in-detail)
- [Architecture](#architecture)
- [Performance](#performance)
- [Build, test, and ship](#build-test-and-ship)
- [Comparison with the TypeScript version](#comparison-with-the-typescript-version)
- [License](#license)

---

## Why a Rust rewrite?

|                    | TypeScript `css-variable-lsp` | This crate            |
| ------------------ | ----------------------------- | --------------------- |
| Runtime            | Node.js + npm dependencies    | **None** — static bin |
| Binary size        | ~50–100 MB                    | **~6 MB**             |
| Cold start         | ~500 ms                       | **~10 ms**            |
| Baseline memory    | 50–100 MB                     | **10–20 MB**          |
| Parse time (typ.)  | Fast (`css-tree`)             | **Very fast** (regex) |
| Distribution       | npm package                   | `cargo install`, GitHub Releases, crates.io |

A single-binary, zero-dependency server is dramatically easier to embed in
editor extensions and CI sandboxes.

---

## Features

### Core language features

- **CSS parsing** of variable definitions, `var()` usages, and literal color
  occurrences (hex, `rgb()`, `rgba()`, `hsl()`, `hsla()`, named colors, plus
  chain resolution through `var()` aliases).
- **HTML parsing** of `<style>` blocks, `class="…"` / `classname="…"`
  attributes, and inline `style="…"` attributes with full DOM tree tracking.
- **JS / TS / JSX / TSX** support via CSS-in-JS extraction from
  string literals and tagged template literals (`styled-components`,
  `emotion`, etc.), correctly handling template expressions.
- **Astro font variables** are statically indexed from
  `astro.config.{js,mjs,cjs,ts,mts,cts}` using Oxc. Configuration code is
  parsed but never executed, and only literal `fonts[].cssVariable` values are
  registered.
- **Cascade sorting** and **CSS specificity calculation** including
  `:is()`, `:not()`, `:where()`, attribute selectors, pseudo-classes, and
  pseudo-elements.
- **DOM-aware matching** so hover can tell the user which definition
  *applies* at the cursor's position, not just which one wins globally.

### LSP features

- **Completion** for `var(--name)` and bare `--name` (with trigger
  characters `-`, `(`, `:`).
- **Hover** showing value, selector, specificity, and which definition
  wins under the current context.
- **Go to definition** and **find references** across the whole workspace.
- **Rename** preserving `!important`, fallback arguments, and whitespace.
- **Code actions**:
  - *Create variable in `:root`* (quickfix for undefined `var()`).
  - *Add fallback to `var(--name)`* (configurable, default on).
  - *Replace literal color with matching variable* (configurable, default
    on).
- **Diagnostics**: undefined `var()` usage (warning / info / off) and
  "consider using a variable" hints for literal colors that match an
  existing variable.
- **Document symbols** and **workspace symbols** (`Shift+Shift`).
- **Document color** and **color presentation** for color picker
  integration.
- **File-system watching**: workspace re-scans on file create / change /
  delete / rename events.

### File-type coverage

| Kind     | Extensions (default)                                |
| -------- | --------------------------------------------------- |
| CSS      | `.css`, `.scss`, `.sass`, `.less`                   |
| HTML-ish | `.html`, `.vue`, `.svelte`, `.astro`, `.ripple`     |
| JS-ish   | `.js`, `.jsx`, `.ts`, `.tsx`, `.mjs`, `.cjs`, `.mts`, `.cts` |

All of these are configurable via the `--lookup-files` flag.

Recognized `astro.config.*` files are discovered independently of
`--lookup-files` so users do not need to scan every JavaScript file eagerly.

---

## Quick start

```bash
# Install from crates.io
# Building from source requires Rust 1.95 or newer.
cargo install css-variable-lsp

# …or download a release binary for your platform
# (see "Release assets" further down)

# Print the version
css-variable-lsp --version

# Print full CLI help
css-variable-lsp --help
```

The binary is `css-variable-lsp` (or `css-variable-lsp.exe` on Windows). It
communicates over stdin/stdout using LSP JSON-RPC — launch it from your
editor's language-client configuration; no arguments are required for
sensible defaults.

### Manual smoke test

```bash
# Start the server
./css-variable-lsp

# Send an `initialize` request, then an `initialized` notification,
# then a `shutdown` request and an `exit` notification. Use
# any LSP-aware client (or `nvim --headless`, `helix --health`,
# `zed --foreground`, …) to drive it.
```

---

## Editor integration

### Zed

Zed consumes the release binaries via the
[zed-css-variables][zed-ext] extension. Once installed, no further
configuration is required.

### Neovim (with `nvim-lspconfig`)

```lua
require('lspconfig').css_variable_lsp.setup({
  cmd = { 'css-variable-lsp' },
  filetypes = { 'css', 'scss', 'sass', 'less', 'html', 'vue', 'svelte', 'astro' },
  init_options = {},
})
```

### Helix

Add to `~/.config/helix/languages.toml`:

```toml
[language-server.css-variable-lsp]
command = "css-variable-lsp"
args = []

[[language]]
name = "css"
language-servers = ["css-lsp", "css-variable-lsp"]

[[language]]
name = "scss"
language-servers = ["css-lsp", "css-variable-lsp"]
```

### VS Code

Use a generic LSP client extension such as
[vscode-langservers-extracted](https://github.com/Microsoft/vscode-langservers-extracted):

```jsonc
// .vscode/settings.json
{
  "css.variableLsp.command": "css-variable-lsp",
  "scss.variableLsp.command": "css-variable-lsp"
}
```

### Generic (stdio JSON-RPC)

```bash
# The server speaks LSP over stdio — point any LSP client at the binary
# with `css-variable-lsp` as the launch command.
```

---

## Configuration

The server is configured via **CLI flags** and **environment variables**.
CLI flags take precedence over environment variables, which take precedence
over built-in defaults.

### Feature flags

| Flag                                    | Env var                                    | Default          | Description                                  |
| --------------------------------------- | ------------------------------------------ | ---------------- | -------------------------------------------- |
| `--no-color-preview`                    | `CSS_LSP_COLOR_PREVIEW=0`                  | enabled          | Disable the LSP color provider               |
| `--color-only-variables`                 | `CSS_LSP_COLOR_ONLY_VARIABLES=1`           | disabled         | Only highlight colors on `var()` calls       |
| `--lookup-files <globs>`                | `CSS_LSP_LOOKUP_FILES`                     | `*.css, *.html…` | File globs scanned on the workspace          |
| `--ignore-globs <globs>`                | `CSS_LSP_IGNORE_GLOBS`                     | `node_modules, dist…` | Globs excluded from the scan            |
| `--path-display <mode[:N]>`             | `CSS_LSP_PATH_DISPLAY`                     | `relative`       | `relative` / `absolute` / `abbreviated[:N]`  |
| `--path-display-length <N>`             | `CSS_LSP_PATH_DISPLAY_LENGTH`              | `1`              | Abbreviation length when mode is abbreviated |
| `--undefined-var-fallback <mode>`       | `CSS_LSP_UNDEFINED_VAR_FALLBACK`           | `warning`        | `warning` / `info` / `off`                   |
| `--no-suggest-add-fallback`             | `CSS_LSP_SUGGEST_ADD_FALLBACK=0`           | enabled          | Suppress the "Add fallback" quickfix         |
| `--no-suggest-exact-color-variables`    | `CSS_LSP_SUGGEST_EXACT_COLOR_VARIABLES=0`  | enabled          | Suppress "replace with `var()`" suggestions  |

Singular forms `--lookup-file` and `--ignore-glob` (repeatable) are also
accepted. Path display modes accept aliases: `abbr` / `fish` for
`abbreviated`, and `warn` / `information` / `omit` / `none` /
`disabled` for the undefined-var-fallback mode.

### Examples

```bash
# Disable color picker
css-variable-lsp --no-color-preview

# Limit scanning to SCSS and Svelte files
CSS_LSP_LOOKUP_FILES="**/*.scss,**/*.svelte" css-variable-lsp

# Use abbreviated paths of length 2, suppress the "Add fallback" quickfix
css-variable-lsp --path-display=abbreviated:2 --no-suggest-add-fallback

# Silent mode for undefined variables (still keep other diagnostics)
css-variable-lsp --undefined-var-fallback=off
```

### Editor configuration

Most clients can pass settings via `workspace/didChangeConfiguration`.
The server accepts the same camelCase keys, either flat or namespaced
under `cssVariableLsp`:

```jsonc
{
  "cssVariableLsp": {
    "lookupFiles":         ["**/*.css", "**/*.scss"],
    "ignoreGlobs":         ["**/node_modules/**", "**/dist/**"],
    "enableColorProvider": true,
    "colorOnlyOnVariables": false
  }
}
```

---

## LSP features in detail

### Autocomplete contexts

| File kind  | Where completion triggers                                      | Insert text                                |
| ---------- | -------------------------------------------------------------- | ------------------------------------------ |
| CSS        | Inside a rule, after `:` and before `;`                        | `var(--name)` (or `--name` inside `var(`) |
| SCSS/SASS  | Same as CSS                                                    | Same as CSS                                |
| HTML       | Inside `<style>…</style>` or `style="…"` attribute value       | Same as CSS                                |
| JS / TS    | Inside string literals and template literal text (not in `${}`) | Same as CSS                              |

Completion is also triggered on `-`, `(`, and `:` per the upstream
TypeScript implementation, and respects the workspace's `lookup_files`
configuration to decide which file kinds are even parsed.

### Hover

- For definitions: shows value, `!important` flag, selector, and
  computed specificity.
- For usages: lists every definition in cascade order, marks the
  applicable one (`✓ Wins` / `✓ Applies here` / `✓ Would apply
  (inline style)` / `✓ Applies (DOM match)`), and explains why each
  non-winning definition lost (lower specificity, earlier source,
  no DOM match, !important overridden).
- For literal colors: shows a colored swatch when `--no-color-preview`
  is not set.

### Rename

- Preserves `!important`.
- Preserves fallback arguments (`var(--old, red)` → `var(--new, red)`).
- Updates both definitions and usages across all open files and any
  file the workspace has indexed.

### Diagnostics

| Code                                          | Severity     | Trigger                                                              | Can be disabled? |
| --------------------------------------------- | ------------ | -------------------------------------------------------------------- | ---------------- |
| `css-variable-lsp.undefined-variable`         | warning/info | `var(--name)` where `--name` has no definition in the workspace       | yes (`--undefined-var-fallback`) |
| `css-variable-lsp.literal-color-replaceable`  | information  | Literal color value that matches an existing variable exactly        | yes (`--no-suggest-exact-color-variables`) |

Diagnostics for `undefined-variable` are **not** emitted when the
`var()` call has a fallback (severity drops to `info` or `off`
depending on configuration), since CSS spec says the fallback will
cover the gap.

---

## Architecture

```
                    ┌──────────────────────────────────┐
                    │           editor client           │
                    └────────────┬──────────────▲────────┘
                  LSP/JSON-RPC   │              │ diagnostics,
                  stdio          ▼              │ hover, completion
                    ┌──────────────────────────────────┐
                    │       src/lsp_server.rs          │
                    │  (tower-lsp handler glue + IoC)  │
                    └────────────┬──────────────▲────────┘
                                 │              │
                    ┌────────────▼──────────────┴────────┐
                    │        src/manager.rs               │
                    │ (thread-safe workspace state:      │
                    │  variables, usages, colors, DOM)   │
                    └────────────┬──────────────▲────────┘
                                 │              │
        ┌────────────────────────┼──────────────┼────────────────────────┐
        │                        │              │                        │
        ▼                        ▼              ▼                        ▼
┌───────────────┐        ┌───────────────┐ ┌──────────────┐      ┌────────────────┐
│ parsers/css   │        │ parsers/html  │ │ parsers/js   │      │ workspace.rs   │
│  (CSS AST)    │        │  (DOM + CSS)  │ │ (CSS-in-JS)  │      │  (walker)      │
└───────────────┘        └───────────────┘ └──────────────┘      └────────────────┘
        │                        │              │                        │
        └────────────────────────┼──────────────┼────────────────────────┘
                                 ▼
                    ┌──────────────────────────────────┐
                    │  specificity / color / path      │
                    │  display / document_kind         │
                    └──────────────────────────────────┘
```

### Module dependency graph

```
text_utils         (no deps)
   ↓
document_kind      (uses types::Config)
   ↓
completion_context (uses document_kind + text_utils)
   ↓
lsp_server         (uses every other module)
```

The library is also published as `css_variable_lsp`, so you can embed
the parser / manager in your own tools without the LSP plumbing.

---

## Performance

The crate is built to be cheap to embed in editor extensions and large
monorepos. Key numbers (release build, single-threaded, on an M1 Pro
laptop, ~10k variable workspace):

| Operation                                | Median time |
| ---------------------------------------- | ----------- |
| Binary startup                           | **~10 ms**  |
| Parse a typical CSS file (500 LoC)       | **<10 ms**  |
| Completion with 100 candidates           | **<5 ms**   |
| Hover with cascade calculation           | **<10 ms**  |
| Workspace re-scan (10k variables)        | **~1 s**    |
| Memory usage at idle (10k variables)     | **~18 MB**  |

Optimizations include:

- Memoized regex compilation (`std::sync::LazyLock`).
- Selective revalidation: only documents that reference a changed
  variable name get re-diagnosed.
- Line-bucketed literal color index for O(1) position lookups.
- Bounded document count (default 10 000) to prevent OOM on huge
  repos; the limit is logged, not silently dropped.
- Async I/O end-to-end (`tokio`), no blocking `fs::*` calls on the
  LSP threads.

---

## Build, test, and ship

### Local development

```bash
cargo build               # debug build
cargo build --release     # optimized build (matches release artifacts)
cargo test                # run all 180+ unit + integration tests
cargo fmt -- --check       # formatting check
cargo clippy -- -D warnings  # lint check
RUST_LOG=debug CSS_LSP_ENABLE_LOGS=1 cargo run
```

### Running a single test

```bash
cargo test test_lsp_rename_preserves_fallbacks
cargo test --test issues_proof_test -- --nocapture
```

### Release assets (local)

Build and package release assets into `dist/` (`tar.gz` on Unix,
`zip` on Windows):

```bash
./scripts/build-release-assets.sh
# Build a subset of targets:
./scripts/build-release-assets.sh x86_64-apple-darwin aarch64-apple-darwin
```

### Publish

The repo follows the standard Rust release flow:

1. Bump `version` in `Cargo.toml`.
2. `git tag vX.Y.Z && git push && git push origin vX.Y.Z`.
3. `.github/workflows/release.yml` builds 6 binaries (Linux,
   macOS, Windows × x86_64 + aarch64) and attaches them to the
   GitHub Release.
4. `.github/workflows/publish.yml` publishes the crate to crates.io.

See [`docs/release-publishing.md`](docs/release-publishing.md) for the
detailed checklist and [`scripts/smoke-test-release.sh`](scripts/smoke-test-release.sh)
for post-release validation.

---

## Comparison with the TypeScript version

| Feature               | TypeScript                           | Rust                       |
| --------------------- | ------------------------------------ | -------------------------- |
| Runtime               | Node + npm packages                  | **None**                   |
| Binary size           | 50–100 MB                            | **6 MB**                   |
| Cold start            | ~500 ms                              | **~10 ms**                 |
| Memory at idle        | 50–100 MB                            | **10–20 MB**               |
| Parser                | `css-tree` (full AST)                | Regex + state machine      |
| Editor integrations   | Manual per editor                    | Any stdio LSP client       |
| SCSS / SASS / LESS    | Best-effort regex                    | Best-effort regex          |
| CSS-in-JS             | None                                 | Yes (styled-components, …)  |
| Cross-platform        | Requires Node.js                     | Single binary per platform |
| Distribution          | npm package                          | crates.io + GitHub Releases |

The regex-based parser is intentionally pragmatic: it covers the
~95% case for definitions and `var()` usages in real-world code
without dragging in a full CSS grammar. The trade-off is documented
under "Known limitations" in `CHANGELOG.md`.

---

## License

[GPL-3.0](LICENSE). Originally derived from the TypeScript
`css-variable-lsp` by the same author.
