# pktui `spectate` Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only `spectate` mode to `pktui` that connects to a running `pkdealer_service` over gRPC, subscribes to `StreamEvents`, and renders the live table (with a per-seat profit/loss column) plus a rolling event log in the terminal.

**Architecture:** A background OS thread runs a current-thread tokio runtime that owns the `DealerServiceClient`, subscribes to `StreamEvents`, and forwards each `TableEvent` (plus connection-state and table-config messages) through a `std::sync::mpsc` channel into pktui's existing **synchronous** poll→reduce→render loop. The reducer never aggregates events — every `TableEvent` carries a full `TableStatus` snapshot, so the viewer just swaps in the latest snapshot and appends the event description to the log. Hole cards are redacted by the server because the stream is opened with an empty `player_token`.

**Tech Stack:** Rust 2024, `ratatui` 0.30, `crossterm` 0.29, `clap` 4, `tonic` 0.12, `tokio` 1 (current-thread runtime), `pkdealer_proto` (path dependency on the sibling `pkdealer` checkout).

**Repo note:** All code changes are in the **`pktui`** repo (`/Users/christoph/src/github.com/ImperialBower/pktui`), a sibling of `pkdealer`. This plan and its spec live in `pkdealer/docs/superpowers/`.

**Spec:** `pkdealer/docs/superpowers/specs/2026-05-29-pktui-spectate-design.md`

**Git note (user rule):** Do NOT run any state-changing git command. Each "Commit" step lists the exact command for the user to run themselves.

---

## File Structure

Files created/modified, all paths relative to the `pktui` repo root:

- **Modify** `Cargo.toml` — add `tokio`, `tonic`, `pkdealer_proto` dependencies.
- **Create** `src/modes/spectate.rs` — `ConnState`, `SpectateMsg`, `SpectateState`, the background-thread bridge, and the apply/drain logic. One responsibility: own the live spectator state and its transport.
- **Modify** `src/modes/mod.rs` — register and re-export the `spectate` module.
- **Modify** `src/ui/table.rs` — add `pnl` to the private `SeatRow`, add the `P/L` column to `render_seats`, and add the spectate renderers (`render_table_view_spectate`, `status_to_rows`, `render_board_str`).
- **Modify** `src/cli.rs` — add the `Spectate` subcommand + `SpectateArgs`.
- **Modify** `src/app.rs` — add `AppMode::Spectate`, the `App::new` arm, the `label` arm, and `App::poll_spectate`.
- **Modify** `src/update.rs` — add `Msg::SpectateTogglePause`, the `spectate_key` handler, and the `update`/`Tick` arms.
- **Modify** `src/ui/mod.rs` — add the `view` dispatch arm and the `render_header` arm for Spectate.
- **Modify** `src/ui/action_bar.rs` — add the Spectate status/help line.
- **Modify** `src/main.rs` — call `app.poll_spectate()` each loop iteration.
- **Modify** `README.md` — document the `spectate` subcommand.

---

## Task 1: Add async + proto dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add dependencies**

In `Cargo.toml`, under `[dependencies]`, after the `pkcore` line, add:

```toml
tokio = { version = "1", features = ["rt", "time"] }
tonic = { version = "0.12", features = ["transport"] }
pkdealer_proto = { path = "../pkdealer/crates/pkdealer_proto" }
```

Rationale: `rt` gives the manually-built current-thread runtime; `time` gives the reconnect backoff sleep. No `macros` — there is no `#[tokio::main]`. `pkdealer_proto` ships a vendored `protoc` (via `protoc-bin-vendored` in its build script) and reads `proto/dealer.proto` relative to its own manifest dir, so the path dependency builds with no host toolchain as long as `pkdealer` is checked out as a sibling.

- [ ] **Step 2: Verify the workspace still builds**

Run: `cargo build`
Expected: PASS (compiles `pkdealer_proto` and its tonic/prost tree, then `pktui`). First build is slow.

- [ ] **Step 3: Confirm the generated client path resolves**

Run: `cargo doc --no-deps -p pkdealer_proto 2>/dev/null; echo done` then sanity-check by adding a throwaway check — instead, just confirm the import resolves with a one-off:

Run: `cargo build 2>&1 | tail -5`
Expected: no errors. (The actual import is exercised in Task 2.)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock && git commit -m "build: add tokio, tonic, and pkdealer_proto deps for spectate mode"
```

---

## Task 2: Spectator state + background-thread bridge

**Files:**
- Create: `src/modes/spectate.rs`
- Modify: `src/modes/mod.rs`
- Test: inline `#[cfg(test)]` in `src/modes/spectate.rs`

- [ ] **Step 1: Write the failing test**

Create `src/modes/spectate.rs` with ONLY the test module first so it fails to compile (red):

