# EPIC-44 Phase 1 — Per-seat Token Accumulator (data plane + debug surface + OTel)

**Date:** 2026-06-19
**Branch:** epic-44
**Epic:** [EPIC-44 Token Accounting & External Cost Simulation](../../EPIC-44_Token_Accounting_and_Cost_Simulation-INC.md)
**Phase 0 status:** ✅ shipped (`crates/pkdealer_costsim`)

## Goal

Add live per-seat LLM token aggregation to `pkdealer_service`, surfaced on the
`SeatInfo` proto so external spectators (pkspectator / pktui, in other repos) can
render a token column. This phase ships the **data plane** plus a small in-repo
**debug surface** and **OpenTelemetry gauges**. It mirrors the existing
`banked_profit` accumulator exactly.

## Scope decisions (settled in brainstorming)

- **Render scope:** Data plane + debug surface. No in-repo renderer exists for the
  proto `SeatInfo` (pkspectator/pktui are external). The visible column lands later
  in the external spectator repo; here we make the numbers correct, transported,
  and observable.
- **OTel gauges:** Included in this pass, mirroring `player_profit_loss`.

## Out of scope

- Notional USD cost column on `SeatInfo` (Phase 2).
- The actual visible token column in pkspectator / pktui (external repo).
- Tokenizer-variance correction (Phase 3).

## Existing pattern to mirror

> **Note:** the EPIC doc calls this struct `SessionState`; the actual struct in
> `main.rs` is **`TableState`** (`:257`). This spec uses the real name, `TableState`.

`TableState.banked_profit: HashMap<u8, i64>` (`crates/pkdealer_service/src/main.rs:286`):
- Declared on `TableState`, initialized empty (`:431`).
- Populated into `SeatInfo.profit_loss` by `build_table_status` (`:462`), which
  threads `banked: &HashMap<u8,i64>` through all ~12 call sites.
- Cleared per-seat on seat departure (`:1337`).
- Surfaced via the `player_profit_loss: Gauge<i64>` OTel gauge (`:338` decl, `:367` init).

`session_tokens` copies this shape with `(u64, u64)` (input, output) values.

## Design

### 1. Proto — `proto/dealer.proto`

`SeatInfo`'s last field is `bet = 9`. Add:

```protobuf
uint64 input_tokens  = 10;  // cumulative prompt tokens this session (LLM seats; 0 for bots)
uint64 output_tokens = 11;  // cumulative completion tokens this session
```

`crates/pkdealer_proto/build.rs` regenerates bindings on build. Bots leave these 0
(their `AgentFidelity` is all `None`).

### 2. Accumulator — `TableState` (main.rs, beside `banked_profit` ~286)

```rust
/// Per-seat cumulative LLM token usage (input, output) over the session.
/// Mirrors `banked_profit`; bots never increment it. Surfaced on SeatInfo.
session_tokens: std::collections::HashMap<u8, (u64, u64)>,
```

- Initialize empty at the `SessionState` constructor (`:431`).
- Clear the seat's entry wherever `banked_profit` is cleared on seat departure
  (`:1337`), so a vacated/reused seat does not carry stale totals.

### 3. Increment — live success path (main.rs:1674–1679)

The accepted-action site, inside the `Ok(())` arm of `apply_action`, under `guard`,
with `seat` in scope and `agent_fidelity` available immediately before it is moved
into the per-hand buffer. Read the token counts *before* the move:

```rust
match guard.session.apply_action(seat, player_action) {
    Ok(()) => {
        // EPIC-44 Phase 1: accumulate per-seat LLM token usage on the accepted
        // path, mirroring banked_profit. Read before the move into the buffer.
        if let (Some(i), Some(o)) = (agent_fidelity.input_tokens, agent_fidelity.output_tokens) {
            let e = guard.session_tokens.entry(seat).or_default();
            e.0 += u64::from(i);
            e.1 += u64::from(o);
        }
        guard.hand_agent_fidelity.push((seat, agent_fidelity));
        // ...existing success-path code...
    }
    // rejected actions never reach here → never counted
}
```

Fires exactly once per **accepted** action. Rejected actions (returned before the
`Ok` arm) never increment.

### 4. Populate — `build_table_status` (main.rs:462)

Add a `session_tokens: &std::collections::HashMap<u8,(u64,u64)>` parameter, threaded
through every call site exactly as `banked` already is. In the `SeatInfo { .. }`
construction (`:480`):

```rust
let (in_tok, out_tok) = session_tokens.get(&i).copied().unwrap_or((0, 0));
// in SeatInfo { }:
input_tokens: in_tok,
output_tokens: out_tok,
```

### 5. OTel gauges (mirror `player_profit_loss`, :338 / :367)

```rust
player_tokens_in:  Gauge<u64>,   // pkdealer.player.tokens_in
player_tokens_out: Gauge<u64>,   // pkdealer.player.tokens_out
```

Record the running per-seat totals with the seat attribute at the same point the
increment happens (Section 3), matching how `player_profit_loss` is recorded.

### 6. Debug surface

At hand end — the `attach_agent_fidelity` block (~main.rs:1872) — emit one `tracing`
line per occupied seat with its running `session_tokens` totals (input/output). Uses
existing log levels; no new CLI flag. Makes the accumulator verifiable from logs
without the external spectator.

### 7. Tests (per CLAUDE.md: happy + edge + error, doc comments + doctests on new public items)

- **Accumulator:** two accepted actions carrying tokens sum per seat; a bot action
  (all-`None` fidelity) contributes 0; a rejected action does not increment.
- **Populate:** `build_table_status` writes `input_tokens`/`output_tokens` onto
  `SeatInfo` from `session_tokens`; seats with no entry render 0.
- **Proto round-trip:** a `SeatInfo` with fields 10/11 set encodes and decodes
  preserving the values.
- **Seat departure:** clearing a seat removes its `session_tokens` entry (no stale
  carry-over).

## Verification gates

- `OTEL_SDK_DISABLED=true cargo test --workspace` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` is clean. *(This gate
  is added because broken intra-doc links are invisible to clippy — the class of
  warning that preceded this work.)*
- `cargo test --doc -p pkdealer_service` passes for any new public items.

## Files to modify

| File | Action |
|---|---|
| `proto/dealer.proto` | Add `input_tokens = 10`, `output_tokens = 11` to `SeatInfo` |
| `crates/pkdealer_service/src/main.rs` | `session_tokens` field + init + departure clear; increment at accepted-action site; thread param through `build_table_status` + all call sites; OTel gauges; hand-end debug `tracing` line; tests |

No new crates. No changes to `pkdealer_costsim`. The Phase 2 cost column reuses
`pkdealer_costsim::pricing` so live and post-hoc dollars agree.
