# pkdealer Development Log

Running record of implementation decisions, architectural choices, and progress.
Updated as each phase or significant change is made.

---

## Phase 1 — Complete the pkdealer gRPC Server

**Goal:** Wire all `DealerService` RPCs to `pkcore::Dealer` so the existing
`pkdealer_client` can connect and play a hand end-to-end.

**Status: ✅ Complete** (all tests passing)

---

### What was built

#### `pkdealer_service/Cargo.toml`

Added `pkcore = "0.0.28"` and the `sync` feature to tokio so we can use
`tokio::sync::broadcast`.

#### `pkdealer_service/src/main.rs`

Full rewrite. All 15 RPCs are now implemented:

| RPC | Notes |
|-----|-------|
| `Ping` | Unchanged from stub |
| `SeatPlayer` | Seats at next empty slot; defaults to 10 000 chips when `chips == 0` |
| `SeatPlayerAt` | Seats at a specific slot |
| `RemovePlayer` | Guarded against empty-seat case (library doesn't error there) |
| `StartHand` | Shuffles deck, posts blinds, deals hole cards |
| `AdvanceStreet` | Consolidates bets, deals flop/turn/river |
| `EndHand` | Evaluates hands, pays out pot, returns `HandResult` |
| `Act` | Routes Bet/Call/Check/Raise/AllIn/Fold to `Dealer::act` |
| `GetStatus` | Returns full `TableStatus` snapshot |
| `GetNextToAct` | Returns `NextToActInfo` or a message when no hand is running |
| `GetBoard` | Community cards as a display string |
| `GetChips` | Chip counts for all occupied seats |
| `GetPot` | Current pot size |
| `GetEventLog` | Full `TableLog` formatted as text |
| `StreamEvents` | Live event stream via broadcast → per-subscriber mpsc bridge |

---

### Architecture

```
pkdealer_service binary
│
├── DealerService (Clone)
│   ├── Arc<Mutex<TableState>>          ← shared game state
│   └── broadcast::Sender<TableEvent>   ← fan-out to StreamEvents subscribers
│
└── TableState
    └── pkcore::Dealer                  ← game engine (owns the Table)
```

#### Thread-safety note

`pkcore::Dealer` (and its inner `Table`) use `Cell`/`RefCell` for interior
mutability, making them `!Send` by default.  We wrap the dealer in a newtype
`TableState` and add `unsafe impl Send for TableState`.  This is sound because
every access to the `Dealer` is gated through the `Mutex`; only one thread ever
touches it at a time.

#### Event streaming

`stream_events` subscribes to a `broadcast::Receiver<TableEvent>`.  Each
subscriber gets its own mpsc channel; a dedicated `tokio::spawn` task forwards
from the broadcast receiver to that mpsc channel so the gRPC stream can use the
`ReceiverStream` wrapper that tonic expects.

After every successful mutating RPC (seat, start, act, etc.) an event is emitted
to the broadcast channel.

---

### Key decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Thread safety | `unsafe impl Send` on `TableState` | Mutex guarantees exclusive access; avoids complex actor indirection |
| Empty-seat guard on `RemovePlayer` | Added in handler | `pkcore` returns `Ok(default_player)` for empty seats; proto contract expects an error |
| Default chips | 10 000 | Sensible starting stack; overridden by non-zero request field |
| Blinds | 50 / 100 | Standard NLH default; not yet configurable via RPC |
| Seat count | 9 | Standard full-ring; not yet configurable via RPC |
| Hole card visibility | All cards shown | No auth in Phase 1; redaction deferred to later |

---

### Tests (27 unit + 2 e2e)

| Test | What it covers |
|------|---------------|
| `ping_happy_path` | Returns `"pong:client-99"` |
| `ping_empty_client_id` | Returns `"pong"` with no client id |
| `seat_player_happy_path` | Seats a player, returns a valid seat number |
| `seat_player_default_chips` | `chips: 0` defaults to 10 000 |
| `seat_player_table_full` | 10th player gets an error response |
| `seat_player_at_happy_path` | Seats at a specific slot |
| `remove_player_happy_path` | Removes a seated player, returns name |
| `remove_player_empty_seat` | Returns error for an already-empty seat |
| `get_status_empty_table` | Empty table has no seats, hand not in progress |
| `start_hand_not_enough_players` | One player → error response |
| `start_hand_happy_path` | Two players → `hand_in_progress: true` |
| `act_fold_happy_path` | Fold routes through dealer, returns `ActionResult` |
| `act_missing_action_field` | `None` action → `InvalidArgument` status |
| `get_pot_before_hand` | Pot is 0 before any hand |
| `get_board_before_hand` | Board string is empty/short before dealing |
| `get_next_to_act_no_hand` | Returns message string when no hand is running |
| `get_next_to_act_during_hand` | Returns `Info` with seat/name/chips/pot during a hand |
| `get_chips_with_players` | Returns one entry per seated player at correct amount |
| `get_chips_after_blinds_posted` | SB 950 + BB 900 = 1850 after blinds |
| `get_event_log_grows_after_start_hand` | Log line count increases after `start_hand` |
| `get_event_log_populated_after_start_hand` | Log has ≥ 3 entries after `start_hand` |
| `end_hand_after_fold` | `end_hand` returns `HandResult` after a fold |
| `end_hand_chips_conserved` | Total chips = 2000 after payout |
| `advance_street_before_betting_complete_returns_error` | Advancing mid-betting round returns error |
| `advance_street_to_flop` | After preflop complete, board is non-empty after flop |
| `full_hand_call_check_all_streets_to_showdown` | Full two-player hand through all streets; chips conserved |
| `stream_events_receives_seat_event` | Subscriber receives `PLAYER_SEATED` event |
| `service_binary_and_client_binary_ping_round_trip` (e2e) | Real binary → real client, checks `"pong:pkdealer-client"` |
| `service_binary_and_client_binary_ping_round_trip_empty_client_id` (e2e) | Empty client id → `"pong"` |

#### Discovery during test writing

`Dealer::new()` writes a `TableOpen` event to the log immediately on construction,
so the event log is never truly empty. The `get_event_log_empty_before_hand` test
was rewritten as `get_event_log_grows_after_start_hand` to compare line counts
before and after `start_hand` rather than asserting emptiness.

---

### Demo tooling

#### `crates/pkdealer_client/examples/demo.rs`

A standalone example binary that plays through one complete hand:

1. **Ping** — confirms the service is alive
2. **SeatPlayer × 2** — seats Alice and Bob with 1 000 chips each
3. **StartHand** — shuffles, posts blinds, deals hole cards (cards printed in output)
4. **GetNextToAct** — shows who must act and the current pot
5. **Act (Fold)** — UTG folds, ending the hand immediately
6. **EndHand** — evaluates and pays out the pot
7. **GetChips** — shows the chip delta for each player
8. **GetStatus** — confirms `hand_in_progress: false`

Run against a live service:
```
cargo run --example demo -p pkdealer_client
```

#### `demo.sh`

A tmux script that opens a single window split into two side-by-side panes:

- **Left pane** — runs `pkdealer_service` (service log and pkcore debug output appear here)
- **Right pane** — waits 2 s for the service to be ready, then runs the demo example

Behaviour:
- Kills any stale `pkdealer-demo` tmux session before starting
- Builds both binaries before opening tmux (no compile delay mid-demo)
- Focuses the right pane so demo output is front and centre
- Prompts "Press any key to quit" when the demo finishes; kills the whole session on keypress

Run:
```
./demo.sh
```

---

## Upcoming Phases

| Phase | Goal |
|-------|------|
| Phase 2 | Web spectator app |
| Phase 3 | OpenTelemetry instrumentation |
| Phase 4 | AI agent clients |
| Future | Multi-table support via `pkcore::TableManager` |
