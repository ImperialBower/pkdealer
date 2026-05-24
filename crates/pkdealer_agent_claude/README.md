# Poker Dealer Claude Agent

A poker bot that connects to `pkdealer_service` over gRPC and asks the
Anthropic Claude API what to do on every street.

## Overview

- **Decisions via the Anthropic Messages API.** Each turn, the current
  `HandState` (hole cards, board, pot, stacks, action history) is formatted
  into a short natural-language prompt and sent to Claude. The text response
  is parsed back into a `Decision` (`Fold` / `Check` / `Call` / `Bet(n)` /
  `Raise(n)` / `AllIn`).
- **Same `PokerAgent` runner as the other agents.** Connect, seat, event
  loop, token metadata, and chip accounting all come from
  `pkdealer_agent_core`. Only `decide()` is bespoke.
- **Fully traced.** Every API call is wrapped in an `llm.decision` span
  with OpenTelemetry `gen_ai.*` semantic-convention attributes, so
  decisions show up in Jaeger alongside the service's `hand`, `street`,
  and `action` spans.
- **Safe on failure.** API errors and malformed responses fall back to
  the safest legal action (`Check`, or `Fold` if facing a bet) — the
  table keeps moving.

## Building

```bash
# From workspace root
cargo build --package pkdealer_agent_claude

# Or from this directory
cargo build
```

## Running

The agent requires `ANTHROPIC_API_KEY` and a running `pkdealer_service`.

```bash
# Minimal — uses defaults (Sonnet 4.6, max_tokens 16, name "claude")
ANTHROPIC_API_KEY=sk-... cargo run --bin pkdealer_agent_claude

# Override the model and token budget
ANTHROPIC_API_KEY=sk-... cargo run --bin pkdealer_agent_claude -- \
  --model claude-opus-4-7 --max-tokens 32 --name claude_opus
```

## Testing

```bash
# Run all tests
cargo test --package pkdealer_agent_claude

# Run with output
cargo test --package pkdealer_agent_claude -- --nocapture
```

The unit tests cover `build_prompt`, `parse_action`, and `pot_odds` in
isolation — no network access is required.

## Configuration

### Environment variables

| Var | Default | Purpose |
|---|---|---|
| `ANTHROPIC_API_KEY` | — | **Required.** Anthropic API key. Empty/unset → process exits 1. |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | Claude model identifier; overridden by `--model`. |
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address; overridden by `--endpoint`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC target for spans. |
| `OTEL_SDK_DISABLED` | unset | If `true`, skips OTel init entirely (useful in tests/CI). |
| `RUST_LOG` | — | tracing filter (e.g. `pkdealer_agent_claude=debug`). |

### CLI flags

Sourced from `Args` in `src/main.rs`:

- `--endpoint <URL>` — gRPC service address. Default `http://127.0.0.1:50051`.
- `--name <STRING>` — player name displayed at the table. Default `claude`.
- `--seat <0..=8>` — specific seat to request. Omit for next available.
- `--chips <N>` — buy-in. Default `10000`.
- `--client-secret <TOKEN>` — opaque seat-resume token (EPIC-20). Empty = no resume.
- `--model <ID>` — Claude model identifier. Default `claude-sonnet-4-6`.
- `--max-tokens <N>` — output token cap per response. Default `16`.

## How the Claude API is called

`call_api` (`src/main.rs:140-177`) POSTs JSON to the Anthropic Messages
endpoint:

- **URL:** `https://api.anthropic.com/v1/messages`
- **Headers:**
  - `x-api-key: $ANTHROPIC_API_KEY`
  - `anthropic-version: 2023-06-01`
  - `content-type: application/json` (set by `reqwest`'s `.json()`)
- **Body:**

  ```json
  {
    "model": "claude-sonnet-4-6",
    "max_tokens": 16,
    "messages": [
      { "role": "user", "content": "<prompt>" }
    ]
  }
  ```

The prompt template lives in `build_prompt` (`src/main.rs:256-297`). A
concrete preflop example for a heads-up table looks like this:

```text
You are a professional poker player at a No-Limit Hold'em table.

Your hand: Ah Kd
Board: (no community cards yet) (preflop)
Pot: 200 chips  |  To call: 100 chips  |  Your stack: 9900 chips
Big blind: 100

Seat stacks: seat 0 alice: 9900, seat 1 bob: 9900

Action history this street:
(no actions yet this street)

Choose ONE action: fold, check, call, bet <amount>, raise <amount>
Respond with only the action, nothing else.
```

### Curl-equivalent

The same request as a one-liner — useful for reproducing a single
decision outside the binary:

```bash
curl https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-6",
    "max_tokens": 16,
    "messages": [
      { "role": "user", "content": "You are a professional poker player at a No-Limit Hold'\''em table.\n\nYour hand: Ah Kd\nBoard: (no community cards yet) (preflop)\nPot: 200 chips  |  To call: 100 chips  |  Your stack: 9900 chips\nBig blind: 100\n\nSeat stacks: seat 0 alice: 9900, seat 1 bob: 9900\n\nAction history this street:\n(no actions yet this street)\n\nChoose ONE action: fold, check, call, bet <amount>, raise <amount>\nRespond with only the action, nothing else." }
    ]
  }'
```

### Response parsing

`parse_action` (`src/main.rs:320-355`) is strict by design — it only
accepts canonical forms (case-insensitive, trimmed):

- `fold`, `check`, `call`
- `all in` / `all-in` / `allin`
- `bet <N>`
- `raise <N>` or `raise to <N>`

Anything else (verbose explanations, hedging, off-topic responses) falls
through to the safe path: `Check` when `to_call == 0`, otherwise `Fold`.
The raw response text is logged so prompt-engineering regressions are
visible in `stderr` and in span attributes.

## Observability

Each `decide()` call emits one `llm.decision` span with these attributes:

| Attribute | Source |
|---|---|
| `gen_ai.system` | constant `"anthropic"` |
| `gen_ai.request.model` | the `--model` value |
| `gen_ai.request.max_tokens` | the `--max-tokens` value |
| `gen_ai.usage.input_tokens` | `usage.input_tokens` from API response |
| `gen_ai.usage.output_tokens` | `usage.output_tokens` from API response |
| `poker.street` | `HandState.street` |
| `poker.pot` | `HandState.pot` |
| `poker.pot_odds` | `pot_odds(state)` (`src/main.rs:381-390`) |
| `poker.action_chosen` | the parsed `Decision`, debug-formatted |

These are the standard OpenTelemetry GenAI semantic conventions — any
GenAI-aware dashboard (Jaeger, Honeycomb, Grafana Tempo) reads them
without further configuration. The `gen_ai.usage.*` fields are the
source of truth for actual cost; the estimates below are guidance only.

To view spans locally, bring up the same compose stack that
`pkdealer_service` documents:

```bash
docker compose up -d otel-collector jaeger prometheus grafana
open http://localhost:16686    # Jaeger
```

## Cost estimation

### Token budget per decision

The prompt size depends on the table state, but for the most common
shape it sits in a narrow range:

- **Heads-up preflop** (no action history yet): ~150-180 input tokens.
- **Multi-way with several actions logged**: ~200-250 input tokens.
- **Output**: bounded by `--max-tokens` (default 16); realistic
  completions are 2-6 tokens — a single action like `raise 300` or
  `fold`.

The numbers below use **200 input tokens** and **5 output tokens** per
decision, with **4 decisions per hand** (preflop / flop / turn / river,
recognising that many hands end earlier). For your workload, read the
actual `gen_ai.usage.*` values out of Jaeger.

### Per-decision / per-hand / per-1000-hands by tier

Prices below are approximate Anthropic list rates; **check
<https://www.anthropic.com/pricing> for current numbers**.

| Model | Input $/1M | Output $/1M | Per decision | Per hand (~4 decisions) | Per 1 000 hands |
|---|---|---|---|---|---|
| `claude-haiku-4-5` | ~$1 | ~$5 | ~$0.000225 | ~$0.0009 | ~$0.90 |
| `claude-sonnet-4-6` (default) | ~$3 | ~$15 | ~$0.000675 | ~$0.0027 | ~$2.70 |
| `claude-opus-4-7` | ~$15 | ~$75 | ~$0.003375 | ~$0.0135 | ~$13.50 |

Math: `cost = (input_tokens * input_rate + output_tokens * output_rate) / 1_000_000`.

### Prompt caching (not currently enabled)

Anthropic's prompt-caching feature can reduce repeat-prompt input cost
to roughly **10% of the standard input rate** — at Sonnet 4.6 list
prices, that drops the input column from ~$3/1M to ~$0.30/1M for cached
segments, which would cut the per-hand input cost by close to an order
of magnitude.

The current implementation in `call_api` (`src/main.rs:144-151`) builds
a fresh `ApiRequest` per call with **no `cache_control` markers**.
Caching is not enabled today. Enabling it would require:

1. Splitting the prompt into a stable system prefix (the role and
   action-vocabulary preamble) and a per-turn user message (the
   hand-specific lines).
2. Attaching `cache_control: { type: "ephemeral" }` to the stable
   prefix.
3. Widening the serde shape of `ApiRequest` / `ApiMessage` to allow
   structured content blocks instead of a single `String`.

This is a worthwhile follow-up for any deployment that plays more than
a few hundred hands per session.

### Cost-reduction levers available today

Without code changes you can:

- **Lower `--max-tokens`.** The default 16 leaves headroom; 8 still
  covers `raise 10000` (the longest reasonable response).
- **Switch tier with `--model claude-haiku-4-5`.** ~3× cheaper than
  Sonnet at the cost of weaker reasoning.
- **Shrink the prompt.** `build_prompt` includes the `Seat stacks` line
  even heads-up; dropping it (or summarising it) saves ~10-20 input
  tokens per decision.

## Development

### Prerequisites

- Rust 1.85.0 or later
- An Anthropic API key
- A running `pkdealer_service` instance

### Code Style

This project follows the guidelines in `CLAUDE.md`:

- All public functions must have doc tests
- All public functions must have unit tests
- No `unwrap()` or `panic!()` in library code
- Comprehensive error handling

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](../../LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.
