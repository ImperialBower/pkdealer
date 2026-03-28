# Phase 1 — Complete the pkdealer gRPC Server

**Goal:** A fully functional gRPC poker table server backed by `pkcore`,
where the existing `pkdealer_client` can connect and play a hand
end-to-end.

---

## Context

The `dealer.proto` already defines all the RPCs needed: `SeatPlayer`,
`StartHand`, `Act`, `AdvanceStreet`, `EndHand`, `StreamEvents`,
`GetStatus`, etc. Only `Ping` is currently implemented. This phase wires
up the rest using `pkcore::Table` and `pkcore::Dealer` as the game engine.

---

## Architecture

```
pkdealer workspace
├── pkdealer_proto    — generated gRPC types (existing)
├── pkdealer_service  — gRPC server + game engine (this phase)
└── pkdealer_client   — test client (existing, already pings)

pkdealer_service
└── Arc<Mutex<Table>> (pkcore)
      ├── DealerService gRPC handlers
      └── tokio::sync::broadcast::Sender<TableEvent>
```

---

## Work Items

### 1. Implement all `DealerService` RPC handlers

Wire each RPC in `pkdealer_service` to the corresponding `pkcore`
operation:

| RPC | pkcore call |
|-----|-------------|
| `SeatPlayer` | `Table::add_player` |
| `StartHand` | `Dealer::deal` |
| `Act` | `Table::act` (fold / call / raise) |
| `AdvanceStreet` | `Table::advance_street` |
| `EndHand` | `Table::end_hand` / showdown |
| `GetStatus` | read from `Arc<Mutex<Table>>` |
| `StreamEvents` | subscribe to broadcast channel |

### 2. Shared table state

Use `Arc<Mutex<Table>>` passed into each gRPC handler via tonic's
`Extension` or a shared service struct. The mutex is held only for the
duration of each state mutation — not across awaits.

```rust
#[derive(Clone)]
pub struct DealerService {
    table: Arc<Mutex<Table>>,
    events: broadcast::Sender<TableEvent>,
}
```

### 3. Event streaming

Implement `StreamEvents` using `tokio::sync::broadcast`:

- `broadcast::channel` is created at service startup
- Each `StreamEvents` caller receives a `broadcast::Receiver`
- Every state mutation (deal, act, street advance, showdown) sends a
  `TableEvent` to the channel before releasing the lock

### 4. Game loop binary

Add `pkdealer_orchestrator` (or drive the loop from `pkdealer_service`
itself) that runs hands autonomously:

```
seat players
loop:
  start hand → deal hole cards
  for each street:
    prompt each active player to act (via pending-action queue)
    advance street when all players have acted
  showdown / award pot
  repeat
```

Streets auto-advance once all active players have acted. A new hand
starts automatically after showdown.

### 5. Hole card visibility

`GetStatus` returns hole cards conditionally:
- A player requesting their own seat receives their hole cards
- Any other seat's hole cards are redacted
- A request bearing the spectator/admin token receives all hole cards

Player identity is a UUID issued at `SeatPlayer` time and passed as gRPC
metadata on subsequent calls.

---

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Shared state | `Arc<Mutex<Table>>` | Simple; lock held only during mutations |
| Out-of-order RPCs | Return `PermissionDenied` | Enforce game phase at the RPC layer |
| Reconnect identity | UUID from `SeatPlayer` | Stateless clients can rejoin |
| Auth (POC) | Shared secret in gRPC metadata | Pluggable layer; replace with JWT later |
| Multi-table | Not in scope | `pkcore::TableManager` exists for later |

---

## Deliverable

`cargo run --bin pkdealer_service` starts a server on port 50051. The
existing `pkdealer_client` can:

1. `Ping` — already works
2. `SeatPlayer` — register two players, receive UUIDs
3. `StartHand` — deal hole cards
4. `Act` — each player folds, calls, or raises
5. `AdvanceStreet` — flop → turn → river
6. `EndHand` — showdown, pot awarded
7. `GetStatus` — returns table state with hole cards visible only to the
   requesting player
8. `StreamEvents` — streams all table events to a subscriber

---

## Out of Scope for Phase 1

- Web spectator app (Phase 2)
- OTel instrumentation (Phase 3)
- AI agent clients (Phase 4)
- Multi-table support (future)
- Production auth (JWT / OAuth2)
