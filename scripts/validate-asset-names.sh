#!/usr/bin/env bash
# Validates that the CI release workflow's asset names match the expected contract.
# This ensures the extension can download binaries from releases.
#
# Usage: ./scripts/validate-asset-names.sh

set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKFLOW_FILE="${ROOT_DIR}/.github/workflows/release.yml"

# Expected asset names (must match zed-css-variables extension's asset_name_for_platform)
declare -a EXPECTED_ASSETS=(
  "css-variable-lsp-macos-aarch64"
  "css-variable-lsp-macos-x86_64"
  "css-variable-lsp-linux-aarch64"
  "css-variable-lsp-linux-x86_64"
  "css-variable-lsp-windows-aarch64.exe"
  "css-variable-lsp-windows-x86_64.exe"
)

echo -e "${YELLOW}Validating asset names in release workflow...${NC}"

if [[ ! -f "${WORKFLOW_FILE}" ]]; then
  echo -e "${RED}Error: Release workflow not found at ${WORKFLOW_FILE}${NC}"
  exit 1
fi

missing=()
for asset in "${EXPECTED_ASSETS[@]}"; do
  if ! grep -q "asset_name: ${asset}" "${WORKFLOW_FILE}"; then
    missing+=("${asset}")
  fi
done

if [[ ${#missing[@]} -gt 0 ]]; then
  echo -e "${RED}Missing asset names in workflow:${NC}"
  for asset in "${missing[@]}"; do
    echo -e "  ${RED}✗ ${asset}${NC}"
  done
  echo ""
  echo -e "${YELLOW}The extension expects these exact names. Update the release workflow matrix.${NC}"
  exit 1
fi

echo -e "${GREEN}✓ All expected asset names found in release workflow${NC}"

# Also validate build script
BUILD_SCRIPT="${ROOT_DIR}/scripts/build-release-assets.sh"
if [[ -f "${BUILD_SCRIPT}" ]]; then
  echo -e "${YELLOW}Validating asset names in build script...${NC}"

  missing_build=()
  for asset in "${EXPECTED_ASSETS[@]}"; do
    if ! grep -q "${asset}" "${BUILD_SCRIPT}"; then
      missing_build+=("${asset}")
    fi
  done

  if [[ ${#missing_build[@]} -gt 0 ]]; then
    echo -e "${RED}Missing asset names in build script:${NC}"
    for asset in "${missing_build[@]}"; do
      echo -e "  ${RED}✗ ${asset}${NC}"
    done
    exit 1
  fi

  echo -e "${GREEN}✓ All expected asset names found in build script${NC}"
fi

echo -e "\n${GREEN}Asset naming validation passed!${NC}"
