# Oxc-Based JavaScript and TypeScript Configuration Analysis Plan

## Status

Phases 0 through 2 are implemented for Astro font variables. The first Phase 3
slice is implemented as a stacked change for static CSS custom properties in
Vite's `css.preprocessorOptions.scss.additionalData`. Other Vite and framework
extractors remain future, use-case-driven work.

The initial macOS aarch64 release build measured 8,039,360 bytes with Oxc versus 7,176,352 bytes on `master`, an increase of 863,008 bytes, or approximately 12%. This result is acceptable for the initial implementation but should continue to be tracked in release review.

## Motivation

The language server currently understands CSS, HTML-like documents, and CSS-like strings in open JavaScript or TypeScript documents. It does not statically understand framework configuration files that generate CSS custom properties or otherwise affect CSS authoring.

The immediate case is Astro's font configuration:

```ts
export default defineConfig({
    fonts: [
        {
            name: "Roboto",
            cssVariable: "--font-roboto",
        },
    ],
});
```

Astro generates `--font-roboto`, but the language server reports `var(--font-roboto)` as undefined because `astro.config.*` is not indexed as a definition source.

A JavaScript and TypeScript AST layer can solve this case while creating a controlled foundation for other configuration-derived CSS information. The server must never execute user configuration code.

## Decision summary

Use Oxc for tolerant, static JavaScript and TypeScript parsing behind an internal configuration-analysis abstraction.

Start with Astro font variables. Do not begin with a generic evaluator and do not attempt to reproduce Astro, Vite, or Node module execution. Add framework extractors incrementally, accepting only syntax whose value can be proven from the local AST.

Oxc is preferred over SWC for this project because:

- the requirement is parsing and read-only AST traversal, not transformation or code generation;
- Oxc exposes a focused parser and AST crate family suitable for analysis tooling;
- Oxc documents JavaScript, JSX, TypeScript, and TSX support and direct ESM information;
- Oxc reports materially higher parser throughput than SWC in its published benchmark;
- its arena-allocated AST is a good fit for parse, extract, and discard processing;
- it is already used as infrastructure by modern tools including Rolldown, which is relevant to the Vite ecosystem;
- its MIT license is simple to redistribute in this GPL-3.0 project.

This is not a claim that SWC is unsuitable. SWC is mature, heavily tested, supports error recovery, and has a much longer production history. It remains the fallback if an Oxc prototype fails the acceptance gates below.

## Goals

1. Index statically declared Astro font CSS variables from `astro.config.*`.
2. Associate generated definitions with accurate ranges in the configuration file.
3. Make those definitions available to completion, diagnostics, hover, workspace symbols, references, and navigation through the existing manager.
4. Update definitions when configuration files are opened, edited, saved, renamed, deleted, or changed on disk.
5. Introduce a reusable framework-extractor architecture without executing configuration code.
6. Bound CPU, memory, dependency, binary-size, and MSRV impact.
7. Preserve useful results from a partially invalid file when Oxc can recover enough syntax.

## Non-goals

- Executing `astro.config.*`, `vite.config.*`, plugins, imports, or arbitrary functions.
- Reproducing Node, Astro, Vite, or bundler module resolution.
- Evaluating environment variables, filesystem reads, promises, network calls, or plugin hooks.
- Guaranteeing extraction from every legal dynamic configuration.
- Replacing the existing CSS-in-JS snippet extractor.
- Adding general JavaScript or TypeScript language-server features.
- Inferring runtime CSS from arbitrary application code.

## Supported files

Framework configuration discovery must be based on exact recognized basenames, not on eagerly parsing every JavaScript file.

Initial Astro set:

- `astro.config.js`
- `astro.config.mjs`
- `astro.config.cjs`
- `astro.config.ts`
- `astro.config.mts`
- `astro.config.cts`

Vite set:

- `vite.config.js`
- `vite.config.mjs`
- `vite.config.cjs`
- `vite.config.ts`
- `vite.config.mts`
- `vite.config.cts`

