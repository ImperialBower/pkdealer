---
type: Reference
title: AI Bill of Materials
description: AI-BOM.md at the repo root — the inventory of AI dev tools, context files, algorithms, external model integrations, cost accounting and gen_ai telemetry for pkdealer.
resource: https://github.com/ImperialBower/pkdealer/blob/main/AI-BOM.md
tags: [ai-bom, sbom, provenance, llm, telemetry, cost]
timestamp: 2026-08-30T12:00:00Z
---

`AI-BOM.md` at the repository root is pkdealer's AI Bill of Materials: an
SBOM-style inventory of every AI component in the repo. `pkcore` keeps a
matching file, and the two are meant to be read together — `pkcore` is the pure
engine with **zero** external AI dependencies, while pkdealer is where the live
model calls actually happen.

# Sections

| § | Contents |
|---|----------|
| 1 | Development tools used to author the code |
| 2 | AI context infrastructure — `CLAUDE.md`, this bundle, `arena.toml`, `pricing.toml` |
| 3 | AI audits recorded under `docs/audits/` |
| 4 | External model integrations: Anthropic Claude, Ollama, planned Gemini |
| 5 | Algorithms implemented in the workspace, per crate |
| 6 | Cost accounting via `AgentFidelity` → `pkdealer_pricing` → `pkdealer_costsim` |
| 7 | `gen_ai.*` and `poker.*` OpenTelemetry attributes |
| 8 | Pointers to the EPIC docs behind each entry |

# Maintenance

Refresh it whenever a change touches what it lists: a new agent crate or model
backend, a new audit report, a change to the `gen_ai.*`/`poker.*` attribute set,
a workspace or `pkcore` version bump, or a new concept count in this bundle.
Update the `_Last updated:_` line in the same pass.

Related: [Workspace crates](/crates/index.md),
[Observability](/runbooks/observability.md),
[EPIC docs](/references/epic-docs.md).
