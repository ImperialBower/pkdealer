# EPIC-70: Collusion & Cheat Detection ("sentinel")

## Context

The dealer service is **server-authoritative and redacts hole cards per
subscriber**: `card_visibility_from_metadata` resolves the `x-player-token` gRPC
metadata to `Hidden` / `Player(seat)` / `Spectator`
(`crates/pkdealer_service/src/main.rs:1037-1056`), and every outbound
`TableStatus` is passed through `filter_cards` before it leaves the process
(`main.rs:655-672`; applied to `stream_events` per subscriber at
`main.rs:2697-2699`). A normal bot only ever sees its own two cards: the runner
finds `s.seat_number == my_seat` and lifts `s.cards` into
`HandState.hole_cards` (`crates/pkdealer_agent_core/src/runner.rs:274-280`), and
`HandState` (`crates/pkdealer_agent_core/src/hand_state.rs:105-131`) carries
**own hole cards only** — `SeatSnapshot` (`hand_state.rs:27-41`) has no cards and
no per-opponent stats. This is the intended "fog of war" anti-cheat posture
noted in `docs/notes/POSSIBLE_ARCHITECTURES.md:121`.

**But one over-privileged read already exists and is already wired into a
shipping bot.** The spectator token (`PKDEALER_SPECTATOR_TOKEN`, default
`"spectator"`, `main.rs:103-104`) grants `Spectator` visibility — *all* hole
cards, on every event, live throughout the hand. The rules bot's `ExploitPuller`
(`crates/pkdealer_agent_rules/src/main.rs:325-395`) opens a **dedicated second
gRPC connection** with that token, polls `GetSessionInfo` for `hand_count`, and
on each new hand calls `ExportSession` (JSON) — whose payload "contains every
player's hole cards" (`proto/dealer.proto:69-70`) — parsing it into a
`pkcore::hand_history::HandCollection` (`agent_rules/src/main.rs:383`). Today it
only distills that into aggregate `opponent_stats` (VPIP/PFR-style) via
`StatsRegistry::ingest_collection` (`main.rs:402-406`) — but the plumbing to read
opponents' actual cards is already sitting in the bot.

That means **collusion is one small step from what already exists**, and
**detection can reuse the identical tap**. This EPIC does both, as a matched
pair: plant colluders, then catch them.

**This EPIC does NOT:**
- Modify `pkcore` (external crate `0.3.1`, `Cargo.lock:1489-1492`). No engine
  change is required — collusion rides the existing spectator token, detection
  rides the existing broadcast/export RPCs.
- Fix the vulnerability. Closing the spectator-token hole (per-agent scoped
  tokens, mental-poker crypto per pkcore EPIC-79) is explicitly out of scope; the
  point here is to *measure detectability*, not to prevent collusion.
- Depend on EPIC-41 (Reproducible Scenarios / seeded deck injection) — that
  EPIC has **not** started (`docs/EPIC-41_Reproducible_Scenarios.md:5-21`), so
  there is no seedable RNG. Detection operates on live/recorded hands, not
  replayable seeds; time-to-detection is reported over hands dealt, not over a
  reproducible corpus.

---

## Status

| Component | Status |
|---|---|
| `arena.toml` `team` field + `bin/arena` `--team` passthrough | ⏳ Planned — Phase 0 |
| Colluding rules bot (`--collude-team`, partner-card read) | ⏳ Planned — Phase 1 |
| `pkdealer_sentinel` crate skeleton + live `StreamEvents` tap | ⏳ Planned — Phase 2 |
| Collusion signal detectors (soft-play EV, chip-dump flow, squeeze) | ⏳ Planned — Phase 3 |
| Sequential detection verdict + time-to-detection scoring | ⏳ Planned — Phase 4 |
| Detection metrics via OTel `Metrics` | ⏳ Planned — Phase 5 |
| Arena harness + honest-baseline false-positive gate | ⏳ Planned — Phase 6 |

> **Not yet started.** Design only. Every factual claim below is cited to a real
> `path:line` at commit `b999673` (2026-07-20). pkcore internals are cited by
> type/import path (external crate, no line access).

---

## Goals

- Make **two or more bots a collusion team** that secretly share hole cards and
  play to maximize the *team's* combined chips, not each seat's.
