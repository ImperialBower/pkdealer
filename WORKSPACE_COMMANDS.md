# Quick Reference: Cargo Workspace Commands

## Testing Commands

```bash
# Test entire workspace
cargo test --workspace

# Test with all features enabled
cargo test --workspace --all-features

# Test specific crate
cargo test -p pkdealer_service
cargo test -p pkdealer_client

# Test with output visible
cargo test --workspace -- --nocapture

# Note: Binary crates (bins) don't support --doc flag
# Regular cargo test will check all tests including doc examples
```

## Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Build in release mode
cargo build --workspace --release

# Build specific crate
cargo build -p pkdealer_service
cargo build -p pkdealer_client

# Build all targets (bins, tests, benches)
cargo build --workspace --all-targets
```

## Code Quality

```bash
# Format code
cargo fmt --all

# Check formatting
cargo fmt --all -- --check

# Run clippy
cargo clippy --workspace --all-targets

# Run clippy with pedantic warnings
cargo clippy --workspace --all-features --all-targets -- -Dclippy::all -Dclippy::pedantic
```

## Documentation

```bash
# Generate docs
cargo doc --workspace

# Generate and open docs
cargo doc --workspace --open

# Generate docs with private items
cargo doc --workspace --document-private-items

# Check doc tests only
cargo test --workspace --doc
```

## Dependency Management

```bash
# Update dependencies
cargo update

# Show dependency tree
cargo tree

# Show duplicate dependencies
cargo tree --duplicates

# Check for security advisories
cargo deny check advisories

# Check for unused dependencies (requires nightly)
cargo +nightly udeps --workspace
```

## Workspace Management

```bash
# Check all workspace members
cargo check --workspace

# Clean build artifacts
cargo clean

# List workspace members
cargo metadata --format-version 1 | jq -r '.workspace_members[]'

# Run command in specific crate directory
cd crates/pkdealer_service && cargo test
```

## CI Workflow Emulation

Run these locally before pushing:

```bash
# Full CI check
cargo fmt --all -- --check && \
cargo clippy --workspace --all-features --all-targets -- -Dclippy::all -Dclippy::pedantic && \
cargo test --workspace --all-features && \
cargo doc --workspace --no-deps --document-private-items

# Quick check
cargo test --workspace && cargo clippy --workspace
```

## Adding New Crates

```bash
# Create new crate in workspace
cargo new crates/my_new_crate

# Or create a library
cargo new --lib crates/my_new_library
```

Then add to workspace `Cargo.toml`:
```toml
[workspace]
members = [
    "crates/pkdealer_service",
    "crates/pkdealer_client",
    "crates/my_new_crate",
]
```

## Useful Environment Variables

```bash
# Treat warnings as errors
RUSTFLAGS="-Dwarnings" cargo build

# Show backtraces
RUST_BACKTRACE=1 cargo test

# Full backtraces
RUST_BACKTRACE=full cargo test

# Enable colored output
CARGO_TERM_COLOR=always cargo build
```

