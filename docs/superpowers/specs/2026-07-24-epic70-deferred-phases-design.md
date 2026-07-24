# EPIC-70 Deferred Phases — Design

**Date:** 2026-07-24
**Branch:** `boss`
**Scope:** Complete the deferred work of EPIC-70 (Collusion & Cheat Detection):
Phase 3 Vector-B arena integration + A/B parity, Phase 4 live boss + OTel, and
Phase 5 calibration harness. Phases 4–5 are **authored but unvalidated** —
they require live multi-container `docker compose` arena runs that this session
cannot execute; the design makes the honesty markers explicit and forbids
fabricated calibration numbers.

Companion to `docs/superpowers/specs/2026-07-23-epic70-phase3-vector-b-backchannel-design.md`
(which produced the landed 3a/3b backchannel code) and the EPIC itself,
`docs/EPIC-70_Collusion_and_Cheat_Detection.md`.

---

## Starting state (ground truth, 2026-07-24)

The EPIC doc's Status table is **stale**. Actual code state on branch `boss`:

- **3a/3b DONE in code:** `pkdealer_backchannel` broker crate (`Broker`,
  `CardShare`, binds `0.0.0.0:9099` via `PKDEALER_BACKCHANNEL_BIND`);
  `pkdealer_agent_core::backchannel::BackchannelClient` (publish /
  `partner_cards` by `hand_no`); `collude::backchannel_source::PeerSource`
  wired into `connect_partner_source` (`main.rs`). `--collusion-channel peer`
  connects to the broker — it is **not** "rejected at startup" as the table
  claims. Identity plumbing (`SeatInfo.player_id` → `SeatSnapshot`) landed.
- **Integration GAP:** `bin/arena:253` **hard-codes** `--collusion-channel
  spectator`. There is no broker service in the generated compose, no
  `PKDEALER_BACKCHANNEL` env, and `arena.toml` teams have no way to *declare*
  the peer channel. The landed Vector-B code is therefore unreachable from a
  real arena.
- **Not done:** 3c (Boss over a Vector-B session), Phase 4 (live boss + OTel),
  Phase 5 (calibration / FP study / write-up).

---

## Phase 3 — Vector-B arena integration *(fully verifiable here)*

### Config surface — per-seat `channel` field

`arena.toml` gains an optional per-seat `channel`, mirroring the existing
`style` idiom exactly (default `spectator`):

```toml
[players.mallory]
type    = "rules"
profile = "gto"
team    = "A"
style   = "dump"
channel = "peer"       # new; "spectator" (default) | "peer"
```

Rationale: `style` is already a per-seat optional field resolved at
`collusion_partner_for`/`emit_service`; `channel` follows the same path with no
new global state, and the choice is recorded visibly in the lineup file.
Documented in the `arena.toml` schema header next to `style`. Honest players and
spectator-channel teams need no change.

### `bin/arena` changes

1. Resolve `channel` per colluding seat via `registry_field "$name" channel`
   (default `spectator` when absent), the same way `style` is read.
2. Replace the hard-coded `spectator` at `emit_service` (line 253) with the
   resolved channel.
3. When the composed lineup contains **any** peer colluder:
   - Emit a `pkdealer_backchannel` compose service, built from the existing
     `Dockerfile.agent` with `BIN_NAME: pkdealer_backchannel` (no dealer
     dependency; it never contacts the service).
   - Give each **peer** seat `PKDEALER_BACKCHANNEL: pkdealer_backchannel:9099`
     in its `environment:` and `pkdealer_backchannel` in its `depends_on:`.
   - Spectator seats are unchanged (they still just carry the spectator token
     path; no broker).

### Verification — `tests/arena_peer.sh`

A grep-over-generated-override test mirroring `tests/arena_team.sh`:

- A peer team (`mallory`+`trudy`, `channel = "peer"`) →
  the override contains `--collusion-channel peer` for both seats, a
  `pkdealer_backchannel:` service block, and `PKDEALER_BACKCHANNEL:
  pkdealer_backchannel:9099` on both peer seats.
- A spectator team → `--collusion-channel spectator`, **no** broker service,
  **no** `PKDEALER_BACKCHANNEL`.
- An honest lineup → no collusion flags and no broker (regression guard that the
  broker is emitted *only* when needed).

No containers are run; the test asserts on generated text, exactly as
`arena_team.sh` does today.

---

## Phase 3c — A/B parity on the Boss *(verifiable, with an honest caveat)*

A `pkdealer_boss` test builds a planted Vector-B-shaped `RedactedHand` corpus
and asserts its per-pair signals match the Vector-A shape within tolerance.