- Build a **live sentinel** that observes hands as they are dealt and decides
  **whether** a set of seats is colluding and, if so, **how many hands it took**
  to reach a confidence threshold.
- Treat the whole thing as a **labelled experiment**: we know the ground-truth
  team (we planted it), the sentinel runs blind, and the deliverable is a curve
  of **detection confidence vs. hands observed**, plus a **false-positive rate**
  against an all-honest table.

---

## Scope — the rules the feature must obey

**Collusion (the cheat):**
1. A **team** is two or more seats sharing a secret `team` id. Team membership is
   config, not observable to honest players or (by construction) to the sentinel.
2. Colluders learn their **partners' current hole cards** via the spectator token
   — the same channel `ExploitPuller` already uses — and adjust play:
   **soft-play** (don't build a pot against a partner), **chip-dump** (fold the
   better hand / underbet to move chips to the partner with more equity), and
   **squeeze** (coordinate raises to isolate a lone non-colluder between two team
   seats).
3. Colluders never send illegal actions — the runner's legality clamp
   (`finalize_decision`, `runner.rs:492-509`) is unchanged; cheating is in
   *choice*, not in protocol violation.

**Detection (the catch):**
4. The sentinel is a **separate client** with no privileged knowledge of team
   membership — it only knows it *may* be watching colluders and must decide.
5. The sentinel may use the spectator token (it is an operator tool, like
   `pkspectator`), so it sees all cards live — the interesting question is not
   "can you see the cards" but "**given the cards, can you tell soft-play from a
   nit folding a marginal hand, and how fast?**"
6. Output is a **per-candidate-pair verdict** with a calibrated confidence and the
   **hand index at which confidence first crossed the flag threshold**
   (time-to-detection).
7. **Honest control:** run against an all-honest table and require the
   false-positive flag rate to stay under a stated bound.

---

## Domain map

| Domain concept | Code construct | Status |
|---|---|---|
| A bot's action | `pkcore::casino::action::PlayerAction` → `Decision` (`agent.rs:23-37`) | ✅ exists |
| A bot's info set | `HandState` (own cards only) (`hand_state.rs:105-131`) | ✅ exists |
| The over-privileged read | spectator token → `Spectator` (`main.rs:1037-1056`) | ✅ exists |
| Cross-hand opponent aggregates | `StatsRegistry::ingest_collection` (pkcore) | ✅ exists (via `ExploitPuller`) |
| A collusion **team** | `team` id in `arena.toml` + `--collude-team` | ❌ new (Phase 0-1) |
| Partner-card-aware decision | colluding decider wrapper | ❌ new (Phase 1) |
| Live hand observation | `StreamEvents` subscriber | ✅ RPC exists (`main.rs:2669`) |
| Per-hand record w/ all cards | `HandHistory` in `HandCollection` (`main.rs:2046-2098`) | ✅ exists |
| Collusion **signal** | soft-play EV / chip-flow / squeeze detectors | ❌ new (Phase 3) |
| Detection **verdict** | sequential confidence + flag-hand | ❌ new (Phase 4) |

---

## Design

The design is two new artifacts plus a config field. Neither touches pkcore.

### 1. Team configuration — `team` field (Phase 0)

`arena.toml` gains an optional per-player `team`, next to `profile`/`type`
(`arena.toml:26-60`). `bin/arena`'s `registry_field` already reads arbitrary
fields generically (`bin/arena:52-64`), and `emit_service` passes a new
`--team <id>` flag into the rules service block exactly as it does `--profile`
today (`bin/arena:232-234`).

```toml
[players.mallory]
type    = "rules"
profile = "tight_aggressive"
team    = "A"           # colludes with anyone else on team "A"

[players.trudy]
type    = "rules"
profile = "loose_aggressive"
team    = "A"
```

Honest players simply omit `team`. The field is inert unless the bot is a
colluding build.

### 2. The colluding rules bot (Phase 1)

The rules bot already has the precedent for this shape: `--spectator-token` and
`--exploit` clap flags (`agent_rules/src/main.rs:151-165`) plus a dedicated
spectator connection (`ExploitPuller`). We add:

