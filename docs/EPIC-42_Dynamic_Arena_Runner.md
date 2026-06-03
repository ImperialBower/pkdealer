# EPIC-42: Dynamic Arena Runner

## Status

| Component | Status |
|---|---|
| `arena.toml` player registry | Not started |
| `bin/arena` CLI driver (compose override generation) | Not started |
| Multiplicity syntax (`gto gto` → two uniquely-named containers) | Not started |
| Seat-count validation (≤ 9) + unknown-name help text | Not started |
| `pkdealer_agent_gemini` crate (`gwen`) | Not started |
| `gwen` registered in arena registry | Not started |
| `Dockerfile.agent` support for gemini binary | Not started |

---

## Context

`bin/aiarena` hard-codes a fixed six-seat table: three rule bots (gto, lag, tag)
and three Ollama LLMs (llama, mistral, gemma). Changing the lineup — adding a
second GTO bot, swapping in a Claude agent, or putting five rule bots against a
single Gemini player — requires editing `docker-compose.yml` and committing a new
profile. There is no way to compose an ad-hoc table from the terminal.

The goal of this epic is a **dynamic arena runner**: pass a space-separated player
list on the command line and get exactly those containers — no edits required.

```bash
./bin/arena gto gto lag llama gwen
./bin/arena claude gto tag ssn maniac lp
./bin/arena gto:3 llama claude          # colon multiplicity shorthand
```

The service, OTel collector, Jaeger, Prometheus, and Grafana start as before. Only
the agent containers change. All existing tooling (pkspectator, EPIC-25 recording,
EPIC-41 scenarios, OTel traces) continues to work without modification because the
agents connect to the service over the same gRPC interface.

**Decisions:**

- **Shell-script driver** (`bin/arena`) following the existing `bin/aiarena` style.
  A generated YAML override file keeps compose as the orchestrator so health
  checks, log tailing (`docker compose logs -f`), and teardown (`docker compose
  down -v`) continue to work.
- **Registry in `arena.toml`** — a TOML file at the repo root mapping short names
  to agent type + config. The bash script sources a small helper that reads it.
- **Multiplicity** via repetition (`gto gto`) or colon shorthand (`gto:2`).
- **`gwen`** — a new `pkdealer_agent_gemini` crate implementing `LlmBackend` for
  Google Gemini. Follows the same pattern as `pkdealer_agent_ollama` (EPIC-40).

---

## Architecture & Phases

### Phase 1 — Player registry (`arena.toml`)

A TOML file at the repo root, consumed both by `bin/arena` and by any future Rust
tooling.

```toml
# arena.toml — maps short names to agent type + config.
# type must be one of: rules | ollama | claude | gemini

[players.gto]
type    = "rules"
profile = "gto"

[players.lag]
type    = "rules"
profile = "loose_aggressive"

[players.tag]
type    = "rules"
profile = "tight_aggressive"

[players.tp]
type    = "rules"
profile = "tight_passive"

[players.lp]
type    = "rules"
profile = "loose_passive"

[players.maniac]
type    = "rules"
profile = "maniac"

[players.abc]
type    = "rules"
profile = "abc"

[players.ssn]
type    = "rules"
profile = "short_stack_ninja"

[players.joker]
type    = "rules"
profile = "joker"

[players.llama]
type  = "ollama"
model = "llama3.1"

[players.mistral]
type  = "ollama"
model = "mistral"

[players.gemma]
type  = "ollama"
model = "gemma2"

[players.claude]
type  = "claude"
model = "claude-sonnet-4-6"

[players.gwen]
type  = "gemini"
model = "gemini-2.0-flash"
```

The registry is additive — new entries require no script changes.

### Phase 2 — `bin/arena` shell driver

Usage:

```
./bin/arena PLAYER [PLAYER ...] [--help]

PLAYER may be:
  <name>          — one instance (e.g. gto)
  <name>:<n>      — n instances   (e.g. gto:2)

Known names are listed in arena.toml.
```

