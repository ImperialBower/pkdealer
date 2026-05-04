# EPIC-21: Web Spectator App

## Status

The spectator now lives in its own repository:
[ImperialBower/pkspectator](https://github.com/ImperialBower/pkspectator). The
in-process `crates/pkdealer_service/src/web.rs` was removed; pkspectator subscribes
to the dealer service over gRPC `StreamEvents` like any other client.

| Component | Status |
|---|---|
| `pkspectator` crate (separate repo) | **Complete** |
| Axum web server (serve embedded UI) | **Complete** |
| `GET /state` — full table snapshot (JSON, all cards) | **Complete** |
| `GET /events` — SSE stream of table events | **Complete** |
| gRPC `StreamEvents` subscriber (spectator token) | **Complete** |
| Service-side `filter_cards` per-subscriber visibility | **Complete** |
| Table UI — oval layout, seat positions, board, pot | **Complete** |
| Card rendering (SVG) | **Complete** |
| Action log sidebar | **Complete** |
| Live chip count updates | **Complete** |

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
pkspectator  (Axum, port 3000)  — https://github.com/ImperialBower/pkspectator
  │  gRPC StreamEvents (spectator token)
  ▼
pkdealer_service    (Tonic, port 50051)
  │  Arc<Mutex<TableState>>
  ▼
pkcore::PokerSession
```

The spectator lives in its own repository and binary. It owns no game state —
it is purely a proxy and renderer that connects to `pkdealer_service` like any
other gRPC client.

---

## Design

### Separate repository: `ImperialBower/pkspectator`

The spectator is not a workspace member of this repo. See
[ImperialBower/pkspectator](https://github.com/ImperialBower/pkspectator) for
its full source, including the Axum server, gRPC subscriber task, and frontend.

### Service-side changes (this repo)

- `filter_cards` in `pkdealer_service/src/main.rs` — enforces per-subscriber
  card visibility based on `CardVisibility` (`Hidden`, `Player(seat)`,
  `Spectator`). Spectator subscribers receive all hole cards; player subscribers
  see only their own.
- `web.rs` was removed — the in-process HTTP server is no longer part of the
  service. The spectator connects over gRPC like any other client.

### Routes (served by pkspectator)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Embedded HTML/JS frontend |
| `GET` | `/state` | Current `TableStatus` as JSON (all cards visible) |
| `GET` | `/events` | SSE stream; each message is a JSON-serialized `TableEvent` |

### Frontend

A single-page app that:
1. Fetches `/state` on load to render the initial table snapshot
2. Opens `EventSource("/events")` to receive live updates
3. Re-renders affected components on each event

**Layout:**

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

## Configuration

### pkdealer_service (this repo)

| Variable | Default | Purpose |
|----------|---------|---------|
| `PKDEALER_ADDR` | `127.0.0.1:50051` | gRPC listen address |
| `PKDEALER_SPECTATOR_TOKEN` | `spectator` | Auth token for full card visibility |

### pkspectator (separate repo)

| Variable | Default | Purpose |
|----------|---------|---------|
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
| `PKDEALER_SPECTATOR_TOKEN` | `spectator` | Must match the service token |
| `PKSPECTATOR_ADDR` | `127.0.0.1:3000` | Axum listen address |

---

## Verification

```bash
# Build this repo
cargo build --workspace

# Start service (with EPIC-20 autonomous loop)
cargo run --bin pkdealer_service &

# Start spectator (from the pkspectator repo)
cargo run --bin pkspectator &

# Open browser
open http://localhost:3000

# Start bot agents (EPIC-23) to generate live traffic
cargo run --bin pkdealer_agent_random -- --name alice --seat 0 &
cargo run --bin pkdealer_agent_random -- --name bob --seat 1 &
```

The browser shows all players' hole cards, the board, and the pot updating in
real time as hands are played.
