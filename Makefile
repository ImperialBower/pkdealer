# Makefile for pkgrpc Cargo Workspace
#
# Common commands for development, testing, and CI emulation

# Number of demo+audit cycles to run (override with COUNT=N on the command line).
# Example: make demo-audit COUNT=5
COUNT ?= 1

# Player line-up for `make arena` (override on the command line).
# Example: make arena PLAYERS="gto gto lag llama"
PLAYERS ?= gto lag tag llama mistral gemma

# Player line-up for `make detect` (EPIC-70 fixed-blind cheat detection).
# Empty ⇒ bin/detect's built-in default: carol dave gto lag boss.
# Example: make detect DETECT_PLAYERS="carol dave gto lag tag boss"
DETECT_PLAYERS ?=

# Docker compose project name (containers are labelled with this). Override only
# if you launched with a custom COMPOSE_PROJECT_NAME.
PROJECT ?= pkdealer

# Few-shot examples baked into each PokerBench-guided Ollama model.
# Example: make pokerbench-models POKERBENCH_EXAMPLES=20
POKERBENCH_EXAMPLES ?= 12

.PHONY: help build test check fmt clippy doc clean all ci-local install-tools serve ddown arena-down demo demo-audit arena detect pokerbench-data pokerbench-models

# Default target
default: ayce

all: ayce

# Default target
help:
	@echo "pkgrpc Workspace Commands"
	@echo "========================="
	@echo ""
	@echo "Development:"
	@echo "  make build          - Build all workspace crates"
	@echo "  make test           - Run all tests"
	@echo "  make check          - Quick compile check"
	@echo "  make fmt            - Format all code"
	@echo "  make clippy         - Run clippy lints"
	@echo "  make doc            - Generate documentation"
	@echo "  make clean          - Clean build artifacts"
	@echo ""
	@echo "CI Emulation:"
	@echo "  make ci-local       - Run all CI checks locally"
	@echo "  make ci-quick       - Run quick CI checks"
	@echo ""
	@echo "Individual Crates:"
	@echo "  make test-service   - Test pkdealer_service"
	@echo "  make test-client    - Test pkdealer_client"
	@echo ""
	@echo "Service:"
	@echo "  make serve          - Build and start the dealer service"
	@echo "  make ddown          - Tear down the demo stack (docker compose down -v)"
	@echo "  make demo           - Run the 9-player client demo (service must be running)"
	@echo "  make demo-audit [COUNT=N] - Run demo+audit N times (default 1)"
	@echo ""
	@echo "Arena (EPIC-42 dynamic line-ups; see ./bin/arena --help):"
	@echo "  make arena [PLAYERS=\"gto lag llama\"] - Launch an ad-hoc arena table"
	@echo "  (or call directly: ./bin/arena gto:2 claude tag)"
	@echo "  make detect [DETECT_PLAYERS=\"...\"]  - Cheat-detection run, blinds FROZEN (EPIC-70)"
	@echo "  make arena-down     - Force-tear-down ALL arena containers + volumes"
	@echo ""
	@echo "Tools:"
	@echo "  make install-tools  - Install cargo-deny, cargo-udeps, etc."
	@echo ""
	@echo "PokerBench (EPIC-43):"
	@echo "  make pokerbench-data   - Download the PokerBench dataset (HuggingFace, ~720MB)"
	@echo "  make pokerbench-models - Build pkpoker-{gemma,llama,mistral} Ollama models"
	@echo "                           with PokerBench few-shot guidance (needs ollama + data)"
	@echo ""

# Run the 9-player client demo (requires the service to already be running)
demo:
	@echo "Running pkdealer client demo..."
	cargo run --example demo -p pkdealer_client

# Run a full demo session then immediately audit the generated file.
# Pass COUNT=N to repeat N times (default 1).  Example: make demo-audit COUNT=5
demo-audit:
	@failed=0; \
	for i in $$(seq 1 $(COUNT)); do \
		echo "── Run $$i/$(COUNT) ─────────────────────────────────────────"; \
		tmpfile=$$(mktemp); \
		cargo run --example demo -p pkdealer_client | tee "$$tmpfile"; \
		file=$$(grep "saved:" "$$tmpfile" | sed 's/.*saved: //' | tr -d '[:space:]'); \
		rm -f "$$tmpfile"; \
		if [ -z "$$file" ]; then echo "ERROR: could not find saved filename in demo output"; exit 1; fi; \
		echo ""; \
		cargo run --example audit -p pkdealer_client -- "$$file" || { failed=1; break; }; \
	done; \
	exit $$failed

# Start the dealer service
serve:
	@echo "Starting pkdealer_service on 127.0.0.1:50051..."
	cargo run --bin pkdealer_service -p pkdealer_service

# Launch an ad-hoc arena table (EPIC-42). Override the line-up with PLAYERS,
# e.g. make arena PLAYERS="gto:2 claude tag". For the full multiplicity and
# registry options, call ./bin/arena directly.
arena:
	@./bin/arena $(PLAYERS)

# Launch the EPIC-70 collusion-detection scenario with the tournament blind
# schedule DISABLED (stable table ⇒ clean chip-flow / bb-per-100 signals for the
# Boss). Empty DETECT_PLAYERS uses bin/detect's default lineup (carol dave gto
# lag boss). See docs/presentations/epic-70-fixed-blinds-cheat-detection.md.
detect:
	@./bin/detect $(DETECT_PLAYERS)

# Tear down the demo stack and drop its named volumes.
ddown:
	@echo "Stopping demo stack and removing volumes..."
	docker compose down -v --remove-orphans