Future extractors may register additional exact names or narrow globs. Framework configuration discovery should remain separate from `lookup_files`, which controls ordinary CSS-bearing documents.

## Static-value policy

Every extractor classifies a candidate as one of:

- `Known`: proven from local syntax and safe to index.
- `Unknown`: dynamic or unsupported, ignored without inventing a value.
- `Invalid`: malformed for the framework contract, ignored and optionally logged at debug level.

Initially accepted string forms:

```ts
cssVariable: "--font-roboto"
cssVariable: '--font-roboto'
cssVariable: `--font-roboto`
```

A template literal is accepted only when it contains no substitutions. Escapes are decoded only if Oxc provides an unambiguous cooked value. The source range continues to refer to the original literal contents.

Initially rejected forms:

```ts
cssVariable: FONT_VARIABLE
cssVariable: prefix + "roboto"
cssVariable: `--font-${family}`
cssVariable: getVariableName()
cssVariable: process.env.FONT_VARIABLE
cssVariable: { toString() { return "--font-roboto" } }
```

A later constant folder may support a deliberately small subset such as `const` bindings and string concatenation. That must be a separate phase with cycle detection, operation limits, and dedicated tests.

## Proposed architecture

### Modules

```text
src/config_analysis/
    mod.rs
    discovery.rs
    parser.rs
    static_value.rs
    types.rs
    extractors/
        mod.rs
        astro.rs
        vite.rs
```

### Core types

```rust
pub struct GeneratedCssVariable {
    pub name: String,
    pub value: Option<String>,
    pub source_range: Range,
    pub name_range: Range,
    pub framework: ConfigFramework,
    pub reason: GeneratedDefinitionReason,
}

pub enum ConfigFramework {
    Astro,
    Vite,
}

pub enum GeneratedDefinitionReason {
    AstroFont,
    ViteDefine,
    PluginDeclaredVariable,
}
```

The exact representation can change during implementation. Framework-specific details should not leak into `CssVariableManager`.

### Extractor interface

```rust
pub trait ConfigExtractor {
    fn matches_path(&self, path: &Path) -> bool;

    fn extract(
        &self,
        parsed: &ParsedConfig<'_>,
        output: &mut Vec<GeneratedCssVariable>,
    );
}
```

A registry selects extractors by exact basename before parsing. Multiple extractors may inspect one file only when explicitly intended.

### Oxc parsing boundary

The Oxc allocator and AST are lifetime-bound. Keep them inside one synchronous parse-and-extract operation:

```text
source text
  -> choose SourceType from extension
  -> create Oxc allocator
  -> parse Program
  -> run selected extractor visitors
  -> convert results to owned project types
  -> drop AST and allocator
```

Do not store Oxc AST nodes in `CssVariableManager` or across `.await` points. Extract owned strings and LSP-compatible offsets before returning.

Configuration files are normally small. Begin by parsing inline, measure latency, and move parsing to `tokio::task::spawn_blocking` if profiling shows editor stalls. Establish a file-size limit before parsing, with a proposed default of 1 MiB for recognized config files.

### Dependencies

Prototype with narrowly selected, version-aligned Oxc crates rather than the umbrella crate:

- `oxc_allocator`
- `oxc_ast`
- `oxc_parser`
- `oxc_span`
- optionally `oxc_ast_visit` if a visitor reduces handwritten traversal

Disable unnecessary default features where possible. Pin all Oxc crates to the same exact minor version because the project publishes frequent coordinated releases.

At the time this plan was written, `oxc_parser 0.144.0` declares Rust 1.95.0 and uses Rust edition 2024. The local toolchain is Rust 1.97.1. Before merging the dependency, the project must decide and document an MSRV. If supporting an older compiler is required, select the newest compatible Oxc release and test it in CI.

## Phase 0: dependency and feasibility spike

Create a short-lived prototype before changing product architecture.

