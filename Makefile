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
# Select the .nvmrc node when nvm is installed, and do nothing when it is not.
#
# The file-existence test is load-bearing, not defensive noise: POSIX says `.` on a file it cannot
# find terminates a non-interactive shell, so `. missing.sh || true` does not survive it — the
# shell is already gone. Arch symlinks /bin/sh to bash, which is forgiving, while Ubuntu uses
# dash, which is not, so sourcing unguarded works locally and dies in CI. CI also sets
# NVM_DIR=/nonexistent on purpose, to fall through to the runner's own node.
NVM := if [ -s "$$NVM_DIR/nvm.sh" ]; then . "$$NVM_DIR/nvm.sh" >/dev/null 2>&1 || true; nvm use >/dev/null 2>&1 || true; fi;
NOUI := SKIP_UI_BUILD=1

.DEFAULT_GOAL := help
.PHONY: help deps ui build run dev check fmt lint test test-unit test-int test-ui \
        coverage db-create db-reset migrate migrate-status seed demo-upstream verify clean

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

# The database is a Postgres you already run locally — there is no container for it.
# ADMIN_URL is DATABASE_URL with the trailing database name swapped for /postgres, which is
# where CREATE/DROP DATABASE have to be issued from.
DB_NAME = $(shell basename "$(DATABASE_URL)")
ADMIN_URL = $(shell echo "$(DATABASE_URL)" | sed 's|/[^/]*$$|/postgres|')

db-create: ## Create the api2mcp role and database on the local Postgres (idempotent)
	@psql "$(ADMIN_URL)" -v ON_ERROR_STOP=1 -q \
	  -c "DO \$$\$$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='api2mcp') \
	      THEN CREATE ROLE api2mcp LOGIN PASSWORD 'api2mcp' CREATEDB; END IF; END \$$\$$;"
	@psql "$(ADMIN_URL)" -tAc "SELECT 1 FROM pg_database WHERE datname='$(DB_NAME)'" | grep -q 1 \
	  || psql "$(ADMIN_URL)" -q -c "CREATE DATABASE $(DB_NAME) OWNER api2mcp"

db-reset: ## Drop and recreate the database in DATABASE_URL, then migrate
	@psql "$(ADMIN_URL)" -v ON_ERROR_STOP=1 -q \
	  -c "DROP DATABASE IF EXISTS $(DB_NAME) WITH (FORCE)" \
	  -c "CREATE DATABASE $(DB_NAME) OWNER api2mcp"
	$(MAKE) migrate

migrate: ## Apply pending migrations
	$(NOUI) cargo run -- migrate up

migrate-status: ## Show migration status
	$(NOUI) cargo run -- migrate status

seed: ## Import the bundled demo pack
	$(NOUI) cargo run -- import examples/demo.pack.yaml

demo-upstream: ## Run the fake upstream the demo pack curates (port 8089)
	$(NOUI) cargo run --example demo_upstream

verify: lint test ## The pre-"done" gate

clean: ## Remove build artifacts
	cargo clean
	rm -rf web/dist web/node_modules
