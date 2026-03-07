[![CI](https://github.com/ImperialBower/pkgrpc/actions/workflows/CI.yaml/badge.svg)](https://github.com/ImperialBower/pkgrpc/actions/workflows/CI.yaml)
[![Workspace Check](https://github.com/ImperialBower/pkgrpc/actions/workflows/workspace-check.yaml/badge.svg)](https://github.com/ImperialBower/pkgrpc/actions/workflows/workspace-check.yaml)
[![Security Audit](https://github.com/ImperialBower/pkgrpc/actions/workflows/audit.yml/badge.svg)](https://github.com/ImperialBower/pkgrpc/actions/workflows/audit.yml)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0.en.html)

---

# pkgrpc - Poker Dealer gRPC Service

A Rust implementation of a poker dealer service using gRPC for client-server communication.

## Overview

This workspace contains two binary crates:

- **pkdealer_service** - gRPC server providing poker dealer functionality
- **pkdealer_client** - CLI client for interacting with the dealer service

## Features

- 🎴 **Card Management** - Shuffle and deal cards
- 🎮 **Game State** - Track and manage poker game state
- 🔐 **Type Safety** - Leverages Rust's type system for correctness
- 🚀 **High Performance** - Built with Tokio and Tonic
- 📡 **gRPC Communication** - Efficient client-server protocol

## Quick Start

### Prerequisites

- Rust 1.85.0 or later
- Protocol Buffers compiler (protoc) - for gRPC code generation

### Building

```bash
# Build entire workspace
cargo build --workspace

# Build individual crates
cargo build --package pkdealer_service
cargo build --package pkdealer_client

# Release build
cargo build --workspace --release
```

### Running

```bash
# Start the service
cargo run --bin pkdealer_service

# In another terminal, run the client
cargo run --bin pkdealer_client
```

### Testing

```bash
# Run all tests
cargo test --workspace

# Test with output
cargo test --workspace -- --nocapture

# Test specific crate
cargo test -p pkdealer_service
cargo test -p pkdealer_client

# Using Makefile
make test
```

## Project Structure

```
pkgrpc/
├── Cargo.toml                    # Workspace configuration
├── crates/
│   ├── pkdealer_service/        # gRPC server
│   │   ├── Cargo.toml           # Service configuration
│   │   ├── README.md            # Service documentation
│   │   └── src/
│   │       └── main.rs          # Service implementation
│   └── pkdealer_client/         # gRPC client
│       ├── Cargo.toml           # Client configuration
│       ├── README.md            # Client documentation
│       └── src/
│           └── main.rs          # Client implementation
├── deny.toml                     # Dependency license checking
└── .github/
    └── workflows/               # CI/CD pipelines
```

## Development

### Code Quality Standards

This project maintains high code quality:

- ✅ **Documentation**: Comprehensive docs with doc tests
- ✅ **Testing**: Unit tests for all public functions
- ✅ **Linting**: Strict clippy lints (pedantic mode)
- ✅ **Formatting**: Consistent with rustfmt
- ✅ **Safety**: No `unwrap()` or `panic!()` in production code
- ✅ **License Compliance**: GPL-3.0-or-later with dependency checking

See [`.github/copilot-instructions.md`](.github/copilot-instructions.md) for detailed guidelines.

### Development Commands

```bash
# Format code
cargo fmt --workspace
make fmt

# Check formatting
cargo fmt --workspace --check
make fmt-check

# Lint with clippy
cargo clippy --workspace --all-targets
make clippy

# Generate documentation
cargo doc --workspace --open
make doc-open

# Check dependencies
cargo deny check
make audit

# Run all CI checks locally
make ci-local
```

## CI/CD

GitHub Actions workflows automatically:
- ✅ Build and test on multiple Rust versions (stable, beta, nightly, MSRV 1.85.0)
- ✅ Run Clippy with pedantic warnings
- ✅ Check code formatting
- ✅ Generate documentation
- ✅ Audit dependencies for security issues
- ✅ Check license compliance

See [`.github/WORKFLOWS.md`](.github/WORKFLOWS.md) for detailed workflow documentation.

## Documentation

### Project Documentation
- **Workflow Guide**: [`.github/WORKFLOWS.md`](.github/WORKFLOWS.md)
- **License Compatibility**: [`GPL_LICENSE_COMPATIBILITY.md`](GPL_LICENSE_COMPATIBILITY.md)
- **Workspace Commands**: [`WORKSPACE_COMMANDS.md`](WORKSPACE_COMMANDS.md)
- **cargo-deny Quick Start**: [`CARGO_DENY_QUICKSTART.md`](CARGO_DENY_QUICKSTART.md)
- **Subprojects Overview**: [`SUBPROJECTS_ADDED.md`](SUBPROJECTS_ADDED.md)

### Crate Documentation
- **Service**: [`crates/pkdealer_service/README.md`](crates/pkdealer_service/README.md)
- **Client**: [`crates/pkdealer_client/README.md`](crates/pkdealer_client/README.md)

## License

This project is licensed under GPL-3.0-or-later.

See [LICENSE-GPL3.0](LICENSE-GPL3.0) for details.

### Dependency Licenses

All dependencies are checked for GPL compatibility using `cargo-deny`. See [`GPL_LICENSE_COMPATIBILITY.md`](GPL_LICENSE_COMPATIBILITY.md) for details on compatible licenses.

## Contributing

Contributions are welcome! Please:

1. Read the [Code of Conduct](CODE_OF_CONDUCT.md)
2. Follow the coding guidelines in [`.github/copilot-instructions.md`](.github/copilot-instructions.md)
3. Ensure all tests pass: `cargo test --workspace`
4. Run clippy: `cargo clippy --workspace --all-targets`
5. Format code: `cargo fmt --workspace`
6. Check licenses: `cargo deny check`

## Roadmap

- [ ] Implement gRPC protocol definitions
- [ ] Add card shuffling and dealing logic
- [ ] Implement game state management
- [ ] Add authentication and authorization
- [ ] Add comprehensive integration tests
- [ ] Performance benchmarks
- [ ] Docker containerization
- [ ] Kubernetes deployment configs

## Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Tonic gRPC Guide](https://github.com/hyperium/tonic)
- [Tokio Documentation](https://tokio.rs/)
- [Project Coding Guidelines](.github/copilot-instructions.md)

