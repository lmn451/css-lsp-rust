#!/usr/bin/env bash
# Smoke test script for css-variable-lsp release assets
# Usage:
#   ./scripts/smoke-test-release.sh vX.Y.Z    # Test assets from GitHub release
#   ./scripts/smoke-test-release.sh --local   # Test assets from local dist/

set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${ROOT_DIR}/.smoke-test-tmp"
REPO="lmn451/css-lsp-rust"

# Asset naming convention (must match extension's asset_name_for_platform)
declare -A ASSETS=(
  ["macos-aarch64"]="css-variable-lsp-macos-aarch64.tar.gz"
  ["macos-x86_64"]="css-variable-lsp-macos-x86_64.tar.gz"
  ["linux-aarch64"]="css-variable-lsp-linux-aarch64.tar.gz"
  ["linux-x86_64"]="css-variable-lsp-linux-x86_64.tar.gz"
  ["windows-aarch64"]="css-variable-lsp-windows-aarch64.exe.zip"
  ["windows-x86_64"]="css-variable-lsp-windows-x86_64.exe.zip"
)

usage() {
  echo "Usage: $0 [vX.Y.Z|--local]"
  echo ""
  echo "Options:"
  echo "  vX.Y.Z   Test assets from GitHub release tag"
  echo "  --local  Test assets from local dist/ directory"
  exit 1
}

cleanup() {
  if [[ -d "${WORK_DIR}" ]]; then
    rm -rf "${WORK_DIR}"
  fi
}

trap cleanup EXIT

test_asset() {
  local platform="$1"
  local asset_name="$2"
  local source="$3"
  local asset_path=""

  echo -e "\n${YELLOW}Testing ${platform}...${NC}"

  if [[ "${source}" == "--local" ]]; then
    asset_path="${ROOT_DIR}/dist/${asset_name}"
    if [[ ! -f "${asset_path}" ]]; then
      echo -e "${YELLOW}⚠ Skipped: ${asset_name} not found in dist/${NC}"
      return 0
    fi
  else
    local url="https://github.com/${REPO}/releases/download/${source}/${asset_name}"
    asset_path="${WORK_DIR}/${asset_name}"

    echo "  Downloading: ${url}"
    if ! curl -fsSL -o "${asset_path}" "${url}" 2>/dev/null; then
      echo -e "${RED}✗ Failed to download ${asset_name}${NC}"
      return 1
    fi
  fi

  # Extract
  local extract_dir="${WORK_DIR}/${platform}"
  mkdir -p "${extract_dir}"

  if [[ "${asset_name}" == *.tar.gz ]]; then
    tar -xzf "${asset_path}" -C "${extract_dir}"
  elif [[ "${asset_name}" == *.zip ]]; then
    unzip -q "${asset_path}" -d "${extract_dir}"
  else
    echo -e "${RED}✗ Unknown archive format: ${asset_name}${NC}"
    return 1
  fi

  # Find binary
  local binary_name="css-variable-lsp"
  if [[ "${platform}" == windows-* ]]; then
    binary_name="css-variable-lsp.exe"
  fi

  local binary_path
  binary_path=$(find "${extract_dir}" -name "${binary_name}" -type f 2>/dev/null | head -1)

  if [[ -z "${binary_path}" ]]; then
    echo -e "${RED}✗ Binary '${binary_name}' not found in archive${NC}"
    return 1
  fi

  echo "  Binary found: ${binary_path}"

  # Check if executable (for current platform only)
  local current_os
  current_os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  local current_arch
  current_arch="$(uname -m)"

  # Map to our platform naming
  local current_platform=""
  case "${current_os}" in
    darwin)
      if [[ "${current_arch}" == "arm64" ]]; then
        current_platform="macos-aarch64"
      else
        current_platform="macos-x86_64"
      fi
      ;;
    linux)
      if [[ "${current_arch}" == "aarch64" ]]; then
        current_platform="linux-aarch64"
      else
        current_platform="linux-x86_64"
      fi
      ;;
    *)
      # Can't run binaries on Windows from bash easily
      ;;
  esac

  if [[ "${platform}" == "${current_platform}" ]]; then
    chmod +x "${binary_path}"
    # Verify the binary responds to --version. This is part of the release
    # contract and confirms that stdio is wired up correctly.
    local version_output
    version_output="$("${binary_path}" --version 2>&1 || true)"
    if [[ ! "${version_output}" =~ ^css-variable-lsp\ v?[0-9]+\.[0-9]+\.[0-9]+ ]]; then
      echo -e "${RED}✗ Binary --version output is unexpected: ${version_output}${NC}"
      return 1
    fi
    if ! "${binary_path}" --help >/dev/null 2>&1; then
      echo -e "${RED}✗ --help failed${NC}"
      return 1
    fi
    echo -e "${GREEN}✓ Binary runs successfully (--version + --help OK)${NC}"
  else
    echo -e "${YELLOW}  (skipped run test - not current platform)${NC}"
  fi

  echo -e "${GREEN}✓ ${platform} passed${NC}"
  return 0
}

main() {
  if [[ $# -lt 1 ]]; then
    usage
  fi

  local source="$1"

  if [[ "${source}" != "--local" ]] && [[ ! "${source}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+ ]]; then
    echo -e "${RED}Error: Invalid version format. Expected vX.Y.Z or --local${NC}"
    usage
  fi

  echo -e "${YELLOW}╔════════════════════════════════════════╗${NC}"
  echo -e "${YELLOW}║  CSS Variable LSP - Release Smoke Test ║${NC}"
  echo -e "${YELLOW}╚════════════════════════════════════════╝${NC}"

  if [[ "${source}" == "--local" ]]; then
    echo -e "Source: local dist/ directory"
  else
    echo -e "Source: GitHub release ${source}"
  fi

  cleanup
  mkdir -p "${WORK_DIR}"

  local passed=0
  local failed=0
  local skipped=0

  for platform in "${!ASSETS[@]}"; do
    if test_asset "${platform}" "${ASSETS[${platform}]}" "${source}"; then
      ((passed++)) || true
    else
      ((failed++)) || true
    fi
  done

  echo -e "\n${YELLOW}════════════════════════════════════════${NC}"
  echo -e "Results: ${GREEN}${passed} passed${NC}, ${RED}${failed} failed${NC}"

  if [[ ${failed} -gt 0 ]]; then
    echo -e "${RED}Smoke test FAILED${NC}"
    exit 1
  fi

  echo -e "${GREEN}Smoke test PASSED${NC}"
}

main "$@"