```rust
//! Spectate mode: a read-only viewer of a live `pkdealer_service` table.
//!
//! Unlike Play/Arena/Replay, this mode owns no `pkcore` engine. A background
//! OS thread holds the gRPC stream and forwards [`SpectateMsg`]s through a
//! channel; [`SpectateState::drain`] applies them to the latest snapshot.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_panel::LogPanel;
    use pkdealer_proto::dealer::{SeatInfo, TableEvent, TableStatus};

    fn sample_status() -> TableStatus {
        TableStatus {
            seats: vec![SeatInfo {
                seat_number: 0,
                player_name: "gto".into(),
                chips: 9_500,
                cards: "??".into(),
                state: 4, // CALLED
                withdrawn: 10_000,
                chips_in_play: 500,
                profit_loss: -500,
            }],
            board: "Ah Kd Qc".into(),
            pot: 1_000,
            next_to_act: 0,
            hand_in_progress: true,
            game_over: false,
            current_street: 2, // FLOP
        }
    }

    #[test]
    fn apply_event_swaps_snapshot_and_logs() {
        let (mut state, _tx) = SpectateState::detached("http://localhost:50051");
        let mut log = LogPanel::new();
        let ev = TableEvent {
            timestamp: 1,
            event_type: 4, // PLAYER_ACTION
            description: "gto calls 500".into(),
            current_status: Some(sample_status()),
        };
        state.apply(SpectateMsg::Event(Box::new(ev)), &mut log);
        assert_eq!(state.status.as_ref().unwrap().pot, 1_000);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn apply_conn_change_updates_state_and_logs_once() {
        let (mut state, _tx) = SpectateState::detached("http://localhost:50051");
        let mut log = LogPanel::new();
        state.apply(SpectateMsg::Conn(ConnState::Connected), &mut log);
        assert_eq!(state.conn, ConnState::Connected);
        assert_eq!(log.len(), 1);
        // Same state again does not re-log.
        state.apply(SpectateMsg::Conn(ConnState::Connected), &mut log);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn paused_drops_snapshot_updates() {
        let (mut state, _tx) = SpectateState::detached("http://localhost:50051");
        let mut log = LogPanel::new();
        state.paused = true;
        let ev = TableEvent {
            timestamp: 1,
            event_type: 4,
            description: "ignored while paused".into(),
            current_status: Some(sample_status()),
        };
        state.apply(SpectateMsg::Event(Box::new(ev)), &mut log);
        assert!(state.status.is_none());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn conn_state_default_is_connecting() {
        assert_eq!(ConnState::default(), ConnState::Connecting);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pktui --lib modes::spectate 2>&1 | tail -20`
Expected: FAIL — `cannot find ... SpectateState`, `SpectateMsg`, `ConnState` (module not yet wired and types undefined).

- [ ] **Step 3: Write the implementation**

At the TOP of `src/modes/spectate.rs` (above the test module), add:

```rust
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pkdealer_proto::dealer::dealer_service_client::DealerServiceClient;
use pkdealer_proto::dealer::{
    EventType, GetTableConfigRequest, StreamEventsRequest, TableConfig, TableEvent, TableStatus,
};

use crate::error::{Error, Result};
use crate::log_panel::{LogPanel, Severity};

/// Default dealer endpoint, matching the gRPC port exposed by the demo stack.
pub const DEFAULT_ENDPOINT: &str = "http://localhost:50051";

/// Connection lifecycle of the background gRPC stream.
///
/// # Examples
///
/// ```
/// use pktui::modes::spectate::ConnState;
/// assert_eq!(ConnState::default(), ConnState::Connecting);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnState {
    /// Attempting to (re)connect to the dealer.
    #[default]
    Connecting,
    /// Stream is live.
    Connected,
    /// Stream dropped or the dealer is unreachable; a retry is scheduled.
    Disconnected,
}

impl ConnState {
    /// Short label for the header / status line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting…",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected — retrying",
        }
    }
}

/// A message produced by the background thread and consumed by the UI loop.
///
/// Network events are deliberately NOT routed through [`crate::update::Msg`]
/// (which is `Copy`); a `TableEvent` is large and owns heap data.
pub enum SpectateMsg {
    /// A table event carrying a full status snapshot + a description line.
    Event(Box<TableEvent>),
    /// Static table configuration, fetched once after connecting.
    Config(Box<TableConfig>),
    /// A connection-state transition.
    Conn(ConnState),
}

/// All state for the read-only spectator view.
pub struct SpectateState {
    /// The dealer endpoint we are watching (for display).
    pub endpoint: String,
    /// Latest table snapshot, or `None` until the first event arrives.
    pub status: Option<TableStatus>,
    /// Static table config (blinds / variant), best-effort.
    pub config: Option<TableConfig>,
    /// Current connection lifecycle state.
    pub conn: ConnState,
    /// When true, incoming snapshots are dropped (display freezes).
    pub paused: bool,
    /// Receiver drained each UI tick.
    rx: Receiver<SpectateMsg>,
    /// Kept alive so the worker thread is not detached prematurely.
    _handle: Option<JoinHandle<()>>,
}