Algorithm:

1. **Parse args** — expand `gto:2` into `(gto, gto)`. Reject unknown names with a
   list of valid names from `arena.toml`. Reject counts that would exceed 9 seats.
2. **Assign instance IDs** — track per-name counters so that `gto gto` produces
   `gto_1` and `gto_2` (unique service names, unique `--name` flags, unique
   `OTEL_SERVICE_NAME` values).
3. **Generate `docker-compose.arena.yml`** — write to `/tmp` with a randomised
   suffix so concurrent runs don't collide. Each player becomes a fully-inlined
   service block (no cross-file YAML anchors):

   ```yaml
   services:
     agent_gto_1:
       image: pkdealer/agent_rules:latest
       build:
         context: .
         dockerfile: Dockerfile.agent
         args:
           BIN_NAME: pkdealer_agent_rules
       depends_on: [pkdealer_service]
       restart: unless-stopped
       command: ["--name", "gto_1", "--profile", "gto"]
       environment:
         PKDEALER_ENDPOINT: http://pkdealer_service:50051
         OTEL_EXPORTER_OTLP_ENDPOINT: http://otel-collector:4317
         OTEL_SERVICE_NAME: agent_gto_1
     agent_gto_2:
       ... identical except name suffix = 2 ...
   ```

   LLM service blocks add the provider-specific env vars:
   - `ollama` → `OLLAMA_HOST: http://host.docker.internal:11434`, `OLLAMA_MODEL`
   - `claude` → `ANTHROPIC_API_KEY` (read from the calling shell's env),
     `ANTHROPIC_MODEL`
   - `gemini` → `GEMINI_API_KEY` (from env), `GEMINI_MODEL`

4. **Soft-check prerequisites** — same pattern as `bin/aiarena`:
   - Ollama reachable warning when an `ollama` player is requested.
   - Missing `ANTHROPIC_API_KEY` warning when `claude` is requested.
   - Missing `GEMINI_API_KEY` warning when `gwen`/`gemini` is requested.

5. **Bring up the stack**:

   ```bash
   docker compose \
     -f docker-compose.yml \
     -f "$OVERRIDE_FILE" \
     up -d --build
   ```

   The base `docker-compose.yml` still starts infra (service, collector, Jaeger,
   Prometheus, Grafana) with no profile flag; only the agent services come from the
   override.

6. **Wait for `:50051`** — same readiness loop as `bin/aiarena`.
7. **Print dashboard URLs** and teardown instructions, including:
   ```
   Tear down:  docker compose -f docker-compose.yml -f /tmp/... down -v
   ```
   (the override file path is echoed for the user to copy).

### Phase 3 — `pkdealer_agent_gemini` crate ("gwen")

Follows the EPIC-40 pattern exactly. `pkdealer_agent_ollama` is the reference.

New crate: `crates/pkdealer_agent_gemini`

**`src/lib.rs` — `GeminiBackend`:**

```rust
pub struct GeminiBackend {
    client:    reqwest::Client,
    api_key:   String,
    model:     String,
    base_url:  String,   // injectable for mock tests
}

#[async_trait]
impl LlmBackend for GeminiBackend {
    async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError> {
        // POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent
        // Request body: { "contents": [{ "parts": [{ "text": prompt }] }] }
        // Response mapping:
        //   candidates[0].content.parts[0].text          → LlmResponse.text
        //   usageMetadata.promptTokenCount                → input_tokens
        //   usageMetadata.candidatesTokenCount            → output_tokens
    }
}
```

**`src/main.rs`** — 20 lines of wiring:

```rust
let backend = GeminiBackend::new(api_key, args.model.clone());
let agent   = LlmPokerAgent::with_model(backend, "google", args.model.clone());
run_agent(agent, config).await?;
```

CLI flags / env vars (mirror the Claude agent):

| Flag | Env var | Default |
|---|---|---|
| `--model` | `GEMINI_MODEL` | `gemini-2.0-flash` |
| *(implicit)* | `GEMINI_API_KEY` | *(required)* |
| `--endpoint` | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` |

Tests follow the same mock-TCP-listener pattern used in `pkdealer_agent_ollama`.

Add to `Cargo.toml` workspace `members`, and add `BIN_NAME = pkdealer_agent_gemini`
support to `Dockerfile.agent` (no changes needed — the Dockerfile already builds
any binary by name).

### Phase 4 — Ergonomics & docs

- Update `DEMO.md` with `bin/arena` examples including the colon multiplicity syntax.
- Update `bin/botarena` and `bin/aiarena` headers to note that `bin/arena` is the
  flexible successor.
- Add `bin/arena --help` output that lists all registry entries and their types.
- Add `make arena` target to `Makefile` as a convenience alias.

---

## Files to create / modify

| File | Action |
|---|---|
| `arena.toml` | Create — player registry |
| `bin/arena` | Create — dynamic arena runner script |
| `crates/pkdealer_agent_gemini/Cargo.toml` | Create |
| `crates/pkdealer_agent_gemini/src/lib.rs` | Create — `GeminiBackend` |
| `crates/pkdealer_agent_gemini/src/main.rs` | Create — CLI wiring |
| `Cargo.toml` | Modify — add `pkdealer_agent_gemini` to workspace members |
| `DEMO.md` | Modify — add `bin/arena` usage examples |
| `Makefile` | Modify — add `arena` target |

---

## Example sessions

```bash
# Classic 6-seat mixed arena (equivalent to bin/aiarena):
./bin/arena gto lag tag llama mistral gemma

# Two GTO bots + a Claude agent + three rule archetypes:
./bin/arena gto:2 claude tag maniac lp

# All-bot 9-seat ring (equivalent to bin/botarena):
./bin/arena gto lag tag tp lp maniac abc ssn joker

# LLM shootout — three different models head to head:
./bin/arena claude llama gwen

# Quick 3-seat debug table:
./bin/arena gto lag claude
```

---

## Verification

1. **Registry parsing:** `./bin/arena --help` lists all 14 known player names with
   their type.
2. **Unknown name rejection:** `./bin/arena gto unknown` exits non-zero and prints
   the list of valid names.
3. **Seat overflow rejection:** `./bin/arena gto:10` exits with a clear error
   (9-seat limit).
4. **Compose override shape:** for `./bin/arena gto gto lag`, inspect the generated
   `/tmp/docker-compose.arena.*.yml` — must have three service blocks:
   `agent_gto_1`, `agent_gto_2`, `agent_lag_1`, each with distinct `--name` flags
   and `OTEL_SERVICE_NAME` values.
5. **End-to-end (manual):**
   - `./bin/arena gto lag llama` — confirm three containers start, the table plays
     hands, traces appear in Jaeger under `agent_gto_1`, `agent_lag_1`,
     `agent_llama_1`.
   - `./bin/arena gto:2 claude` — confirm two GTO containers + one Claude container.
   - `./bin/arena claude llama gwen` — confirm Gemini agent connects and decides
     (requires `GEMINI_API_KEY` set in the calling shell).
6. **`pkdealer_agent_gemini` unit tests:** `cargo test -p pkdealer_agent_gemini` —
   mock HTTP backend test verifies `GeminiBackend::complete` maps
   `candidates[0].content.parts[0].text` → `LlmResponse.text` and
   `usageMetadata.*TokenCount` → token fields.
7. **Workspace build:** `OTEL_SDK_DISABLED=true cargo test --workspace` passes;
   `cargo clippy --workspace --all-targets -- -D warnings` is clean.
8. Every new public fn/struct gets doc comment + doctest + unit test per
   `CLAUDE.md`; `cargo test --doc -p pkdealer_agent_gemini` passes.
