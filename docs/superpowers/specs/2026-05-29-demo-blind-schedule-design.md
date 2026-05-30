# Demo Blind Schedule — Design

Date: 2026-05-29
Status: Approved (pending spec review)

## Goal

Give the `aiarena` and `botarena` demos an escalating tournament blind
structure, mirroring the behaviour of the `pkarena0-web` web application.
Blinds rise one level every N hands (default 20). After the final level
plays out its N hands, the table recycles: every player's stack above the
default rebuy value is capped back down to it, the blinds drop to the
lowest level, and the escalation starts over.

This is a `pkdealer_service`-only change. No `pkcore`, proto, or
client/agent changes are required.

## Background

`pkarena0-web` implements blind progression client-side in JavaScript
(`BLIND_LEVELS` + `getBlindLevelForHand` in `www/index.html`), calling
`mod.set_blinds(sb, bb)` on its WASM module whenever the level changes.
It uses 10 hands per level and a *terminal* top level.

pkdealer has no such client — the bot and LLM agents only call `act`/
`start_hand` over gRPC. So the schedule must live server-side, driven off
a hand counter the service maintains itself. Two behavioural differences
from `pkarena0-web` are required by this feature:

1. **20 hands per level** (configurable) instead of 10.
2. The top level is **not terminal**: completing its N hands recycles the
   table (stack cap + blinds reset) and repeats.

### Reused pkcore primitives (pkcore 0.1.2)

- `PokerSession::set_blinds(ForcedBets)` — applies immediately between
  hands, defers to the next hand if one is in progress.
- `PokerSession::is_hand_in_progress()` — guards against the multi-agent
  `start_hand` race.
- `PlayerNoCell::chips: usize` — public field; the stack cap is a direct
  assignment.
- `ForcedBets::new(sb, bb)` — blinds-only forced bets.
- `TableStatus` already carries `small_blind` / `big_blind`, so live blind
  values reach spectators automatically; no proto change is needed.

## Schedule

The 12-level SB/BB table is identical to `pkarena0-web`:

| Level | SB    | BB    |
|-------|-------|-------|
| 1     | 50    | 100   |
| 2     | 100   | 200   |
| 3     | 150   | 300   |
| 4     | 200   | 400   |
| 5     | 300   | 600   |
| 6     | 400   | 800   |
| 7     | 500   | 1,000 |
| 8     | 750   | 1,500 |
| 9     | 1,000 | 2,000 |
| 10    | 1,500 | 3,000 |
| 11    | 2,000 | 4,000 |
| 12    | 3,000 | 6,000 |

With the default 20 hands per level, one full cycle is `12 × 20 = 240`
hands. Hand 241 wraps back to level 1 and triggers a stack reset.

## Components

### 1. `crates/pkdealer_service/src/blind_schedule.rs` (new)

A pure, fully unit- and doc-tested module. Keeping the arithmetic out of
the 3,600-line `main.rs` makes it independently testable per the project's
testing standard.

```rust
/// (small_blind, big_blind) for each level — values match pkarena0-web.
pub const BLIND_LEVELS: [(usize, usize); 12] = [
    (50, 100), (100, 200), (150, 300), (200, 400), (300, 600), (400, 800),
    (500, 1000), (750, 1500), (1000, 2000), (1500, 3000), (2000, 4000),
    (3000, 6000),
];

/// The blind decision for the hand about to start.
pub struct BlindUpdate {
    pub small_blind: usize,
    pub big_blind: usize,
    /// 0-based level for the upcoming hand.
    pub level: usize,
    /// True exactly when this hand begins a fresh cycle (stack reset due).
    pub reset_stacks: bool,
}

/// Decides blinds for the upcoming hand from the count of completed hands.
///
/// `hands_per_level` of 0 is treated as 1 to avoid division by zero.
pub fn blind_update_for(hands_completed: u64, hands_per_level: usize) -> BlindUpdate;
```

Logic:

```
let per   = hands_per_level.max(1);
let cycle = BLIND_LEVELS.len() * per;             // 12 * per
let pos   = (hands_completed % cycle as u64) as usize;
let level = pos / per;                             // 0..=11
let reset_stacks = hands_completed > 0 && pos == 0;
let (sb, bb) = BLIND_LEVELS[level];
```

`hands_completed` is the number of hands already finished, so the hand
about to start is hand number `hands_completed + 1`. Level 0 covers
completed-counts `0..per`, i.e. the first `per` hands.

Tests (names without a `test_` prefix, per project convention):
- `blind_update_for_first_hand` — `(0, 20)` → level 0, 50/100, no reset.
- `blind_update_for_level_boundary` — `(20, 20)` → level 1, 100/200.
- `blind_update_for_last_level` — `(220, 20)` → level 11, 3000/6000.
- `blind_update_for_cycle_wrap` — `(240, 20)` → level 0, 50/100,
  `reset_stacks == true`.
- `blind_update_for_mid_level` — `(15, 20)` → level 0 (no reset).
- `blind_update_for_zero_per_level` — `(5, 0)` → no panic.

### 2. `DealerConfig` (in `main.rs`)

