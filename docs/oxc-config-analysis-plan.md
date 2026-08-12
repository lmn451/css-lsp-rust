# Oxc-Based JavaScript and TypeScript Configuration Analysis Plan

## Status

The concrete plan is implemented for Astro font variables and Vite SCSS
`additionalData`. The shared Oxc layer includes exact config discovery, ESM and
CommonJS framework proof, bounded static strings and structures, safe object and
array spreads, simple unconditional Vite function returns, source-accurate
navigation, atomic lifecycle replacement, recoverable parse handling,
generated-source UI, and debug telemetry. Broader schemas remain explicitly
future and use-case driven.

Final macOS aarch64 release measurements recorded 8,229,136 bytes versus
7,176,352 bytes on `master`, an increase of 1,052,784 bytes, or approximately
14.7%. A clean release build took 51.59 seconds versus 40.94 seconds on
`master`; incremental builds took 0.22 and 0.17 seconds respectively. The size
and clean-build increases are accepted for this implementation and must remain
visible during future Oxc upgrades.

## Implemented contract

- Only exact `astro.config.*` and `vite.config.*` basenames listed below are
  analyzed. Ordinary JavaScript files are not eagerly parsed as framework
  configs.
- JavaScript, TypeScript, ESM, and CommonJS variants are supported. Explicit
  `.mjs` and `.mts` files reject CommonJS exports.
- `defineConfig` is trusted only when a direct named, aliased, namespace, or
  supported CommonJS binding is proven to originate from `astro/config` or
  `vite`. Type-only, reassigned, mutated, nested, conditional, and unrelated
  helpers are rejected.
- Accepted static values are unescaped string literals, no-substitution
  templates, immutable unexported module-level `const` aliases, static object
  and array aliases, and known object or array spreads. Resolution preserves
  the originating literal span.
- Static resolution is bounded to 16 recursive levels, 64 expression visits
  per string lookup, and 1,024 structural property or spread visits per
  requested property traversal. Cycles, computed runtime keys, unknown spreads,
  mutation, concatenation, calls, imports, environment access, and filesystem
  access propagate `Unknown`.
- Object properties follow effective last-property semantics. A later unknown
  computed property or spread prevents extraction rather than guessing.
- Astro indexes `fonts[].cssVariable` and legacy
  `experimental.fonts[].cssVariable`. Vite indexes only static
  `css.preprocessorOptions.scss.additionalData`; SCSS control flow and dynamic
  values are rejected.
- Vite accepts direct objects, proven `defineConfig(object)`, and simple
  unconditional function or arrow-function returns. It does not evaluate
  branches, promises, mode, command, or environment values.
- Files larger than 1 MiB are skipped while retaining the last valid analysis.
  Recoverable Oxc ASTs atomically replace stale state with the safe current
  subset. Catastrophic parses retain the last valid state.
- Diagnostics, references, rename, goto-definition, and workspace symbols use
  the existing manager. Completion, hover, and document symbols additionally
  identify Astro or Vite provenance without changing the public `CssVariable`
  type.
- Workspace scans and open-document lifecycle events replace definitions and
  usages atomically. Delete, rename, ignore, and workspace-folder changes remove
  stale config state and revalidate affected consumers.

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

Accepted string forms include direct literals and statically proven aliases:

```ts
const FONT_VARIABLE = "--font-roboto";
const FONT_ALIAS = FONT_VARIABLE;
cssVariable: "--font-roboto"
cssVariable: '--font-roboto'
cssVariable: `--font-roboto`
cssVariable: FONT_ALIAS
```

A template literal is accepted only when it contains no substitutions or
escapes. Source-backed aliases continue to navigate and rename at the
originating literal contents.

Rejected forms include:

```ts
let FONT_VARIABLE = "--font-roboto"
export const FONT_VARIABLE = "--font-roboto"
cssVariable: prefix + "roboto"
cssVariable: `--font-${family}`
cssVariable: getVariableName()
cssVariable: process.env.FONT_VARIABLE
cssVariable: { toString() { return "--font-roboto" } }
```

Unknown values are ignored conservatively. The analyzer does not fold string
concatenation, execute calls, inspect imports, or read runtime state.

## Implementation architecture