# Forcefully tear down EVERY container in the '$(PROJECT)' compose project — no
# matter what. Works even for agents launched by ./bin/arena (whose /tmp compose
# override may already be deleted), and even if compose's own state is confused:
# it targets the project by name, then sweeps any survivors by compose label.
# Idempotent and safe to run when nothing is up.
arena-down:
	@echo "Tearing down the '$(PROJECT)' arena (all containers + volumes)..."
	-docker compose -p $(PROJECT) down -v --remove-orphans
	@leftover=$$(docker ps -aq --filter "label=com.docker.compose.project=$(PROJECT)"); \
	if [ -n "$$leftover" ]; then \
		echo "Force-removing leftover containers..."; \
		docker rm -f $$leftover; \
	else \
		echo "No leftover '$(PROJECT)' containers — clean."; \
	fi

# Build all crates
build:
	@echo "Building workspace..."
	cargo clean
	cargo build --workspace --all-features

# Build in release mode
build-release:
	@echo "Building workspace in release mode..."
	cargo build --workspace --all-features --release

# Run all tests
test:
	@echo "Running workspace tests..."
	cargo test --workspace --all-features

# Run tests with output
test-verbose:
	@echo "Running workspace tests (verbose)..."
	cargo test --workspace --all-features -- --nocapture


# Test individual crates
test-service:
	@echo "Testing pkdealer_service..."
	cargo test -p pkdealer_service --all-features

test-client:
	@echo "Testing pkdealer_client..."
	cargo test -p pkdealer_client --all-features

# Quick compile check
check:
	@echo "Checking workspace..."
	cargo check --workspace --all-features

# Format code
fmt:
	@echo "Formatting code..."
	cargo fmt --all

# Check formatting
fmt-check:
	@echo "Checking code formatting..."
	cargo fmt --all -- --check

# Run clippy
clippy:
	@echo "Running clippy..."
	cargo clippy --workspace --all-features --all-targets

# Run clippy with pedantic warnings (as in CI)
clippy-pedantic:
	@echo "Running clippy with pedantic warnings..."
	cargo clippy --workspace --all-features --all-targets -- -Dclippy::all -Dclippy::pedantic

# Generate documentation
doc:
	@echo "Generating documentation..."
	cargo doc --workspace --no-deps --document-private-items --all-features

# Generate and open documentation
doc-open:
	@echo "Generating and opening documentation..."
	cargo doc --workspace --no-deps --document-private-items --all-features --open

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	cargo clean

# Update dependencies
update:
	@echo "Updating dependencies..."
	cargo update

# Show dependency tree
tree:
	@echo "Showing dependency tree..."
	cargo tree --workspace

# Show duplicate dependencies
tree-duplicates:
	@echo "Showing duplicate dependencies..."
	cargo tree --workspace --duplicates

# Security audit with cargo-deny
audit:
	@echo "Running security audit..."
	cargo deny check advisories

# Check for unused dependencies (requires nightly)
unused-deps:
	@echo "Checking for unused dependencies..."
	cargo +nightly udeps --workspace --all-features

# Run all checks (quick CI emulation)
ci-quick: fmt-check check test

# Run full CI checks locally
ci-local: fmt-check clippy-pedantic test doc
	@echo ""
	@echo "✓ All CI checks passed!"
	@echo ""

# Run everything
ayce: fmt build test clippy doc

# Install required tools
install-tools:
	@echo "Installing development tools..."
	cargo install cargo-deny
	cargo install cargo-udeps
	@echo ""
	@echo "✓ Tools installed!"
	@echo ""

# Watch mode for development (requires cargo-watch)
watch:
	cargo watch -x "check --workspace" -x "test --workspace"

# Install cargo-watch
install-watch:
	cargo install cargo-watch

# Download the PokerBench dataset (EPIC-43) from HuggingFace into
# data/pokerbench (~720MB). Idempotent: existing files are skipped. Override the
# destination with POKERBENCH_DATA_DIR. Mirrors pkcore's `make pokerbench-data`.
pokerbench-data:
	@dir="$${POKERBENCH_DATA_DIR:-data/pokerbench}"; \
	base="https://huggingface.co/datasets/RZ412/PokerBench/resolve/main"; \
	mkdir -p "$$dir"; \
	for f in \
		preflop_1k_test_set_game_scenario_information.csv \
		preflop_1k_test_set_prompt_and_label.json \
		preflop_60k_train_set_game_scenario_information.csv \
		preflop_60k_train_set_prompt_and_label.json \
		postflop_10k_test_set_game_scenario_information.csv \
		postflop_10k_test_set_prompt_and_label.json \
		postflop_500k_train_set_game_scenario_information.csv \
		postflop_500k_train_set_prompt_and_label.json ; do \
		if [ -f "$$dir/$$f" ]; then \
			echo "exists, skipping: $$f"; \
		else \
			echo "downloading:    $$f"; \
			curl -fL --progress-bar -o "$$dir/$$f" "$$base/$$f" \
				|| { echo "FAILED: $$f"; rm -f "$$dir/$$f"; exit 1; }; \
		fi; \
	done; \
	echo "PokerBench dataset ready in $$dir"

# Build PokerBench-guided Ollama models (pkpoker-gemma / pkpoker-llama /
# pkpoker-mistral) by baking sampled solver-optimal decisions into each base
# model's system prompt. Depends on `pokerbench-data` (idempotent — skips files
# already downloaded), and needs a running ollama with the base models pulled
# (gemma2, llama3.1, mistral). 16GB-Mac friendly: no weight training, runs
# entirely on local ollama. Override example count with POKERBENCH_EXAMPLES; use
# ARGS="--dry-run" to inspect Modelfiles without creating.
pokerbench-models: pokerbench-data
	uv run scripts/pokerbench_models.py --examples $(POKERBENCH_EXAMPLES) $(ARGS)