impl SpectateState {
    /// Connects to `endpoint` and starts the background streaming thread.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Io`] if the worker thread cannot be spawned.
    /// Connection failures themselves are NOT errors here — they surface as
    /// [`ConnState::Disconnected`] through the channel so the UI keeps running.
    pub fn new(endpoint: String) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let ep = endpoint.clone();
        let handle = thread::Builder::new()
            .name("pktui-spectate".to_string())
            .spawn(move || run_stream(ep, &tx))
            .map_err(Error::Io)?;
        Ok(Self {
            endpoint,
            status: None,
            config: None,
            conn: ConnState::Connecting,
            paused: false,
            rx,
            _handle: Some(handle),
        })
    }

    /// Drains all pending channel messages, applying each to `self`.
    pub fn drain(&mut self, log: &mut LogPanel) {
        while let Ok(msg) = self.rx.try_recv() {
            self.apply(msg, log);
        }
    }

    /// Applies a single [`SpectateMsg`]. Pure with respect to the network —
    /// unit-tested directly via [`SpectateState::detached`].
    fn apply(&mut self, msg: SpectateMsg, log: &mut LogPanel) {
        match msg {
            SpectateMsg::Event(ev) => {
                if self.paused {
                    return;
                }
                let ev = *ev;
                if let Some(status) = ev.current_status {
                    self.status = Some(status);
                }
                if !ev.description.is_empty() {
                    log.push(severity_for(ev.event_type), ev.description);
                }
            }
            SpectateMsg::Config(cfg) => {
                self.config = Some(*cfg);
            }
            SpectateMsg::Conn(state) => {
                if self.conn != state {
                    log.push(Severity::Info, format!("{} ({})", state.label(), self.endpoint));
                }
                self.conn = state;
            }
        }
    }

    /// Test-only constructor: builds a detached state with no worker thread,
    /// returning the channel sender so tests can inject messages.
    #[cfg(test)]
    pub(crate) fn detached(endpoint: &str) -> (Self, Sender<SpectateMsg>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                endpoint: endpoint.to_string(),
                status: None,
                config: None,
                conn: ConnState::Connecting,
                paused: false,
                rx,
                _handle: None,
            },
            tx,
        )
    }
}

/// Maps a proto `EventType` discriminant to a log [`Severity`].
fn severity_for(event_type: i32) -> Severity {
    match EventType::try_from(event_type) {
        Ok(EventType::PlayerAction) => Severity::Action,
        Ok(EventType::HandEnded) => Severity::Win,
        _ => Severity::Info,
    }
}

/// Worker-thread entry point: owns a current-thread tokio runtime and the
/// reconnect loop. Exits when the receiver is dropped (UI quit).
fn run_stream(endpoint: String, tx: &Sender<SpectateMsg>) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        let _ = tx.send(SpectateMsg::Conn(ConnState::Disconnected));
        return;
    };
    rt.block_on(async {
        loop {
            let _ = connect_and_stream(&endpoint, tx).await;
            // Either a connect failure or a clean stream end: signal and retry.
            if tx.send(SpectateMsg::Conn(ConnState::Disconnected)).is_err() {
                break; // receiver dropped → UI quit → stop the thread
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            if tx.send(SpectateMsg::Conn(ConnState::Connecting)).is_err() {
                break;
            }
        }
    });
}

/// Connects, fetches config once, then forwards every streamed event.
/// `Err(())` means either a transport failure or that the receiver is gone.
async fn connect_and_stream(endpoint: &str, tx: &Sender<SpectateMsg>) -> std::result::Result<(), ()> {
    let mut client = DealerServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|_| ())?;
    tx.send(SpectateMsg::Conn(ConnState::Connected)).map_err(|_| ())?;

    if let Ok(resp) = client.get_table_config(GetTableConfigRequest {}).await
        && let Some(cfg) = resp.into_inner().config
    {
        tx.send(SpectateMsg::Config(Box::new(cfg))).map_err(|_| ())?;
    }

    let mut stream = client
        .stream_events(StreamEventsRequest {
            player_token: String::new(),
        })
        .await
        .map_err(|_| ())?
        .into_inner();

    while let Some(ev) = stream.message().await.map_err(|_| ())? {
        tx.send(SpectateMsg::Event(Box::new(ev))).map_err(|_| ())?;
    }
    Ok(())
}
```

- [ ] **Step 4: Register the module**

In `src/modes/mod.rs`, add to the module declarations (after `pub mod replay;`):

```rust
pub mod spectate;
```

And to the re-exports (after `pub use replay::ReplayState;`):

```rust
pub use spectate::{ConnState, SpectateMsg, SpectateState};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p pktui --lib modes::spectate 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 6: Run the doc test**

Run: `cargo test -p pktui --doc modes::spectate 2>&1 | tail -10`
Expected: PASS (the `ConnState::default` doc test).

- [ ] **Step 7: Commit**

```bash
git add src/modes/spectate.rs src/modes/mod.rs && git commit -m "feat(spectate): add SpectateState + background gRPC stream bridge"
```

---

## Task 3: Add the profit/loss column to the seat table

**Files:**
- Modify: `src/ui/table.rs`
- Test: inline `#[cfg(test)]` in `src/ui/table.rs`