The implementation intentionally remains in `src/config_analysis.rs`. The Oxc
allocator, AST, framework proof, static resolver, and extractors share one
synchronous lifetime boundary, while only owned project types cross into the
manager. Splitting this stable behavior into submodules may be considered later,
but the proposed directory tree was not required for correctness or reviewability.

### Owned result boundary

Framework extractors produce owned variable definitions or owned CSS snippets
with source spans. Astro definitions are converted directly to `CssVariable`;
Vite snippets pass through the existing CSS parser so definitions and usages are
collected together. Framework provenance is derived internally from exact config
URIs when rendering LSP output. It does not add framework fields to the public
`CssVariable` type or leak Oxc nodes into `CssVariableManager`.

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

Configuration files are parsed inline and capped at 1 MiB. Final measurements
were 0.0040 ms for 512-byte configs, 0.2782 ms for 64 KiB configs, and 4.4222 ms
for a near-limit 1 MiB config on the development macOS aarch64 machine. These
results do not justify `spawn_blocking`; revisit that decision if production
telemetry shows editor stalls.

### Dependencies

The implementation uses narrowly selected, version-aligned Oxc crates rather
than the umbrella crate:

- `oxc_allocator`
- `oxc_ast`
- `oxc_parser`
- `oxc_span`
- `oxc_ast_visit`

All Oxc crates are pinned to exactly `0.144.0` because the project publishes
frequent coordinated releases.

At implementation time, `oxc_parser 0.144.0` declares Rust 1.95.0 and uses
Rust edition 2024 internally. The project now declares Rust 1.95 as its MSRV and
checks all targets and features with that toolchain in CI.

## Phase 0: dependency and feasibility spike

