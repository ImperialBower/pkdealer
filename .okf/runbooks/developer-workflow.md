---
type: Runbook
title: Developer workflow — build, test, lint
description: The everyday commands and CI-equivalent checks for the pkdealer workspace.
tags: [make, cargo, ci, testing, runbook]
timestamp: 2026-07-22T13:10:00Z
---

GNU make wraps everything (`make help` lists all targets; on macOS use
`gmake` if BSD make errors).

# Examples

```sh
make build            # cargo build --workspace (debug)
make test             # cargo test --workspace --all-features
make check            # fast compile check
make clippy-pedantic  # clippy with -Dclippy::pedantic (same as CI)
make fmt / fmt-check  # format / verify formatting
make ci-local         # full local CI: fmt-check clippy-pedantic test doc
make audit            # cargo-deny advisories check
```

# CI workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `CI.yaml` | push / PR | fmt-check, clippy-pedantic, test, doc |
| `workspace-check.yaml` | push / PR | `cargo-deny`, `cargo-udeps` |
| `audit.yml` | schedule + push | `cargo audit` advisory scan |

# Project conventions

* Rust ≥ 1.85, edition 2024; nightly only for `cargo-udeps`.
* No `unwrap()` / `expect()` / `panic!()` in library code; every public item
  needs doc comments with doc tests and unit tests (see repo `CLAUDE.md`).
* Test fn names are not prefixed with `test_`.