This adds an `Option<i32>` `pnl` field to the private `SeatRow` and a `P/L`
column to the shared `render_seats`. Play/Arena pass `None` (shown as `—`);
the spectate adapter (Task 4) passes `Some(profit_loss)`.

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)] mod tests` block in `src/ui/table.rs`, add:

```rust
#[test]
fn render_seats_shows_pnl_column_header() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let rows = vec![SeatRow {
        seat: 0,
        name: "gto".to_string(),
        chips: 9_500,
        hole: "??".to_string(),
        bet: 500,
        tag: String::new(),
        folded: false,
        accent: Accent::None,
        pnl: Some(-500),
    }];
    let backend = TestBackend::new(120, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_seats(f, f.area(), &rows))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let header: String = (0..120).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(header.contains("P/L"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pktui --lib ui::table::tests::render_seats_shows_pnl_column_header 2>&1 | tail -20`
Expected: FAIL — `SeatRow` has no field `pnl` (compile error).

- [ ] **Step 3: Add the field to `SeatRow`**

In `src/ui/table.rs`, in the `struct SeatRow { ... }` definition, add a field after `accent`:

```rust
    /// Signed profit/loss for the seat. `None` when the mode does not track
    /// it (Play / Arena); `Some(_)` in Spectate mode from the dealer.
    pnl: Option<i32>,
}
```

- [ ] **Step 4: Set `pnl: None` in the `seat_rows` builder**

In `src/ui/table.rs`, in `fn seat_rows(...)`, in the `out.push(SeatRow { ... })` literal, add `pnl: None,` after `accent,`:

```rust
        out.push(SeatRow {
            seat: i,
            name: name_of(i),
            chips,
            hole,
            bet,
            tag,
            folded,
            accent,
            pnl: None,
        });
```

- [ ] **Step 5: Add the column to `render_seats`**

In `src/ui/table.rs`, in `fn render_seats(...)`:

(a) Add a `P/L` header cell — change the `header` `Row::new(vec![...])` to end with:

```rust
        Cell::from("Pos"),
        Cell::from("P/L"),
    ])
```

(b) Add a width for it — change the `widths` array to append `Constraint::Length(10)` as the final element:

```rust
        Constraint::Length(8),
        Constraint::Length(10),
    ];
```

(c) Build a per-cell `pnl_cell` inside the `.map(|r| { ... })` closure, just before the `Row::new(vec![...])`:

```rust
            let pnl_cell = match r.pnl {
                None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
                Some(v) => {
                    let color = if v >= 0 { Color::Green } else { Color::Red };
                    Cell::from(format!("{v:+}")).style(Style::default().fg(color))
                }
            };
```

(d) Append `pnl_cell` to the body `Row::new(vec![...])`, after the `Pos` cell `Cell::from(r.tag.clone())`:

```rust
                Cell::from(r.tag.clone()),
                pnl_cell,
            ])
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p pktui --lib ui::table 2>&1 | tail -20`
Expected: PASS (the new test plus all existing table tests — they construct `SeatNoCell`, not `SeatRow`, so they are unaffected).

- [ ] **Step 7: Verify Play/Arena still render**

Run: `cargo test -p pktui --lib ui::tests 2>&1 | tail -20`
Expected: PASS (`play_renders_without_panic`, `arena_renders_without_panic`).

- [ ] **Step 8: Commit**

```bash
git add src/ui/table.rs && git commit -m "feat(ui): add profit/loss column to the seat table"
```

---

## Task 4: Spectate renderers (adapter + table + board)

**Files:**
- Modify: `src/ui/table.rs`
- Test: inline `#[cfg(test)]` in `src/ui/table.rs`

Adds `status_to_rows` (proto `TableStatus` → `Vec<SeatRow>`),
`render_table_view_spectate`, and `render_board_str`. No `AppMode` changes yet
— these take `&SpectateState` directly, so the crate still compiles.

- [ ] **Step 1: Write the failing tests**

In the `#[cfg(test)] mod tests` block in `src/ui/table.rs`, add:

