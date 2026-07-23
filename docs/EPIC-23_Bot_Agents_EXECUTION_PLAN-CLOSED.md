# EPIC-23a: Random Bot Agent — Execution Plan

## Context

EPIC-23 adds three gRPC bot agent binaries so the autonomous game loop (EPIC-20) has actual players. EPIC-23a specifically delivers the random baseline agent plus the shared `pkdealer_agent_core` library that all three agents will use. Prerequisites are complete: seat resume via `client_secret` (EPIC-20) and OTel tracing (EPIC-22).

---

## New Crates

### 1. `crates/pkdealer_agent_core` (library)

Shared infrastructure used by all three agent binaries.

**Cargo.toml key dependencies:**
```toml
pkdealer_proto = { path = "../pkdealer_proto", version = "0.1.9" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tonic = { version = "0.12", features = ["transport"] }
async-trait = "0.1"
tokio-stream = "0.1"
```

**Module structure:**

- `src/lib.rs` — module declarations + re-exports of `HandState`, `Decision`, `PokerAgent`, `AgentConfig`, `run_agent`
- `src/error.rs` — `AgentError` enum (`Connect`, `Seat`, `Stream`, `Act`, `InvalidMetadata`)
- `src/hand_state.rs` — `HandState` struct (matches EPIC-23 spec verbatim); a `street_name(Street) -> &'static str` helper
- `src/agent.rs` — `Decision` enum (Fold / Check / Call / Bet(u32) / Raise(u32) / AllIn); `PokerAgent` async trait returning `Decision`
- `src/runner.rs` — `AgentConfig` struct + `pub async fn run_agent<A: PokerAgent>(agent: A, config: AgentConfig)`

> **Note:** The EPIC spec names the return type `PlayerAction`, but `pkdealer_proto` already generates a struct by that name. We use `Decision` to avoid the clash.

**`AgentConfig`:**
```rust
pub struct AgentConfig {
    pub endpoint: String,
    pub name: String,
    pub seat: Option<u32>,    // None → next available
    pub chips: u32,
    pub client_secret: String, // empty string = no resume
}
```

**`run_agent` logic:**
1. `DealerServiceClient::connect(config.endpoint)`
2. `SeatPlayer` (or `SeatPlayerAt` when `config.seat.is_some()`) → `my_seat: u8`, `my_token: String`
3. `GetTableConfig` once → cache `big_blind`
4. `StreamEvents { player_token: my_token }` so hole cards are visible in event status snapshots
5. Accumulate `action_history: Vec<String>` from event descriptions; clear on `HandStarted` / `StreetAdvanced`
6. On each event: if `status.hand_in_progress && status.next_to_act == my_seat as u32`:
   - Call `GetNextToAct` → `to_call`
   - Extract hole cards, board, pot, stacks from `event.current_status`
   - Build `HandState`, call `agent.decide(&state).await`
   - Build `ActRequest`, insert `x-player-token` metadata via `req.metadata_mut().insert(...)`
   - Call `client.act(req)`

**Token metadata pattern** (mirrors the service's own test helper at `crates/pkdealer_service/src/main.rs:1412`):
```rust
let mut req = tonic::Request::new(ActRequest { action: Some(proto_action) });
req.metadata_mut().insert("x-player-token", my_token.parse()?);
```

---

### 2. `crates/pkdealer_agent_random` (binary)

**Cargo.toml key dependencies:**
```toml
pkdealer_agent_core = { path = "../pkdealer_agent_core" }
clap = { version = "4", features = ["derive"] }
rand = "0.9"
async-trait = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

**CLI args via `clap::Parser`:**
```
--endpoint   http://127.0.0.1:50051
--name       rando
--seat       (optional)
--chips      10000
```

**`RandomAgent::decide` logic (per EPIC-23 spec):**
- `to_call > 0`: fold / call / raise with 1/3 probability each; raise amount = random 25–100% of pot, minimum `to_call`
- `to_call == 0`: check / bet with 1/2 probability; bet amount = random 25–100% of pot, minimum `big_blind`

**`main`**: parse args → build `AgentConfig` → `run_agent(RandomAgent, config).await`

---

## Workspace Change

`Cargo.toml` `members` array: add `"crates/pkdealer_agent_core"` and `"crates/pkdealer_agent_random"`.

---

## Testing

**Unit tests** (per CLAUDE.md requirements):
- `hand_state.rs`: test `HandState` construction, `street_name` for all Street variants
- `agent.rs`: test `Decision` derives (Debug, PartialEq), `PokerAgent` impl with a stub agent
- `runner.rs`: test `AgentConfig` construction, default field values
- `main.rs` (random): test that `RandomAgent::decide` returns legal actions for `to_call > 0` and `to_call == 0`; run many iterations and assert the probability distribution is approximately correct (fold/call/raise each ~33%, check/bet each ~50%)

**Doc tests**: every public item gets an `# Examples` doc test.

**Manual verification:**
```bash
# Terminal 1
cargo run --bin pkdealer_service

# Terminal 2 (after service starts)
cargo run --bin pkdealer_agent_random -- --name alice

# Terminal 3
cargo run --bin pkdealer_agent_random -- --name bob
```
Watch for continuous hand play in service logs; no panics; chip totals in log entries sum to 20 000 throughout.

---

## Build Order

1. `crates/pkdealer_agent_core` (lib) — no binary, no external network required for tests
2. `crates/pkdealer_agent_random` (bin) — depends on step 1
3. Workspace `Cargo.toml` update (can be done first, before either crate compiles)

---

## Out of Scope for 23a

- Chip conservation integration test (requires running service + two agent processes; noted as EPIC-23 work item 8)
- `pkdealer_agent_rules` (23b), `pkdealer_agent_claude` (23c), Langfuse (23d)
- OTel `gen_ai.*` spans (23c only)