```rust
// crates/pkdealer_agent_rules/src/main.rs — Args
#[arg(long, env = "PKDEALER_COLLUDE_TEAM")]
collude_team: Option<String>,     // presence => this bot is a colluder
```

A colluding bot, on each `decide`, refreshes a **live table view** from its
spectator connection (reuse the `ExploitPuller` refresh path,
`main.rs:355-395`, but read the *current* hand's per-seat cards, not just the
finished-hand aggregates), identifies which seated `player_id`s share its
`team`, and threads the **partner hole cards + equity** into the decision. The
mechanism mirrors how `opponent_stats` was threaded onto `TableSnapshot`
(commit `762b7d5`, `snapshot_with_stats` at `main.rs:478-537`): add a
`partner_cards: Option<Cards>` input and a thin `ColludingDecider` that wraps the
existing `RuleBasedDecider` (`main.rs:308-311`):

```rust
struct ColludingDecider { inner: RuleBasedDecider, team: String }

impl ColludingDecider {
    fn decide(&self, profile, snapshot, partners: &[PartnerView]) -> PlayerAction {
        let base = self.inner.decide(profile, snapshot);
        match self.coordination(snapshot, partners) {
            SoftPlay  => downgrade(base),          // Raise->Call, Call->Check/Fold
            ChipDump  => yield_pot_to(best_partner),// fold better hand / underbet
            Squeeze   => amplify(base),            // raise to isolate the lone mark
            None      => base,
        }
    }
}
```

`PartnerView` = partner's seat, `player_id`, and current `Cards`, plus a shared
equity read (`EquityMode::Fast{samples}` already available,
`main.rs:169-177`) so the team can compare its members' hand strengths.
`coordination` fires only when a partner is contesting the same pot. The output
is still a legal `Decision`; nothing downstream of the decider changes.

**Why a wrapper, not a new profile:** collusion is orthogonal to archetype — a
`gto` colluder and a `maniac` colluder are both interesting. Wrapping keeps every
existing `BotProfile` (`load_profile`, `main.rs:585-608`) usable as the
underlying honest strategy, so soft-play is measured as a *delta* from that
bot's honest baseline.

### 3. `pkdealer_sentinel` — the live detector (Phase 2-4)

A new crate + bin, structured like `pkspectator` / `ExploitPuller`: a gRPC
client that connects with the spectator token and observes. Two ingest modes,
both already supported by the service:

- **Live:** subscribe to `stream_events` (`main.rs:2669-2726`). With the
  spectator token the subscriber's visibility resolves to `Spectator`
  (`main.rs:2677-2688`), so every `PlayerAction` / `StreetAdvanced` / `HandEnded`
  event carries a full-visibility `TableStatus` — **all hole cards, live**
  (`main.rs:1886-1904`, `2140-2148`). This is the real-time path.
- **Record:** poll `GetSessionInfo` + `ExportSession` for the accumulating
  `HandCollection` (the exact `ExploitPuller` loop, `main.rs:355-395`) — used for
  batch replay of a completed session and for cross-hand `StatsRegistry`
  aggregates.

```
crates/pkdealer_sentinel/
  src/
    lib.rs          public API: Sentinel::observe(event) -> Option<Verdict>
    tap.rs          gRPC StreamEvents + ExportSession client (spectator token)
    handview.rs     per-hand reconstruction from TableStatus/HandHistory
    signals/
      mod.rs        Signal trait: score(hand, pair) -> f64 in [0,1]
      softplay.rs   EV-sacrifice: known-cards optimal line vs. line actually taken
      chipflow.rs   directed net-chip transfer per ordered pair, pot-attributed
      squeeze.rs    isolation pattern: two team seats sandwich a lone caller
    detector.rs     sequential test: per-pair log-likelihood ratio accumulator
    verdict.rs      Verdict { pair, confidence, flagged_at_hand }
    score.rs        time-to-detection + false-positive over a labelled run
    bin/sentinel.rs clap entrypoint -> tap -> detector -> report
```

**Signal design (the heart of it).** Because the sentinel sees all cards, each
signal is an *EV/behaviour discrepancy given perfect information*, not a guess:

