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

#### `demo.sh` (now `bin/simpletmux`)

> **Renamed 2026-05-29:** this tmux launcher moved to `bin/simpletmux`. A
> separate full-stack docker launcher added in EPIC-24 took the `demo.sh`
> name and later moved to `bin/aiarena`.

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
./bin/simpletmux
```

---

---

## EPIC-22 — OpenTelemetry Instrumentation

**Status: ✅ Complete**

### What was built

- `crates/pkdealer_service/src/otel.rs` — `init_otel()`, `OtelGuards`,
  `MetadataExtractor`. W3C TraceContext propagator + OTLP gRPC trace +
  metric exporters; `OTEL_SDK_DISABLED=true` short-circuits init for
  tests and CI.
- Span hierarchy in `pkdealer_service`:
  - `hand` span — opened in `start_hand`, closed on `SessionStep::HandComplete`.
  - `street` span — opened on each `SessionStep::StreetAdvanced`, parent =
    current hand span.
  - `action` span — opened in `act` handler; parent = remote `traceparent`
    context (when present) or `current_street_span` (service-internal
    fallback). Records `seat`, `action_type`, `amount`, `pot_after`, and
    `linked_hand_trace` for cross-reference to the in-process tree.
- Four metrics: `pkdealer.hands_played` (counter on `HandComplete`),
  `pkdealer.pot_size` (histogram, chips, on `HandComplete`),
  `pkdealer.action_duration_ms` (histogram, ms, attributes `action_type`
  + `seat`, recorded on every `act` call where a prior NextActor prompt
  is known), and `pkdealer.ai_decision_latency_ms` (histogram, ms,
  **reserved for EPIC-23 agent clients** — service does not emit).
- `crates/pkdealer_service/Dockerfile` — multi-stage `cargo-chef` build,
  non-root user, default `OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317`,
  default `PKDEALER_ADDR=0.0.0.0:50051` so the container is reachable
  from other compose services and from the host.
- `docker-compose.yml` — service + `otel-collector` + Jaeger + Prometheus +
  Grafana. Host ports: 50051 (gRPC), 4317 (OTLP), 8889 (collector → Prom),
  16686 (Jaeger UI), 9090 (Prometheus), 3001 (Grafana).
- `ops/otel-collector.yaml`, `ops/prometheus.yml`, hand-authored
  `ops/grafana/dashboards/pkdealer.json` with 6 panels (hands/min, pot
  heatmap, action latency p50/p95/p99, action mix, agent latency
  placeholder, Jaeger link).

### Test infrastructure

- New dev-dep `tracing-test = "0.2"` (currently unused — kept for future
  span-attribute capture work).
- New dev-dep `serial_test = "3"` to gate the span-lifecycle tests that
  install thread-local subscribers.
- Inline lifecycle test `hand_span_spans_full_hand_lifecycle` uses a
  custom `SpanCounter` + `SpanCounterLayer` (instead of `tracing-test`'s
  formatted-log scan) for reliable open/close counting; annotated
  `#[serial_test::serial]` + `#[tokio::test(flavor = "current_thread")]`.
- Propagation test `action_span_inherits_agent_context` soft-asserts
  that injecting a `traceparent` header produces an action span — strict
  parent trace_id matching is deferred to a future task (the
  `tracing-opentelemetry` SDK doesn't expose the span's OTel context
  from inside `Layer::on_new_span` without also installing the full OTel
  layer in the test registry).

### Notes

- Action-span parent selection: when EPIC-23 agents inject `traceparent`,
  action spans become children of the agent span; the in-process
  cross-reference is preserved via the `linked_hand_trace` field
  (`trace_id` of the local hand span). A true OTel span-link can replace
  this when the SDK exposes the API on `tracing::Span`.
- `init_otel`'s disabled path no longer installs a fmt subscriber as a
  side effect — that would have clobbered the thread-local subscribers
  test code relies on.
- The `pkdealer_service` container defaults `PKDEALER_ADDR=0.0.0.0:50051`;
  host `cargo run` still defaults to `127.0.0.1:50051` so the dev
  experience is unchanged.
