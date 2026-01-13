#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"

mkdir -p "${DIST_DIR}"

if [[ "$#" -gt 0 ]]; then
  TARGETS=("$@")
else
  TARGETS=(
    x86_64-unknown-linux-gnu
    aarch64-unknown-linux-gnu
    x86_64-apple-darwin
    aarch64-apple-darwin
    x86_64-pc-windows-msvc
    aarch64-pc-windows-msvc
  )
fi

HOST_TARGET="$(rustc -vV | awk '/host:/ {print $2}')"

can_build_target() {
  local target="$1"
  case "${target}" in
    *apple-darwin)
      [[ "${HOST_TARGET}" == *apple-darwin ]]
      ;;
    *pc-windows-msvc)
      [[ "${HOST_TARGET}" == *windows-msvc ]]
      ;;
    *unknown-linux-gnu)
      [[ "${HOST_TARGET}" == *unknown-linux-gnu ]] || command -v cross >/dev/null 2>&1
      ;;
    *)
      return 1
      ;;
  esac
}

build_target() {
  local target="$1"
  local bin_name="css-variable-lsp"
  local asset_base=""
  local build_cmd="cargo"

  if ! can_build_target "${target}"; then
    echo "Skipping ${target} (unsupported on host: ${HOST_TARGET})"
    return 0
  fi

  case "${target}" in
    x86_64-unknown-linux-gnu)
      asset_base="css-variable-lsp-linux-x86_64"
      ;;
    aarch64-unknown-linux-gnu)
      asset_base="css-variable-lsp-linux-aarch64"
      ;;
    x86_64-apple-darwin)
      asset_base="css-variable-lsp-macos-x86_64"
      ;;
    aarch64-apple-darwin)
      asset_base="css-variable-lsp-macos-aarch64"
      ;;
    x86_64-pc-windows-msvc)
      asset_base="css-variable-lsp-windows-x86_64.exe"
      bin_name="css-variable-lsp.exe"
      ;;
    aarch64-pc-windows-msvc)
      asset_base="css-variable-lsp-windows-aarch64.exe"
      bin_name="css-variable-lsp.exe"
      ;;
    *)
      echo "Unknown target: ${target}"
      return 1
      ;;
  esac

  if [[ "${target}" == *"unknown-linux-gnu" ]] && [[ "${target}" != "${HOST_TARGET}" ]]; then
    if command -v cross >/dev/null 2>&1; then
      build_cmd="cross"
    fi
  fi

  echo "==> Building ${target} using ${build_cmd}"
  if ! rustup target list --installed | grep -q "${target}"; then
    rustup target add "${target}"
  fi

  "${build_cmd}" build --release --target "${target}"

  local bin_path="${ROOT_DIR}/target/${target}/release/${bin_name}"
  if [[ ! -f "${bin_path}" ]]; then
    echo "Missing binary at ${bin_path}"
    return 1
  fi

  if [[ "${target}" == *"windows"* ]]; then
    local zip_path="${DIST_DIR}/${asset_base}.zip"
    if command -v zip >/dev/null 2>&1; then
      (cd "${ROOT_DIR}/target/${target}/release" && zip -q "${zip_path}" "${bin_name}")
    elif command -v powershell.exe >/dev/null 2>&1; then
      powershell.exe -NoProfile -Command "Compress-Archive -Path '${bin_path}' -DestinationPath '${zip_path}'"
    else
      echo "zip not found; skipping archive for ${target}"
      return 1
    fi
  else
    local tar_path="${DIST_DIR}/${asset_base}.tar.gz"
    tar -C "${ROOT_DIR}/target/${target}/release" -czf "${tar_path}" "${bin_name}"
  fi
}

failures=()
for target in "${TARGETS[@]}"; do
  if ! build_target "${target}"; then
    failures+=("${target}")
  fi
done

if [[ "${#failures[@]}" -gt 0 ]]; then
  echo "Failed targets: ${failures[*]}"
  exit 1
fi

echo "Artifacts are in ${DIST_DIR}"