```rust
#[test]
fn status_to_rows_maps_seat_fields() {
    use pkdealer_proto::dealer::{SeatInfo, TableStatus};

    let status = TableStatus {
        seats: vec![
            SeatInfo {
                seat_number: 0,
                player_name: "gto".into(),
                chips: 9_500,
                cards: "??".into(),
                state: 4, // CALLED
                withdrawn: 10_000,
                chips_in_play: 500,
                profit_loss: -500,
            },
            SeatInfo {
                seat_number: 1,
                player_name: "lag".into(),
                chips: 0,
                cards: "??".into(),
                state: 8, // FOLDED
                withdrawn: 10_000,
                chips_in_play: 0,
                profit_loss: -10_000,
            },
        ],
        board: "Ah Kd Qc".into(),
        pot: 1_000,
        next_to_act: 0,
        hand_in_progress: true,
        game_over: false,
        current_street: 2,
    };

    let rows = status_to_rows(&status);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "gto");
    assert_eq!(rows[0].chips, 9_500);
    assert_eq!(rows[0].bet, 500);
    assert_eq!(rows[0].pnl, Some(-500));
    assert_eq!(rows[0].accent, Accent::Active); // seat 0 == next_to_act
    assert!(!rows[0].folded);
    assert!(rows[1].folded); // FOLDED state
    assert_eq!(rows[1].accent, Accent::None);
}

#[test]
fn render_table_view_spectate_does_not_panic() {
    use crate::modes::SpectateState;
    use pkdealer_proto::dealer::{SeatInfo, TableStatus};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (mut state, _tx) = SpectateState::detached("http://localhost:50051");
    state.status = Some(TableStatus {
        seats: vec![SeatInfo {
            seat_number: 0,
            player_name: "gto".into(),
            chips: 9_500,
            cards: "??".into(),
            state: 4,
            withdrawn: 10_000,
            chips_in_play: 500,
            profit_loss: -500,
        }],
        board: "Ah Kd Qc".into(),
        pot: 1_000,
        next_to_act: 0,
        hand_in_progress: true,
        game_over: false,
        current_street: 2,
    });

    let backend = TestBackend::new(120, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_table_view_spectate(&state, f, f.area()))
        .unwrap();
}
```