- `jaegertracing/all-in-one:1.62` does not exist on Docker Hub; the
  compose stack uses `:latest` for local dev (EPIC-24 will pin Jaeger
  to a specific version as part of the production-packaging pass).

## Upcoming Phases

| Phase | Goal | Status |
|-------|------|--------|
| Phase 2 / EPIC-21 | Web spectator app | Shipped (separate `pkspectator` repo) |
| Phase 4 / EPIC-23 | AI agent clients | ✅ Shipped — see EPIC-23 below |
| EPIC-40 | Local LLM backend | ✅ Shipped — see EPIC-40 below |
| EPIC-41 | Reproducible agent-vs-bots scenarios (deck injection + comparison harness) | Not started — see `docs/EPIC-41_Reproducible_Scenarios.md` |
| EPIC-42 | Dynamic arena runner — compose any table from a CLI player DSL | Not started — see `docs/EPIC-42_Dynamic_Arena_Runner.md` |
| Future | Multi-table support via `pkcore::TableManager` | Not started |

---

## EPIC-20 close-out — Seat resume via `client_secret` (2026-05-23)

**Status: ✅ Complete**

### What was added

A client-chosen `client_secret` string can now be passed on `SeatPlayer` and
`SeatPlayerAt`. If the same secret is seen on a later call (and the seat
has not been removed), the service returns the original seat number and
`x-player-token` and sets `resumed: true` in the response. This lets a
crashed agent process re-attach to its seat on restart without losing
chips or identity.

### Why this matters

The service is the only authoritative state for an agent's seat — when an
agent crashed, the proto offered no way to re-claim the seat without
either (a) calling `RemovePlayer` and starting fresh (losing chips) or
(b) the user manually arranging seat numbers. Both are unacceptable for
EPIC-23's autonomous bot agents.

### Scope of changes

- `proto/dealer.proto`: added `client_secret` (request) and `resumed`
  (response) to both `SeatPlayer*` message pairs.
- `crates/pkdealer_service/src/main.rs`: added `secret_to_token` map to
  `TableState`; resume branches in both handlers; cleanup in
  `remove_player`. Also added `PKDEALER_PORT` env-var support to
  `run_from_env` (the e2e harness uses it; the original code only
  honored `PKDEALER_ADDR`).
- `crates/pkdealer_service/tests/e2e_seat_resume.rs`: 5 new e2e tests
  covering happy path, no-secret path, `SeatPlayerAt` happy path, seat
  mismatch, and removal cleanup.

### Out of scope (deliberately)

- **Service-side persistence to disk.** Service restart wipes the map.
- **Authentication of the secret.** Anyone with the file can take over
  the seat; acceptable for local-demo scope.
- **Action timeout / auto-fold.** A bot that crashes mid-turn does not
  block the table only because nothing forces it to act yet. If demos
  surface this, add it in a future EPIC.

### Sets up

EPIC-23 (`pkdealer_agent_core`) can now ship a `load_or_create_secret`
helper that persists a per-agent UUID to `~/.pkdealer/agents/<name>.secret`
and threads it into every `SeatPlayer` call. See
`docs/EPIC-23_Bot_Agents.md`.

---

## License migration — GPL-3.0 → MIT OR Apache-2.0 (2026-05-24)

**Status: ✅ Complete** (`d7ad344`)

Relicensed the whole repo to dual MIT OR Apache-2.0 to match `pkcore`.
`LICENSE-GPL3.0` removed; `LICENSE-MIT` and `LICENSE-APACHE` copied
verbatim from pkcore. Root `Cargo.toml` `license` field switched; READMEs
and `docs/notes/*` updated. `deny.toml` allow-list rewritten — copyleft is
now disallowed; `docs/notes/GPL_LICENSE_COMPATIBILITY.md` deleted and
`CARGO_DENY_QUICKSTART.md` rewritten to drop the GPL framing.

---

## EPIC-23 — Bot Agents (2026-05-24)

