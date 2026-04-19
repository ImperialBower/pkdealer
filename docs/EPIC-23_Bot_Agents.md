# EPIC-23: Bot Agent Clients

## Status

| Component | Status |
|---|---|
| `PokerAgent` trait (shared agent interface) | Planned |
| `HandState` snapshot type (derived from `GetStatus` + hole cards) | Planned |
| `pkdealer_agent_random` — random legal action baseline | Planned |
| `pkdealer_agent_rules` — rule-based agent via pkcore `BotProfile` | Planned |
| `pkdealer_agent_claude` — LLM agent using Anthropic API | Planned |
| OTel `gen_ai.*` span emission from LLM agents | Planned |
| Langfuse scoring integration (post-hand outcome scores) | Planned |

---

## Context

With EPIC-20's autonomous game loop and EPIC-21's spectator view, the platform
needs players. EPIC-23 delivers bot agent binaries that connect via gRPC, receive
game state from `StreamEvents`, and call `Act` on their turn — no human required.

Three agents are built in order of increasing complexity:

1. **Random** — proves the plumbing works; establishes a performance baseline
2. **Rule-based** — demonstrates pkcore's analytical capabilities; no API cost
3. **Claude** — showcases LLM-driven decision-making with full OTel observability

The same `PokerAgent` trait governs all three. Only the transport layer
(gRPC `Act`) is shared infrastructure; the decision logic is entirely per-agent.

---

## Shared Infrastructure

### New workspace crate: `crates/pkdealer_agent_core`

A shared library crate containing:

```rust
/// TableCelled state visible to one agent seat.
pub struct HandState {
    pub seat: u8,
    pub hole_cards: String,          // "Ah Kd"
    pub board: String,               // "Qh Js Tc" (empty preflop)
    pub pot: u32,
    pub to_call: u32,
    pub my_chips: u32,
    pub stacks: Vec<(u8, String, u32)>,  // (seat, name, chips)
    pub big_blind: u32,
    pub street: String,              // "preflop" | "flop" | "turn" | "river"
    pub action_history: Vec<String>, // human-readable action log for this street
}

/// Decision-making interface for all agent types.
#[async_trait]
pub trait PokerAgent: Send + Sync {
    async fn decide(&self, state: &HandState) -> PlayerAction;
}
```

`HandState` is constructed from `GetStatusResponse` (using the player's token
for hole card visibility) plus the event log. The agent main loop:

```rust
loop {
    let event = event_stream.next().await?;
    if event.next_to_act == my_seat && hand_in_progress {
        let state = build_hand_state(&client, my_seat, &my_token).await?;
        let action = agent.decide(&state).await;
        client.act(action_request(my_seat, action, &my_token)).await?;
    }
}
```

### Common binary structure

Each agent binary accepts these arguments (via `clap`):

```
--endpoint   gRPC service address (default: http://127.0.0.1:50051)
--name       Player name to display at the table
--seat       Optional: specific seat number (0-8); defaults to next available
--chips      Buy-in amount (default: 10 000)
```

---

## Agent Implementations

### 23a. `pkdealer_agent_random`

Picks a legal action uniformly at random:
- If `to_call > 0`: fold / call / raise with equal probability (1/3 each)
- If `to_call == 0`: check / bet with equal probability (1/2 each)
- Raise/bet amount: random fraction of the pot (25%–100%)

No external dependencies beyond `pkdealer_agent_core` and the gRPC client.
Establishes the baseline win rate against which other agents are measured.

### 23b. `pkdealer_agent_rules`

Uses `pkcore::bot::profile::BotProfile` and `pkcore::bot::decider::BotDecider`
to drive decisions. Bridges pkcore's local simulation API to the gRPC transport:

```rust
struct RulesAgent {
    profile: BotProfile,
    decider: RuleBasedDecider,
}

#[async_trait]
impl PokerAgent for RulesAgent {
    async fn decide(&self, state: &HandState) -> PlayerAction {
        let snapshot = TableSnapshot::from_hand_state(state);
        self.decider.decide(&self.profile, &snapshot)
    }
}
```

`TableSnapshot::from_hand_state` converts the gRPC-derived `HandState` into
pkcore's `TableSnapshot` type (the same type used by `SimTable` in EPIC-19).
This is the key bridge: the decision logic that ran locally against `TableNoCell`
now runs identically over gRPC.

The profile is loaded from a YAML file at startup (`--profile path/to/bot.yaml`),
defaulting to `data/bots/gto.yaml`.

### 23c. `pkdealer_agent_claude`

Sends the hand state as a natural-language prompt to the Anthropic Claude API
and parses the response into a `PlayerAction`.

