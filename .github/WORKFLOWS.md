# GitHub Actions Workflows

This document explains the GitHub Actions workflows configured for this Cargo workspace project.

## Workspace Structure

This project uses a Cargo workspace with the following structure:

```
pkgrpc/
├── Cargo.toml           # Workspace root
├── crates/
│   ├── pkdealer_service/
│   │   ├── Cargo.toml
│   │   └── src/
│   └── pkdealer_client/
│       ├── Cargo.toml
│       └── src/
```

## Workflows Overview

### 1. CI.yaml - Main Continuous Integration

**Triggers:** Push to main/master/develop, Pull Requests

**Jobs:**

- **test** - Tests the entire workspace across multiple Rust versions
  - Matrix: stable, beta, 1.85.0 (MSRV), nightly
  - Runs: `cargo test --workspace --all-features`
  - Includes doc tests: `cargo test --workspace --all-features --doc`
  - Uses caching for faster builds

- **test-crates** - Tests each crate individually
  - Matrix: pkdealer_service, pkdealer_client
  - Ensures each crate can be built and tested independently
  - Useful for detecting dependency issues specific to individual crates

- **clippy** - Linting with Clippy
  - Runs: `cargo clippy --workspace --all-features --all-targets`
  - Enforces: `-Dclippy::all -Dclippy::pedantic`
  - Helps maintain code quality across all workspace members

- **fmt** - Code formatting check
  - Runs: `cargo fmt --all -- --check`
  - Ensures consistent formatting across the workspace

- **doc** - Documentation generation
  - Runs: `cargo doc --workspace --no-deps --document-private-items --all-features`
  - Treats warnings as errors to ensure documentation quality

- **miri** - Unsafe code testing
  - Tests for undefined behavior in unsafe code
  - Uses strict provenance checking

### 2. workspace-check.yaml - Workspace Health Checks

**Triggers:** Push to main/master/develop, Pull Requests

**Jobs:**

- **workspace-check** - Verifies workspace structure
  - Checks all workspace members compile
  - Lists all workspace members for verification

- **dependency-check** - Dependency consistency
  - Uses `cargo-deny` to check for:
    - Security advisories
    - License compliance
    - Banned dependencies
    - Multiple versions of the same dependency

- **unused-deps** - Detects unused dependencies
  - Uses `cargo-udeps` to find unused dependencies
  - Helps keep `Cargo.toml` files clean

### 3. audit.yml - Security Audit

**Triggers:** 
- Weekly schedule (Sunday at 1 AM)
- Push to any `Cargo.toml` or `Cargo.lock` file

**Jobs:**

- **security_audit** - Scans for vulnerabilities
  - Uses `cargo-deny` to check advisories
  - Helps maintain security across the workspace

## Working with the Workspace

### Running Tests Locally

```bash
# Test entire workspace
cargo test --workspace --all-features

# Test specific crate
cargo test --package pkdealer_service --all-features

# Test with doc tests
cargo test --workspace --all-features --doc

# Run all tests for a specific crate
cargo test -p pkdealer_client --all-features
```

### Running Checks Locally

```bash
# Format check
cargo fmt --all -- --check

# Auto-format all code
cargo fmt --all

# Clippy for all workspace members
cargo clippy --workspace --all-features --all-targets -- -Dclippy::all -Dclippy::pedantic

# Generate documentation
cargo doc --workspace --no-deps --document-private-items --all-features

# Check for security advisories
cargo deny check advisories

# Check for unused dependencies (requires nightly)
cargo +nightly udeps --workspace --all-features
```

### Adding New Crates to the Workspace

1. Create the new crate:
   ```bash
   cargo new crates/my_new_crate
   ```

2. Add it to the workspace `Cargo.toml`:
   ```toml
   [workspace]
   members = [
       "crates/pkdealer_service",
       "crates/pkdealer_client",
       "crates/my_new_crate",  # Add this line
   ]
   ```

3. Add it to the test-crates matrix in `.github/workflows/CI.yaml`:
   ```yaml
   matrix:
     crate:
       - pkdealer_service
       - pkdealer_client
       - my_new_crate  # Add this line
   ```

### Caching Strategy

All workflows use GitHub Actions caching to speed up builds:

- **Cargo registry**: `~/.cargo/registry`
- **Cargo git index**: `~/.cargo/git`
- **Build artifacts**: `target/`

Cache keys are based on:
- Operating system
- Rust toolchain version
- `Cargo.lock` file hash

### Environment Variables

- `RUSTFLAGS: -Dwarnings` - Treat warnings as errors
- `CARGO_TERM_COLOR: always` - Enable colored output in CI logs

## Best Practices

1. **Always run tests before pushing:**
   ```bash
   cargo test --workspace --all-features && cargo clippy --workspace --all-features --all-targets
   ```

2. **Keep dependencies up to date:**
   - Monitor the security audit workflow
   - Review and update dependencies regularly

3. **Maintain consistent formatting:**
   ```bash
   cargo fmt --all
   ```

4. **Document your code:**
   - Add doc comments to all public APIs
   - Include doc tests that demonstrate usage
   - Follow the project's copilot-instructions.md guidelines

5. **Test individual crates:**
   ```bash
   cargo test --package <crate-name> --all-features
   ```

## Troubleshooting

### Build Failures

1. **Check the specific job that failed** in the GitHub Actions tab
2. **Run the same command locally** to reproduce the issue
3. **Check for dependency conflicts:**
   ```bash
   cargo tree --duplicates
   ```

### Cache Issues

If builds are slow or failing due to cache corruption:

1. Clear local cache:
   ```bash
   cargo clean
   ```

2. In GitHub Actions, you can delete the cache from the Actions tab

### Dependency Issues

1. **Update all dependencies:**
   ```bash
   cargo update
   ```

2. **Check for security advisories:**
   ```bash
   cargo deny check
   ```

3. **Find unused dependencies:**
   ```bash
   cargo +nightly udeps --workspace --all-features
   ```

## Maintenance

### Regular Tasks

- **Weekly:** Review security audit results
- **Monthly:** Update dependencies and check for breaking changes
- **As needed:** Update MSRV (Minimum Supported Rust Version) in CI matrix

### Updating Rust Versions

Update the matrix in `.github/workflows/CI.yaml`:

```yaml
matrix:
  rust: [stable, beta, 1.85.0]  # Update MSRV here
```

## Additional Resources

- [Cargo Workspaces Documentation](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [GitHub Actions for Rust](https://github.com/actions-rs)
- [cargo-deny Documentation](https://embarkstudios.github.io/cargo-deny/)
- [Project Copilot Instructions](.github/copilot-instructions.md)