(The `detached` constructor is `#[cfg(test)] pub(crate)`, so it is visible to
this test module within the crate's test build.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p pktui --lib ui::table::tests::status_to_rows_maps_seat_fields 2>&1 | tail -20`
Expected: FAIL — `cannot find function status_to_rows` / `render_table_view_spectate`.

- [ ] **Step 3: Add the imports**

At the top of `src/ui/table.rs`, after the existing `use crate::modes::{ArenaState, Awaiting, PlayState};` line, add:

```rust
use crate::modes::SpectateState;
use pkdealer_proto::dealer::{PlayerState, Street, TableStatus};
```

- [ ] **Step 4: Implement the adapter and renderers**

In `src/ui/table.rs`, after `render_table_view_arena` (before the `Accent`
enum is fine, or after `render_board` — anywhere at module scope), add:

```rust
/// Renders the read-only spectator table from the latest dealer snapshot.
///
/// Shows a "waiting for dealer" placeholder until the first snapshot arrives.
pub fn render_table_view_spectate(state: &SpectateState, frame: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(11), Constraint::Length(3)])
        .split(area);

    match &state.status {
        Some(status) => {
            let rows = status_to_rows(status);
            render_seats(frame, chunks[0], &rows);
            render_board_str(&status.board, status.pot, status.current_street, frame, chunks[1]);
        }
        None => {
            let placeholder = Paragraph::new("Waiting for the dealer…")
                .block(Block::default().borders(Borders::ALL).title(" Table "));
            frame.render_widget(placeholder, chunks[0]);
            render_board_str("", 0, Street::Unspecified as i32, frame, chunks[1]);
        }
    }
}

/// Builds `SeatRow`s from a proto `TableStatus`. Hole cards are already
/// redacted by the dealer (empty `player_token`), so they are copied verbatim.
fn status_to_rows(status: &TableStatus) -> Vec<SeatRow> {
    status
        .seats
        .iter()
        .map(|s| {
            let folded = s.state == PlayerState::Folded as i32
                || s.state == PlayerState::Out as i32;
            let active = status.hand_in_progress && s.seat_number == status.next_to_act;
            let accent = if active { Accent::Active } else { Accent::None };
            SeatRow {
                seat: u8::try_from(s.seat_number).unwrap_or(u8::MAX),
                name: s.player_name.clone(),
                chips: s.chips as usize,
                hole: s.cards.clone(),
                bet: s.chips_in_play as usize,
                // Position tags require the button seat, which TableStatus does
                // not expose; left blank in v1 (documented gap).
                tag: String::new(),
                folded,
                accent,
                pnl: Some(s.profit_loss),
            }
        })
        .collect()
}

/// Board renderer driven by the proto's pre-formatted board string + pot.
fn render_board_str(board: &str, pot: u32, street: i32, frame: &mut Frame, area: Rect) {
    let street_label = match Street::try_from(street) {
        Ok(Street::Preflop) => "pre-flop",
        Ok(Street::Flop) => "flop",
        Ok(Street::Turn) => "turn",
        Ok(Street::River) => "river",
        _ => "—",
    };
    let board_display = if board.is_empty() {
        "(pre-flop)".to_string()
    } else {
        board.to_string()
    };
    let spans = vec![
        Span::styled("Board: ", Style::default().fg(Color::Gray)),
        Span::styled(
            board_display,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled("Pot: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{pot}"),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(
            format!("street: {street_label}"),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    let p = Paragraph::new(Text::from(vec![Line::from(spans)]))
        .block(Block::default().borders(Borders::ALL).title(" Board "));
    frame.render_widget(p, area);
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p pktui --lib ui::table 2>&1 | tail -20`
Expected: PASS (new adapter + render tests plus all existing table tests).

- [ ] **Step 6: Commit**

```bash
git add src/ui/table.rs && git commit -m "feat(spectate): add TableStatus->SeatRow adapter and spectate renderers"
```

---

## Task 5: Spectate status/help line in the action bar

**Files:**
- Modify: `src/ui/action_bar.rs`
- Test: inline `#[cfg(test)]` in `src/ui/action_bar.rs`

Adds a free function `spectate_hints(&SpectateState) -> Line` now; it gets
wired into `render`'s match in Task 6 (when `AppMode::Spectate` exists).

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)] mod tests` block in `src/ui/action_bar.rs`, add:

```rust
#[test]
fn spectate_hints_contains_connection_and_quit() {
    use crate::modes::SpectateState;
    let (state, _tx) = SpectateState::detached("http://localhost:50051");
    let line = spectate_hints(&state);
    let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(joined.contains("connecting"));
    assert!(joined.contains("quit"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pktui --lib ui::action_bar::tests::spectate_hints_contains_connection_and_quit 2>&1 | tail -20`
Expected: FAIL — `cannot find function spectate_hints`.

- [ ] **Step 3: Implement `spectate_hints`**

In `src/ui/action_bar.rs`, add the import at the top (after the existing
`use crate::modes::...` lines):

```rust
use crate::modes::SpectateState;
```

Then add the function (near `arena_hints`):

```rust
/// One-line status / help bar for the read-only spectator.
fn spectate_hints(state: &SpectateState) -> Line<'static> {
    let pause = if state.paused { "paused" } else { "live" };
    Line::from(vec![
        Span::styled(
            "Spectate",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{}  [{}]", state.conn.label(), pause),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("   "),
        keystyle(" space "),
        Span::raw(" pause   "),
        keystyle(" q "),
        Span::raw(" quit"),
    ])
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p pktui --lib ui::action_bar 2>&1 | tail -20`
Expected: PASS.

Note: the compiler will warn that `spectate_hints` is unused until Task 6
wires it into `render`. That is expected; the next task removes the warning.

- [ ] **Step 5: Commit**

```bash
git add src/ui/action_bar.rs && git commit -m "feat(spectate): add spectator status/help line"
```

---

## Task 6: Wire up the `spectate` mode end-to-end

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/app.rs`
- Modify: `src/update.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/action_bar.rs`
- Modify: `src/main.rs`
- Test: inline `#[cfg(test)]` in `src/cli.rs`, `src/app.rs`, `src/update.rs`

This task introduces `Command::Spectate`, `AppMode::Spectate`, and
`Msg::SpectateTogglePause`, and adds the matching arms to **every** `match`
so the crate compiles. All rendering helpers already exist (Tasks 2–5).

- [ ] **Step 1: Write the failing tests**

(a) In `src/cli.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn parses_spectate_with_endpoint() {
    let cli =
        Cli::try_parse_from(["pktui", "spectate", "--endpoint", "http://h:1"]).unwrap();
    match cli.resolved() {
        Command::Spectate(s) => assert_eq!(s.endpoint, "http://h:1"),
        _ => panic!("expected spectate"),
    }
}

#[test]
fn spectate_endpoint_defaults_to_localhost() {
    let cli = Cli::try_parse_from(["pktui", "spectate"]).unwrap();
    match cli.resolved() {
        Command::Spectate(s) => assert_eq!(s.endpoint, "http://localhost:50051"),
        _ => panic!("expected spectate"),
    }
}
```

(b) In `src/update.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn space_in_spectate_toggles_pause() {
    // Build a spectate App without a live dealer; the bg thread just retries.
    let cmd = crate::cli::Command::Spectate(crate::cli::SpectateArgs {
        endpoint: "http://localhost:1".to_string(),
    });
    let app = App::new(cmd).unwrap();
    let k = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    let m = event_to_msg(&app, &Event::Key(k));
    assert!(matches!(m, Msg::SpectateTogglePause));
}
```

(c) In `src/app.rs` `#[cfg(test)] mod tests` (create the block if missing —
see Step 7), add:

```rust
#[test]
fn spectate_mode_label() {
    let cmd = crate::cli::Command::Spectate(crate::cli::SpectateArgs {
        endpoint: "http://localhost:1".to_string(),
    });
    let app = App::new(cmd).unwrap();
    assert_eq!(app.mode.label(), "Spectate");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p pktui --lib 2>&1 | tail -20`
Expected: FAIL to compile — `SpectateArgs`, `Command::Spectate`,
`Msg::SpectateTogglePause`, and the `"Spectate"` label do not exist yet.

- [ ] **Step 3: Add the CLI subcommand**

In `src/cli.rs`:

(a) Import the default endpoint at the top (after `use std::path::PathBuf;`):

```rust
use crate::modes::spectate::DEFAULT_ENDPOINT;
```

(b) Add a variant to `enum Command`:

```rust
    /// Read-only viewer of a live `pkdealer_service` table over gRPC.
    Spectate(SpectateArgs),
```

(c) Add the args struct (after `ReplayArgs`):

```rust
/// Arguments to the `spectate` subcommand.
///
/// # Examples
///
/// ```
/// use pktui::cli::SpectateArgs;
/// let args = SpectateArgs::default();
/// assert_eq!(args.endpoint, "http://localhost:50051");
/// ```
#[derive(Args, Debug, Clone)]
pub struct SpectateArgs {
    /// gRPC endpoint of the dealer service.
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
}

impl Default for SpectateArgs {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
        }
    }
}
```

- [ ] **Step 4: Add `AppMode::Spectate`, the `new` arm, and the label**

In `src/app.rs`:

(a) Update the `use` for modes — change:

```rust
use crate::modes::{ArenaState, PlayState, ReplayState};
```
to:
```rust
use crate::modes::{ArenaState, PlayState, ReplayState, SpectateState};
```

(b) Add a variant to `enum AppMode`:

```rust
    /// Read-only spectator of a remote dealer.
    Spectate(Box<SpectateState>),
```

(c) Add the `label` arm in `AppMode::label`:

```rust
            Self::Spectate(_) => "Spectate",
```

(d) Add the `App::new` arm (in the `match command { ... }`):

```rust
            Command::Spectate(args) => {
                AppMode::Spectate(Box::new(SpectateState::new(args.endpoint)?))
            }
```

- [ ] **Step 5: Add `App::poll_spectate`**

In `src/app.rs`, inside `impl App`, add:

```rust
    /// Drains any pending spectate messages into the model. No-op outside
    /// Spectate mode. Called once per UI loop iteration by the binary.
    pub fn poll_spectate(&mut self) {
        let Self { mode, log, .. } = self;
        if let AppMode::Spectate(s) = mode {
            s.drain(log);
        }
    }
```

- [ ] **Step 6: Add the `Msg` variant, key handler, and update arms**

In `src/update.rs`:

(a) Add a variant to `enum Msg` (before `Noop`):

```rust
    /// Spectate: freeze/unfreeze the live snapshot display.
    SpectateTogglePause,
```

(b) Add the mode arm in `key_to_msg`'s `match &app.mode`:

```rust
        AppMode::Spectate(_) => spectate_key(key),
```

(c) Add the `spectate_key` handler (near `arena_key`):

```rust
fn spectate_key(key: &KeyEvent) -> Msg {
    use KeyCode::Char;
    match key.code {
        Char(' ') => Msg::SpectateTogglePause,
        _ => Msg::Noop,
    }
}
```

(d) Handle the new `Msg` in `update`'s `match msg` (add an arm):

```rust
        Msg::SpectateTogglePause => {
            if let AppMode::Spectate(s) = &mut app.mode {
                s.paused = !s.paused;
            }
        }
```

(e) Add a `Tick` arm for Spectate (in the `Msg::Tick => match &mut app.mode`
block) — Spectate is driven by the channel, not ticks, so it is a no-op:

```rust
            AppMode::Spectate(_) => {}
```

- [ ] **Step 7: Add the `app.rs` test module if missing**

If `src/app.rs` has no `#[cfg(test)] mod tests`, add at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectate_mode_label() {
        let cmd = crate::cli::Command::Spectate(crate::cli::SpectateArgs {
            endpoint: "http://localhost:1".to_string(),
        });
        let app = App::new(cmd).unwrap();
        assert_eq!(app.mode.label(), "Spectate");
    }
}
```

(If a test module already exists, just add the `spectate_mode_label` test from
Step 1c into it instead.)

- [ ] **Step 8: Add the view + header arms**

In `src/ui/mod.rs`:

(a) In `pub fn view`, add the dispatch arm in `match &app.mode`:

```rust
        AppMode::Spectate(s) => table::render_table_view_spectate(s, frame, chunks[1]),
