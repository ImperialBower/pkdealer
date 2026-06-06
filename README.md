[![CI](https://github.com/ImperialBower/pkdealer/actions/workflows/CI.yaml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/CI.yaml)
[![Workspace Check](https://github.com/ImperialBower/pkdealer/actions/workflows/workspace-check.yaml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/workspace-check.yaml)
[![Security Audit](https://github.com/ImperialBower/pkdealer/actions/workflows/audit.yml/badge.svg)](https://github.com/ImperialBower/pkdealer/actions/workflows/audit.yml)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE-APACHE)

---

# PKDealer — gRPC Poker Dealer Service

PKDealer is a Rust workspace providing a gRPC poker dealer service, a matching gRPC client,
shared Protobuf definitions, and a family of bot agents. The service manages a poker table:
seating players, dealing hands, processing actions (bet / call / raise / fold), advancing
streets, and resolving showdowns.

Agents connect over gRPC and play autonomously — rule-based archetypes from `pkcore`, a random
baseline, and live LLM players backed by Claude or a local Ollama model. The **arena** scripts
compose ad-hoc tables from these agents and bring up a full OpenTelemetry observability stack
(collector + Jaeger + Prometheus + Grafana) alongside the dealer. A browser spectator lives in
the separate [`pkspectator`](https://github.com/ImperialBower/pkspectator) repo.

---

## Table of Contents

- [Repository Structure](#repository-structure)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
- [Building](#building)
- [Running](#running)
- [Agents and the Arena](#agents-and-the-arena)
- [Observability](#observability)
- [Testing](#testing)
- [Development Workflow](#development-workflow)
- [Make Targets Reference](#make-targets-reference)
- [Configuration](#configuration)
- [CI and Workflows](#ci-and-workflows)
- [Contributing](#contributing)
- [License](#license)

---

## Repository Structure

```
pkdealer/
├── Cargo.toml               # Workspace root (9 member crates)
├── Makefile                 # Developer convenience targets
├── deny.toml                # cargo-deny configuration
├── arena.toml               # Player registry for ./bin/arena (name → agent type)
├── docker-compose.yml       # Dealer + agents + OTel collector + Jaeger + Prometheus + Grafana
├── bin/                     # Launcher scripts (arena, aiarena, botarena, …)
├── ops/                     # Observability stack config (collector, Grafana, Prometheus)
├── proto/                   # Protobuf workspace assets
├── crates/
│   ├── pkdealer_proto/      # Shared Protobuf definitions + generated Rust types
│   │   ├── proto/dealer.proto
│   │   ├── build.rs         # tonic-build code generation
│   │   └── src/lib.rs
│   ├── pkdealer_service/    # gRPC server binary (the dealer)
│   ├── pkdealer_client/     # gRPC client binary
│   ├── pkdealer_agent_core/    # Shared gRPC agent infrastructure
│   ├── pkdealer_agent_random/  # Random baseline bot
│   ├── pkdealer_agent_rules/   # Rule-based bot driven by a pkcore BotProfile
│   ├── pkdealer_agent_llm/     # Shared LlmBackend trait + poker-prompt logic
│   ├── pkdealer_agent_claude/  # Claude LLM agent
│   └── pkdealer_agent_ollama/  # Ollama (local LLM) agent
└── docs/                    # EPIC specs, notes, and presentations
```

### Crate Roles

| Crate | Type | Purpose |
|---|---|---|
| `pkdealer_proto` | library | Protobuf schema (`dealer.proto`) + tonic-generated Rust types |
| `pkdealer_service` | binary | gRPC server that implements `DealerService` (the dealer) |
| `pkdealer_client` | binary | gRPC client that connects to the service |
| `pkdealer_agent_core` | library | Shared gRPC agent plumbing used by every bot |
| `pkdealer_agent_random` | binary | Random baseline bot agent |
| `pkdealer_agent_rules` | binary | Rule-based bot driven by a `pkcore` `BotProfile` archetype |
| `pkdealer_agent_llm` | library | Shared `LlmBackend` trait + poker-prompt logic for LLM agents |
| `pkdealer_agent_claude` | binary | Claude-backed LLM agent (OTel `gen_ai` instrumented) |
| `pkdealer_agent_ollama` | binary | Ollama-backed local-LLM agent (OTel `gen_ai` instrumented) |

The browser spectator lives in a separate repo: [`pkspectator`](https://github.com/ImperialBower/pkspectator).

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

## Agents and the Arena

Bot agents are standalone gRPC clients that seat themselves at the dealer and play
autonomously. Three launcher scripts in `bin/` bring up a dealer plus a line-up of agents
via Docker Compose, from simplest to most flexible:

| Script | Line-up | External deps | Notes |
|---|---|---|---|
| `./bin/botarena` | Full 9-handed ring of every `pkcore` rule archetype | none | Pure offline rule-bot shootout |
| `./bin/aiarena` | Fixed 3 rule bots + 3 local LLMs | Ollama on the host | The full demo stack — see [DEMO.md](DEMO.md) |
| `./bin/arena` | Any line-up you name | depends on agents chosen | Reads [`arena.toml`](arena.toml), generates a one-off compose override |

The **dynamic arena runner** composes ad-hoc tables from the registry in `arena.toml`,
which maps short names to an agent type (`rules`, `ollama`, `claude`, `gemini`) and config:

```sh
# Two GTO bots, a loose-aggressive bot, and one llama
./bin/arena gto gto lag llama

# Colon multiplicity shorthand: three GTO bots + one Claude
./bin/arena gto:3 claude

# Makefile wrappers
make arena PLAYERS="gto lag llama"
make arena-down            # force-tear-down ALL arena containers + volumes
```

Live LLM players need credentials in the calling shell: the `claude` agent requires
`ANTHROPIC_API_KEY` (live, billed). Rule bots and Ollama agents run fully locally.

---

## Observability

The service is OpenTelemetry-instrumented. `docker compose up -d --build` brings up the
dealer plus the full telemetry stack — OTel collector, Jaeger (traces), Prometheus (metrics),
and Grafana (dashboards). Stack config lives in `ops/`. The LLM agents emit `gen_ai`
spans so model calls are traceable end-to-end.

See [`crates/pkdealer_service/README.md`](crates/pkdealer_service/README.md) for environment
variables and the full quickstart. Toggle OTel off with `OTEL_SDK_DISABLED=true` when running
tests or `cargo run` without a collector.

```sh
make ddown                 # tear down the demo stack (docker compose down -v)
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
| `make serve` | Build and start the dealer service |
| `make ddown` | Tear down the demo stack (`docker compose down -v`) |
| `make demo` | Run the 9-player client demo (service must be running) |
| `make demo-audit [COUNT=N]` | Run demo + audit N times (default 1) |
| `make arena [PLAYERS="gto lag llama"]` | Launch an ad-hoc arena table (see `./bin/arena --help`) |
| `make arena-down` | Force-tear-down ALL arena containers + volumes |
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

## Contributing

Please read [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before contributing. All contributions are
expected to follow the [Contributor Covenant](https://www.contributor-covenant.org/) v2.1.

---

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

---

## Rust Resources

- [The Rust Programming Language](https://doc.rust-lang.org/book/)
- [Cargo Guide](https://doc.crates.io/guide.html)
- [Asynchronous Programming in Rust](https://rust-lang.github.io/async-book/)
- [tonic gRPC for Rust](https://github.com/hyperium/tonic)
- [prost Protobuf for Rust](https://github.com/tokio-rs/prost)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
