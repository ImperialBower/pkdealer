# Implementation Plan — Arena Hand Recorder & Session Export

**Status:** Proposed (implementation plan for Step 1 of the evaluation roadmap)
**Date:** 2026-05-30
**Companion to:** `EVAL-pkcore_play_evaluation.md` (§6, item 1 — "Capture first")
**Target repo:** `ImperialBower/pkdealer`
**Touches:** `proto/dealer.proto`, `crates/pkdealer_service`, (optional) `crates/pkdealer_client`

---

## 1. Goal

Make every hand played in the live gRPC arena persist as a `pkcore::hand_history::HandHistory`,
collected per session and exportable as YAML/JSON, so the existing `pkcore` analysis surface
(`replay()`, `StatsRegistry`, `RangeEquity`/`Ev`) can be run against real model-vs-model play.

Success criteria:

1. A completed hand in the **gRPC service** (not just the in-process `demo.rs`) produces a
   `HandHistory` with full hole-card visibility and correct settlement.
2. A session's hands are retrievable as a `HandCollection` YAML that **`audit.rs` parses and
   replays with `is_consistent == true`** and no chip leaks.
3. No change to agent behavior or to the existing happy-path RPCs.

---

## 2. Decision: record server-side, not from a spectator client

`HandHistory::from_table_state[_with_ids]` consumes **native pkcore types** — `ForcedBets`,
`Winnings`, `&[TableAction]`, plus hole-card strings and an optional deck string:

```rust
// pkcore/src/hand_history.rs
pub fn from_table_state_with_ids(
    hand_num: usize,
    ts_secs: u64,
    button: u8,
    forced: &ForcedBets,
    player_snapshot: &[(u8, String, usize, Option<String>, Option<Uuid>)],
    board_str: &str,
    winnings: &Winnings,
    event_log: &[TableAction],
    ending_stacks: &[(u8, usize)],
    source: &str,
    shuffled_deck: Option<String>,
) -> Self
```

A spectator client only receives **proto** messages over `StreamEvents` — free-text
descriptions, redacted hole cards, no deck. It would have to *re-derive* native types from text:
lossy and brittle.

The **service**, by contrast, already holds everything natively on `session.table`, behind the
same `Arc<Mutex<TableState>>` the hand-end path locks:

| Needed input | Source in service (confirmed) |
|---|---|
| `hand_num` | `session.hand_number` |
| `button` | `session.table.button` |
| `forced` (`ForcedBets`) | `session.table.forced` |
| `board_str` | `session.table.board.to_string()` |
| `event_log` (`Vec<TableAction>`) | `session.table.event_log` |
| hole cards / handle / stack / id | `session.table.seats.get_seat(s)` → `seat.player.{handle,chips,id}` |
| `winnings` (`Winnings`) | return value of `session.end_hand()` |
| `ending_stacks` | seats, read **after** `end_hand()` settles |

**Conclusion:** record in `pkdealer_service`. The spectator client stays a pure viewer.
This also sidesteps the proto's hole-card redaction entirely — the server is the authority and
sees all cards natively. (The *export RPC* then becomes the access-controlled surface; see §5.)

---

## 3. Data flow

```
Agents ──gRPC Act──▶ DealerService.act()
                         │
                         │ last action of hand → auto-advance → end_hand()
                         ▼
              ┌──────────────────────────────┐
              │ HAND-END HOOK (inside act())  │
              │ 1. snapshot button/forced/    │
              │    board/event_log/holecards  │   ◀── BEFORE end_hand() / reset
              │ 2. winnings = end_hand()      │
              │ 3. ending_stacks (post-settle)│   ◀── AFTER end_hand()
              │ 4. hh = from_table_state_with_│
              │      ids(...)                 │
              │ 5. recorder.push(hh)          │
              │ 6. (opt) append hh to YAML on │
              │      disk                     │
              └──────────────────────────────┘
                         │
   ExportSession RPC ◀───┘   (returns HandCollection YAML/JSON)
                         │
                         ▼
            pkcore: HandCollection::from_yaml → replay() / StatsRegistry / Ev
            (audit.rs already does the replay + chip-conservation pass)
```