```

(b) In `fn render_header`, add the arm in the `match &app.mode` that builds
`(subtitle, seed)`:

```rust
        AppMode::Spectate(s) => {
            let blinds = s
                .config
                .as_ref()
                .map(|c| format!("blinds {}/{}", c.small_blind, c.big_blind))
                .unwrap_or_default();
            (format!("{}  {}  {blinds}", s.endpoint, s.conn.label()), None)
        }
```

- [ ] **Step 9: Wire `spectate_hints` into the action bar**

In `src/ui/action_bar.rs`, in `pub fn render`, add the arm in `match &app.mode`:

```rust
        AppMode::Spectate(s) => vec![spectate_hints(s)],
```

This also removes the "unused function" warning from Task 5.

- [ ] **Step 10: Call `poll_spectate` in the loop**

In `src/main.rs`, in `fn run`, add `app.poll_spectate();` after the draw call:

```rust
    while !app.should_quit {
        terminal.draw(|f| ui::view(app, f))?;
        app.poll_spectate();
        let event = next_event(tick, &mut last_tick)?;
        let msg = event_to_msg(app, &event);
        update(app, msg)?;
    }
```

- [ ] **Step 11: Run the full test suite**

Run: `cargo test -p pktui 2>&1 | tail -25`
Expected: PASS (unit + doc tests). The new tests from Step 1 pass; existing
tests are unaffected.

- [ ] **Step 12: Lint clean**

Run: `cargo clippy -p pktui --all-targets 2>&1 | tail -25`
Expected: no warnings (in particular, no "unused `spectate_hints`").

- [ ] **Step 13: Commit**

```bash
git add src/cli.rs src/app.rs src/update.rs src/ui/mod.rs src/ui/action_bar.rs src/main.rs && git commit -m "feat(spectate): wire spectate subcommand into CLI, app, update, and UI"
```

---

## Task 7: Documentation + manual verification

**Files:**
- Modify: `README.md`
- Modify: `src/lib.rs` (module list doc)

- [ ] **Step 1: Document the subcommand in the README**

In `README.md`, in the "Install / run" code block, add a line after the
`replay` example:

```sh
cargo run --release -- spectate                          # watch a live pkdealer table (http://localhost:50051)
cargo run --release -- spectate --endpoint http://host:50051
```

Then add a short prose paragraph below that block:

```markdown
### Spectate mode

