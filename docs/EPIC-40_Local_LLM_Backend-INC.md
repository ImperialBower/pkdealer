# EPIC-40: Local-LLM Backend & Multi-Model Agents

## Status

| Component | Status |
|---|---|
| `pkdealer_agent_llm` — shared `LlmBackend` trait + poker prompt/parse | Complete |
| `pkdealer_agent_claude` refactored onto `LlmBackend` | Complete |
| `pkdealer_agent_ollama` — Ollama-backed agent binary | Complete |
| Mock-HTTP backend tests (Claude + Ollama) | Complete |
| Live smoke test against `ollama serve` (llama3.1) | Pending |
| `pkdealer_agent_openai` (or similar) sibling backend | Future |
| Per-backend prompt tuning | Future |
| Backend comparison harness (same hand, multiple models) | Future |

---

## Context

EPIC-23 introduced a single LLM-driven agent — `pkdealer_agent_claude` — built
as a 740-line monolithic binary that talked directly to Anthropic's Messages
API. That worked for one backend, but the next time we wanted to add a model
provider (Ollama for local, free dev; later OpenAI / Gemini / etc.) the only
path was copy-paste: each binary would re-implement prompt construction,
response parsing, OTel span emission, and fallback logic. The poker-side code
would drift between agents over time, and adding a fourth backend would mean a
fourth near-duplicate crate.

EPIC-40 fixes the abstraction. The poker-side logic — turning a `HandState`
into a prompt, parsing free text into a `Decision`, choosing a safe fallback
on error — is extracted into `pkdealer_agent_llm`. Each model provider becomes
a thin `LlmBackend` impl owning only its HTTP transport, auth, and
request/response shape. Adding the next backend becomes a single struct +
trait impl, not a whole crate from scratch.

