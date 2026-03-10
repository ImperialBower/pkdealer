[![CI](https://github.com/ImperialBower/pkdealer/actions/workflows/CI.yaml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/CI.yaml)
[![Workspace Check](https://github.com/ImperialBower/pkdealer/actions/workflows/workspace-check.yaml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/workspace-check.yaml)
[![Security Audit](https://github.com/ImperialBower/pkdealer/actions/workflows/audit.yml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/audit.yml)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0.en.html)

---

# PKDealer — gRPC Poker Dealer Service

PKDealer is a Rust workspace providing a gRPC poker dealer service, a matching gRPC client, and
shared Protobuf definitions. The service manages a poker table: seating players, dealing hands,
processing actions (bet / call / raise / fold), advancing streets, and resolving showdowns.

---

## Table of Contents

- [Repository Structure](#repository-structure)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
- [Building](#building)
- [Running](#running)
- [Testing](#testing)
- [Development Workflow](#development-workflow)
- [Make Targets Reference](#make-targets-reference)
- [Configuration](#configuration)
- [CI and Workflows](#ci-and-workflows)
- [Private Dependency Authentication](#private-dependency-authentication)
- [Contributing](#contributing)
- [License](#license)

---

## Repository Structure

```
pkdealer/
├── Cargo.toml               # Workspace root
├── Makefile                 # Developer convenience targets
├── deny.toml                # cargo-deny configuration
├── crates/
│   ├── pkdealer_proto/      # Shared Protobuf definitions + generated Rust types
│   │   ├── proto/dealer.proto
│   │   ├── build.rs         # tonic-build code generation
│   │   └── src/lib.rs
│   ├── pkdealer_service/    # gRPC server binary
│   │   ├── src/main.rs
│   │   └── tests/e2e_ping.rs
│   └── pkdealer_client/     # gRPC client binary
│       └── src/main.rs
└── docs/notes/              # Development notes and decision records
```

### Crate Roles

| Crate | Type | Purpose |
|---|---|---|
| `pkdealer_proto` | library | Protobuf schema (`dealer.proto`) + tonic-generated Rust types |
| `pkdealer_service` | binary | gRPC server that implements `DealerService` |
| `pkdealer_client` | binary | gRPC client that connects to the service |

---

## Prerequisites

| Tool | Version | Install |
|---|---|---|
| Rust toolchain | ≥ 1.85 (edition 2024) | `rustup update stable` |
| Rust nightly | for `cargo-udeps` only | `rustup toolchain install nightly` |
| `cargo-deny` | latest | `cargo install cargo-deny` |
| `cargo-udeps` | latest | `cargo install cargo-udeps` |
| `cargo-watch` | optional | `cargo install cargo-watch` |
| GNU make | any | pre-installed on Linux; `brew install make` on macOS |

> **macOS note:** the system `make` is BSD make. If you hit GNU-specific errors use `gmake`
> (installed by `brew install make`).

Install all optional cargo tools at once:

```sh
make install-tools
```

---

## Getting Started

```sh
git clone https://github.com/ImperialBower/pkdealer.git
cd pkdealer

# Build the entire workspace
cargo build --workspace

# Or use the Makefile shortcut
make build
```

If `pkcore` (a private git dependency used by `pkdealer_proto`) cannot be fetched, see
[Private Dependency Authentication](#private-dependency-authentication).

---

## Building

```sh
# Debug build (all crates, all features)
cargo build --workspace --all-features

# Release build
cargo build --workspace --all-features --release

# Single crate
cargo build -p pkdealer_service
cargo build -p pkdealer_client
cargo build -p pkdealer_proto

# Makefile shortcuts
make build          # debug
make build-release  # release
```

The `pkdealer_proto` build script (`build.rs`) uses `protoc-bin-vendored` to compile
`proto/dealer.proto` — no separate `protoc` installation is required.

---

## Running

### Service

```sh
# Debug binary
cargo run -p pkdealer_service

# Release binary
cargo run -p pkdealer_service --release

# Custom bind address (default: 127.0.0.1:50051)
PKDEALER_ADDR=0.0.0.0:9090 cargo run -p pkdealer_service
```

### Client

```sh
# Connect to the default address and send a ping
cargo run -p pkdealer_client

# Override endpoint or client-id via environment variables
PKDEALER_ENDPOINT=http://127.0.0.1:9090 \
PKDEALER_CLIENT_ID=my-client \
cargo run -p pkdealer_client
```

---

## Testing

```sh
# All tests (unit + integration)
cargo test --workspace --all-features

# With printed output
cargo test --workspace --all-features -- --nocapture

# Doc tests only
cargo test --doc

# Single crate
cargo test -p pkdealer_service --all-features
cargo test -p pkdealer_client  --all-features

# End-to-end ping test (starts the service binary automatically)
cargo test -p pkdealer_service --test e2e_ping

# Makefile shortcuts
make test
make test-verbose
make test-service
make test-client
```

---

## Development Workflow

### Quick compile check (no binary output)

```sh
make check
# or
cargo check --workspace --all-features
```

### Linting

```sh
# Standard clippy
make clippy

# Pedantic (same flags as CI)
make clippy-pedantic
```

### Formatting

```sh
# Format in place
make fmt

# Check only (no changes written — used by CI)
make fmt-check
```

### Documentation

```sh
# Generate docs (no-deps, all features, private items)
make doc

# Generate and open in browser
make doc-open
```

### Watch mode

Requires `cargo-watch` (`make install-watch`):

```sh
make watch
```

### Dependency tree

```sh
make tree              # full tree
make tree-duplicates   # highlight duplicates
```

### Security audit

```sh
make audit             # cargo-deny advisories check
```

### Unused dependency check (nightly)

```sh
make unused-deps
```

---

## Make Targets Reference

Run `make help` to print a summary at any time.

| Target | Description |
|---|---|
| `make build` | Debug build of the full workspace |
| `make build-release` | Release build |
| `make test` | Run all workspace tests |
| `make test-verbose` | Tests with `--nocapture` |
| `make test-service` | Tests for `pkdealer_service` only |
| `make test-client` | Tests for `pkdealer_client` only |
| `make check` | Fast compile check (no output) |
| `make fmt` | Auto-format all code |
| `make fmt-check` | Check formatting without modifying |
| `make clippy` | Run clippy |
| `make clippy-pedantic` | Run clippy with `-Dclippy::pedantic` |
| `make doc` | Generate workspace docs |
| `make doc-open` | Generate docs and open in browser |
| `make clean` | Remove all build artifacts |
| `make update` | `cargo update` |
| `make tree` | Dependency tree |
| `make tree-duplicates` | Highlight duplicate deps |
| `make audit` | Security audit via `cargo-deny` |
| `make unused-deps` | Unused dep check (nightly) |
| `make ci-quick` | `fmt-check` + `check` + `test` |
| `make ci-local` | Full local CI: `fmt-check clippy-pedantic test doc` |
| `make ayce` | Full pipeline: `fmt build test clippy doc` |
| `make install-tools` | Install `cargo-deny` and `cargo-udeps` |
| `make watch` | Watch mode (requires `cargo-watch`) |

---

## Configuration

### Service bind address

| Variable | Default | Description |
|---|---|---|
| `PKDEALER_ADDR` | `127.0.0.1:50051` | Address the service listens on |

### Client

| Variable | Default | Description |
|---|---|---|
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Service endpoint the client connects to |
| `PKDEALER_CLIENT_ID` | `pkdealer-client` | Client identifier sent in ping requests |

---

## CI and Workflows

| Workflow | File | Trigger | What it does |
|---|---|---|---|
| CI | `CI.yaml` | push / PR | fmt-check, clippy-pedantic, test, doc |
| Workspace Check | `workspace-check.yaml` | push / PR | `cargo-deny`, `cargo-udeps` |
| Security Audit | `audit.yml` | schedule + push | `cargo audit` advisory scan |

---

## Private Dependency Authentication

`pkdealer_proto` depends on `pkcore`, which is hosted in a private GitHub repository
(`ImperialBower/pkcore`). Cargo must be able to authenticate before resolving dependencies.

### Local development

Generate a [GitHub personal access token](https://github.com/settings/tokens) with `repo` (read)
scope, then configure git:

```sh
git config --global \
  url."https://x-access-token:<YOUR_TOKEN>@github.com/".insteadOf \
  "https://github.com/"
```

### GitHub Actions

Add the token as a repository secret named `PKCORE_READ_TOKEN`, then insert the following step
**immediately after `actions/checkout`** in every job that runs Cargo:

```yaml
- uses: actions/checkout@v4

- name: Auth for private git dependencies
  run: |
    git config --global \
      url."https://x-access-token:${{ secrets.PKCORE_READ_TOKEN }}@github.com/".insteadOf \
      "https://github.com/"

- uses: dtolnay/rust-toolchain@stable
```

This must appear in every job that fetches dependencies (`build`, `test`, `clippy`, `cargo-deny`,
`cargo-udeps`, etc.).

---

## Contributing

Please read [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before contributing. All contributions are
expected to follow the [Contributor Covenant](https://www.contributor-covenant.org/) v2.1.

---

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE-GPL3.0).

---

## Rust Resources

- [The Rust Programming Language](https://doc.rust-lang.org/book/)
- [Cargo Guide](https://doc.crates.io/guide.html)
- [Asynchronous Programming in Rust](https://rust-lang.github.io/async-book/)
- [tonic gRPC for Rust](https://github.com/hyperium/tonic)
- [prost Protobuf for Rust](https://github.com/tokio-rs/prost)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