The dependency and feasibility spike is complete. Representative JS, ESM,
CommonJS, TS, MTS, and CTS fixtures exercise source types, Unicode and CRLF
ranges, malformed recovery, aliases, comments, wrappers, and lifecycle behavior.
The final measurements and packaging evidence are recorded under
[Validation evidence](#validation-evidence).

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

Aliases and namespaces are supported only when a direct ESM import or supported
CommonJS `require` proves the helper package. Re-exports, imported wrappers,
type-only imports, and mutated helpers are rejected.

### Registration semantics

Each generated font variable is registered as a synthetic global definition:

- `name`: configured CSS variable name;
- `value`: empty, so it is never mistaken for a CSS value or color;
- `uri`: Astro config URI;
- `range`: containing property for direct literals, or the originating literal
  range for an alias;
- `name_range`: literal content range, excluding quotes when safely representable;
- `selector`: `:root`;
- `important`: `false`;
- `inline`: `false`;
- `source_position`: property start offset.

Completion, hover, and document-symbol text identify the definition as generated
by Astro rather than pretending the config literal is a CSS declaration.
Provenance is derived from the exact config URI at the LSP boundary, preserving
the published `CssVariable` structure and ordinary manager behavior.

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

The Vite extractor accepts direct string literals, no-substitution template
literals, immutable static aliases, static config objects and arrays, and known
spreads under `scss`. It parses those snippets through the existing CSS parser
so definitions and `var()` usages preserve their source ranges in
`vite.config.*`. Completion, diagnostics, references, symbols, and navigation
then use the same workspace indexes as ordinary CSS. Function-valued
`additionalData`, escaped literals, non-SCSS preprocessors, unknown spreads,
and unrelated properties are ignored.

SCSS control-flow and reusable-code directives such as `@if`, `@for`,
`@mixin`, and `@include` cause the entire snippet to be ignored. This avoids
indexing declarations from branches or mixins that may never emit CSS. CRLF is
preserved by using the exact template source when it contains no escapes.

As with ordinary CSS files, the current manager is workspace-global rather
than import-graph-aware. A statically extracted SCSS definition is therefore
offered across CSS-like documents in the workspace.

### Future candidates

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

Vite supports object configs, `defineConfig(object)`, synchronous functions,
and asynchronous functions. The analyzer accepts direct objects, proven
`defineConfig` calls, and simple unconditional returned object expressions:

```ts
export default defineConfig(() => ({
    // statically inspectable
}));
```

It does not attempt control-flow evaluation of `command`, `mode`, environment
variables, promises, or arbitrary branches.

## Phase 4: reusable static values

The shared static resolver is used by both Astro and Vite.

### Implemented values and structures

Astro `fonts[].cssVariable` and Vite SCSS `additionalData` may reference a
direct, unexported module-level `const` string or a chain of such aliases:

```ts
const FONT_VARIABLE = "--font-body";
const FONT_ALIAS = FONT_VARIABLE;
const SHARED_SCSS = `:root { --brand: #123456; }`;
```

Resolution remains syntax-only and preserves the originating literal span for
rename and navigation. It supports module-level `const` strings, object and
array aliases, parenthesized and TypeScript wrappers, known object and array
spreads, nested property traversal, and effective last-property semantics. A
binding is ignored when it is declared after the reference, exported,
reassigned or updated, declared with `let` or `var`, nested inside a function,
cyclic, escaped, substitution-bearing, or beyond the configured limits.

Implemented safeguards:

- maximum recursion depth of 16;
- maximum 64 string-expression visits per string lookup and 1,024 structural
  visits per requested property traversal;
- cycle detection for bindings;
- no getters, calls, imports, computed runtime properties, or proxy behavior;
- deterministic `Unknown` propagation;
- no filesystem or environment access.

Concatenation, destructuring, imported values, calls, `JSON.stringify`, getters,
and conditional evaluation remain unsupported until a concrete extractor needs
them and can define conservative output semantics.

## Other future uses enabled by Oxc

Add these only when connected to a concrete CSS LSP feature:

1. **Framework-generated design tokens** from known configuration schemas.
2. **Tailwind configuration compatibility**, where older or plugin-specific configs declare CSS variable names. Tailwind v4 CSS-first configuration should continue to be parsed as CSS rather than JavaScript.
3. **CSS-in-JS improvements**, using AST structure to identify tagged templates and preserve exact ranges more reliably than lexical heuristics.
4. **Typed token exports**, for example a local `tokens.ts` file exporting proven `--*` strings, when explicitly configured as a token source.
5. **Known plugin schemas** for PostCSS, Vite, Astro, or framework integrations that generate custom properties.
6. **Configuration diagnostics**, beyond silently rejecting invalid
   custom-property names, only after false-positive behavior is well understood.

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

### Why Oxc was chosen before SWC

1. The project does not need SWC transformations, code generation, hygiene, or compiler pipeline features.
2. Oxc's parse-and-discard arena model maps naturally to extracting a few owned definitions from small config files.
3. The Oxc crate split allows a focused parser/AST dependency set, subject to spike measurements.
4. Oxc's current focus on high-performance analysis tooling and its relationship to Rolldown make it a reasonable long-term base for Vite-adjacent static analysis.
5. Introducing a new AST layer is easier to justify when it has a narrow boundary. Oxc's parser API supports that boundary without adopting a broader compiler architecture.

### Reasons we might still choose SWC

Reconsider SWC if future production evidence demonstrates any of the following:

- materially better recovery on the incomplete configurations users actually edit;
- substantially lower supported MSRV requirements;
- smaller release binaries or build times with the required features;
- more stable visitor APIs for the nodes this project needs;
- a blocking Oxc parser or span correctness issue;
- unsupported release targets;
- an existing SWC-based dependency enters the project, making reuse cheaper than adding Oxc.

The dependency decision is based on the completed Phase 0 measurements and
acceptance tests, not only published parser benchmarks.

## Performance and resource controls

- Discover only recognized configuration basenames.
- Skip ignored directories using existing workspace ignore globs.
- Apply a 1 MiB maximum source size before parsing.
- Do not retain ASTs.
- Track parse count, elapsed time, and ignored-oversize files under debug tracing.
- Replace variables and usages together so no partial config state is visible.
- Preserve `max_documents` behavior for generated-definition sources.

A content-hash cache or `spawn_blocking` boundary remains a profiling-driven
optimization, not part of the current contract.

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
- A recoverable parser error atomically replaces stale state with definitions
  and usages extracted from Oxc's safe current AST.
- A catastrophic parse, empty recovered program, or oversized file retains the
  last valid analysis until the file becomes parseable, is removed, or leaves
  the workspace.
- One malformed config must not abort workspace scanning.
- Duplicate declarations in one config should preserve separate source locations while completion deduplicates names through existing behavior.
- Definitions from multiple workspace roots remain isolated by URI and workspace lifecycle.

## Packaging and compatibility gates

Completed gates:

1. Add and document `rust-version` in `Cargo.toml`.
2. Add an MSRV CI job.
3. Run `cargo fmt --check`.
4. Run strict Clippy with warnings denied.
5. Run `cargo test --all-features`.
6. Build release binaries for all supported targets.
7. Compare release asset sizes against the prior release.
8. Record clean and incremental build-time changes.
9. No dependency security command was introduced because neither `cargo-deny`
   nor `cargo-audit` is part of this repository's toolchain. Oxc remains pinned
   exactly and is covered by lockfile review and normal dependency updates.
10. The crate package contains 54 intentional files after adding the canonical
    GPL-3.0 license text and Oxc's MIT notice; `.pi/`, logs, fixtures, and build
    output are excluded. Release archives include both `LICENSE` and
    `THIRD_PARTY_NOTICES.md` alongside the binary.

## Rollout strategy

1. The framework, Astro extractor, Vite extractor, static resolver, and
   hardening are delivered as stacked pull requests with red-first tests.
2. No user-facing feature flag is required. Exact discovery and conservative
   `Unknown` propagation bound the false-positive surface.
3. Debug logs report parse counts, extraction counts, diagnostics, elapsed
   parsing time, catastrophic recovery, and oversized skips without source text.
4. Broader Vite, token-file, Tailwind, plugin, and CSS-in-JS extraction remains
   gated on concrete issue reports and defined LSP output semantics.
5. Every future schema must be a separate change with analyzer and real LSP
   acceptance tests.

## Validation evidence

Final branch evidence on macOS aarch64:

- stable and Rust 1.95: formatting, strict Clippy, all targets, all features,
  release build, and the complete test suite pass;
- real LSP tests cover Astro and Vite initialization, diagnostics, completion,
  hover, document and workspace symbols, goto-definition, rename, edits, and
  malformed recovery. Workspace-level tests cover deleted or newly ignored
  config files and workspace rescans;
- 192 library tests, 5 binary tests, 30 diagnostics integration tests, 14
  workflow integration tests, 4 Issue #16 guards, and 9 issue proof tests pass;
- release binary: 8,229,136 bytes, approximately 14.7% above `master`;
- clean release build: 51.59 seconds; incremental release build: 0.22 seconds;
- parser averages: 0.0040 ms at 512 bytes, 0.2782 ms at 64 KiB, and
  4.4222 ms at 1,048,575 bytes;
- ten public workspace scans over 64 configs averaged 12.117 ms per scan,
  produced exactly 64 definitions, and reported 7,847,936 bytes maximum RSS;
- `cargo package` contains 54 files and is 711.2 KiB uncompressed and
  152.7 KiB compressed after including `LICENSE` and `THIRD_PARTY_NOTICES.md`;
- the release workflow covers Linux, macOS, and Windows on x86_64 and aarch64;
- an external scratch crate compiles a pre-existing public `CssVariable` struct
  literal, confirming the provenance UI did not break that source interface.

Performance values are development-machine observations rather than hard service
level objectives. They establish the baseline to compare future Oxc upgrades.

## Resolved decisions

1. The MSRV is Rust 1.95 and is enforced in CI.
2. The measured 14.7% macOS aarch64 binary increase is accepted.
3. Generated provenance stays internal and is rendered from exact config URIs;
   the public `CssVariable` struct is unchanged.
4. Unescaped no-substitution template literals are accepted.
5. Recoverable ASTs replace stale state; catastrophic and oversized inputs
   retain the last valid state.
6. Static SCSS `additionalData` is the first Vite workflow because it injects
   real CSS with existing LSP semantics.
7. Additional token sources and schemas are not generic configuration options.
   They require a separate, concrete feature request and reviewed syntax contract.

## References

- Astro configuration overview: <https://docs.astro.build/en/guides/configuring-astro/>
- Astro fonts guide: <https://docs.astro.build/en/guides/fonts/>
- Vite configuration: <https://vite.dev/config/>
- Oxc parser guide: <https://oxc.rs/docs/guide/usage/parser>
- Oxc repository: <https://github.com/oxc-project/oxc>
- SWC parser documentation: <https://rustdoc.swc.rs/swc_ecma_parser/>
- SWC repository: <https://github.com/swc-project/swc>
