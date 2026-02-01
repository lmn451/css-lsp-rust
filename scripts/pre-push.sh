#!/usr/bin/env bash
# Pre-push hook to run tests and validation
# To install: ln -s ../../scripts/pre-push.sh .git/hooks/pre-push

set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}Running pre-push checks...${NC}"

# 1. Check formatting
echo -e "${YELLOW}Checking formatting...${NC}"
cargo fmt

if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
    echo -e "${YELLOW}Formatting changes detected. Amending commit...${NC}"
    git add -u
    git commit --amend --no-edit
    echo -e "${GREEN}Commit amended with formatting fixes.${NC}"

    echo -e "${YELLOW}Automatically pushing with --force-with-lease...${NC}"
    if git push --force-with-lease; then
        echo -e "${GREEN}Successfully pushed updated commit.${NC}"
    else
        echo -e "${RED}Failed to push updated commit. Please push manually.${NC}"
    fi
    exit 1
fi

# 2. Run Clippy
echo -e "${YELLOW}Running Clippy...${NC}"
if ! cargo clippy --all-targets --all-features -- -D warnings; then
    echo -e "${RED}Clippy check failed.${NC}"
    exit 1
fi

# 3. Run tests
echo -e "${YELLOW}Running tests...${NC}"
if ! cargo test --verbose; then
    echo -e "${RED}Tests failed.${NC}"
    exit 1
fi

# 3b. Run diagnostics integration tests explicitly
echo -e "${YELLOW}Running diagnostics integration tests...${NC}"
if ! cargo test --test diagnostics_integration_test --verbose; then
    echo -e "${RED}Diagnostics integration tests failed.${NC}"
    exit 1
fi

# 4. Validate asset naming contract
echo -e "${YELLOW}Validating asset naming contract...${NC}"
if ! ./scripts/validate-asset-names.sh; then
    echo -e "${RED}Asset naming validation failed.${NC}"
    exit 1
fi

echo -e "${GREEN}All pre-push checks passed!${NC}"
exit 0
