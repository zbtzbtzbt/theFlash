.PHONY: check fmt setup docs-index

# Formatting, lint, type checks, build and tests for backend and frontend.
# CI runs the same script.
check:
	@bash scripts/check.sh

fmt:
	cd backend && cargo fmt
	cd frontend && npx biome check --write .

setup:
	cd frontend && npm ci --no-audit --no-fund

# Rewrite the two docs index tables from the files they list (nobody edits them
# by hand, so parallel pull requests stop colliding on the same row).
docs-index:
	@python3 scripts/docs_index.py --write

# One command starts the read-only UI on an OS-assigned loopback port.
.PHONY: serve
serve: setup
	cd frontend && npm run build
	cd backend && cargo build
	cd frontend && npm run serve