- **Soft-play (`softplay.rs`):** for every hand where seats `i` and `j` both
  reach a betting decision contesting the same pot, compute the aggression a
  profile-agnostic solver-lite line would take with `i`'s known cards vs. the
  action `i` actually took *when `j` is live in the pot*, and compare it to `i`'s
  aggression when only non-team seats are live. A colluder systematically
  under-bets/​under-raises into a partner. The per-hand score is the EV the
  bot *declined* to capture against the partner.
- **Chip-flow (`chipflow.rs`):** attribute each pot's net chips to ordered pairs
  and accumulate a directed transfer matrix. Honest play nets to ~zero between
  any pair over time (mirroring is not in play — EPIC-45); persistent one-way
  flow between two specific seats beyond a variance band is the dump signal.
  Reuse per-hand `ResultEntry.net` from the `HandCollection`
  (the EPIC-45 leaderboard uses the same field).
- **Squeeze (`squeeze.rs`):** count hands where two candidate seats raise/re-raise
  around a single third seat far above the base rate that position/holdings
  justify.

**Sequential verdict (`detector.rs`).** Each ordered pair carries a running
log-likelihood ratio: `LLR += log P(signal | collusion) − log P(signal |
honest)`, where the honest distribution is calibrated from an all-honest
control run (Phase 6). Flag the pair the first hand `LLR` crosses an upper
threshold (Wald SPRT bounds set from target false-positive/false-negative
rates). The **flag-hand index is the time-to-detection** — the headline metric.
This is a sequential test precisely because the question is "how few hands," not
"given the whole session."

**Why calibrated LLR, not a fixed heuristic threshold:** a nit folding marginal
hands and a colluder soft-playing look similar for a few hands; only the
*accumulated* discrepancy separates them, and the honest control gives us the
null distribution to measure that accumulation against. A raw threshold would
either flag tight honest players (false positive) or need so much evidence it
buries the time-to-detection result.

### 4. Detection metrics (Phase 5)

Detection counters/gauges slot into the existing OTel `Metrics` struct
(`main.rs:427-487`, constructed at `main.rs:445`) and export through the same
OTLP pipeline (collector → Prometheus/Jaeger/Grafana, `docs/EPIC-42...md:41-43`)
— e.g. `pkdealer.sentinel.pair_llr` (gauge per pair),
`pkdealer.sentinel.flag_hand` (histogram), `pkdealer.sentinel.false_positive`
(counter). The sentinel is a separate process, so it initializes its own
`init_otel` (`crates/pkdealer_service/src/otel.rs:105`) under its own
`OTEL_SERVICE_NAME`, honoring `OTEL_SDK_DISABLED=true` for tests.

---

## Work Items

### Phase 0 — Team configuration plumbing

- [ ] **0a.** Add optional `team` to the `arena.toml` schema doc header
  (`arena.toml:11-15`) and to two example colluder entries.
- [ ] **0b.** In `bin/arena`, read `team` via the existing `registry_field`
  (`bin/arena:52-64`) and emit `--team <id>` in the rules service block beside
  `--profile` (`bin/arena:232-234`). No-op when absent.
- [ ] **0c.** Test: `./bin/arena mallory trudy gto` generates a compose override
  where `mallory`/`trudy` carry `--team A` and `gto` carries none.

### Phase 1 — Colluding rules bot

- [ ] **1a.** Add `--collude-team` / `PKDEALER_COLLUDE_TEAM` to the rules bot
  `Args`, mirroring `--spectator-token`/`--exploit` (`agent_rules/src/main.rs:151-165`).
  A colluder requires a spectator token (reuse the same validation as `--exploit`
  requiring the puller, `main.rs:617-643`).
- [ ] **1b.** Extend the spectator refresh to expose the *current* hand's per-seat
  `Cards` (not only finished-hand aggregates) — read from the live `TableStatus`
  the puller already receives.
- [ ] **1c.** Add `ColludingDecider` wrapping `RuleBasedDecider`
  (`main.rs:308-311`) with `soft-play` / `chip-dump` / `squeeze` coordination;
  thread `partner_cards` through a `snapshot_with_stats`-style path
  (`main.rs:478-537`). Doc comment + doctest + unit test per `CLAUDE.md`.
- [ ] **1d.** Unit tests: colluder folds the *worse* of two team hands preflop to
  concentrate chips; colluder checks back a made hand vs. a live partner
  (soft-play) but bets it vs. a lone non-teammate (proves the coordination is
  partner-conditioned, not just passive).

