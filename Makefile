# Developer tasks for TraceQA. Run `make` to list them.
#
# Needs GNU make 3.81 or later, bash, Rust and uv. On Windows, run make from Git Bash.
# Every target runs the same commands as CI (.github/workflows/ci.yml), so a green `make ci`
# means a green pull request.

SHELL := bash
# Ignored by make 3.81 (macOS), which runs recipes with plain `bash -c`.
.SHELLFLAGS := -eu -o pipefail -c
.DELETE_ON_ERROR:
# One target at a time: the targets share target/ and .venv.
.NOTPARALLEL:
MAKEFLAGS += --no-builtin-rules
.DEFAULT_GOAL := help

# Browsers for `make setup` and `make install`.
BROWSERS ?= chromium firefox webkit
# Extra arguments for pytest, for example PYTEST_ARGS="-k codegen -x".
PYTEST_ARGS ?=

ifeq ($(OS),Windows_NT)
EXE := .exe
# No system folder on Windows: the same per-user folder as uv's own installer.
PREFIX ?= $(HOME)/.local
else
EXE :=
PREFIX ?= /usr/local
endif
BINDIR ?= $(PREFIX)/bin
# Staging root for packagers. When it is set, `make install` installs no drivers.
DESTDIR ?=

BIN := bin/traceqa$(EXE)
DEVCONTAINER := npx --yes @devcontainers/cli@0.89.0

.PHONY: help
help: ## List the targets
	@printf 'Usage: make <target> [VARIABLE=value]\n\n'
	@awk -F ':.*## ' '/^##@ /{printf "\n%s\n", substr($$0, 5)} /^[a-zA-Z0-9_-]+:.*## /{printf "  %-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@printf '\nVariables: PREFIX=%s  BROWSERS="%s"  PYTEST_ARGS="%s"\n' '$(PREFIX)' '$(BROWSERS)' '$(PYTEST_ARGS)'

##@ Set up and build

.PHONY: setup
setup: ## Install the Python packages, the browsers and the TypeScript test tools, and build
	uv sync --locked
	uv run playwright install $(BROWSERS)
	@if command -v npm >/dev/null 2>&1; then npm ci --prefix tests/codegen_ts; \
	else echo "npm not found: the TypeScript codegen tests will install their tools when they run"; fi
	cargo build --locked

.PHONY: browser-deps
browser-deps: ## Linux only: install the browsers' system libraries (uses sudo)
	sudo .venv/bin/playwright install-deps $(BROWSERS)

.PHONY: debug
debug: ## Build the debug binary the Python tests use (target/debug/traceqa)
	cargo build --locked

.PHONY: build
build: ## Build the release binary into bin/
	cargo build --release --locked --bin traceqa
	mkdir -p bin
	install -m 0755 target/release/traceqa$(EXE) $(BIN)
	@echo "Built $(BIN)"

##@ Test and check

.PHONY: test
test: test-rust test-python ## Run the Rust and the Python tests

.PHONY: test-rust
test-rust: ## Run the Rust tests
	cargo test --locked

.PHONY: test-python
test-python: debug ## Run the Python tests (run `make setup` once first)
	uv run pytest $(PYTEST_ARGS)

.PHONY: test-installers
test-installers: ## Test the one-line installers against a local fake release
	uv run pytest tests/test_installers.py $(PYTEST_ARGS)

.PHONY: bench
bench: debug ## Run the end-to-end bench on the demo shop
	uv run testbed/bench/run.py

.PHONY: lint
lint: ## Run every check of the CI lint job, and clippy
	uv run scripts/check_version.py
	cargo fmt --all --check
	cargo clippy --all-targets --locked -- -D warnings
	uv run ruff check .
	uv run ruff format --check .
	uv run shellcheck --shell=sh scripts/install.sh
	uv run scripts/check_spec_refs.py
	uv run scripts/sync_pristine.py --check
	uv run pyright
	uv run scripts/gen_python_protocol.py --check

.PHONY: fmt
fmt: ## Format the Rust and the Python code
	cargo fmt --all
	uv run ruff format .

.PHONY: gen
gen: ## Regenerate the files made from spec/, uv.lock and page-script/pristine.js
	uv run scripts/gen_python_protocol.py
	uv run scripts/gen_driver_requirements.py
	uv run scripts/sync_pristine.py

.PHONY: ci
ci: lint test bench ## Run everything CI runs

.PHONY: mutants
mutants: ## Mutation-test the component id (slow; see docs/testing.md)
	CARGO_BUILD_JOBS=2 cargo mutants -p traceqa-engine -j 2 -- --lib

##@ Install the latest local code

.PHONY: install
install: build install-bin $(if $(DESTDIR),,install-drivers) ## Build this checkout and install it: binary into BINDIR, drivers and browsers for you

.PHONY: install-bin
install-bin: ## Copy bin/traceqa into BINDIR, with sudo only for the copy when BINDIR is not writable
	@test -f "$(BIN)" || { echo "$(BIN) is missing: run make build first, as yourself"; exit 1; }
	@dir="$(DESTDIR)$(BINDIR)"; bindir="$(BINDIR)"; sudo=""; \
	if command -v cygpath >/dev/null 2>&1; then dir=$$(cygpath -u "$$dir"); bindir=$$(cygpath -u "$$bindir"); fi; \
	if ! mkdir -p "$$dir" 2>/dev/null || [ ! -w "$$dir" ]; then \
		sudo="sudo"; echo "$$dir is not writable by you: using sudo for the copy"; fi; \
	$$sudo mkdir -p "$$dir"; \
	$$sudo install -m 0755 "$(BIN)" "$$dir/traceqa$(EXE)"; \
	echo "Installed $$dir/traceqa$(EXE)"; \
	case ":$$PATH:" in *":$$bindir:"*) ;; \
	*) [ -n "$(DESTDIR)" ] || echo "Note: $$bindir is not on your PATH. Add it in your shell's profile.";; esac

.PHONY: install-drivers
install-drivers: ## Install the browser drivers from this checkout, and the browsers (never as root)
	@test -f "$(BIN)" || { echo "$(BIN) is missing: run make build first"; exit 1; }
	@if [ "$$(id -u)" = 0 ]; then \
		echo "Run this as yourself: the drivers go in your home folder, ~/.traceqa"; exit 1; fi
	"$(BIN)" install --from . $(BROWSERS)

.PHONY: uninstall
uninstall: ## Remove the installed binary (your ~/.traceqa folder is kept)
	@file="$(DESTDIR)$(BINDIR)/traceqa$(EXE)"; \
	if command -v cygpath >/dev/null 2>&1; then file=$$(cygpath -u "$$file"); fi; \
	if [ -e "$$file" ]; then \
		sudo=""; [ -w "$$(dirname "$$file")" ] || sudo="sudo"; \
		$$sudo rm -f "$$file"; echo "Removed $$file"; \
	else echo "$$file is not installed"; fi; \
	echo "The drivers and browsers stay in ~/.traceqa: delete that folder to remove them."

##@ Other

.PHONY: demo
demo: ## Serve the demo shop at http://127.0.0.1:4173/
	uv run python testbed/sites/buggy-shop/serve.py

.PHONY: devcontainer
devcontainer: ## Build and start the devcontainer (needs Docker)
	$(DEVCONTAINER) up --workspace-folder .

.PHONY: devcontainer-shell
devcontainer-shell: ## Open a shell in the running devcontainer
	$(DEVCONTAINER) exec --workspace-folder . bash

.PHONY: clean
clean: ## Delete the build output: target/ and bin/
	cargo clean
	rm -rf bin