**Honesty caveat (written into the test doc-comment and the EPIC):** at the
`RedactedHand` layer, Vector A and Vector B are *identical by construction* —
both emit the same public actions; the redaction firewall erases every trace of
*how* the cards arrived. So this test guards the redact/signal path against
regression, but it is close to tautological as a channel-equivalence claim. The
genuine parity claim (same table behavior, different wire) still requires a live
Vector-B run; the test states this explicitly and does not overclaim.

---

## Phase 4 — Live boss + OTel *(authored, unvalidated)*

### `crates/pkdealer_agent_boss/` (new binary)

An observer process that never takes a seat:

- Dials the dealer with the spectator token (same `ExportSession` gating as a
  Vector-A cheater — the trust-boundary honesty is already documented in the
  EPIC's "Live boss binary" section).
- Polls `ExportSession` on the **watermark-throttle** cadence reused from
  `ExploitPuller` (re-export only when the completed-hand count grows).
- Calls `redact()` at ingest — the un-redacted `HandCollection` is dropped
  before any detection code touches it.
- Feeds the existing `pkdealer_boss` detector to maintain rolling per-pair LLR;
  flags a pair the first hand its LLR crosses the Wald bound with the
  `Confidence` floor met.
- Emits flags via structured log **and** OTel.

### OTel instruments

Boss-local `init_otel` mirroring `crates/pkdealer_service/src/otel.rs`, under
its own `OTEL_SERVICE_NAME`, honoring `OTEL_SDK_DISABLED=true`:

- `pkdealer.boss.pair_llr` — gauge, per pair.
- `pkdealer.boss.flag_hand` — histogram, hand index at flag.
- `pkdealer.boss.false_positive` — counter.

### `bin/arena` — `boss` type

A `boss` type in `emit_service`: emits a boss container with the spectator
token and OTel env, `depends_on: pkdealer_service`, no seat. Selected like any
other lineup entry.

### Honesty markers

Unit-tested where a unit can be isolated: redact-at-ingest drops cards; one LLR
update step; the `OTEL_SDK_DISABLED` no-op path. The **end-to-end poll loop is
not run against a live service.** The crate carries a module doc note
("authored, not validated against a live arena — EPIC-70 Phase 4") and the EPIC
Status/corrigendum records it as authored-not-validated.

---

## Phase 5 — Calibration harness *(authored, unvalidated — hardest honesty case)*

Real calibration needs seeded/live runs (EPIC-41 is unstarted). Inventing
median-hands-to-detection or FP numbers would **fabricate results**, which the
EPIC explicitly forbids. Phase 5 therefore ships **the harness, not the
results**:

- **5a / 5c** — a calibration module + CLI that, *given* K session files, fits
  the honest null distribution per signal and sets Wald bounds / computes an FP
  rate with a confidence interval. Unit-tested on **synthetic fixture corpora**
  with known planted signatures — not real runs.
- **5b** — a win-rate-lift function: pooled bb/100 (collusion) − pooled bb/100
  (control) over a session pair, fixture-tested.
- **5d / 5e** — the statistical write-up and DEVLOG close-out are authored as
  **templates with every result table left as an explicit `pending live run`
  placeholder.** No fabricated numbers.

---

## Cross-cutting

- **Workspace:** add `crates/pkdealer_agent_boss` to `Cargo.toml` members; OTel
  dependencies mirror `pkdealer_service`.
- **Doc reconciliation:** fix the stale EPIC-70 Status table + Phase 3
  checkboxes (3a/3b are done); add a corrigendum entry recording the
  arena-integration channel surface and the Phase 4/5 honesty markers.
- **Git:** per the repo owner's global rule, no state-changing git is run by the
  agent. Exact `git add && git commit` commands are surfaced at each checkpoint,
  including for this design doc.

## Build order

1. Phase 3 arena integration (`arena.toml`, `bin/arena`, `tests/arena_peer.sh`).
2. Phase 3c parity test.
3. Phase 4 live boss crate + OTel + `bin/arena` boss type.
4. Phase 5 calibration harness + fixture tests + templated write-up.
5. Doc reconciliation (Status table, checkboxes, corrigendum).

Each step is independently testable. Verification per step: `cargo build`,
`cargo test`, `cargo clippy -- -D warnings` (feature-scoped where relevant),
`tests/arena_peer.sh`. What ran vs. what is authored-unvalidated is reported
honestly at each checkpoint.

## Non-goals

- No live `docker compose` arena run (cannot execute/validate here).
- No fabricated calibration/FP/hands-to-detection numbers.
- No change to the dealer service, proto, or pkcore.
- No fix to the spectator-token vulnerability (pkcore EPIC-79, out of scope).
