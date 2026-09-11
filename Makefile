# api2mcp — build/test/lint targets.
#
# IMPORTANT: the Vue SPA is embedded into the binary by rust-embed
# (`#[folder = "web/dist"]`), so web/dist must exist before any cargo build. build.rs
# guarantees that with a placeholder and then builds the real bundle, so plain `cargo build`
# works on a clean clone. Set SKIP_UI_BUILD=1 to skip the npm step — every target below that
# does not need the bundle already does.
#
# Node version is pinned in .nvmrc.

export CARGO_BUILD_JOBS ?= 4
NVM := . "$$NVM_DIR/nvm.sh" >/dev/null 2>&1 && nvm use >/dev/null 2>&1 || true;
NOUI := SKIP_UI_BUILD=1

.DEFAULT_GOAL := help
.PHONY: help deps ui build run dev check fmt lint test test-unit test-int test-ui \
        coverage db-up db-down db-reset migrate migrate-status seed verify clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n",$$1,$$2}'

deps: ## Install frontend dependencies
	cd web && $(NVM) npm ci

ui: ## Build the SPA into web/dist
	cd web && $(NVM) npm run build

build: ## Build the binary (build.rs builds the SPA first)
	cargo build --release

run: ## Run the server
	cargo run -- serve

dev: ## Hot-reload the SPA against a locally running server on :8080
	cd web && $(NVM) npm run dev

check: ## Fast typecheck, no SPA build
	$(NOUI) cargo check --all-targets

fmt: ## Apply Rust formatting
	cargo fmt

lint: ## Rust fmt-check + clippy, then eslint + vue-tsc
	cargo fmt --check
	$(NOUI) cargo clippy --all-targets -- -D warnings
	cd web && $(NVM) npm run lint && npm run typecheck

test-unit: ## Rust unit tests (lib), no database needed
	$(NOUI) cargo test --lib

test-int: ## Rust integration tests (tests/); needs TEST_DATABASE_URL
	$(NOUI) cargo test --test '*'

test-ui: ## Frontend unit tests
	cd web && $(NVM) npm run test

test: test-unit test-int test-ui ## All tests

coverage: ## Backend coverage summary
	$(NOUI) cargo llvm-cov --summary-only

# Optional containerised Postgres, published on 5433 so it never fights a Postgres already
# listening on 5432. If you already run one locally, skip these and point DATABASE_URL at it.
db-up: ## Start the containerised dev Postgres on :5433
	docker compose up -d db

db-down: ## Stop the containerised dev Postgres
	docker compose down

db-reset: ## Drop and recreate the database in DATABASE_URL, then migrate
	@psql "$${DATABASE_URL:?set DATABASE_URL}" -c 'SELECT 1' >/dev/null
	@psql "$$(echo "$$DATABASE_URL" | sed 's|/[^/]*$$|/postgres|')" \
	  -c "DROP DATABASE IF EXISTS $$(basename "$$DATABASE_URL") WITH (FORCE)" \
	  -c "CREATE DATABASE $$(basename "$$DATABASE_URL")"
	$(MAKE) migrate

migrate: ## Apply pending migrations
	$(NOUI) cargo run -- migrate up

migrate-status: ## Show migration status
	$(NOUI) cargo run -- migrate status

seed: ## Import the bundled demo pack
	$(NOUI) cargo run -- import examples/demo.pack.yaml

verify: lint test ## The pre-"done" gate

clean: ## Remove build artifacts
	cargo clean
	rm -rf web/dist web/node_modules