**Dependencies:**
- `reqwest` — HTTP client for the Anthropic REST API
- `serde_json` — request/response serialization
- `opentelemetry` — `gen_ai.*` span emission

**Prompt template:**

```
You are a professional poker player at a No-Limit Hold'em table.

Your hand: {hole_cards}
Board: {board} ({street})
Pot: {pot} chips  |  To call: {to_call} chips  |  Your stack: {my_chips} chips
Big blind: {big_blind}

Seat stacks: {stacks}

Action history this street:
{action_history}

Choose ONE action: fold, check, call, bet <amount>, raise <amount>
Respond with only the action, nothing else.
```

**Response parsing:**

```rust
fn parse_llm_action(response: &str, state: &HandState) -> PlayerAction {
    let lower = response.trim().to_lowercase();
    if lower == "fold" { return PlayerAction::Fold; }
    if lower == "check" { return PlayerAction::Check; }
    if lower == "call" { return PlayerAction::Call; }
    if let Some(amount) = lower.strip_prefix("bet ").and_then(|s| s.parse().ok()) {
        return PlayerAction::Bet(amount);
    }
    if let Some(amount) = lower.strip_prefix("raise ").and_then(|s| s.parse().ok()) {
        return PlayerAction::Raise(amount);
    }
    // Fallback: check if possible, else fold
    if state.to_call == 0 { PlayerAction::Check } else { PlayerAction::Fold }
}
```

**OTel `gen_ai.*` span attributes** (per EPIC-22 trace context propagation):

| Attribute | Value |
|-----------|-------|
| `gen_ai.system` | `"anthropic"` |
| `gen_ai.request.model` | e.g. `"claude-sonnet-4-6"` |
| `gen_ai.usage.input_tokens` | from API response |
| `gen_ai.usage.output_tokens` | from API response |
| `gen_ai.request.max_tokens` | configured limit |
| `poker.hand_id` | from `HandState` |
| `poker.street` | `"preflop"` etc. |
| `poker.pot_odds` | computed from state |
| `poker.action_chosen` | parsed action |

The `traceparent` is injected into the gRPC `Act` metadata so the action span
in the service becomes a child of this decision span (see EPIC-22).

### 23d. Langfuse scoring (stretch)

After each hand resolves (`HandEnded` event), the agent compares its chip
delta to the starting stack. For each LLM decision made in that hand, it
posts a score to the Langfuse HTTP API:

```
POST /api/public/scores
{ "traceId": "<otel_trace_id>", "name": "hand_outcome", "value": <chip_delta> }
```

Over many hands this builds a per-decision quality dataset indexed by trace,
enabling the Langfuse UI to show which prompt versions and model choices
produced the best outcomes.

---

## Work Items

1. Create `crates/pkdealer_agent_core/` with `HandState`, `PokerAgent` trait, and shared gRPC client helpers
2. Add `pkdealer_agent_core` to workspace `Cargo.toml`
3. Implement `pkdealer_agent_random` binary
4. Implement `TableSnapshot::from_hand_state()` adapter in `pkdealer_agent_core` (or agent_rules crate)
5. Implement `pkdealer_agent_rules` binary loading a `BotProfile` from YAML
6. Implement `pkdealer_agent_claude` binary with prompt template + response parser
7. Add `gen_ai.*` OTel span emission and `traceparent` injection to the Claude agent
8. Write integration test: two random agents play 5 hands; assert chip conservation
9. Add `--profile`, `--model`, `--max-tokens` CLI flags to appropriate agents
10. Document `ANTHROPIC_API_KEY` env var requirement for `pkdealer_agent_claude`

---

## Configuration

| Variable | Default | Purpose |
|----------|---------|---------|
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
| `ANTHROPIC_API_KEY` | — | Required for `pkdealer_agent_claude` |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | Model override |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTel collector |

---

## Verification

```bash
# Start the service (with EPIC-20 autonomous loop)
cargo run --bin pkdealer_service &

# Run two random agents — chips should be conserved across all hands
cargo run --bin pkdealer_agent_random -- --name alice &
cargo run --bin pkdealer_agent_random -- --name bob &

# Run a rule-based agent against a random agent
cargo run --bin pkdealer_agent_rules -- --name gto --profile data/bots/gto.yaml &
cargo run --bin pkdealer_agent_random -- --name rando &

# Run the Claude agent (requires ANTHROPIC_API_KEY)
ANTHROPIC_API_KEY=sk-... cargo run --bin pkdealer_agent_claude -- --name claude &
cargo run --bin pkdealer_agent_random -- --name rando &

# Watch in spectator (EPIC-21)
open http://localhost:3000

# Verify OTel traces (EPIC-22)
open http://localhost:16686   # Jaeger — confirm action spans are nested under decision spans
```