### Phase 2 — `pkdealer_sentinel` skeleton + live tap

- [ ] **2a.** New crate in workspace `members` (`Cargo.toml`). `tap.rs`: connect,
  `StreamEvents` subscribe with spectator token (pattern from `main.rs:2669-2726`
  / `ExploitPuller` `main.rs:329-335`).
- [ ] **2b.** `handview.rs`: reconstruct a per-hand record (seats, hole cards,
  action sequence, pots, winners) from the `TableStatus` snapshots / `HandHistory`.
- [ ] **2c.** Smoke test: point at an `e2e`-style running service
  (`crates/pkdealer_service/tests/e2e_two_players.rs`) and assert the sentinel
  reconstructs the same hand count and winners the recorder did.

### Phase 3 — Collusion signals

- [ ] **3a.** `signals/softplay.rs` — EV-sacrifice score with unit tests on
  hand-crafted spots (known cards, partner-live vs. lone-opponent).
- [ ] **3b.** `signals/chipflow.rs` — directed net-transfer matrix from
  `ResultEntry.net`; test that honest zero-sum play nets ~0 per pair.
- [ ] **3c.** `signals/squeeze.rs` — isolation-pattern counter with a
  position-adjusted base rate; test.

### Phase 4 — Verdict & time-to-detection

- [ ] **4a.** `detector.rs` — per-pair SPRT log-likelihood accumulator; flag on
  upper-bound crossing; record `flagged_at_hand`.
- [ ] **4b.** `verdict.rs` + `score.rs` — emit per-pair `Verdict` and a run-level
  report: detected pairs, time-to-detection, missed pairs, false positives.
- [ ] **4c.** `bin/sentinel.rs` — clap entrypoint: connect, observe until N hands
  or session end, print the report.

### Phase 5 — Metrics

- [ ] **5a.** Add sentinel instruments to a sentinel-local `Metrics` and export via
  `init_otel` (`otel.rs:105`); honor `OTEL_SDK_DISABLED`.

### Phase 6 — Arena harness & honest baseline

- [ ] **6a.** Run the colluding arena: `./bin/arena mallory trudy gto lag tag`
  (mallory+trudy on team A) and pipe the session into the sentinel; produce the
  detection-confidence-vs-hands curve.
- [ ] **6b.** Run the **honest control** (identical lineup, no `team`) and measure
  the false-positive flag rate; use it to calibrate the SPRT null distribution
  and assert FP rate ≤ the stated bound.
- [ ] **6c.** Report a table of `(team archetype, aggression of collusion) →
  median hands-to-detection`.

---

## Test Plan

| Test | Asserts |
|---|---|
| `colluder_folds_worse_team_hand` | with two known team hands preflop, the weaker folds to concentrate equity |
| `colluder_softplays_partner_only` | made hand checked vs. live partner, bet vs. lone non-teammate (partner-conditioned) |
| `chipflow_honest_nets_zero` | over a synthetic honest `HandCollection`, every pair's directed net ≈ 0 within band |
| `chipflow_detects_dump` | a planted one-way transfer trips the flow signal |
| `softplay_ev_sacrifice_scores` | hand-crafted soft-play spot scores > honest fold of same cards |
| `sprt_flags_colluders` | on a labelled colluding corpus, both team pairs flagged; `flagged_at_hand` recorded |
| `sprt_honest_under_fp_bound` | on the honest control corpus, flag rate ≤ bound |
| `sentinel_reconstructs_hand` | reconstructed hands match the recorder's `HandCollection` |

---

## Key Files

| File | Role |
|---|---|
| `arena.toml` | + `team` field (Phase 0) |
| `bin/arena` | + `--team` passthrough (Phase 0) |
| `crates/pkdealer_agent_rules/src/main.rs` | + `--collude-team`, `ColludingDecider` (Phase 1) |
| `crates/pkdealer_sentinel/` | new crate — detector (Phases 2-5) |
| `crates/pkdealer_service/src/main.rs` | reference only — tap points, no change |
| `Cargo.toml` | + `pkdealer_sentinel` workspace member |
| `DEMO.md` | + colluding-arena + sentinel walkthrough |

---

