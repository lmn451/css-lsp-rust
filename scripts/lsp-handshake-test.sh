#!/usr/bin/env bash
# lsp-handshake-test.sh - quick end-to-end check that a css-variable-lsp
# binary speaks the LSP protocol correctly.
#
# Sends initialize + initialized + shutdown + exit over stdio, then verifies
# that the server replies with a valid InitializeResult (capabilities object,
# serverInfo.version present) and a clean shutdown result:null reply.
#
# Usage:
#   ./scripts/lsp-handshake-test.sh [path/to/css-variable-lsp]
#
# Exit codes:
#   0  handshake OK
#   2  binary not found / not executable / python3 missing
#   3  server response did not look like a valid InitializeResult
#   4  server response did not advertise expected capabilities
#   5  server crashed during shutdown
set -euo pipefail

LSP_BIN="${1:-./target/release/css-variable-lsp}"
if [[ ! -x "${LSP_BIN}" ]]; then
  echo "error: binary not executable: ${LSP_BIN}" >&2
  exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required for this script" >&2
  exit 2
fi

INIT_REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}'
INIT_NOT='{"jsonrpc":"2.0","method":"initialized","params":{}}'
SHUTDOWN='{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}'
EXIT_NOT='{"jsonrpc":"2.0","method":"exit","params":null}'

send() {
  local body="$1"
  printf 'Content-Length: %d\r\n\r\n%s' "${#body}" "$body"
}

# Run the server, capture stdout+stderr, time out after 5s.
OUTPUT=$(
  {
    send "${INIT_REQ}"
    sleep 0.1
    send "${INIT_NOT}"
    sleep 0.1
    send "${SHUTDOWN}"
    sleep 0.1
    send "${EXIT_NOT}"
  } | timeout 5 "${LSP_BIN}" 2>&1 || true
)

# Use python to parse LSP frames (Content-Length header + JSON body) and
# extract the responses we care about. python writes the result of the
# initialization handshake and shutdown handshake to separate files.
TMPDIR_HANDSHAKE=$(mktemp -d)
trap 'rm -rf "${TMPDIR_HANDSHAKE}"' EXIT

printf '%s' "${OUTPUT}" > "${TMPDIR_HANDSHAKE}/output.txt"
python3 - "${TMPDIR_HANDSHAKE}" <<'PY'
import json, os, re, sys

tmpdir = sys.argv[1]
buf = open(os.path.join(tmpdir, "output.txt")).read()
parts = re.split(r"Content-Length:\s*\d+\s*\r?\n\r?\n", buf)

init_ok = False
shutdown_ok = False
version = ""
caps_missing = []

for p in parts:
    p = p.strip()
    if not p:
        continue
    try:
        msg = json.loads(p)
    except Exception:
        continue
    if msg.get("id") == 1 and isinstance(msg.get("result"), dict):
        result = msg["result"]
        if "serverInfo" in result and "version" in result["serverInfo"]:
            init_ok = True
            version = result["serverInfo"]["version"]
        caps = result.get("capabilities", {})
        for c in ("completionProvider", "hoverProvider", "definitionProvider",
                  "referencesProvider", "renameProvider"):
            if c not in caps:
                caps_missing.append(c)
    if msg.get("id") == 2 and msg.get("result") is None:
        shutdown_ok = True

with open(os.path.join(tmpdir, "init_ok"), "w") as f:
    f.write("1" if init_ok else "0")
with open(os.path.join(tmpdir, "shutdown_ok"), "w") as f:
    f.write("1" if shutdown_ok else "0")
with open(os.path.join(tmpdir, "version"), "w") as f:
    f.write(version)
with open(os.path.join(tmpdir, "caps_missing"), "w") as f:
    f.write("\n".join(caps_missing))
PY

INIT_OK=$(cat "${TMPDIR_HANDSHAKE}/init_ok")
SHUTDOWN_OK=$(cat "${TMPDIR_HANDSHAKE}/shutdown_ok")
VERSION=$(cat "${TMPDIR_HANDSHAKE}/version")
CAPS_MISSING=$(cat "${TMPDIR_HANDSHAKE}/caps_missing")

if [[ "${INIT_OK}" != "1" ]]; then
  echo "error: server did not return an InitializeResult containing serverInfo" >&2
  echo "raw output:" >&2
  echo "${OUTPUT}" >&2
  exit 3
fi

if [[ -n "${CAPS_MISSING}" ]]; then
  echo "error: InitializeResult is missing capabilities:" >&2
  echo "${CAPS_MISSING}" >&2
  exit 4
fi

if [[ "${SHUTDOWN_OK}" != "1" ]]; then
  echo "error: server did not respond to shutdown with result:null" >&2
  echo "raw output:" >&2
  echo "${OUTPUT}" >&2
  exit 5
fi

echo "✓ LSP handshake OK (server version: ${VERSION})"