**Status: ✅ Complete** (PRs #10, #11)

Autonomous bot agents that connect to the dealer as gRPC clients, resume
their seat via the EPIC-20 `client_secret`, and play hands unattended.

### What was built

- `pkdealer_agent_core` — shared infrastructure for every agent binary:
  `HandState` (the table state visible to one seat), `Decision` (the
  action an agent may take), the `run_agent` loop, and the
  `load_or_create_secret` helper that persists a per-agent UUID to
  `~/.pkdealer/agents/<name>.secret`.
- `pkdealer_agent_rules` — rule-based bot driven by a pkcore `BotProfile`
  (aggression, bluff frequency, bet sizing) via `RuleBasedDecider`.
  Ships the `gto` / `loose_aggressive` (lag) / `tight_aggressive` (tag)
  profiles.
- `pkdealer_agent_random` — uniform-random legal-action baseline that
  establishes a performance floor for the other agents.
- `pkdealer_agent_claude` — `ClaudeBackend` targeting the Anthropic
  Messages API (see also the crate README, `8d7df9b`).

### Why it matters

Delivers the "AI agent clients" milestone (old Phase 4). The dealer was
already the sole authority for seat state after EPIC-20; agents simply
drive it from the outside, so no server changes were needed beyond seat
resume.

---

## EPIC-24 — Demo packaging (2026-05-25)

**Status: ✅ Complete** (`55ada60`, `9c28e55`, `059ce16`; PR #13)

One-command "conference demo" of the full platform.

- Five-agent demo stack added to `docker-compose.yml` (gto / lag / tag
  rule bots, random, ollama) wired to the dealer + OTel collector.
- `Dockerfile.agent` — shared `cargo-chef` multi-stage build
  parametrized by `BIN_NAME` so every agent binary builds from one
  Dockerfile.
- A `demo.sh` one-command launcher (Ollama preflight check + dealer
  `:50051` readiness wait). **Renamed 2026-05-29 to `bin/aiarena`.**
- `deny.toml` allow-list extended with `ISC` and `CDLA-Permissive-2.0`
  for new transitive deps.
- Presenter guide and runbook (`DEMO.md`).

This also closed out the EPIC-22 TODO to pin Jaeger as part of the
production-packaging pass.

---

## EPIC-40 — Local LLM backend / multi-model shootout (2026-05-25)

**Status: ✅ Complete** (`4ea4c66`, `9a8cf89`; PRs #12, #14)

Renumbered from EPIC-25 → EPIC-40 to avoid a number collision with
pkcore's Range Frequencies epic.

- `pkdealer_agent_llm` — shared building blocks for LLM-backed agents:
  the `LlmBackend` trait (HTTP transport, auth, request/response shape
  per provider) and `LlmPokerAgent`, which adapts a backend into the
  `pkdealer_agent_core::run_agent` loop. `pkdealer_agent_claude` was
  refactored onto this trait.
- `pkdealer_agent_ollama` — `OllamaBackend` targeting a locally-running
  Ollama server (`/api/chat`), enabling a fully offline LLM agent.
- Presentation: `docs/presentations/epic-40-multi-model-shootout.md`.

---

## Rebuy (2026-05-27)

**Status: ✅ Complete** (PR #15, `11b2862`)

Added a `Rebuy` RPC so a busted seat can reload chips. Guards:

- `chips == 0` (busted) requires `rebuy_on_bust_enabled`.
- Rebuy is **rejected mid-hand** — an all-in player can have `chips == 0`
  while `chips_in_play > 0`, and reloading then would corrupt pot
  accounting.
- `default_rebuy_amount` configures the reload size.

Covered by `crates/pkdealer_service/tests/e2e_rebuy.rs`.

---

## Agent loop self-heal (2026-05-29)

**Status: ✅ Complete** (`9bd042a`)

Hardened the shared agent loop (`pkdealer_agent_core/src/runner.rs`)
against a silently-stuck seat:

- The blocking `event_stream.message()` is now wrapped in a 3-second
  `tokio::time::timeout`. On timeout the agent pulls authoritative state
  via `GetStatus` and acts if the table is waiting on its seat.
- Extracted `act_if_my_turn` so the event-driven path and the reconcile
  path recover an idle seat identically; added a `fetch_status` helper.
- Declared tokio's `time` feature explicitly in `Cargo.toml`.

This complements EPIC-20 seat resume: resume recovers a *crashed* agent
on restart; the self-heal recovers a *live* agent that missed an event.
