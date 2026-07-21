# EPIC-46: Collusion Simulation & Detection (COLLUDE)

Two players who share what's in their hands stop playing poker and start
robbing the table. This EPIC builds both halves of that story as a domain kata:
**colluders** who cheat by sharing hole cards, and a **Boss** who must catch them
knowing only what any honest observer could know. The Boss never sees a hole
card. Its only evidence is the shape of the betting. The question the simulation
answers is: *how many hands until the Boss can say "seats 3 and 5 are working
together" — and how often does it wrongly accuse the innocent?*

---

## Context

The `sbot` branch shipped the machinery this EPIC stands on. `pkdealer_agent_rules`
already opens a **second** service connection with the spectator token, pulls
completed-hand history via `ExportSession`, rebuilds a pkcore `StatsRegistry`, and
feeds real opponent stats into its decider — the `ExploitPuller`
(`crates/pkdealer_agent_rules/src/main.rs:325`), its watermark-throttled
`refresh()` (`crates/pkdealer_agent_rules/src/main.rs:355`), and the
`snapshot_with_stats` threading (`crates/pkdealer_agent_rules/src/main.rs:478`).
That same spectator path is, unmodified, a card leak: a spectator-token stream
sees **every** seat's hole cards live.

The information-gating that makes this possible is all in the service:
`CardVisibility` (`crates/pkdealer_service/src/main.rs:305`), `filter_cards`
(`crates/pkdealer_service/src/main.rs:655`), token → visibility resolution
(`crates/pkdealer_service/src/main.rs:1037`), and the spectator secret
(`crates/pkdealer_service/src/main.rs:104`). `ExportSession` is gated to the
spectator token precisely *because* its payload carries all hole cards
(`crates/pkdealer_service/src/main.rs:2404`).

The detection substrate already exists too. pkcore 0.3.1 ships `PlayerStats`
(`player_stats.rs:55`) and `StatsRegistry` (`player_stats.rs:265`) with
`vpip()`/`pfr()`/`aggression_factor()`/`confidence()` — every one derivable from
**public** actions, no hole cards required — plus `ingest_collection`
(`player_stats.rs:342`) and a sample-size `Confidence` band
(`player_stats.rs:220,233`). And the EPIC-25 recorder captures, per hand, stable
player UUIDs, hole cards, per-street actions with amounts, stacks, board, and the
shuffled deck (`HandHistory` `hand_history.rs:128`, `PlayerEntry.player_id`
`hand_history.rs:1468`, `PlayerEntry.hole_cards` `hand_history.rs:1477`, `Action`
`hand_history.rs:2286`; pushed at `crates/pkdealer_service/src/main.rs:2098`) —
enough to *label* a session's ground truth after the fact.

**What does not exist today:** any bot-to-bot channel (verified by grep — nothing
under `crates/`, `bin/`, `docs/` shares cards between agent processes), any
collusion strategy, any detector, and any redacted view type. The only prior
mention of cheating anywhere in the repo is two *prevention* notes in
`docs/notes/POSSIBLE_ARCHITECTURES.md:15,121` (fog-of-war), not detection.

**This EPIC does NOT:**
- Change the dealer service. Vector A rides the existing spectator token; Vector B
  runs entirely outside the service. The proto is untouched. The service stays honest.
- Implement EPIC-45's in-process reproducible arena. The sim runs on today's
  gRPC/`bin/arena` substrate. EPIC-45's deterministic decks would sharpen the
  benchmark later (see Dependencies), but are not a prerequisite.
- Cover LLM-agent collusion. The colluders are **rules agents** only — cheating
  must be a deterministic, testable strategy, not a prompt.

---

## Status

