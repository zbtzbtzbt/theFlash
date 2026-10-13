#!/usr/bin/env bash
# The checks every change passes before it merges or ships. `make check` and
# CI both run this script, so a green local run means the same thing as a
# green CI run.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== backend (Rust)"
(
  cd backend
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  cargo build
)

echo "== frontend (TypeScript)"
(
  cd frontend
  [ -d node_modules ] || npm ci --no-audit --no-fund
  npm run check
)