**Ordering is load-bearing** and mirrors `demo.rs`: snapshot hole cards, board, button, forced
bets, and a `.clone()` of `event_log` **before** `end_hand()` (which resets the table); read
ending stacks **after** settlement.

---

## 4. Proto additions

Append to `proto/dealer.proto`. Additive only — no field renumbering, no breaking changes.

```proto
service DealerService {
  // ... existing RPCs ...

  // Export every hand recorded so far this session as a serialized
  // HandCollection. Requires a spectator/admin token (see x-player-token note
  // below) because the export contains every player's hole cards.
  rpc ExportSession(ExportSessionRequest) returns (ExportSessionResponse);

  // Lightweight progress check: how many hands are buffered, and the id range.
  rpc GetSessionInfo(GetSessionInfoRequest) returns (GetSessionInfoResponse);
}

enum RecordFormat {
  RECORD_FORMAT_UNSPECIFIED = 0;  // server default (YAML)
  RECORD_FORMAT_YAML        = 1;
  RECORD_FORMAT_JSON        = 2;
}

message ExportSessionRequest {
  RecordFormat format = 1;
  // If true, the in-memory buffer is cleared after a successful export so the
  // next export starts fresh. Default false (idempotent reads).
  bool drain = 2;
}
message ExportSessionResponse {
  oneof result {
    SessionExport export = 1;
    string        error  = 2;  // e.g. "recording disabled"
  }
}
message SessionExport {
  RecordFormat format    = 1;
  uint32       hand_count = 2;
  string       payload    = 3;  // HandCollection serialized in `format`
  string       source     = 4;  // session/source tag embedded in each hand
}

message GetSessionInfoRequest {}
message GetSessionInfoResponse {
  bool   recording_enabled = 1;
  uint32 hand_count        = 2;
  string first_hand_id     = 3;
  string last_hand_id      = 4;
  string record_dir        = 5;  // empty if disk persistence is off
}
```

