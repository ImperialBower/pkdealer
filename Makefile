# Makefile for pkgrpc Cargo Workspace
#
# Common commands for development, testing, and CI emulation

.PHONY: help build test check fmt clippy doc clean all ci-local install-tools

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
	@echo "Tools:"
	@echo "  make install-tools  - Install cargo-deny, cargo-udeps, etc."
	@echo ""

# Build all crates
build:
	@echo "Building workspace..."

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

