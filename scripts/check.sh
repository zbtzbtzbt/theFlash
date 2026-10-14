#!/usr/bin/env bash
# The checks every change passes before it merges or ships. `make check` and
# CI both run this script, so a green local run means the same thing as a
# green CI run.
set -euo pipefail
cd "$(dirname "$0")/.."

# The only step that may use the network: a step that has to download
# something does it here.
echo "== dependencies"
(cd backend && cargo fetch --target host-tuple)
(cd frontend && { [ -d node_modules ] || npm ci --no-audit --no-fund; })

# Tests never reach the real damai.cn (docs/decisions/0003): from here on,
# a request for anything but this machine goes to a proxy nothing listens on
# and fails, whatever language the step is in and whatever it starts. Rust's
# reqwest and Python's urllib read these variables, Node's fetch and http
# read them with NODE_USE_ENV_PROXY. Test runs that start theflash replace
# the proxy with a tripwire that says what was attempted
# (backend/tests/support/mod.rs, frontend/src/test-support.ts). New steps go
# below this line, and are covered without doing anything (#34);
# backend/tests/dead_proxy.rs fails when a step other than the downloads
# runs without these.
dead=http://127.0.0.1:9
export HTTP_PROXY=$dead HTTPS_PROXY=$dead ALL_PROXY=$dead \
  http_proxy=$dead https_proxy=$dead all_proxy=$dead \
  NO_PROXY=127.0.0.1,localhost,::1 no_proxy=127.0.0.1,localhost,::1 \
  NODE_USE_ENV_PROXY=1

echo "== validation scheduler (Python, offline)"
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -v

# The docs index tables are generated from the files they list, so parallel
# pull requests never edit the same row (#45). Catch the change that adds a
# record without regenerating them.
echo "== docs indexes (generated from docs/)"
python3 scripts/docs_index.py --check

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
  npm run check
)