**Format choice.** `HandHistory`/`HandCollection` derive `Serialize`/`Deserialize`, so:
- **YAML** via `HandCollection::to_yaml()` (already provided under the `hand-histories`
  feature, which the service's default-feature `pkcore` dep already enables). This is what
  `audit.rs` consumes today — pick it as the default for zero-friction reuse.
- **JSON** via `serde_json::to_string(&collection)` (add `serde_json` to the service crate).
  Useful for web/JS consumers; the same structs round-trip.

---

## 5. Service changes (`crates/pkdealer_service`)

### 5.1 State: add the recorder buffer

```rust
// In TableState (src/main.rs, ~line 187)
struct TableState {
    session: PokerSession,
    // ... existing fields ...

    /// Recorded hands for this session. Appended on every successful end_hand().
    recorder: pkcore::hand_history::HandCollection,
    /// Optional disk sink. When Some, each hand is also appended to a YAML file.
    record_dir: Option<std::path::PathBuf>,
}
```

Initialize in the constructor (~line 297) from config/env:

```rust
recorder: pkcore::hand_history::HandCollection::new(),
record_dir: std::env::var("PKDEALER_RECORD_DIR").ok().map(Into::into),
```

Recording is **on by default** (in-memory). Disk persistence is opt-in via
`PKDEALER_RECORD_DIR`, mirroring how `audit.rs` already scans a directory of YAML files.

### 5.2 The hand-end hook (inside `act()`, ~line 1256)

Today the hand-end branch calls `guard.session.end_hand()` and builds the proto `HandResult`.
Insert recording around it, preserving the snapshot-before / stacks-after ordering:

```rust
// --- BEFORE end_hand(): snapshot everything that reset() will clear ---
let hand_num    = guard.session.hand_number as usize;
let button      = guard.session.table.button;
let forced      = guard.session.table.forced;          // ForcedBets: Copy
let board_str   = guard.session.table.board.to_string();
let event_log   = guard.session.table.event_log.clone();
let ts_secs     = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

// 5-tuple snapshot incl. Uuid so StatsRegistry can correlate players across hands.
let player_snapshot: Vec<(u8, String, usize, Option<String>, Option<uuid::Uuid>)> =
    (0u8..SEAT_COUNT)
        .filter_map(|s| {
            let seat = guard.session.table.seats.get_seat(s)?;
            if seat.is_empty() { return None; }
            let hole = /* same hole-card stringify as demo.rs */;
            Some((s, seat.player.handle.clone(), seat.player.chips, hole, Some(seat.player.id)))
        })
        .collect();

// --- settle ---
match guard.session.end_hand() {
    Ok(winnings) => {
        // --- AFTER end_hand(): ending stacks reflect settlement ---
        let ending_stacks: Vec<(u8, usize)> = (0u8..SEAT_COUNT)
            .filter_map(|s| {
                let seat = guard.session.table.seats.get_seat(s)?;
                (!seat.is_empty()).then(|| (s, seat.player.chips))
            })
            .collect();

        let hh = pkcore::hand_history::HandHistory::from_table_state_with_ids(
            hand_num, ts_secs, button, &forced, &player_snapshot,
            &board_str, &winnings, &event_log, &ending_stacks,
            "arena",                       // source tag
            None,                          // shuffled_deck — see §6
        );

        if let Some(dir) = &guard.record_dir {
            // best-effort: log on failure, never abort the hand
            let _ = append_hand_yaml(dir, &hh);
        }
        guard.recorder.push(hh);

        // ... existing proto HandResult construction continues unchanged ...
    }
    Err(e) => { /* existing error handling unchanged */ }
}
```

Notes:
- Use the existing seat-iteration bound (the codebase uses `0u8..9`); `SEAT_COUNT` above is a
  stand-in for whatever constant/loop the surrounding code uses.
- `forced` is `Copy` (`ForcedBets`); the rest is cloned to outlive `end_hand()`.
- The hole-card stringify already exists in `demo.rs`; lift it into a shared helper.

### 5.3 Export RPC handlers

```rust
async fn export_session(&self, req: Request<ExportSessionRequest>)
    -> Result<Response<ExportSessionResponse>, Status>
{
    // Access control: require the spectator/admin token, since the payload
    // contains all hole cards. Reuse the existing token-metadata check.
    let guard = self.lock()?;
    let fmt = req.into_inner().format();
    let payload = match fmt {
        RecordFormat::Json =>
            serde_json::to_string(&guard.recorder).map_err(internal)?,
        _ /* Yaml | Unspecified */ =>
            guard.recorder.to_yaml().map_err(internal)?,
    };
    Ok(Response::new(ExportSessionResponse {
        result: Some(export_session_response::Result::Export(SessionExport {
            format: fmt as i32,
            hand_count: guard.recorder.len() as u32,
            payload,
            source: "arena".into(),
        })),
    }))
}
```

`GetSessionInfo` is a trivial read of `recorder.len()` and first/last `hand.id`.

### 5.4 Dependency / feature wiring

- `pkcore` in the service already pulls **default features** (incl. `hand-histories`,
  `player-stats`) — `to_yaml()` and the `HandHistory` API are available with **no version
  bump**. The recorder uses only published `0.1.2` API.
- Add `serde_json = "1"` to `pkdealer_service/Cargo.toml` only if JSON export is wanted.
- `uuid` is already a service dependency (token maps), so the 5-tuple snapshot is free.

---

## 6. Deck capture (optional, for exact replay)

`from_table_state*` accepts `shuffled_deck: Option<String>`; when present a hand "can be fully
replayed from this deck alone." `demo.rs` passes `None`, and `replay()` still works by
re-running the recorded actions against the recorded board — so this is a **nice-to-have**, not
a blocker.

Action: confirm the accessor for the post-shuffle deck on `TableNoCell` (e.g. a
`deck`/`to_deck_string()` method). If it exists and can be read at `start_hand` time, stash it in
`TableState` for the duration of the hand and pass `Some(deck_str)` into the recorder. If not,
ship with `None` and file a follow-up to expose it in `pkcore`.

---

## 7. Concurrency, memory, failure

- The recorder lives under the existing `Arc<Mutex<TableState>>`; the append happens while the
  hand-end path already holds the lock. No new synchronization.
- **Memory growth:** long sessions accumulate hands in RAM. Mitigations: (a) enable
  `PKDEALER_RECORD_DIR` so hands also stream to disk; (b) support `drain = true` on export to
  clear the buffer; (c) optional `PKDEALER_RECORD_MAX_HANDS` cap that drops oldest in-memory
  hands once flushed to disk.
- **Failure isolation:** recording is best-effort. A serialization or disk error must **log and
  continue** — never fail the hand or the `Act` RPC. Wrap disk writes in `let _ = ...;` with a
  `tracing::warn!`.

---

## 8. Testing

Reuse the existing verifier — `audit.rs` is already the replay + chip-conservation oracle.

1. **Unit (service):** after a scripted heads-up hand, assert `recorder.len() == 1` and that the
   recorded hand's `replay()` yields `is_consistent == true`.
2. **e2e:** extend `tests/e2e_two_players.rs` — play a hand, call `ExportSession(YAML)`, parse
   with `HandCollection::from_yaml`, replay, assert consistent and no chip leak across hands.
3. **Round-trip:** `to_yaml → from_yaml` and `serde_json round-trip` equal the in-memory
   collection.
4. **Regression harness:** point `audit.rs` at a `PKDEALER_RECORD_DIR` from a real multi-bot run
   and assert zero inconsistencies/leaks — this doubles as a pkcore settlement-bug detector.

---

## 9. Rollout phases

1. **Phase 1 — in-memory recorder + `ExportSession` (YAML).** Smallest change that unblocks all
   downstream analysis. Hook in `act()`, buffer in `TableState`, one RPC.
2. **Phase 2 — disk sink (`PKDEALER_RECORD_DIR`) + `GetSessionInfo` + JSON format.** Durability
   and web-friendly output; drain/cap controls.
3. **Phase 3 — deck capture** for exact replay (pending the `pkcore` accessor in §6).
4. **Phase 4 — agent-fidelity annotations** (Step 2 of the eval roadmap): thread each agent's
   raw response + `was_coerced` flag into the hand history as per-action metadata. This is a
   separate change in `agent_core`/`agent_llm` and likely needs a small `pkcore` schema
   extension, hence sequenced last.

---

## 10. Open decisions for sign-off

| # | Decision | Recommendation |
|---|---|---|
| 1 | Default export format | YAML (reuses `audit.rs` as-is); add JSON in Phase 2 |
| 2 | Recording on by default? | Yes, in-memory; disk opt-in via env |
| 3 | Access control on `ExportSession` | Require spectator/admin token (payload has all hole cards) |
| 4 | Capture deck for exact replay? | Phase 3, after confirming the `pkcore` accessor |
| 5 | Use `from_table_state_with_ids` (Uuid)? | Yes — needed for `StatsRegistry` correlation in Step 3 |
| 6 | Does anything reach the `pkcore 0.1.2` pin? | No bump needed for the recorder; revisit only for Phase 4 schema work |

---

## 11. Why this is the keystone

Once hands land as a `HandCollection`, the rest of the evaluation roadmap is mostly wiring over
existing `pkcore` primitives: `StatsRegistry::ingest_collection` gives per-model
VPIP/PFR/AF immediately; `RangeEquity` + `Ev` annotate decisions; `audit.rs` already validates
correctness. No replayable capture exists today, so this single change converts the entire
analysis surface from "available in principle" to "runnable on real arena games."
