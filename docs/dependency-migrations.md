# Dependency migrations

This update starts from `a13c93d` (package version 0.3.8). `Cargo.lock` records
the resolved versions, including transitive dependencies. Oxc's five direct
crates must remain on the same exact release.

| Direct dependency | Previous resolved version | Updated resolved version |
| --- | --- | --- |
| csscolorparser | 0.6.2 | 0.9.0 |
| oxc_allocator, oxc_ast, oxc_ast_visit, oxc_parser, oxc_span | 0.144.0 | 0.157.0 |
| tokio | 1.50.0 | 1.54.1 |
| serde | 1.0.228 | 1.0.229 |
| serde_json | 1.0.149 | 1.0.154 |
| globset | 0.4.18 | 0.4.21 |
| regex | 1.12.3 | 1.13.1 |
| futures (tests) | 0.3.32 | 0.3.34 |
| tower (tests) | 0.4.13 | 0.5.3 |
| tower-lsp-server | 0.23.0 | 0.23.0 |
| ls-types | 0.0.6 | 0.0.6 |
| walkdir | 2.5.0 | 2.5.0 |
| pathdiff | 0.2.3 | 0.2.3 |
| url | 2.5.8 | 2.5.8 |
| tracing | 0.1.44 | 0.1.44 |
| tracing-subscriber | 0.3.23 | 0.3.23 |

## Color parsing

