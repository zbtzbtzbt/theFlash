#!/usr/bin/env bash
# The checks every change passes before it merges or ships. `make check` and
# CI both run this script, so a green local run means the same thing as a
# green CI run.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== validation scheduler (Python, offline)"
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -v

echo "== backend (Rust)"
(
  cd backend
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  # Tests never reach the real damai.cn: a test process that sends a request
  # to anything but this machine goes to a proxy nothing listens on and
  # fails. Tests that start the theflash binary replace this with a tripwire
  # that says what was attempted (backend/tests/support/mod.rs).
  dead=http://127.0.0.1:9
  HTTP_PROXY=$dead HTTPS_PROXY=$dead ALL_PROXY=$dead \
    http_proxy=$dead https_proxy=$dead all_proxy=$dead \
    NO_PROXY=127.0.0.1,localhost,::1 no_proxy=127.0.0.1,localhost,::1 \
    cargo test
  cargo build
)

echo "== frontend (TypeScript)"
(
  cd frontend
  [ -d node_modules ] || npm ci --no-audit --no-fund
  npm run check
)