| Component | Status |
|---|---|
| `GroundTruthLabels` — session metadata (who colludes, which vector, which style) | Planned |
| `RedactedHand` / `RedactedHandView` — typed hole-card firewall | Planned |
| `CollusionConfig` + CLI/env on `pkdealer_agent_rules` | Planned |
| Vector A — `SpectatorLeak` partner-card puller | Planned |
| Collusion strategies — soft-play / whipsaw / chip-dump | Planned |
| Vector B — `Backchannel` peer card-sharing | Planned |
| `pkdealer_boss` offline analyzer (pairwise metrics + `SuspicionScore`) | Planned |
| Ground-truth scorer — hands-to-detection + false-positive rate | Planned |
| Live boss binary (`pkdealer_agent_boss`) + OTel flagging | Planned |
| Calibration report — threshold sweep + honest-lineup FP study | Planned |

---

## Goals

- Make **cheating** a first-class, configurable, deterministic behavior: a rules
  agent can be told to **collude** with a named partner via a chosen **channel**
  and a chosen **style**, and the collusion must produce a *measurable* chip edge.
- Model **two distinct cheating vectors** that differ only in *how* cards leak, not
  in the resulting table behavior: the **spectator leak** (Vector A) and the
  **peer backchannel** (Vector B). A good detector should flag both identically.
- Build a **Boss** that classifies colluding pairs from **public information
  alone**, proven by a **typed firewall** — the detection API cannot receive a
  hole card, structurally.
- Measure the Boss on the two numbers that matter: **hands-to-detection** (speed)
  and **false-positive rate** against honest control lineups (precision).
- Do it **test-first**: every collusion style and every detection metric is driven
  out against synthetic hand fixtures before it runs in a live sim.

## Scope

- Colluders are **pairs** in this EPIC (the smallest cheating unit). N-way rings
  are out of scope; the metrics are defined pairwise so a ring falls out as
  multiple flagged pairs, but the EPIC does not build ring-specific scoring.
- Both channels share **identical** downstream behavior: once a colluder knows its
  partner's cards, the decision adjustment is the same regardless of how the cards
  arrived. The channel is an information source, not a strategy.
- The Boss sees only what `RedactedHand` carries: seats, player UUIDs, positions,
  per-street public actions with amounts, blinds, board, and chip deltas. **No
  hole cards, no deck.**
- The **ground-truth scorer** is a *separate* module that MAY read hole cards
  (to label who actually had the goods) — it exists to grade the Boss, and its
  output never flows back into the Boss's inputs.