Two new fields, read in `from_env` with safe fallbacks, mirroring the
existing rebuy config:

- `blind_schedule_enabled: bool` ← `PKDEALER_BLIND_SCHEDULE_ENABLED`
  (default `false`).
- `hands_per_level: usize` ← `PKDEALER_HANDS_PER_LEVEL` (default `20`;
  unparseable or `0` → 20).

The stack-cap target reuses the existing `default_rebuy_amount` (10,000),
which is exactly the "default rebuy value" in the requirement — no new
field.

`Default` impl gains `blind_schedule_enabled: false`, `hands_per_level: 20`.

### 3. `TableState` (in `main.rs`)

Add `hands_completed: u64` (initialised to 0 in `new_with_config`).
Increment it immediately after a successful `end_hand()`, alongside the
existing `self.metrics.hands_played.add(1, &[])` call (~line 1314).

### 4. `start_hand` wiring (in `main.rs`)

After the `count_funded() < 2` guard and **before**
`guard.session.start_hand()`, gated on
`self.config.blind_schedule_enabled && !guard.session.is_hand_in_progress()`
(the `is_hand_in_progress` guard prevents a multi-agent race — where
`start_hand` is about to return the benign "already in progress" error —
from spuriously capping stacks):

1. `let upd = blind_update_for(guard.hands_completed, self.config.hands_per_level);`
2. If `upd.reset_stacks`: iterate occupied seats; for each seat with
   `chips > default_rebuy_amount`, set `chips = default_rebuy_amount`.
   Stacks at or below the cap are untouched. Collect the affected seats
   for one summary event.
3. `guard.session.set_blinds(ForcedBets::new(upd.small_blind, upd.big_blind));`

Event surfacing (no new proto `EventType`):
- The cycle reset, when it happens, emits one `EVENT_TYPE_HAND_STARTED`
  event before the hand begins, e.g.
  `"Blind cycle reset — stacks capped to 10000, blinds back to 50/100"`.
- The normal per-hand level is folded into the existing "Hand started"
  description, e.g. `"Hand started — blinds 100/200 (level 2)"`.
- Live blind values also reach spectators through the `TableStatus`
  snapshot on every event, independent of the description text.

#### Profit/loss behaviour (decided)

The stack cap touches **only** `chips`; `withdrawn` is left unchanged.
Per-seat P&L (`chips + chips_in_play − withdrawn`) therefore stays
cumulative across cycles, so the `pkdealer.player.profit_loss` gauge shows
a downward step at each reset for any capped stack. This matches the
literal requirement and keeps the change minimal.

### 5. `docker-compose.yml`

Add to the `pkdealer_service` `environment:` block:

```yaml
PKDEALER_BLIND_SCHEDULE_ENABLED: "true"
PKDEALER_HANDS_PER_LEVEL: "20"
```

`pkdealer_service` is shared, un-profiled infrastructure, and no compose
profile other than `aiarena` / `botarena` starts it. Setting these here
scopes the feature to exactly those two demos; tests and plain
`cargo run` (which don't read this compose file) keep the fixed 50/100
default because the flag defaults to `false`.

### 6. Documentation

- `crates/pkdealer_service/README.md`: env-var table rows + a short
  "Tournament blind schedule" subsection describing the 12-level
  structure, the N-hands cadence, and the recycle behaviour.
- `main.rs` module-level env-var table: add the two new vars.

## Data flow

```
agent → start_hand RPC
  └─ blind_schedule_enabled && !hand_in_progress?
       ├─ blind_update_for(hands_completed, hands_per_level)
       ├─ reset_stacks? → cap each seat's chips to default_rebuy_amount
       │                  + emit reset event
       └─ session.set_blinds(sb, bb)
  └─ session.start_hand()  (posts forced bets at the new blinds)
        ...
  end_hand() succeeds → hands_completed += 1 ; hands_played metric += 1
```

## Error handling

- `hands_per_level == 0` is normalised to 20 in config and defended again
  in `blind_update_for` (`.max(1)`), so no division by zero is possible.
- The schedule is applied only when no hand is in progress, so a losing
  `start_hand` race cannot mutate stacks or blinds.
- No `unwrap`/`expect`/`panic!` in the new library code; array indexing is
  bounded because `level` is always `0..BLIND_LEVELS.len()`.
- Stack cap never reduces a seat to 0 (cap is the rebuy amount, > 0), so
  it cannot wedge the table or interact badly with auto-rebuy.

## Testing

- Unit + doc tests for `blind_update_for` (cases above).
- Service-level test: with `blind_schedule_enabled` and a small
  `hands_per_level`, drive enough hands to (a) confirm blinds escalate via
  `get_status`, and (b) confirm a stack seeded above the cap is reduced to
  the cap on cycle wrap while a stack below it is preserved.
- `cargo test` and `cargo test --doc` green; `cargo clippy` clean.

## Out of scope

- No proto / `EventType` additions.
- No pkspectator changes (it already renders blinds from `TableStatus`).
- No P&L rebasing on reset.
- No per-level ante or wall-clock-timed levels.
```
