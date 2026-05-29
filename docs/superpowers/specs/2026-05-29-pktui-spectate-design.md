# pktui `spectate` mode — terminal viewer for the aiarena demo

**Date:** 2026-05-29
**Status:** Approved design, pending implementation plan
**Code lives in:** the `pktui` repo (`../pktui`, sibling to `pkdealer`)
**Spec lives in:** this repo (`pkdealer`), by request

## Goal

Add a read-only **Spectate** mode to `pktui` that connects to a running
`pkdealer_service` over gRPC, subscribes to the `StreamEvents` RPC, and
renders the live table plus a rolling event log in the terminal. It is the
terminal-native counterpart to the web `pkspectator`, intended for the
`aiarena` demo where five agent containers fill the seats and a human only
watches.

Launch:

```sh
cargo run -- spectate                                 # defaults to http://localhost:50051
cargo run -- spectate --endpoint http://host:50051
```

## Decisions (locked during brainstorming)

1. **Placement:** a new mode *inside* the pktui repo (not a new crate in
   pkdealer, not a fork). Reuses pktui's existing renderer.
2. **Scope:** read-only spectator. No seating, no acting, no mutating RPCs.
3. **Display:** pktui's existing layout (header + seat table + board/pot +
   event log) **plus a per-seat profit/loss column**.
4. **Async bridge:** a background OS thread running a current-thread tokio
   runtime owns the gRPC client and forwards events through a
   `std::sync::mpsc` channel into pktui's existing synchronous render loop.

## Why this fits pktui

pktui is built on The Elm Architecture: an immutable `App` model, a
`Msg`-based reducer (`update`), and a pure `ui::view` render. The render loop
is synchronous (`poll → reduce → render` on a 50 ms tick, no tokio). The
dealer client is async (tonic/tokio).

The `StreamEvents` RPC makes the viewer simple: every `TableEvent` carries a
**complete `TableStatus` snapshot** (`current_status`) plus a human-readable
`description`. The reducer never has to aggregate events into state — it just
swaps in the latest snapshot and appends the description to the log. With an
empty `player_token`, the server redacts hole cards exactly as a spectator
should see them.

## Architecture

### New mode

Add `AppMode::Spectate(Box<SpectateState>)` alongside `Play` / `Arena` /
`Replay`. `SpectateState` does **not** own a `pkcore` `PokerSession`. It owns:

- `status: Option<TableStatus>` — the latest snapshot (None until first event).
- `config: Option<TableConfig>` — variant/blinds for the header (best-effort).
- `conn: ConnState` — `Connecting | Connected | Disconnected`.
- `endpoint: String` — for display.
- `rx: std::sync::mpsc::Receiver<SpectateMsg>` — drained each tick.

The shared `LogPanel` stays on `App` (unchanged).

### Background thread bridge

`main` (or `SpectateState::new`) spawns a dedicated OS thread that builds a
current-thread tokio runtime and:

1. Connects `DealerServiceClient::connect(endpoint)`.
2. Calls `stream_events(StreamEventsRequest { player_token: String::new() })`.
3. Loops `event_stream.message().await`, sending each `TableEvent` into the
   channel as `SpectateMsg::Event(Box<TableEvent>)`.
4. On stream end or any transport error: send `SpectateMsg::Conn(Disconnected)`,
   back off ~1 s, reconnect, send `SpectateMsg::Conn(Connected)` on success.

Reconnection lives entirely in this thread, so the demo's container restarts
do not require the user to relaunch the viewer. The channel `Sender` is moved
into the thread; the `Receiver` lives in `SpectateState`.

The synchronous render loop is otherwise untouched: `event::next_event` gains
a non-blocking `try_recv` drain of the spectate receiver (when in Spectate
mode) so channel messages become `Msg`s alongside crossterm input.

### Message & state flow

Two new top-level `Msg` variants:

- `Msg::TableEvent(Box<TableEvent>)` → reducer sets
  `SpectateState.status = event.current_status` (ignored if `None`, logged at
  debug) and appends `event.description` to the `LogPanel`, with `Severity`
  inferred from `EventType` (e.g. `HAND_ENDED` highlighted, `PLAYER_ACTION`
  normal).
- `Msg::Connection(ConnState)` → updates `SpectateState.conn`, appends a log
  line on transitions.

The reducer stays pure and only ever *replaces* the snapshot.

## Rendering (reuse seam)