The first dividend of the new abstraction is `pkdealer_agent_ollama`: a
local-LLM agent that runs against [Ollama](https://ollama.com/) with no API
key and no per-token cost. It makes the demo cheaper to run, makes CI viable
(no secrets needed for live tests), and lets us swap models (`llama3.1`,
`mistral`, `gemma2`, …) with a CLI flag.

---

## Architecture

### New library crate: `crates/pkdealer_agent_llm`

The home for everything LLM-shared but backend-agnostic.

```rust
#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError>;
}

pub struct LlmResponse {
    pub text: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}
```

A backend owns the model-specific concerns: base URL, auth, request body
shape, response field mapping. Everything above that line is shared:

| Module | Provides |
|---|---|
| `backend` | `LlmBackend` trait, `LlmResponse`, `LlmError` |
| `prompt` | `build_prompt(&HandState) -> String`, `pot_odds(&HandState) -> f64` |
| `parse` | `parse_action(&str, u32) -> Decision` |
| `agent` | Generic `LlmPokerAgent<B>` implementing `PokerAgent`; OTel span emission with `gen_ai.*` attributes; `fallback_decision()` |

`LlmPokerAgent<B>` is the bridge: it composes any `LlmBackend` with the
shared prompt-builder, response-parser, and OTel instrumentation to satisfy
`pkdealer_agent_core::PokerAgent`. Backend authors only write HTTP code.

### Refactor: `crates/pkdealer_agent_claude`

The crate gained a `src/lib.rs` exposing `ClaudeBackend` (with both
`ClaudeBackend::new` and `ClaudeBackend::with_base_url` for mock testing);
`src/main.rs` shrank to argument parsing, OTel init, and four lines of
wiring:

```rust
let backend = ClaudeBackend::new(api_key, args.model.clone(), args.max_tokens);
let agent = LlmPokerAgent::with_model(backend, "anthropic", args.model.clone());
run_agent(agent, config).await?;
```

All poker-side logic (prompt, parse, pot odds, fallback) moved out of the
crate and into `pkdealer_agent_llm`.

### New crate: `crates/pkdealer_agent_ollama`

Mirrors the post-refactor Claude crate. `OllamaBackend` POSTs to
`{host}/api/chat` with `stream: false` and maps response fields:

| Ollama response field | `LlmResponse` field |
|---|---|
| `message.content` | `text` |
| `prompt_eval_count` | `input_tokens` |
| `eval_count` | `output_tokens` |

No auth header. Configuration:

| CLI flag | Env var | Default |
|---|---|---|
| `--host` | `OLLAMA_HOST` | `http://localhost:11434` |
| `--model` | `OLLAMA_MODEL` | `llama3.1` |

Tests include a hand-rolled TCP-listener fixture that returns one canned JSON
response and verifies the field mapping, so the backend can be exercised in
CI without `ollama serve` running.

### Workspace

Both new crates are added to the root `[workspace] members`. `pkdealer_agent_core`
is untouched — random and rules agents remain LLM-free and don't depend on the
new shared crate.

---

## Work Items

1. **Done** — Create `crates/pkdealer_agent_llm` with `LlmBackend` trait, `LlmResponse`, `LlmError`, `build_prompt`, `pot_odds`, `parse_action`, `fallback_decision`, and generic `LlmPokerAgent<B>`.
2. **Done** — Refactor `pkdealer_agent_claude`: add `src/lib.rs` with `ClaudeBackend`; shrink `src/main.rs` to wiring.
3. **Done** — Move prompt-building and parsing tests from Claude into the shared crate; add mock-HTTP test for `ClaudeBackend::complete` via `with_base_url`.
4. **Done** — Create `crates/pkdealer_agent_ollama` with `OllamaBackend`, CLI mirroring Claude's, OTel init, mock-HTTP test, and README.
5. **Done** — Add both new crates to root `Cargo.toml` `[workspace] members`.
6. **Pending** — Live smoke test: run `ollama serve` + `ollama pull llama3.1`, start `pkdealer_service`, run `pkdealer_agent_ollama` against it, confirm a hand completes end-to-end and traces appear in Jaeger.
7. **Future** — Add a third backend (`pkdealer_agent_openai` or similar) using the same trait; verify the `LlmBackend` shape generalizes.
8. **Future** — Add a comparison harness that runs the same `HandState` through multiple backends and logs the chosen actions side-by-side.

---

## Configuration

| Variable | Default | Used by |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | `pkdealer_agent_ollama` |
| `OLLAMA_MODEL` | `llama3.1` | `pkdealer_agent_ollama` |
| `ANTHROPIC_API_KEY` | — | `pkdealer_agent_claude` (unchanged from EPIC-23) |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | `pkdealer_agent_claude` (unchanged) |
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | both LLM agents |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | both LLM agents |
| `OTEL_SDK_DISABLED` | — | set to `true` to skip OTel init |

---

## Verification

### Automated (CI)

```bash
# Workspace builds, all tests pass, clippy is clean.
OTEL_SDK_DISABLED=true cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Doc tests for the new crate exercise the public API surface.
OTEL_SDK_DISABLED=true cargo test --doc -p pkdealer_agent_llm
OTEL_SDK_DISABLED=true cargo test --doc -p pkdealer_agent_ollama
```

### Live smoke test (manual)

Prereqs: install Ollama, then:

```bash
ollama serve              # in one terminal
ollama pull llama3.1      # one-time per model
```

Run the platform:

```bash
# 1. Start the gRPC service
cargo run -p pkdealer_service &

# 2. Start the Ollama agent
cargo run -p pkdealer_agent_ollama -- --seat 0 --model llama3.1

# 3. Optional: seat a Claude agent in another window for head-to-head
ANTHROPIC_API_KEY=sk-... cargo run -p pkdealer_agent_claude -- --seat 1
```

Confirm:
- Each agent's decision span shows `gen_ai.system` = `ollama` / `anthropic` with token counts populated.
- A hand completes; chip totals are conserved.
- In Jaeger, the service-side `action` spans nest under each agent's `llm.decision` span (per EPIC-22 trace context propagation).

---

## Out of Scope

- Streaming responses (Ollama supports it; not needed for short poker decisions).
- Function calling / tool use (neither backend uses it; pure text parse stays).
- Per-backend prompt tuning (same prompt for both at v1; revisit if `llama3.1` quality is poor).
- The Langfuse scoring stretch from EPIC-23 — still future work, applies equally to all LLM backends once any of them lands.