## Reuse (do NOT recreate)

- **Spectator tap:** `ExploitPuller` (`agent_rules/src/main.rs:325-395`) — the
  complete connect / poll `GetSessionInfo` / `ExportSession` / parse
  `HandCollection` loop. The sentinel and the colluder both reuse this.
- **Live event stream:** `stream_events` (`main.rs:2669-2726`) with per-subscriber
  `filter_cards` — spectator token → full visibility.
- **Per-hand records with all cards:** `HandHistory` / `HandCollection`
  (`main.rs:2046-2098`), incl. `AgentFidelity` provenance (model id, coercions).
- **Opponent aggregates:** `pkcore::analysis::player_stats::StatsRegistry::ingest_collection`.
- **Decider + profiles:** `RuleBasedDecider` (`main.rs:308-311`), `BotProfile`
  (`load_profile`, `main.rs:585-608`), `EquityMode::Fast` (`main.rs:169-177`).
- **Snapshot threading precedent:** `snapshot_with_stats` (`main.rs:478-537`,
  commit `762b7d5`) — how `opponent_stats` reached the decider; partner cards
  follow the same seam.
- **OTel:** `Metrics` (`main.rs:427-487`), `init_otel` (`otel.rs:105`).
- **Deterministic harness:** `e2e_two_players.rs` style spawn-real-service test.

---

## Compatibility

- **pkcore untouched** — collusion and detection both ride existing gRPC surface;
  no engine change, no version bump.
- **Honest bots unchanged** — `team`/`--collude-team` absent ⇒ byte-identical
  behavior. `ColludingDecider` wraps, never replaces, `RuleBasedDecider`.
- **Existing arena/spectator/recorder tooling unaffected** — the sentinel is one
  more spectator-token client, exactly like `pkspectator`.

---

## Dependencies

- **Built on:** EPIC-25 (Arena Recorder / `ExportSession`), EPIC-42 (Dynamic
  Arena Runner / `arena.toml` + `bin/arena`), commit `762b7d5` (spectator-token
  `ExploitPuller`).
- **Related:** EPIC-45 (Bot-Evaluation Format — shares the `HandCollection` /
  `ResultEntry.net` scoring substrate; a headless mirror-mode collusion run could
  later live there). pkcore EPIC-79 (Mental Poker) is the *fix* for the hole this
  EPIC exploits — explicitly out of scope here.
- **Not blocked by, but limited by:** EPIC-41 (Reproducible Scenarios) is
  unstarted, so there is no seeded replay; time-to-detection is reported over
  hands dealt in live/recorded sessions, not over a reproducible seed.

---

## Verification

```bash
# Colluding arena (mallory + trudy secretly on team A) vs three honest bots
./bin/arena mallory trudy gto lag tag
# In another shell, watch them get caught:
cargo run -p pkdealer_sentinel --bin sentinel -- \
  --endpoint http://127.0.0.1:50051 --spectator-token spectator --hands 500

# Honest control — same lineup, no teams — must stay under the FP bound
./bin/arena tag lag gto tp lp
cargo run -p pkdealer_sentinel --bin sentinel -- --hands 500

# Unit + doc tests, workspace build, clippy clean
OTEL_SDK_DISABLED=true cargo test -p pkdealer_sentinel -p pkdealer_agent_rules
cargo test --doc -p pkdealer_sentinel
OTEL_SDK_DISABLED=true cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Exit criteria:

1. `./bin/arena mallory trudy gto` emits a compose override where the two
   colluders carry `--team A` and honest seats carry no team flag.
2. Colluder unit tests prove partner-conditioned soft-play and equity-concentrating
   folds (Phase 1 tests green).
3. The sentinel flags **both** planted colluders on the colluding run and reports
   a `flagged_at_hand` for each.
4. On the honest control run the false-positive flag rate is ≤ the stated bound
   (SPRT calibrated from the same control).
5. A report table of `(archetype, collusion aggression) → median hands-to-detection`
   is produced.
6. `OTEL_SDK_DISABLED=true cargo test --workspace` passes; clippy clean; every new
   public fn/struct has doc comment + doctest + unit test per `CLAUDE.md`;
   `cargo test --doc -p pkdealer_sentinel` passes.
</content>
</invoke>