`spectate` is a read-only viewer of a live
[`pkdealer`](https://github.com/ImperialBower/pkdealer) table. It connects to
the dealer's gRPC `StreamEvents` endpoint and renders the table, a per-seat
profit/loss column, and a rolling event log — the terminal counterpart to the
web `pkspectator`. It needs the `pkdealer` repo checked out as a sibling
(`../pkdealer`) so the shared protobuf crate is available. Press `space` to
freeze/unfreeze the display, `q` to quit. The viewer auto-reconnects if the
dealer restarts.
```

- [ ] **Step 2: Mention the mode in the crate docs**

In `src/lib.rs`, in the `# Modules` doc list, update the `modes` bullet to
mention spectate:

```rust
//! * [`modes`] — per-mode initialisation (`play`, `arena`, `replay`,
//!   `spectate`).
```

- [ ] **Step 3: Verify it builds and the help text shows the subcommand**

Run: `cargo run -p pktui -- --help 2>&1 | tail -20`
Expected: lists `spectate` among the subcommands.

Run: `cargo run -p pktui -- spectate --help 2>&1 | tail -20`
Expected: shows `--endpoint` with default `http://localhost:50051`.

- [ ] **Step 4: Manual end-to-end check against the demo (out-of-band)**

This step is manual and requires the demo stack. In the `pkdealer` repo:

```bash
./bin/aiarena            # brings up dealer + 5 agents
```

Then in the `pktui` repo:

```bash
cargo run --release -- spectate
```

Expected: the header shows `connected`, seats fill with the five agents,
chips/bets/board update live as hands play, the P/L column shows green/red
signed values, and the log scrolls action descriptions. Stop the dealer
(`docker compose stop dealer` in `pkdealer`) and confirm the header switches
to `disconnected — retrying` without the TUI crashing; restart it and confirm
it reconnects. Press `space` to freeze, `q` to quit cleanly (terminal
restored).

- [ ] **Step 5: Commit**

```bash
git add README.md src/lib.rs && git commit -m "docs(spectate): document the spectate subcommand"
```

---

## Self-Review Notes (filled in by plan author)

- **Spec coverage:** Overview/launch → Task 1+6; new mode/state → Task 2;
  background bridge + reconnection → Task 2; message/state flow (refined: a
  separate `SpectateMsg`, since `Msg` is `Copy`) → Task 2+6; rendering reuse
  via `SeatRow`/`render_seats` → Task 3+4; profit/loss column → Task 3+4;
  board from string → Task 4; action bar → status line → Task 5+6; deps →
  Task 1; error handling (no panics, disconnect surfaces in header/log,
  empty `current_status` ignored) → Task 2+4; tests (adapter, reducer/apply,
  render smoke) → Tasks 2/3/4/6; known position-tag gap → Task 4 (blank tag);
  out-of-scope items not implemented.
- **Refinement vs spec:** the spec named `Msg::TableEvent` / `Msg::Connection`.
  Because `Msg` derives `Copy`, network events instead flow through
  `SpectateMsg` applied directly via `SpectateState::apply`/`drain`; only the
  keyboard-driven `Msg::SpectateTogglePause` is added to `Msg`. Behavior is
  identical; this is an implementation detail the spec's design intent allows.
- **Type consistency:** `SpectateState`, `SpectateMsg`, `ConnState`,
  `status_to_rows`, `render_table_view_spectate`, `render_board_str`,
  `spectate_hints`, `SpectateArgs`, `Command::Spectate`, `AppMode::Spectate`,
  `Msg::SpectateTogglePause`, `App::poll_spectate` are used consistently across
  tasks. `SeatRow.pnl: Option<i32>` is defined in Task 3 and consumed in Task 4.
- **Compilation ordering:** rendering helpers (Tasks 3–5) take `&SpectateState`
  and exist before the `AppMode::Spectate` variant is introduced (Task 6), so
  every task compiles and its tests run. The enum variant and all its `match`
  arms land together in Task 6.