1. Add the minimum Oxc crates on an experiment commit.
2. Parse representative `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, and `.cts` files.
3. Confirm byte spans for object property keys and string literal contents.
4. Confirm useful AST output with incomplete objects, missing delimiters, comments, decorators, `satisfies`, and common TypeScript syntax.
5. Measure:
   - clean build time;
   - incremental build time;
   - release binary size;
   - parse time for small, medium, and 1 MiB configuration files;
   - peak allocations for repeated workspace scans.
6. Run all release targets or at minimum check Linux, macOS, and Windows cross-build workflows.
7. Record the compatible Oxc version and MSRV.

Exit criteria:

- source ranges map correctly to LSP positions, including UTF-8 and CRLF;
- malformed input does not panic;
- release binary growth is accepted explicitly;
- workspace initialization remains within the agreed performance budget;
- all supported release targets compile.

If the spike fails, compare the same fixture set with SWC before falling back to a dedicated lexer.

## Phase 1: shared configuration-analysis framework

1. Add recognized-config discovery independent of `lookup_files`.
2. Add path-to-framework matching by exact basename and supported extension.
3. Add Oxc `SourceType` selection from the file extension.
4. Add owned extraction result types.
5. Add offset-to-LSP-range conversion tests for ASCII, Unicode, CRLF, and escaped literals.
6. Integrate configuration parsing with:
   - initial workspace scans;
   - `didOpen` and `didChange`;
   - watched file creation and changes;
   - file rename and deletion;
   - workspace folder addition and removal.
7. Reuse `manager.remove_document()` before replacement so stale generated definitions disappear.
8. Revalidate affected open documents after the set of generated variable names changes.

The existing JS CSS-snippet parser remains active for ordinary open JS and TS documents. Recognized framework config files use both relevant paths only if doing so cannot duplicate definitions.

## Phase 2: Astro font extraction

### Recognized structures

Support the current form:

```ts
export default defineConfig({
    fonts: [{ cssVariable: "--font-roboto" }],
});
```

Support a direct default object:

```ts
export default {
    fonts: [{ cssVariable: "--font-roboto" }],
};
```

Support legacy experimental placement where practical:

```ts
export default defineConfig({
    experimental: {
        fonts: [{ cssVariable: "--font-roboto" }],
    },
});
```

Support multiple font entries and both quoted and identifier property keys:

```ts
{
    "fonts": [
        { "cssVariable": "--font-body" },
        { cssVariable: '--font-heading' },
    ],
}
```

### Structural strictness

The extractor should first identify a default-exported configuration object or an object passed to a recognized `defineConfig` call. It must not index every object property named `cssVariable` in the file.

Import aliases introduce ambiguity:

```ts
import { defineConfig as astroConfig } from "astro/config";
export default astroConfig({ ... });
```

Phase 2 should support aliases only when they can be proven from a direct import declaration. Do not resolve re-exports or imported wrapper functions.

### Registration semantics

Register each generated font variable as a synthetic global definition:

- `name`: configured CSS variable name;
- `value`: descriptive non-color value such as `"Astro generated font"`, or an empty value if UI behavior is cleaner;
- `uri`: Astro config URI;
- `range`: string literal or containing property range;
- `name_range`: literal content range, excluding quotes when safely representable;
- `selector`: `:root`;
- `important`: `false`;
- `inline`: `false`;
- `source_position`: property start offset.

The hover text should identify the definition as generated by Astro rather than pretending the config literal is a CSS declaration. If `CssVariable` cannot express that distinction cleanly, add provenance metadata instead of encoding it into `value` or `selector`.

### Astro acceptance tests

Use the real LSP service for end-to-end tests:

1. `.mjs` config removes the undefined diagnostic from a CSS consumer.
2. `.ts` config removes the undefined diagnostic.
3. The variable appears in completion.
4. Workspace symbol and goto-definition point to the config literal.
5. Multiple font entries are indexed exactly once.
6. Changing a literal revalidates consumers of the old and new names.
7. Removing an entry restores the undefined diagnostic.
8. Deleting or renaming the config removes stale definitions.
9. Dynamic values are not indexed.
10. Fake declarations in comments and unrelated objects are not indexed.
11. Aliased `defineConfig` imports work when directly provable.
12. Legacy `experimental.fonts` works if retained in scope.
13. Invalid or incomplete config content does not panic or erase unrelated workspace definitions.

## Phase 3: Vite extraction

Vite support must be use-case driven. Parsing `vite.config.*` is not itself valuable unless an extractor produces CSS-language information.

### Static preprocessor `additionalData`

Vite documents
[`css.preprocessorOptions[extension].additionalData`](https://vite.dev/config/shared-options.html#css-preprocessoroptions-extension-additionaldata)
as code prepended to each stylesheet handled by that preprocessor. A static
string can therefore contain real CSS custom-property declarations:

```ts
export default defineConfig({
    css: {
        preprocessorOptions: {
            scss: {
                additionalData: `:root { --brand-color: #123456; }`,
            },
        },
    },
});
```

The initial Vite extractor accepts only direct string literals and
no-substitution template literals under `scss`. It parses those snippets
through the existing CSS parser so definitions and `var()` usages preserve
their source ranges in `vite.config.*`. Completion, diagnostics, references,
symbols, and navigation then use the same workspace indexes as ordinary CSS.
Function-valued `additionalData`, escaped literals, non-SCSS preprocessors, and
unrelated properties are ignored.

SCSS control-flow and reusable-code directives such as `@if`, `@for`,
`@mixin`, and `@include` cause the entire snippet to be ignored. This avoids
indexing declarations from branches or mixins that may never emit CSS. CRLF is
preserved by using the exact template source when it contains no escapes.

As with ordinary CSS files, the current manager is workspace-global rather
than import-graph-aware. A statically extracted SCSS definition is therefore
offered across CSS-like documents in the workspace.

### Initial candidates

#### `define` constants

Vite's `define` option replaces identifiers with static values. Some projects use it to expose design-token names or CSS-variable strings:

```ts
export default defineConfig({
    define: {
        __BRAND_COLOR_VAR__: JSON.stringify("--brand-color"),
    },
});
```

Do not automatically register every string in `define` as a CSS variable. A safe first implementation may expose recognized `--*` literal values as completion/navigation aliases only when a concrete CSS-variable use case and UI behavior are defined.

#### Plugin-declared configuration

Known first-party or project-specific Vite plugins may have options that declare generated CSS variables. Add support as separate extractors keyed by a proven imported plugin binding and a documented option schema. Never interpret arbitrary plugin options heuristically.

#### Aliases and CSS preprocessing

Vite aliases, CSS Modules settings, and ordinary preprocessor options can
influence file resolution and class naming, but they do not directly define CSS
custom properties. Keep them out of scope until the language server has a
feature that consumes them. Static `additionalData` is the narrow exception
because Vite injects that source into processed stylesheets.

### Vite configuration forms

Vite supports object configs, `defineConfig(object)`, synchronous functions, and asynchronous functions. Initially analyze only direct objects and direct `defineConfig(object)` calls.

For function configs, the analyzer may later inspect unconditional returned object literals:

```ts
export default defineConfig(() => ({
    // statically inspectable
}));
```

Do not attempt control-flow evaluation of `command`, `mode`, environment variables, promises, or arbitrary branches in the first implementation.

## Phase 4: reusable static values

Only add a static-value engine after at least two extractors need it.

Potential supported operations:

- string, boolean, number, null, array, and object literals;
- parenthesized and TypeScript `as`/`satisfies` wrappers;
- no-substitution template literals;
- local immutable `const` references;
- object and array spreads from locally known constants;
- string concatenation where both operands are known;
- `JSON.stringify` of a known primitive, if an extractor explicitly requests it.

Required safeguards:

- maximum recursion depth;
- maximum visited nodes;
- cycle detection for bindings;
- no getters, calls, imports, computed runtime properties, or proxy behavior;
- deterministic `Unknown` propagation;
- no filesystem or environment access.

## Other future uses enabled by Oxc

Add these only when connected to a concrete CSS LSP feature:

1. **Framework-generated design tokens** from known configuration schemas.
2. **Tailwind configuration compatibility**, where older or plugin-specific configs declare CSS variable names. Tailwind v4 CSS-first configuration should continue to be parsed as CSS rather than JavaScript.
3. **CSS-in-JS improvements**, using AST structure to identify tagged templates and preserve exact ranges more reliably than lexical heuristics.
4. **Typed token exports**, for example a local `tokens.ts` file exporting proven `--*` strings, when explicitly configured as a token source.
5. **Known plugin schemas** for PostCSS, Vite, Astro, or framework integrations that generate custom properties.
6. **Import-aware `defineConfig` recognition**, proving that helpers come from the expected package rather than matching by identifier text.
7. **Configuration diagnostics**, such as invalid custom-property names, only after false-positive behavior is well understood.

Avoid turning the server into a general-purpose JavaScript indexer. Every extractor must declare recognized paths, syntax roots, output semantics, and acceptance tests.

## Why Oxc instead of SWC

### Comparison

| Criterion | Oxc | SWC | Project impact |
|---|---|---|---|
| Primary fit | Parser and analysis-oriented crate family within a modern JS toolchain | Mature compiler platform with parsing, transforms, minification, and code generation | This project needs parsing and traversal only |
| JS/TS support | JS, JSX, TS, and TSX | JS, JSX, TS, TSX, and additional established compiler workflows | Both satisfy syntax requirements |
| Error behavior | Reports parser errors while producing parser output where possible | Mature documented recovery for many syntax errors | Both require fixture-based validation for incomplete configs |
| Performance | Oxc publishes a benchmark claiming roughly 3x SWC parser throughput | Fast and production proven | Configs are small, so verify end-to-end impact rather than selecting on benchmark alone |
| AST allocation | Arena-based, designed for fast parse/analyze/discard workflows | `swc_common` source maps and owned AST ecosystem | Oxc aligns with an ephemeral extraction boundary |
| Dependency surface | Can select focused Oxc crates | Parser commonly involves `swc_common`, parser, AST, and optionally visitor crates | Must measure both with `cargo tree` and release builds |
| Ecosystem maturity | Newer, rapidly evolving, frequent coordinated releases | Older, broad adoption, extensive production history | Oxc requires exact version pinning and upgrade discipline |
| Vite ecosystem direction | Oxc is part of VoidZero; Rolldown uses Oxc and Vite uses Rolldown for config bundling by default | SWC remains widely used across JS tooling | Oxc offers strategic alignment, not functional necessity |
| License | MIT | Apache-2.0 | Both are compatible; neither decides the choice |
| Current latest metadata | `oxc_parser 0.144.0`, Rust 1.95 | `swc_ecma_parser 44.0.0`, MSRV not declared in crate metadata | Oxc forces an explicit MSRV decision |

### Why not choose SWC initially

1. The project does not need SWC transformations, code generation, hygiene, or compiler pipeline features.
2. Oxc's parse-and-discard arena model maps naturally to extracting a few owned definitions from small config files.
3. The Oxc crate split allows a focused parser/AST dependency set, subject to spike measurements.
4. Oxc's current focus on high-performance analysis tooling and its relationship to Rolldown make it a reasonable long-term base for Vite-adjacent static analysis.
5. Introducing a new AST layer is easier to justify when it has a narrow boundary. Oxc's parser API supports that boundary without adopting a broader compiler architecture.

### Reasons we might still choose SWC

Switch to SWC if the prototype demonstrates any of the following:

- materially better recovery on the incomplete configurations users actually edit;
- substantially lower supported MSRV requirements;
- smaller release binaries or build times with the required features;
- more stable visitor APIs for the nodes this project needs;
- a blocking Oxc parser or span correctness issue;
- unsupported release targets;
- an existing SWC-based dependency enters the project, making reuse cheaper than adding Oxc.

The final dependency decision must be based on the Phase 0 measurements, not only published parser benchmarks.

## Performance and resource controls

- Discover only recognized configuration basenames.
- Skip ignored directories using existing workspace ignore globs.
- Apply a configurable or internal maximum source size.
- Parse once per document version or disk content hash.
- Do not retain ASTs.
- Avoid reparsing unchanged config files during overlapping workspace scans.
- Track parse count, elapsed time, and ignored-oversize files under debug tracing.
- Consider a small content-hash cache of owned extraction results only after profiling.
- Preserve `max_documents` behavior for generated-definition sources.

## Security model

- Never execute source text.
- Never invoke Node, Astro, Vite, package scripts, or plugin hooks.
- Never resolve or load arbitrary imports during initial phases.
- Never read files referenced by config values unless a future feature defines a separately reviewed safe resolver.
- Bound file size, AST traversal, and static-value recursion.
- Treat parser failures as recoverable and avoid logging source content.
- Validate extracted names as CSS custom-property names before registration.
- Associate all generated results with their actual source URI and range.

## Failure handling

- Unsupported dynamic syntax produces no definition, not a guessed definition.
- A parser error must not clear previously valid state until replacement semantics are defined. For open documents, prefer replacing with the successfully extracted subset from the current version. For catastrophic parse failure, decide whether stale state or empty state is less surprising and cover that choice with tests.
- One malformed config must not abort workspace scanning.
- Duplicate declarations in one config should preserve separate source locations while completion deduplicates names through existing behavior.
- Definitions from multiple workspace roots remain isolated by URI and workspace lifecycle.

## Packaging and compatibility gates

Before merging Oxc:

1. Add and document `rust-version` in `Cargo.toml`.
2. Add an MSRV CI job.
3. Run `cargo fmt --check`.
4. Run strict Clippy with warnings denied.
5. Run `cargo test --all-features`.
6. Build release binaries for all supported targets.
7. Compare release asset sizes against the prior release.
8. Record clean and incremental build-time changes.
9. Run `cargo deny` or the project's chosen license/advisory check if introduced.
10. Confirm crates.io packaging includes required license attribution and no fixtures unintentionally inflate the package.

## Rollout strategy

1. Merge the framework and Astro extractor behind normal behavior only after all Astro acceptance tests pass.
2. Do not add a user-facing feature flag unless binary/MSRV risk or false positives justify one.
3. Emit debug logs for recognized configs, extraction counts, dynamic candidates skipped, and parse failures.
4. Observe issue reports before enabling broader Vite or token-file extraction.
5. Add each new framework schema in a separate change with its own fixtures and public-interface tests.

## Proposed commits

1. `docs: plan Oxc configuration analysis`
2. `build: spike minimal Oxc parser dependencies`
3. `feat(config): add static configuration analysis framework`
4. `feat(astro): index generated font CSS variables`
5. `test(astro): cover config lifecycle through LSP`
6. Later, use-case-specific Vite or token extractor commits.

The dependency spike may be discarded or squashed after measurements. Do not combine the dependency experiment and Astro behavior into one unreviewable change.

## Open questions

1. What MSRV does the project intend to support?
2. What release binary-size increase is acceptable?
3. Should generated definitions require provenance fields in `CssVariable`, or should the manager store a broader definition enum?
4. Should no-substitution template literals be accepted in the first Astro version?
5. When an open config becomes temporarily unparsable, should the server retain the last valid definitions or replace them with the current recoverable subset?
6. Which concrete Vite-generated CSS-variable workflow should be the first Vite extractor?
7. Should users be able to configure additional static token source files and schemas?

## References

- Astro configuration overview: <https://docs.astro.build/en/guides/configuring-astro/>
- Astro fonts guide: <https://docs.astro.build/en/guides/fonts/>
- Vite configuration: <https://vite.dev/config/>
- Oxc parser guide: <https://oxc.rs/docs/guide/usage/parser>
- Oxc repository: <https://github.com/oxc-project/oxc>
- SWC parser documentation: <https://rustdoc.swc.rs/swc_ecma_parser/>
- SWC repository: <https://github.com/swc-project/swc>