Reviewed the [complete csscolorparser changelog](https://github.com/mazznoer/csscolorparser-rs/blob/master/CHANGELOG.md)
for 0.7.0 through 0.9.0. Color fields changed from `f64` to `f32`, conversion
tuples became arrays, deprecated methods were removed, and the library adopted
edition 2024 with a Rust 1.85 minimum. This server only uses the color fields,
so it removes the redundant casts and retains its finite-value guard before
clamping channels to LSP's inclusive `[0, 1]` range. In particular, it does not
use the new `Color::clamp` behavior that converts NaN to zero.

The parser gains OKLab, OKLCH, relative colors, and `color()` with sRGB,
linear sRGB, XYZ D50, and XYZ D65. Literal scanning now recognizes these full
function calls, as well as HWB, Lab, and LCH, so the features are reachable by
the server's literal color index. Balanced calls preserve nested `calc()` and
relative-color ranges. Quoted strings and `var()` fallbacks remain excluded
from that index. Wide-gamut spaces listed under upstream's **Unreleased**
section are not part of this update.

## Configuration analysis and toolchain

Oxc directs Rust consumers to per-crate changelogs. Reviewed every release entry
after 0.144.0 through 0.157.0 for
[parser](https://github.com/oxc-project/oxc/blob/main/crates/oxc_parser/CHANGELOG.md),
[AST](https://github.com/oxc-project/oxc/blob/main/crates/oxc_ast/CHANGELOG.md),
[visitors](https://github.com/oxc-project/oxc/blob/main/crates/oxc_ast_visit/CHANGELOG.md),
[allocator](https://github.com/oxc-project/oxc/blob/main/crates/oxc_allocator/CHANGELOG.md),
and [spans](https://github.com/oxc-project/oxc/blob/main/crates/oxc_span/CHANGELOG.md).

`ParserReturn::panicked` became `fatal_error`. The recovery policy still keeps
the previous analysis for fatal or empty failed parses and accepts recoverable
ASTs. Static property names and string literals now preserve JavaScript lone
surrogates through `JSStr`; comparisons use its exact equality support, which
also handles escaped valid property names without lossy conversion. The
server does not construct AST nodes, read comment attachments, call the
allocator's newly private cursor methods, or use Oxc's UTF-16 converter, so
those changes need no application API migration. The server's 1 MiB input
limit is below Oxc's reduced maximum source length.

Oxc 0.157.0 requires Rust 1.97.0. `Cargo.toml`, CI's minimum-toolchain job,
README, Quickstart, and third-party notices reflect the upgrade. The
application remains edition 2021. Stable CI and release jobs already install
the current stable toolchain.

## Runtime, serialization, and test service

Reviewed [Tokio's release notes](https://github.com/tokio-rs/tokio/blob/master/tokio/CHANGELOG.md)
for 1.51 through 1.54.1, including the `AsyncFd` registration safety changes.
The server uses standard Tokio filesystem, stdio, synchronization, task, and
timer APIs; it has no direct `AsyncFd` construction or zero-capacity I/O
configuration to migrate. Version 1.54.1 includes the timer regression revert.

Reviewed [Tower's 0.5 changelog](https://github.com/tower-rs/tower/blob/master/tower/CHANGELOG.md).
Tests use `Service`, `ready`, and `call`, which remain supported. Removed
`ready_and`, changed retry policies, buffer generics, and `Either` variants
are unused. Aligning tests to Tower 0.5 removes the duplicate Tower 0.4 chain.

Also reviewed [Serde 1.0.229](https://github.com/serde-rs/serde/releases/tag/v1.0.229),
serde_json releases
[1.0.150](https://github.com/serde-rs/json/releases/tag/v1.0.150),
[1.0.151](https://github.com/serde-rs/json/releases/tag/v1.0.151),
[1.0.152](https://github.com/serde-rs/json/releases/tag/v1.0.152),
[1.0.153](https://github.com/serde-rs/json/releases/tag/v1.0.153), and
[1.0.154](https://github.com/serde-rs/json/releases/tag/v1.0.154),
[regex releases](https://github.com/rust-lang/regex/blob/master/CHANGELOG.md),
[globset's upstream ripgrep changelog](https://github.com/BurntSushi/ripgrep/blob/master/CHANGELOG.md),
and [Futures 0.3.33–0.3.34](https://github.com/rust-lang/futures-rs/blob/master/CHANGELOG.md).
These upgrades require no changes to the APIs or enabled features used here.
Notable fixes include regex match offsets, Futures soundness and waker
identity, and serde_json invalid UTF-8 error positions. Serde's derive now
uses syn 3; that dependency is resolved alongside syn 2 where needed.

## Advisory check

An OSV batch query of every registry package and exact version in the baseline
lockfile found `RUSTSEC-2026-0386` for Tokio 1.50.0 and
`RUSTSEC-2026-0097` / `GHSA-cq8v-f236-94qc` for rand 0.8.5. The rand dependency
was used by the old phf build-time generator. The update upgrades Tokio and
removes rand 0.8.5 and phf 0.11 entirely. The same query of all 156 updated
registry packages returned no known affected packages. This is a database
snapshot, not a guarantee about future advisories.

## Query consistency

Document and workspace mutations now hold a lifecycle write guard until their
index updates and unsaved-buffer restoration finish. Requests that read the
index hold read guards, so an immediate rename or color query sees complete
analysis after preceding notifications. Read requests can still run together;
cancellation drops a queued read without blocking subsequent work. Initialization,
configuration changes, and workspace-folder changes use the same guard so a
scan cannot expose an empty or partially rebuilt index.

A deterministic regression pauses `didOpen` after publishing text but before
parsing it. On the previous implementation, rename completed in that window
with incomplete references. It now waits and returns both the definition and
nested fallback edits; the color query resolves the fallback variable. Another
regression queues a query behind a workspace scan and checks that it sees the
authoritative unsaved buffer after disk scanning. Neither regression relies
on sleeps or retries.

## Validation

Local validation used stable Rust 1.97.1 on macOS aarch64. Formatting,
`cargo check --locked --all-targets --all-features`, strict Clippy with all
targets and features, and `cargo build --locked --release` passed. The complete
`cargo test --locked --all-features` run passed 283 tests: 211 library, 5 binary,
38 diagnostics integration, 16 workflow integration, 4 Issue #16 guards, and
9 issue proof tests. This includes the new modern-color/range, escaped-name/
surrogate, and fatal-parse regressions, plus existing JS/TS Unicode and CRLF
range coverage, document/query ordering, scan restoration, and queued-query
cancellation. The release binary's version and real stdio initialize,
shutdown, and exit were smoke tested.

The asset-name validator passed; generated platform archives were not built.
Exact Rust 1.97.0, Linux, and Windows checks remain CI responsibilities. No
release or package publication is part of this migration.