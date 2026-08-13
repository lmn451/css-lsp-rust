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
# Expected generated archive names and the binary each archive must contain.
# Unix targets use tar.gz; Windows targets retain the .exe in the asset name
# and use zip archives.
declare -a EXPECTED_ARCHIVES=(
  "css-variable-lsp-macos-aarch64.tar.gz|css-variable-lsp"
  "css-variable-lsp-macos-x86_64.tar.gz|css-variable-lsp"
  "css-variable-lsp-linux-aarch64.tar.gz|css-variable-lsp"
  "css-variable-lsp-linux-x86_64.tar.gz|css-variable-lsp"
  "css-variable-lsp-windows-aarch64.exe.zip|css-variable-lsp.exe"
  "css-variable-lsp-windows-x86_64.exe.zip|css-variable-lsp.exe"
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

for required_notice in "LICENSE" "THIRD_PARTY_NOTICES.md"; do
  if [[ ! -f "${ROOT_DIR}/${required_notice}" ]]; then
    echo -e "${RED}Missing release notice file: ${required_notice}${NC}"
    exit 1
  fi
  if ! grep -q "${required_notice}" "${WORKFLOW_FILE}"; then
    echo -e "${RED}Release workflow does not package ${required_notice}${NC}"
    exit 1
  fi
done

echo -e "${GREEN}✓ Release archives include project and third-party notices${NC}"

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

  for required_notice in "LICENSE" "THIRD_PARTY_NOTICES.md"; do
    if ! grep -q "${required_notice}" "${BUILD_SCRIPT}"; then
      echo -e "${RED}Build script does not package ${required_notice}${NC}"
      exit 1
    fi
  done
  echo -e "${GREEN}✓ Local release archives include project and third-party notices${NC}"

  if grep -q "powershell.exe" "${BUILD_SCRIPT}" && ! grep -q "cygpath -w" "${BUILD_SCRIPT}"; then
    echo -e "${RED}PowerShell fallback must convert POSIX archive paths with cygpath${NC}"
    exit 1
  fi
  echo -e "${GREEN}✓ Windows PowerShell fallback converts archive paths${NC}"
fi

validate_archive_contents() {
  local archive_path="$1"
  local archive_name="$2"
  local expected_binary="$3"
  local archive_entries
  local entry
  local missing_entries=()

  if [[ ! -r "${archive_path}" ]]; then
    echo -e "${RED}Cannot inspect ${archive_name}: archive is not readable${NC}"
    return 1
  fi

  case "${archive_name}" in
    *.tar.gz)
      if ! archive_entries="$(tar -tzf "${archive_path}" 2>&1)"; then
        echo -e "${RED}Cannot inspect ${archive_name}: unreadable or malformed tar.gz archive${NC}"
        [[ -z "${archive_entries}" ]] || printf '  tar: %s\n' "${archive_entries}"
        return 1
      fi
      ;;
    *.zip)
      if ! command -v unzip >/dev/null 2>&1; then
        echo -e "${RED}Cannot inspect ${archive_name}: unzip is required to validate zip archives${NC}"
        return 1
      fi
      if ! archive_entries="$(unzip -Z1 "${archive_path}" 2>&1)"; then
        echo -e "${RED}Cannot inspect ${archive_name}: unreadable or malformed zip archive${NC}"
        [[ -z "${archive_entries}" ]] || printf '  unzip: %s\n' "${archive_entries}"
        return 1
      fi
      ;;
    *)
      echo -e "${RED}Cannot inspect ${archive_name}: unsupported archive format${NC}"
      return 1
      ;;
  esac

  for entry in "${expected_binary}" "LICENSE" "THIRD_PARTY_NOTICES.md"; do
    if ! grep -Fqx -- "${entry}" <<<"${archive_entries}"; then
      missing_entries+=("${entry}")
    fi
  done

  if [[ ${#missing_entries[@]} -gt 0 ]]; then
    echo -e "${RED}Archive ${archive_name} is missing required entries:${NC}"
    for entry in "${missing_entries[@]}"; do
      echo -e "  ${RED}✗ ${entry}${NC}"
    done
    return 1
  fi

  echo -e "${GREEN}✓ ${archive_name} contains ${expected_binary}, LICENSE, and THIRD_PARTY_NOTICES.md${NC}"
}

DIST_DIR="${ROOT_DIR}/dist"
archive_checks=0
archive_validation_failed=0

if [[ -d "${DIST_DIR}" ]]; then
  shopt -s nullglob
  present_archives=("${DIST_DIR}"/*.tar.gz "${DIST_DIR}"/*.zip)
  shopt -u nullglob

  for archive_path in "${present_archives[@]}"; do
    archive_name="${archive_path##*/}"
    expected_binary=""
    for archive_spec in "${EXPECTED_ARCHIVES[@]}"; do
      IFS='|' read -r expected_archive expected_archive_binary <<<"${archive_spec}"
      if [[ "${archive_name}" == "${expected_archive}" ]]; then
        expected_binary="${expected_archive_binary}"
        break
      fi
    done

    archive_checks=$((archive_checks + 1))
    if [[ -z "${expected_binary}" ]]; then
      echo -e "${RED}Unexpected archive under dist/: ${archive_name}${NC}"
      archive_validation_failed=1
      continue
    fi

    if ! validate_archive_contents "${archive_path}" "${archive_name}" "${expected_binary}"; then
      archive_validation_failed=1
    fi
  done
fi

if [[ ${archive_checks} -eq 0 ]]; then
  echo -e "${YELLOW}No generated archives found under dist/; archive content checks were skipped until assets are built.${NC}"
elif [[ ${archive_validation_failed} -ne 0 ]]; then
  exit 1
else
  echo -e "${GREEN}✓ Generated archive content validation passed${NC}"
fi

echo -e "\n${GREEN}Asset naming validation passed!${NC}"
