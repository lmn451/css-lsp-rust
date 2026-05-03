# Release Publishing Process

This document describes the release and publishing process for `css-variable-lsp`.

## Version Bump

Update the version in `Cargo.toml`:

```toml
[package]
version = "X.Y.Z"
```

## Release Steps

1. **Make and commit your changes**
   ```bash
   git add -A
   git commit -m "chore: bump version to X.Y.Z"
   ```

2. **Create and push the version tag** — this triggers the release workflow:
   ```bash
   git tag vX.Y.Z
   git push && git push origin vX.Y.Z
   ```

3. **Monitor workflows** — the following GitHub Actions run automatically:
   - `release.yml` — builds cross-platform binaries
   - `publish.yml` — publishes to crates.io

4. **Verify the release**:
   - Crates.io: https://crates.io/crates/css-variable-lsp
   - GitHub Releases: https://github.com/lmn451/css-lsp-rust/releases

## Automated Workflows

### Release Workflow (`.github/workflows/release.yml`)

Builds and uploads binaries for:
- Linux (x86_64, aarch64)
- macOS (x86_64, aarch64)
- Windows (x86_64, aarch64)

Binaries are attached to the GitHub Release automatically.

### Publish Workflow (`.github/workflows/publish.yml`)

Publishes the crate to crates.io when a version tag is pushed.

## Manual Publish (if automated fails)

```bash
# Ensure you're logged in
cargo login

# Publish
cargo publish
```

## Pre-push Checks

The repository has a pre-push hook (via Husky + lefthook or similar) that runs:
- `cargo fmt` — code formatting
- `cargo clippy` — linting
- `cargo test` — all tests

All checks must pass before pushing.

## Versioning Policy

- **Patch** (`0.2.5` → `0.2.6`): Bug fixes, small improvements
- **Minor** (`0.2.5` → `0.3.0`): New features, backward compatible
- **Major** (`0.2.5` → `1.0.0`): Breaking changes

## Troubleshooting

### Tag already exists

```bash
git tag -d vX.Y.Z
git push origin :refs/tags/vX.Y.Z
```

### Version already exists on crates.io

Bump to a newer version in `Cargo.toml` and try again.