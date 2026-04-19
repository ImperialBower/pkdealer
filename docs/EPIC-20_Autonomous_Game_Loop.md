# EPIC-20: Autonomous Game Loop

## Status

| Component | Status |
|---|---|
| All 16 RPC handlers implemented | **Complete** |
| UUID-based auth (`x-player-token`) | **Complete** |
| Event broadcast via `tokio::sync::broadcast` | **Complete** |
| E2E tests (`e2e_ping`, `e2e_two_players`) | **Complete** |
| pkcore dependency update (0.0.39 → latest) | **Planned** |
| Migrate from `Dealer` → `PokerSession` (removes `unsafe impl Send`) | **Planned** |
| Auto-advance street when betting is complete | **Planned** |
| Auto-end hand when game is over | **Planned** |

---

## Context

Phase 1 wired all 16 RPCs to `pkcore::Dealer`. The service is functional:
clients can seat players, start a hand, act, advance streets manually, and
end the hand. The ROADMAP specifies that `pkdealer_service` should drive the
game loop autonomously — streets auto-advance once all players have acted,
and a new hand does not require the client to call `AdvanceStreet` or
`EndHand`. Without this, bot agent binaries (EPIC-22) would each need their
own orchestration logic.

Two related issues are addressed in this EPIC:

1. **`unsafe impl Send for TableState {}`** — `pkcore::Dealer` wraps `TableCelled`,
   which uses `Cell`/`RefCell` and is `!Send`. The current workaround is an
   `unsafe impl Send`, sound only because every access goes through a `Mutex`.
   pkcore's `PokerSession` (wraps `TableNoCell`, a proper `Send` type) provides
   exactly the step-by-step API the service needs and eliminates the `unsafe` block.

2. **pkcore version** — the service depends on `pkcore = "0.0.39"`; the latest
   version ships `PokerSession`, `BotDecider`, `SimTable`, and related types
   needed by subsequent EPICs.

---

## Design

### Autonomous progression after `act`

After every successful `apply_action` call, the service checks whether the
hand can progress without waiting for more player input:

```
act(seat, action) succeeds
  → is betting complete for this street?
      yes → auto-advance street (bring_it_in + deal board card)
            → emit StreetAdvanced event
            → is hand complete? (all-but-one folded, or river done)
                  yes → auto-end hand
                        → emit HandEnded event
      no  → emit PlayerAction event, return
```

Street transitions happen inside the same request/response cycle as the
`act` call. The `ActResponse` already includes `next_to_act` and
`hand_complete`; clients use these to decide whether to act again.

### Migrate `TableState` from `Dealer` to `PokerSession`

```rust
// Before — requires unsafe
struct TableState {
    dealer: Dealer,
    token_to_seat: HashMap<Uuid, u8>,
    seat_to_token: HashMap<u8, Uuid>,
}
unsafe impl Send for TableState {}

// After — no unsafe required
struct TableState {
    session: PokerSession,          // wraps TableNoCell — properly Send + Sync
    token_to_seat: HashMap<Uuid, u8>,
    seat_to_token: HashMap<u8, Uuid>,
}
```

`PokerSession` provides the step-by-step API the service needs:

| Method | Purpose |
|--------|---------|
| `start_hand(&mut self) -> Result<(), PKError>` | shuffle, post blinds, deal hole cards |
| `next_actor(&mut self) -> Option<u8>` | `None` when street or hand is complete |
| `apply_action(&mut self, seat, PlayerAction) -> Result<(), PKError>` | process one action |
| `is_hand_complete(&self) -> bool` | hand-level completion check |
| `end_hand(&mut self) -> Result<Winnings, PKError>` | evaluate + award pot |
| `eliminate_busted(&mut self) -> Vec<u8>` | clear zero-chip seats between hands |

### RPC handler mapping

| RPC | New call |
|-----|----------|
| `StartHand` | `session.start_hand()` |
| `Act` | `session.apply_action(seat, action)` + auto-advance loop |
| `AdvanceStreet` | no-op if auto-advanced; returns error if hand not active |
| `EndHand` | no-op if auto-ended; returns error if hand still active |
| `GetStatus` | read from `session.table` (a `&TableNoCell`) |

### `PlayerAction` mapping

```rust
use pkcore::bot::player_action::PlayerAction;

fn to_pkcore_action(action_type: ActionType, amount: usize) -> PlayerAction {
    match action_type {
        ActionType::Bet   => PlayerAction::Bet(amount),
        ActionType::Call  => PlayerAction::Call,
        ActionType::Check => PlayerAction::Check,
        ActionType::Raise => PlayerAction::Raise(amount),
        ActionType::AllIn => PlayerAction::AllIn,
        ActionType::Fold  => PlayerAction::Fold,
    }
}
```

### Auto-advance loop (inside `act` handler)

```rust
// After apply_action succeeds, drive the hand forward:
loop {
    if state.session.is_hand_complete() {
        let winnings = state.session.end_hand()?;
        // emit HandEnded event with chip deltas
        break;
    }
    match state.session.next_actor() {
        Some(_seat) => break, // a player seat is next — stop and wait
        None => {
            // next_actor returned None mid-hand → PokerSession advanced
            // the street internally; emit StreetAdvanced event and continue
        }
    }
}
```

`PokerSession::next_actor` drives street transitions internally (calls
`bring_it_in`, deals flop/turn/river board cards) when it detects that all
active players have acted. The service just needs to emit the appropriate
`StreetAdvanced` events at each `None` return.

---

## Work Items

1. Update `pkdealer_service/Cargo.toml`: bump `pkcore` to latest published version
2. Replace `pkcore::casino::dealer::{Dealer, DealerAction, DealerError}` imports
   with `pkcore::casino::session::PokerSession` and `pkcore::bot::player_action::PlayerAction`
3. Rewrite `TableState` — swap `dealer: Dealer` for `session: PokerSession`; remove `unsafe impl Send`
4. Update `DealerService::new()` to construct `PokerSession::new(TableNoCell::nlh(...))` instead of `Dealer::new(...)`
5. Update `build_table_status` to read from `session.table` (`&TableNoCell`) instead of `dealer.table`
6. Implement auto-advance loop in the `act` handler; emit `StreetAdvanced` / `HandEnded` events
7. Update `AdvanceStreet` and `EndHand` RPCs to return a meaningful error if called when the hand has already progressed autonomously
8. Update `e2e_two_players` integration test to verify auto-advance: the test should drive play with only `Act` calls; no explicit `AdvanceStreet` or `EndHand` calls needed
9. Update `pkdealer_proto` version if required by the pkcore update

---

## Verification

```bash
# Build
cargo build --workspace

# All tests
cargo test --workspace

# Confirm no unsafe blocks remain in the service
grep -rn "unsafe" crates/pkdealer_service/src/

# Manual smoke test: start service, seat two players, play a hand via Act only
cargo run --bin pkdealer_service &
# In another terminal: run a client that seats two players and acts until hand ends
# without calling AdvanceStreet or EndHand — service should auto-drive to completion
```

The `e2e_two_players` test should complete a full hand (preflop through
showdown) using only `StartHand` + repeated `Act` calls.
