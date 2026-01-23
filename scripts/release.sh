#!/usr/bin/env bash
set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh [--dry-run|-n] vX.Y.Z

Creates and pushes a release tag. GitHub Actions builds and publishes binaries
for all platforms from the tag.

Options:
  --dry-run, -n  Validate and show what would happen without tagging or pushing.
EOF
}

dry_run=0
if [[ "${1:-}" == "--dry-run" || "${1:-}" == "-n" ]]; then
  dry_run=1
  shift
fi

if [[ "${1:-}" == "" || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 1
fi

version="${1#v}"
tag="v${version}"

if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo -e "${RED}Invalid version: ${1}${NC}"
  usage
  exit 1
fi

cd "${ROOT_DIR}"

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo -e "${RED}Not a git repository.${NC}"
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  if [[ "${dry_run}" -eq 1 ]]; then
    echo -e "${YELLOW}Working tree is dirty; continuing due to --dry-run.${NC}"
  else
    echo -e "${RED}Working tree is dirty. Commit or stash changes before releasing.${NC}"
    exit 1
  fi
fi

if git rev-parse "${tag}" >/dev/null 2>&1; then
  if [[ "${dry_run}" -eq 1 ]]; then
    echo -e "${YELLOW}Tag ${tag} already exists; continuing due to --dry-run.${NC}"
  else
    echo -e "${RED}Tag ${tag} already exists.${NC}"
    exit 1
  fi
fi

upstream="$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null || true)"
if [[ -n "${upstream}" ]]; then
  if [[ -n "$(git rev-list "${upstream}"..HEAD)" ]]; then
    if [[ "${dry_run}" -eq 1 ]]; then
      echo -e "${YELLOW}Unpushed commits detected; continuing due to --dry-run.${NC}"
    else
      echo -e "${RED}Unpushed commits detected. Push your branch before tagging.${NC}"
      exit 1
    fi
  fi
fi

if command -v rg >/dev/null 2>&1; then
  cargo_version="$(rg -m 1 '^version = ' Cargo.toml | sed -E 's/.*version = "([^"]+)".*/\1/')"
else
  cargo_version="$(grep -E '^version = ' Cargo.toml | head -n 1 | sed -E 's/version = "([^"]+)"/\1/')"
fi
if [[ "${cargo_version}" != "${version}" ]]; then
  if [[ "${dry_run}" -eq 1 ]]; then
    echo -e "${YELLOW}Cargo.toml version (${cargo_version}) does not match ${version}; continuing due to --dry-run.${NC}"
  else
    echo -e "${RED}Cargo.toml version (${cargo_version}) does not match ${version}.${NC}"
    exit 1
  fi
fi

echo -e "${YELLOW}Validating asset naming contract...${NC}"
./scripts/validate-asset-names.sh

if [[ "${dry_run}" -eq 1 ]]; then
  echo -e "${YELLOW}Dry run: would create tag ${tag} and push to origin.${NC}"
  exit 0
fi

echo -e "${YELLOW}Tagging ${tag}...${NC}"
git tag -a "${tag}" -m "Release ${tag}"

echo -e "${YELLOW}Pushing tag to origin...${NC}"
git push origin "${tag}"

echo -e "${GREEN}Tag pushed. GitHub Actions will build and publish all release assets.${NC}"