- Honesty of the simulation: Vector A colluders technically *can* see all cards
  but are constrained by construction to use only their partner's — this
  "honor filter" is a documented simulation constraint (a real cheat has no such
  filter; the sim models the *behavior*, not the temptation). The honor filter is
  **load-bearing for the A/B equivalence**: Vector A's information position (sees
  everyone) collapses to Vector B's (sees only the partner) *only* when non-partner
  cards are discarded at ingest. Exit criterion 4 ("catches the behavior, not the
  channel") therefore holds only if the filter is enforced — if A leaked
  third-party cards into its decisions, it would play a strictly stronger, and
  differently-detectable, game than B.

---

## Domain map

| Domain concept | Code construct | Status |
|---|---|---|
| A cheating pair sharing hole cards | `CollusionConfig` (rules agent) | ❌ absent |
| Card leak via insider secret | `SpectatorLeak` (Vector A) | ❌ absent |
| Card leak via peer side-channel | `Backchannel` (Vector B) | ❌ absent |
| Ways two colluders exploit shared cards | `CollusionStyle` (soft / whipsaw / dump) | ❌ absent |
| What an honest observer may know | `RedactedHand` / `RedactedHandView` | ❌ absent |
| The cheat-catcher | `pkdealer_boss` crate + live `pkdealer_agent_boss` | ❌ absent |
| Per-pair suspicion from public play | `SuspicionScore` + pairwise metrics | ❌ absent |
| Truth of who colluded | `GroundTruthLabels` + scorer | ❌ absent |
| Public per-player behavior | `pkcore::StatsRegistry` / `PlayerStats` | ✅ done (reuse) |
| Completed-hand record | `pkcore::HandCollection` / `HandHistory` | ✅ done (reuse) |
| Card visibility gating | `CardVisibility` / `filter_cards` (service) | ✅ done (reuse) |

---

## Design

### `RedactedHand` / `RedactedHandView` — the typed firewall

`crates/pkdealer_boss/src/redacted.rs` (new):

```rust
/// A single completed hand as an honest observer may see it: public actions and
/// chip movements, with every hole card and the deck structurally removed.
///
/// There is no field that can hold a hole card. The Boss's detection API accepts
/// only `&[RedactedHand]`, so "the Boss cannot peek" is enforced by the type
/// system, not by discipline.
pub struct RedactedHand {
    pub hand_no: u32,
    pub button_seat: u8,
    pub seats: Vec<RedactedSeat>,           // player_id, seat, starting_stack, ending_stack
    pub actions: Vec<RedactedAction>,       // seat, player_id, street, ActionType, amount, all_in
    pub board: Option<String>,              // community cards ARE public
    pub big_blind: u32,
}

pub struct RedactedSeat { pub player_id: Uuid, pub seat: u8, pub starting_stack: u32, pub ending_stack: u32 }
pub struct RedactedAction { pub player_id: Uuid, pub seat: u8, pub street: Street, pub action: ActionType, pub amount: u32, pub all_in: bool }

/// The ONLY constructor. Consumes a `HandCollection`, drops `hole_cards` and
/// `shuffled_deck` at the boundary. Once redacted, the cards are gone.
pub fn redact(collection: &HandCollection) -> Vec<RedactedHand>;
```

Rationale: the plan offered "convention only" (Boss reads the full
`HandCollection`, just doesn't touch `hole_cards`). We reject it. The whole *point*
of the EPIC is a credible claim that the detector works blind; a claim that rests
on a reviewer trusting that no code path reads a field is not credible. `redact`
is the single choke point, unit-tested to prove the output contains no card data,
and the Boss depends on `RedactedHand` — never on `HandCollection`. Board cards
are public (they're dealt face-up) and are retained.

### `CollusionConfig` + CLI on `pkdealer_agent_rules`

Extends the existing decision-override flags (`apply_decision_overrides`
`crates/pkdealer_agent_rules/src/main.rs:232`):

```rust
struct CollusionConfig {
    partner: String,          // --collude-with <name>: partner's agent name
    channel: CollusionChannel,// --collusion-channel spectator|peer
    style: CollusionStyle,    // --collusion-style soft|whipsaw|dump
}
enum CollusionChannel { Spectator, Peer }
enum CollusionStyle   { SoftPlay, Whipsaw, ChipDump }
```

Partner cards reach the decider by extending the existing snapshot path
(`snapshot_with_stats` `crates/pkdealer_agent_rules/src/main.rs:478`) with an
optional `partner_hole: Option<Cards>`; the collusion wrapper reads it. When no
collusion is configured the field is `None` and behavior is byte-for-byte the
current agent. Cheating is strictly additive and off by default.

### Vector A — `SpectatorLeak`

`crates/pkdealer_agent_rules/src/collude/spectator.rs` (new). It borrows only the
**connection plumbing** of `ExploitPuller` (`crates/pkdealer_agent_rules/src/main.rs:325`)
— a dedicated second client with the spectator token injected into request metadata
(`crates/pkdealer_agent_rules/src/main.rs:367,617`) — **not** its refresh cadence.
Where `ExploitPuller` throttles on completed hands, `SpectatorLeak` reads *live*
card state **on every decision** (`GetStatus`/`StreamEvents`, which return all hole
cards under the spectator token — the gating at `crates/pkdealer_service/src/main.rs:597`
grants `Spectator` every seat's cards), because the partner's *current-hand* cards
are the whole point. It extracts **only the partner seat's** hole cards and discards
the rest at ingest (the honor filter). Requires `--spectator-token`, exactly as the
exploit path does today (`crates/pkdealer_agent_rules/src/main.rs:165`).

Rationale: reuse over recreation. The second-connection + token-injection pattern
is already proven on this branch; Vector A is that pattern pointed at a partner
seat instead of at a `StatsRegistry`.

### Vector B — `Backchannel`

`crates/pkdealer_agent_core/src/backchannel.rs` (new, feature-gated
`collusion`). The smallest thing that works: a local line-delimited JSON channel
between colluding processes, brokered by neither the dealer nor a spectator token.

```rust
// One line per share, over a local TCP or Unix socket the colluders agree on.
struct CardShare { hand_no: u32, seat: u8, player_id: Uuid, hole_cards: String }

trait Backchannel {
    async fn publish(&self, share: CardShare);          // "here are my cards this hand"
    async fn partner_cards(&self, hand_no: u32) -> Option<CardShare>;
}
```

Each colluder publishes its own cards each hand and reads its partner's. Addressed
by env (`PKDEALER_BACKCHANNEL=127.0.0.1:PORT`). Explicitly **not** the dealer
service — the service never learns cards are shared. This is the "realistic"
vector: partner-only information, no privileged token.

Rationale: Vectors A and B must be *behaviorally indistinguishable* at the table
so the Boss can be shown to catch collusion, not catch a token. Keeping the
backchannel dead-simple (publish/subscribe cards, nothing else) keeps the strategy
code identical across both vectors — only the source differs.

### `CollusionStyle` — the strategies

`crates/pkdealer_agent_rules/src/collude/strategy.rs` (new). A wrapper around the
existing `RuleBasedDecider` that, given `partner_hole`, adjusts the chosen action:

- **SoftPlay** — never raise into a pot where the partner is the only other live
  player; check/call down instead. Denies the house the rake of two friends
  betting each other. Detectable as anomalously low aggression *between the pair*.
- **Whipsaw** — when the partner has raised and a third party sits between them,
  re-raise to squeeze the victim out of pots the pair will contest. Detectable as
  coordinated raise→re-raise sequences that fold the field.
- **ChipDump** — fold strong hands *to the partner* when the partner is committed
  with a stronger holding (or is short and needs the chips). Detectable as chip
  flow that runs one direction between two specific players.

Each style is a pure function of `(base_decision, snapshot, partner_hole)` and is
unit-tested against fixtures asserting the *behavioral* signature it must leave.

### `pkdealer_boss` — offline analyzer

New crate, modeled on `pkdealer_costsim` (`crates/pkdealer_costsim/src/{lib,app,report,main}.rs`):
a binary that reads exported sessions and emits a report. Its detection library
takes **only** `&[RedactedHand]` and computes, per unordered seat-pair:

- **Chip-flow asymmetry** — directional chips moved between the pair (catches
  ChipDump). Attribution is only clean when a pot is contested **down to the pair**
  (heads-up between them at showdown/fold); in multiway pots stack deltas can't
  cleanly assign A's loss to B, so this metric is computed **per pot at
  resolution**, restricted to pots the pair contests alone, rather than from raw
  session stack deltas.
- **Soft-play index** — aggression frequency of A when heads-up with B, vs A's
  baseline aggression against everyone (catches SoftPlay). Reuses `PlayerStats`
  aggression accessors (`player_stats.rs:163`).
- **Whipsaw count** — frequency of raise→re-raise-by-partner→field-folds patterns
  (catches Whipsaw).
- **Mutual VPIP/PFR conditioning** — how each player's `vpip()`/`pfr()`
  (`player_stats.rs:119,125`) shifts when the partner is in the pot.
- **Combined win-rate lift** — the pair's pooled chip result vs the **collusion-off
  baseline**: the same two agents, same lineup and seats, run with collusion
  disabled (the Phase 5 calibration captures this control run). Lift = pooled
  bb/100 with collusion minus pooled bb/100 in the control.

These fold into a per-pair `SuspicionScore` carrying a `pkcore::Confidence`
(`player_stats.rs:220`) driven by sample size — the Boss says nothing with
conviction after five hands. The score is a weighted composite; weights are
calibrated in Phase 5.

The **ground-truth scorer** is a sibling module (`pkdealer_boss/src/scorer.rs`)
that reads `GroundTruthLabels` and the full (un-redacted) `HandCollection`, and
grades a Boss run: for each labeled colluding pair, the **hands-to-detection**
(first hand at which suspicion crossed threshold with ≥ Medium confidence) and the
**false-positive rate** (honest pairs flagged). It is the only code in the crate
allowed to touch hole cards, and it is import-isolated from the detection library.

### `GroundTruthLabels`

`crates/pkdealer_boss/src/labels.rs` (new). Session-level metadata written by the
sim harness: `{ colluding_pairs: [(name, name)], vector: Spectator|Peer, style }`.
Serialized alongside the exported session (a sidecar YAML, since the proto is
untouched). Consumed only by the scorer.

### Live boss binary (later phase)

`crates/pkdealer_agent_boss/` (new binary). An observer process that polls
`ExportSession` on the exploit-puller cadence (watermark throttle reuse,
`crates/pkdealer_agent_rules/src/main.rs:355`), **redacts at ingest**, maintains
rolling per-pair suspicion, and flags via structured log + an OTel gauge
`pkdealer.boss.suspicion{pair}`. It sits in the arena like any other agent but
never takes a seat.

**Trust boundary (be honest about it):** because `ExportSession` is spectator-gated
(`crates/pkdealer_service/src/main.rs:2404`), the *live* Boss must hold the **same
spectator token as a Vector-A cheater**, and its process momentarily holds the
un-redacted `HandCollection` (cards included) in the window between receiving the
export and calling `redact()`. The typed firewall guarantees the *detection library*
never receives a card — **not** that the process never touches card bytes. The
**offline analyzer sidesteps this entirely**: it reads an already-exported session
file, needs no token, and can run `redact()` before any detection code is linked in.
Prefer the offline path for anything that must be provably blind.

---

## Work Items

### Phase 0 — Firewall & labels (prerequisites)

- [ ] **0a.** Create `crates/pkdealer_boss` crate skeleton (mirror
  `pkdealer_costsim`); add to workspace `Cargo.toml` members.
- [ ] **0b.** Implement `RedactedHand` + `redact()` in
  `crates/pkdealer_boss/src/redacted.rs`, consuming `HandCollection`.
- [ ] **0c.** Implement `GroundTruthLabels` + sidecar YAML (de)serialization.
- [ ] **0d.** Feature-gate: add a `collusion` feature to `pkdealer_agent_rules`
  and `pkdealer_agent_core`; confirm `cargo check` is green with and without it.

### Phase 1 — Vector A colluders + strategies

- [ ] **1a.** `CollusionConfig` + CLI/env flags on `pkdealer_agent_rules`
  (`--collude-with`, `--collusion-channel`, `--collusion-style`).
- [ ] **1b.** `SpectatorLeak` puller (partner-card extraction, honor filter),
  cloned from `ExploitPuller` (`crates/pkdealer_agent_rules/src/main.rs:325`).
- [ ] **1c.** `CollusionStyle` strategies (soft/whipsaw/dump) as decider wrappers.
- [ ] **1d.** `arena.toml` entries for a colluding lineup; thread the new flags
  through `emit_service` (`bin/arena:234`).
- [ ] **1e.** Sim check: colluding pair's combined chips beat an honest control
  over N hands (the cheat must actually pay).

### Phase 2 — The Boss (offline) — *heart of the EPIC*

- [ ] **2a.** Pairwise metrics over `&[RedactedHand]` (chip-flow, soft-play index,
  whipsaw count, conditioned VPIP/PFR, win-rate lift).
- [ ] **2b.** `SuspicionScore` composite + `Confidence` gating.
- [ ] **2c.** Ground-truth scorer: hands-to-detection + FP rate.
- [ ] **2d.** `pkdealer_boss` binary: read exported session + labels → report.

### Phase 3 — Vector B backchannel

- [ ] **3a.** `Backchannel` (local JSON socket) in `pkdealer_agent_core`.
- [ ] **3b.** Wire `--collusion-channel peer` to publish/subscribe partner cards.
- [ ] **3c.** Re-run the Boss against a Vector-B session; assert the detection
  signature matches Vector A (same behavior, different channel).

### Phase 4 — Live boss

- [ ] **4a.** `pkdealer_agent_boss` binary: `ExportSession` polling + redact-at-ingest.
- [ ] **4b.** OTel `pkdealer.boss.suspicion` gauge + structured flag log.
- [ ] **4c.** Arena wiring: a `boss` type in `emit_service` (`bin/arena:218`).

### Phase 5 — Calibration & report

- [ ] **5a.** Threshold sweep: tune suspicion weights/cutoff for best speed vs FP.
- [ ] **5b.** Honest-lineup FP study (no colluders present → zero flags).
- [ ] **5c.** DEVLOG close-out section `## EPIC-46 — Collusion Detection (YYYY-MM-DD)`.

---

## Test Plan

- `redact_drops_hole_cards` — `redact()` output serialized contains no card
  strings and no deck; a property test over recorded fixtures.
- `redacted_hand_has_no_card_field` — compile-time/structural: the detection API
  signature takes `&[RedactedHand]`; a doc test shows a `HandCollection` cannot be
  passed.
- `soft_play_never_raises_partner_heads_up` — fixture where hero holds a raising
  hand heads-up vs partner → action is check/call, not raise.
- `whipsaw_squeezes_third_party` — three-handed fixture, partner raises → hero
  re-raises the victim.
- `chip_dump_folds_strong_to_partner` — hero holds a strong hand, partner
  committed stronger → hero folds.
- `metric_chip_flow_flags_dump` / `metric_soft_play_index_flags_soft` /
  `metric_whipsaw_count_flags_whipsaw` — each metric on a synthetic session with a
  known planted signature scores the guilty pair above honest pairs.
- `suspicion_confidence_low_on_small_sample` — < ~50 hands → `Confidence` below
  the flagging band regardless of signal.
- `scorer_reports_hands_to_detection` — labeled colluding session → scorer returns
  a finite hands-to-detection for the true pair.
- `boss_zero_fp_on_honest_control` — an all-honest session → no pair flagged.
- `vector_a_and_b_same_signature` — the same style over both channels yields
  detection scores within tolerance (Phase 3).

## Key Files

| File | Role |
|---|---|
| `crates/pkdealer_boss/src/redacted.rs` | `RedactedHand` + `redact()` firewall |
| `crates/pkdealer_boss/src/{metrics,score}.rs` | pairwise metrics + `SuspicionScore` |
| `crates/pkdealer_boss/src/scorer.rs` | ground-truth grading (may read hole cards) |
| `crates/pkdealer_boss/src/labels.rs` | `GroundTruthLabels` sidecar |
| `crates/pkdealer_agent_rules/src/collude/` | `CollusionConfig`, `SpectatorLeak`, strategies |
| `crates/pkdealer_agent_core/src/backchannel.rs` | Vector B peer channel |
| `crates/pkdealer_agent_boss/` | live observer binary (Phase 4) |
| `arena.toml`, `bin/arena` | colluding + boss lineups |

## Reuse (do NOT recreate)

- `crates/pkdealer_agent_rules/src/main.rs:367,617` — `ExploitPuller`'s
  **second-connection + spectator-token-into-metadata** plumbing: this is the part
  Vector A copies (it reads *live* state per decision, so it does NOT reuse the
  refresh cadence). The **watermark throttle** (`crates/pkdealer_agent_rules/src/main.rs:355`),
  which re-pulls only on a completed hand, is instead reused by the *live Boss*,
  whose `ExportSession` polling is completed-hand-driven.
- `crates/pkdealer_agent_rules/src/main.rs:478` — `snapshot_with_stats`: the snapshot
  threading point to extend with `partner_hole`.
- `player_stats.rs:55,163,265,342` — `PlayerStats` / `StatsRegistry` /
  `aggression_factor` / `ingest_collection`: the public-stat engine the Boss reuses.
- `player_stats.rs:220,233` — `Confidence` / `from_sample_size`: sample-size gating.
- `hand_history.rs:128,1468,1477,2286` — `HandHistory` fields `redact()` consumes.
- `crates/pkdealer_costsim/src/{lib,app,report}.rs` — the offline-analyzer crate
  shape `pkdealer_boss` mirrors.
- `crates/pkdealer_service/src/main.rs:655,2404` — `filter_cards` / ExportSession
  gating: the visibility contract the sim depends on (read-only; unchanged).

## Compatibility

- **Preserves** the dealer service, proto, and default agent behavior — collusion
  is behind a `collusion` feature and off unless flags are set. Vector A uses the
  existing spectator token; Vector B never touches the service.
- **Adds** the `pkdealer_boss` + `pkdealer_agent_boss` crates, collusion flags on
  the rules agent, a peer backchannel in agent-core, and a labels sidecar.
- **Breaks** nothing. No existing test should change behavior; new behavior only
  appears under new flags/features.

## Dependencies

- **Built on:** EPIC-23 (bot agents + `PokerAgent`/runner), EPIC-25 (recorder +
  `ExportSession`), and the `sbot` exploit wiring (`ExploitPuller`, spectator-token
  puller pattern).
- **Related:** EPIC-45 (Bot Evaluation Format) — its planned in-process,
  deterministic-deck arena would let the Boss be benchmarked under variance
  control (same deck, colluding vs honest). Not required; the gRPC path suffices
  now. When EPIC-45 lands, a Phase 6 could re-run the FP/speed study on mirrored decks.
- **Blocks:** nothing yet.

## Verification

```bash
cargo build --workspace --features collusion
cargo clippy --workspace --features collusion -- -D warnings
cargo test -p pkdealer_boss --all-features
cargo test -p pkdealer_agent_rules --features collusion
cargo test --doc -p pkdealer_boss

# End-to-end sim: 2 colluders + 4 honest, run N hands, grade the Boss.
# Colluding seats are declared as arena.toml players carrying the collusion flags
# (e.g. `cheat_a`/`cheat_b` with --collude-with each other); the lineup just names them.
./bin/arena cheat_a cheat_b honest honest honest honest   # names resolve via arena.toml
PKDEALER_RECORD_DIR=./out docker compose ... up           # capture the session to disk
cargo run -p pkdealer_boss -- --session ./out/session-*.yaml --labels ./out/labels.yaml
```

Exit criteria:

1. A configured colluding pair's combined chip result **beats** an honest control
   lineup over a fixed hand count (the cheat pays).
2. The Boss, reading **only** `RedactedHand`, flags the true colluding pair with a
   finite, reported **hands-to-detection**.
3. On an all-honest control session the Boss flags **no** pair (false-positive rate
   zero at the calibrated threshold).
4. Vectors A and B produce detection scores within tolerance of each other — the
   Boss catches the *behavior*, not the channel.
5. `redact()` provably emits no hole cards or deck (test `redact_drops_hole_cards`),
   and the detection API cannot accept a `HandCollection`.
6. No **existing** crate's tests change behavior: the pre-46 crates' test results
   (run without the `collusion` feature) are identical to HEAD. New behavior appears
   only in the added `pkdealer_boss`/`pkdealer_agent_boss` crates and behind the
   `collusion` feature. (Note: `cargo test --workspace` itself now *runs more* — the
   new crates — so compare per-crate, not by raw workspace pass count.)