pktui's renderer is layered: `render_table_view_play/arena` read pkcore types
but funnel into `render_seats(&[SeatRow])`, where `SeatRow` is a plain
view-model struct. That is the reuse seam.

- New `ui::table::render_table_view_spectate(state, frame, area)` builds
  `Vec<SeatRow>` from `TableStatus.seats` and calls the existing
  `render_seats`. Field mapping from proto `SeatInfo`:
  - `seat_number → seat`
  - `player_name → name`
  - `chips → chips`
  - `cards → hole` (already redacted to `??` server-side with no token)
  - `chips_in_play → bet`
  - `state == FOLDED || state == OUT → folded`
  - `seat_number == next_to_act → Accent::Active`
  - `profit_loss → pnl` (new field, see below)
- **New `pnl: i32` field on `SeatRow`**, rendered as a signed column colored
  green (≥0) / red (<0). Play and Arena populate it from pkcore engine state
  (or `0` as an interim value); Spectate populates it from
  `SeatInfo.profit_loss`, which the server pre-computes.
- New `render_board_str` draws the board from `TableStatus.board` (already a
  ready-to-print string), parallel to the existing pkcore `render_board`.
- The **action bar becomes a status/help line** in Spectate mode: connection
  state, endpoint, and `pause` / `q` / `?` hints. No input actions.
- The log panel and help overlay are reused unchanged.
- The header shows variant/blinds (from `TableConfig` when available), the
  current street, pot, and connection status.

### Known v1 gap: position tags

`TableStatus` does not carry the button position. v1 derives SB/BB tags from
`PlayerState::BLIND` where present and otherwise shows no position tag. This
is an accepted limitation; full BTN/SB/BB tagging would require the server to
expose the button seat.

## Dependencies

Add to pktui's `Cargo.toml`:

- `tokio` (features: `rt` for the current-thread runtime built manually via
  `runtime::Builder::new_current_thread().enable_all()`, plus `time` for the
  reconnect backoff). No `macros` — there is no `#[tokio::main]`; the runtime
  is owned by the spawned OS thread.
- `tonic` (feature: `transport`)
- `pkdealer_proto = { path = "../pkdealer/crates/pkdealer_proto" }`

`pkdealer_proto`'s `build.rs` uses `protoc-bin-vendored`, so no host `protoc`
is required, and it reads `proto/dealer.proto` relative to its own manifest
dir — a path dependency from pktui builds correctly as long as `pkdealer` is
checked out as a sibling. This is the same "checked out alongside this repo"
assumption DEMO.md already states for `pkspectator`, and mirrors how pktui
already path-patches `../pkcore`.

## Error handling

- Connection failures never panic the TUI. They surface as a header status
  (e.g. `⚠ disconnected — retrying`) and a log line; the table keeps showing
  the last good snapshot.
- A `TableEvent` with an empty `current_status` is ignored (logged at debug).
- Terminal raw-mode restore on quit/panic is unchanged — pktui already
  installs a panic hook.

## Testing

- **Adapter unit tests** (pure, no network): `TableStatus → Vec<SeatRow>`
  covering redacted cards, folded/out seats, the active-seat highlight, and
  signed profit/loss formatting.
- **Render smoke test**: `render_table_view_spectate` against a `TestBackend`,
  mirroring the existing `arena_renders_without_panic`.
- **Reducer tests**: `Msg::TableEvent` swaps the snapshot and appends to the
  log; `Msg::Connection` updates the header status.
- The background thread / live gRPC path is verified manually against the
  running `aiarena` demo; it is out of scope for unit tests.
- Test function names follow the repo convention: no `test_` prefix.

## Out of scope (YAGNI)

- Seating a human or acting (`SeatPlayer` / `Act` and token management).
- A separate `GetPlayerStats` panel.
- Multi-table or table selection.
- Hole-card reveal via a spectator token.
- Config-file endpoint (CLI flag only for v1).

## Build sequence (for the implementation plan)

1. CLI: add the `spectate` subcommand + `--endpoint` flag.
2. Proto/async deps + the background thread bridge with reconnection.
3. `SpectateState`, `AppMode::Spectate`, and the channel drain in `next_event`.
4. `Msg::TableEvent` / `Msg::Connection` reducer arms.
5. `SeatRow.pnl` field + the `TableStatus → SeatRow` adapter +
   `render_table_view_spectate` + `render_board_str`.
6. Header + status-line rendering for Spectate.
7. Tests (adapter, reducer, render smoke).
