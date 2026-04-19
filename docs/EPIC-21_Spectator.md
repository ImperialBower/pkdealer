# EPIC-21: Web Spectator App

## Status

| Component | Status |
|---|---|
| `pkdealer_spectator` crate | Planned |
| Axum web server (serve static UI) | Planned |
| `GET /state` — full table snapshot (JSON, all cards) | Planned |
| `GET /events` — SSE stream of table events | Planned |
| gRPC `StreamEvents` subscriber (spectator token) | Planned |
| Table UI — oval layout, seat positions, board, pot | Planned |
| Card rendering (SVG) | Planned |
| Action log sidebar | Planned |
| Live chip count updates | Planned |

---

## Context

With EPIC-20 delivering an autonomous game loop, the service can run hands
continuously without manual orchestration. EPIC-21 adds the broadcast layer:
a web app that subscribes to the service's event stream and renders the live
table state in a browser — all hole cards visible, real-time updates. This is
the "PokerGo-style" spectator view described in the ROADMAP Phase 2.

The spectator is read-only. It connects to `pkdealer_service` as a privileged
subscriber (using the spectator token) and re-broadcasts table events to
browsers over Server-Sent Events.

---

## Architecture

```
Browser
  │  EventSource("/events")          HTTP GET /state (initial load)
  ▼
pkdealer_spectator  (Axum, port 3000)
  │  gRPC StreamEvents (spectator token)
  ▼
pkdealer_service    (Tonic, port 50051)
  │  Arc<Mutex<TableState>>
  ▼
pkcore::PokerSession
```

The spectator crate is a new workspace member. It owns no game state — it is
purely a proxy and renderer.

---

## Design

### New crate: `crates/pkdealer_spectator`

```
crates/pkdealer_spectator/
├── Cargo.toml
└── src/
    ├── main.rs          — Axum server entry point + gRPC subscriber task
    ├── state.rs         — shared AppState (latest TableStatus snapshot)
    └── handlers.rs      — route handlers: /, /state, /events
```

**Key dependencies:**
- `axum` — web framework
- `tokio` + `tokio-stream` — async runtime and streaming
- `tonic` — gRPC client for `StreamEvents`
- `pkdealer_proto` — `TableEvent`, `TableStatus`, `StreamEventsRequest`
- `serde_json` — serialize `TableStatus` for `/state` and SSE payloads
- `tower-http` — static file serving for the frontend assets

### Shared state

```rust
#[derive(Clone)]
struct AppState {
    /// Most recently received TableStatus from the service.
    latest: Arc<RwLock<Option<TableStatus>>>,
    /// SSE broadcast channel — one sender, many browser receivers.
    sse_tx: broadcast::Sender<String>,   // JSON-serialized TableEvent
}
```

On startup, a background task connects to `pkdealer_service` via gRPC
`StreamEvents` (with the spectator token in metadata), receives each
`TableEvent`, updates `latest`, and broadcasts the event to all connected
SSE clients.

### Routes

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Serve the HTML/JS frontend (embedded via `include_str!` or static dir) |
| `GET` | `/state` | Return current `TableStatus` as JSON (all cards visible) |
| `GET` | `/events` | SSE stream; each message is a JSON-serialized `TableEvent` |

### Frontend

A single-page app that:
1. Fetches `/state` on load to render initial table snapshot
2. Opens `EventSource("/events")` to receive live updates
3. Re-renders the affected components on each event

**Technology:**
- React (Vite build) or plain HTML/CSS/JS — TBD; either serves the goal
- [SVG-cards](https://github.com/htdebeer/SVG-cards) for card assets
- Tailwind CSS for layout
- Oval table layout with 9 seat positions, dealer button, pot display
- Action log panel showing last N events

**Layout sketch:**

```
┌─────────────────────────────────────────────────────┐
│  seat 8  seat 0  seat 1  seat 2                     │
│                                                     │
│  seat 7    ┌─── TABLE ───┐    seat 3               │
│            │  Board: A♠K♥Q♦  │                     │
│  seat 6    │  Pot: 1,200     │    seat 4            │
│            └─────────────────┘                     │
│  seat 5                                             │
│                                                     │
│  ─────────────── Action Log ──────────────────────  │
│  Seat 2: raises to 400  │  Seat 3: folds            │
└─────────────────────────────────────────────────────┘
```

Each seat shows: player name, chip count, hole cards (all visible in spectator
mode), and an indicator for the active seat.

---

## Work Items

1. Add `pkdealer_spectator` to the workspace `Cargo.toml` members list
2. Create `crates/pkdealer_spectator/Cargo.toml` with required dependencies
3. Implement `AppState` with `RwLock<Option<TableStatus>>` and SSE broadcast sender
4. Write the background gRPC subscriber task:
   - Connect to service with spectator token in metadata
   - Receive each `TableEvent`, update `latest`, broadcast JSON to SSE channel
   - Reconnect on disconnect with exponential backoff
5. Implement Axum route handlers (`/`, `/state`, `/events`)
6. Build the frontend (React/Vite or plain HTML):
   - Oval table with 9 seat positions
   - SVG card rendering
   - SSE listener that patches the rendered state on each event
   - Dealer button, blinds indicator, pot display
7. Write integration test: start service + spectator, assert `/state` returns valid JSON
8. Add `pkdealer_spectator` binary to workspace `[[bin]]` or as its own crate entry point

---

## Configuration

| Variable | Default | Purpose |
|----------|---------|---------|
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
| `PKDEALER_SPECTATOR_TOKEN` | `spectator` | Auth token for full card visibility |
| `PKSPECTATOR_ADDR` | `127.0.0.1:3000` | Axum listen address |

---

## Verification

```bash
# Build
cargo build --workspace

# Start service (with EPIC-20 autonomous loop)
cargo run --bin pkdealer_service &

# Start spectator
cargo run --bin pkdealer_spectator &

# Open browser
open http://localhost:3000

# Start bot agents (EPIC-23) to generate live traffic
cargo run --bin pkdealer_agent_random -- --name alice --seat 0 &
cargo run --bin pkdealer_agent_random -- --name bob --seat 1 &
```

The browser should show both players' hole cards, the board, and the pot
updating in real time as hands are played.
