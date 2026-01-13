# Release Process

This document describes how to release a new version of `css-variable-lsp`.

## Asset Naming Contract

The **Zed extension** (`zed-css-variables`) expects assets with these **exact names**:

| Platform | Architecture | Asset Name                                 |
| -------- | ------------ | ------------------------------------------ |
| macOS    | aarch64      | `css-variable-lsp-macos-aarch64.tar.gz`    |
| macOS    | x86_64       | `css-variable-lsp-macos-x86_64.tar.gz`     |
| Linux    | aarch64      | `css-variable-lsp-linux-aarch64.tar.gz`    |
| Linux    | x86_64       | `css-variable-lsp-linux-x86_64.tar.gz`     |
| Windows  | aarch64      | `css-variable-lsp-windows-aarch64.exe.zip` |
| Windows  | x86_64       | `css-variable-lsp-windows-x86_64.exe.zip`  |

> [!CAUTION] > **Do not change these names.** The extension's `asset_name_for_platform()` function expects this exact convention. Any changes require a coordinated update in both repos.

## Supported Targets

| Target Triple               | Runner         | Notes                             |
| --------------------------- | -------------- | --------------------------------- |
| `x86_64-unknown-linux-gnu`  | ubuntu-latest  | Native build                      |
| `aarch64-unknown-linux-gnu` | ubuntu-latest  | Uses `cross`                      |
| `x86_64-apple-darwin`       | macos-latest   | Native build                      |
| `aarch64-apple-darwin`      | macos-latest   | Native build                      |
| `x86_64-pc-windows-msvc`    | windows-latest | Native build                      |
| `aarch64-pc-windows-msvc`   | windows-latest | Uses `msvc-dev-cmd` (best-effort) |

> [!NOTE]
> Windows ARM64 is **best-effort**. If the toolchain is unavailable, the build may be skipped without failing the release.

## Release Checklist

### Pre-Release

- [ ] Update version in `Cargo.toml`
- [ ] Update `CHANGELOG.md` with release notes
- [ ] Ensure all tests pass: `cargo test`
- [ ] (Optional) Build locally: `./scripts/build-release-assets.sh`
- [ ] (Optional) Smoke test local assets: `./scripts/smoke-test-release.sh --local`

### Create Release

1. **Commit and push changes**

   ```bash
   git add -A
   git commit -m "Release vX.Y.Z"
   git push origin main
   ```

2. **Create and push tag**

   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

3. **Verify CI release**

   - Wait for GitHub Actions to complete
   - Check [Releases](../../releases) for all 6 assets
   - Ensure asset names match the contract above

4. **Smoke test the release**
   ```bash
   ./scripts/smoke-test-release.sh vX.Y.Z
   ```

### Post-Release (Extension Update)

After the LSP release is verified:

1. **Update the extension** (`zed-css-variables`):

   - Update `CSS_VARIABLES_RELEASE_TAG` in `src/lib.rs`
   - Bump version in `extension.toml`
   - Update `CHANGELOG.md`
   - Build `extension.wasm`
   - Run tests

2. **Publish the extension**
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

## Local Build Script

Build assets locally for smoke testing:

```bash
# Build all targets (skips unsupported on your host)
./scripts/build-release-assets.sh

# Build specific targets
./scripts/build-release-assets.sh aarch64-apple-darwin x86_64-apple-darwin

# Assets output to dist/
ls -la dist/
```

## Troubleshooting

### Linux ARM64 build fails

Install `cross`: `cargo install cross --locked`

### Windows ARM64 build fails

This is best-effort. Ensure the `amd64_arm64` MSVC toolchain is available or skip this target.

### Asset name mismatch

Compare the uploaded asset names with the table above. The extension will fail to download if names don't match exactly.
