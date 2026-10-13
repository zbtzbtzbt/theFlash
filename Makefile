.PHONY: check fmt setup

# Formatting, lint, type checks, build and tests for backend and frontend.
# CI runs the same script.
check:
	@bash scripts/check.sh

fmt:
	cd backend && cargo fmt
	cd frontend && npx biome check --write .

setup:
	cd frontend && npm ci --no-audit --no-fund

# One command starts the read-only UI on an OS-assigned loopback port.
.PHONY: serve
serve: setup
	cd frontend && npm run build
	cd backend && cargo build
	cd frontend && npm run serve
