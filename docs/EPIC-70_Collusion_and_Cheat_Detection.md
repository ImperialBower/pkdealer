# EPIC-70: Collusion Simulation & Cheat Detection (COLLUDE)

Two players who share what's in their hands stop playing poker and start
robbing the table. This EPIC builds both halves of that story as a domain kata:
**colluders** who cheat by sharing hole cards, and a **Boss** who must catch them
knowing only what any honest observer could know. The Boss never sees a hole
card. Its only evidence is the shape of the betting. The question the simulation
answers is: *how many hands until the Boss can say "seats 3 and 5 are working
together" — and how often does it wrongly accuse the innocent?*

---

## Consolidation note (2026-07-22)

This document consolidates two overlapping epics authored in parallel:

- **pkdealer `EPIC-46_Collusion_Detection.md`** (committed `4752a73`, `dea36c9`)
  — the deeper design: dual leak vectors, the typed `RedactedHand` firewall,
  UUID identity plumbing, three collusion styles, six phases. Its number is
  **retired**: the cross-repo registry allocates EPIC-46–49 to `pkarena0-web`
  (`pkcore/ROADMAP.md:411` — the 40-block is full), so pkdealer keeping 46 would
  be the family's third numbering collision.
- **`EPIC-70_Collusion_and_Cheat_Detection.md` ("sentinel")** (committed
  `3cbccf4`) — the registry-correct number (`pkcore/ROADMAP.md:414`, block
  claimed 2026-07-20). Absorbed from it: the `team` field in `arena.toml`
  (Phase 0 config surface), the **SPRT sequential detector** (per-pair
  log-likelihood ratio with Wald bounds — replacing the weighted
  `SuspicionScore` composite), the card-aware **EV-sacrifice oracle signal**
  (repositioned into the ground-truth scorer tier, where hole-card access is
  legal), the OTel instrument set, and the sharpened non-goals (pkcore
  untouched; the *fix* is pkcore EPIC-79 Mental Poker).

Dropped in the merge: EPIC-70's `pkdealer_sentinel` naming (the detector persona
here is the **Boss**, per the pkdealer backlog and commit history; "sentinel"
survives only as this note's alias) and its omniscient-detector framing — the
headline detector is **blind by construction**; perfect-information detection
lives only in the oracle/grading tier. EPIC-70's "squeeze" strategy is EPIC-46's
"whipsaw" under another name; **whipsaw** is kept.

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

The information-gating that makes this possible is all in the service — it is
**server-authoritative and redacts hole cards per subscriber**:
`card_visibility_from_metadata` resolves the `x-player-token` gRPC metadata to
`Hidden` / `Player(seat)` / `Spectator`
(`crates/pkdealer_service/src/main.rs:1037-1056`), and every outbound
`TableStatus` passes through `filter_cards` before it leaves the process
(`main.rs:655-672`; applied per subscriber in `stream_events` at
`main.rs:2697-2699`). A normal bot only ever sees its own two cards: the runner
lifts `s.cards` for `s.seat_number == my_seat` into `HandState.hole_cards`
(`crates/pkdealer_agent_core/src/runner.rs:274-280`), and `SeatSnapshot`
(`crates/pkdealer_agent_core/src/hand_state.rs:27-41`) carries no cards. This is
the intended fog-of-war posture noted in
`docs/notes/POSSIBLE_ARCHITECTURES.md:121`. The one over-privileged read is the
spectator token (`PKDEALER_SPECTATOR_TOKEN`, default `"spectator"`,
`main.rs:103-104`); `ExportSession` is gated to it precisely *because* its
payload carries all hole cards (`main.rs:2404`).

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
- Modify `pkcore` (external crate `0.3.1`). No engine change is required —
  collusion rides the existing spectator token, detection rides the existing
  export/broadcast RPCs.
- **Fix the vulnerability.** Closing the spectator-token hole (per-agent scoped
  tokens; mental-poker crypto per **pkcore EPIC-79**) is explicitly out of scope.
  The point is to *measure detectability*, not to prevent collusion.
- Implement EPIC-45's in-process reproducible arena. The sim runs on today's
  gRPC/`bin/arena` substrate. EPIC-41 (Reproducible Scenarios) has not started,
  so there is no seeded replay: time-to-detection is reported over hands dealt in
  live/recorded sessions, not over a reproducible corpus. EPIC-45's deterministic
  decks would sharpen the benchmark later (see Dependencies), but are not a
  prerequisite.
- Cover LLM-agent collusion. The colluders are **rules agents** only — cheating
  must be a deterministic, testable strategy, not a prompt.

---

## Status

| Component | Status |
|---|---|
| `arena.toml` `team` field + `bin/arena` expansion into collusion flags | Planned |
| Identity + hand-sequence plumbing (`hand_no` on `HandState`, name→`Uuid` resolution) | Planned |
| `GroundTruthLabels` — UUID-keyed session metadata (who colludes, which vector, style) | Planned |
| `RedactedHand` / `RedactedHandView` — typed hole-card firewall | Planned |
| `CollusionConfig` + CLI/env on `pkdealer_agent_rules` | Planned |
| Vector A — `SpectatorLeak` partner-card puller | Planned |
| Collusion strategies — soft-play / whipsaw / chip-dump | Planned |
| Vector B — `Backchannel` peer card-sharing + broker service | Planned |
| `pkdealer_boss` offline analyzer (pairwise metrics + SPRT verdict) | Planned |
| Ground-truth scorer — hands-to-detection + FP rate + EV-sacrifice oracle | Planned |
| Live boss binary (`pkdealer_agent_boss`) + OTel instruments | Planned |
| Calibration report — SPRT thresholds + honest-lineup FP study | Planned |

> **Not yet started.** Design only, consolidated 2026-07-22 from the two source
> epics (see Consolidation note). Citations grounded on the `sbot` branch
> (`ae8c145`) and commit `b999673`.

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
- Decide **sequentially**: per-pair evidence accumulates as a log-likelihood
  ratio (Wald **SPRT**), so the headline metric is the **hand index at which
  confidence first crossed the flag threshold** — *how few hands*, not "given
  the whole session."
- Measure the Boss on the two numbers that matter: **hands-to-detection** (speed)
  and **false-positive rate** against honest control lineups (precision) — and
  bound what is *achievable* with a card-aware **oracle** in the grading tier.
- Do it **test-first**: every collusion style and every detection metric is driven
  out against synthetic hand fixtures before it runs in a live sim.

## Scope

- Colluders are **pairs** in this EPIC (the smallest cheating unit). The
  `arena.toml` `team` field admits larger rings syntactically, but N-way scoring
  is out of scope; the metrics are defined pairwise so a ring falls out as
  multiple flagged pairs.
- Both channels share **identical** downstream behavior: once a colluder knows its
  partner's cards, the decision adjustment is the same regardless of how the cards
  arrived. The channel is an information source, not a strategy.
- Colluders never send illegal actions — the runner's legality clamp
  (`finalize_decision`, `crates/pkdealer_agent_core/src/runner.rs:492-509`) is
  unchanged. Cheating lives in *choice*, not protocol violation.
- The Boss sees only what `RedactedHand` carries: seats, player UUIDs, positions,
  per-street public actions with amounts, blinds, board, and chip deltas. **No
  hole cards, no deck.**
- The **ground-truth scorer** is a *separate* module that MAY read hole cards
  (to label who actually had the goods, and to run the EV-sacrifice oracle) — it
  exists to grade the Boss, and its output never flows back into the Boss's inputs.
- **Honest control:** every detection claim is calibrated against an all-honest
  lineup; the false-positive flag rate must stay under a stated bound.
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
| A collusion **team** (config surface) | `team` id in `arena.toml` + `bin/arena` expansion | ❌ absent |
| A cheating pair sharing hole cards | `CollusionConfig` (rules agent) | ❌ absent |
| Card leak via insider secret | `SpectatorLeak` (Vector A) | ❌ absent |
| Card leak via peer side-channel | `Backchannel` (Vector B) | ❌ absent |
| Ways two colluders exploit shared cards | `CollusionStyle` (soft / whipsaw / dump) | ❌ absent |
| What an honest observer may know | `RedactedHand` / `RedactedHandView` | ❌ absent |
| The cheat-catcher | `pkdealer_boss` crate + live `pkdealer_agent_boss` | ❌ absent |
| Sequential per-pair verdict | SPRT LLR accumulator + `Verdict` | ❌ absent |
| Truth of who colluded | `GroundTruthLabels` + scorer (incl. EV oracle) | ❌ absent |
| Public per-player behavior | `pkcore::StatsRegistry` / `PlayerStats` | ✅ done (reuse) |
| Completed-hand record | `pkcore::HandCollection` / `HandHistory` | ✅ done (reuse) |
| Card visibility gating | `CardVisibility` / `filter_cards` (service) | ✅ done (reuse) |

---

## Design

### Team configuration — `team` field + arena expansion

`arena.toml` gains an optional per-player `team`, next to `profile`/`type`
(`arena.toml:26-60`). `bin/arena`'s `registry_field` already reads arbitrary
fields generically (`bin/arena:52-64`). Because a colluding *agent* must know
**who** its partner is (a bare team id names no one), `emit_service`
(`bin/arena:232-234`) — which sees the whole lineup — **expands** team
membership into the explicit pairwise flags the agent consumes:
`--collude-with <partner-name> --collusion-channel <c> --collusion-style <s>`.

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

Honest players simply omit `team`. The field is inert unless expansion finds ≥2
members. Rationale: the team id is the *human* config surface (one word, one
place); the explicit partner flag is the *agent* contract (unambiguous,
UUID-resolvable). Keeping both means neither the lineup author nor the agent
has to guess.

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

Rationale: the alternative was "convention only" (Boss reads the full
`HandCollection`, just doesn't touch `hole_cards`) — EPIC-70's sentinel went
further and *deliberately* read all cards. We reject both for the headline
detector. The whole *point* of the EPIC is a credible claim that the detector
works blind; a claim that rests on a reviewer trusting that no code path reads a
field is not credible, and a detector that needs the cards has answered a less
interesting question. `redact` is the single choke point, unit-tested to prove
the output contains no card data, and the Boss depends on `RedactedHand` —
never on `HandCollection`. Board cards are public (dealt face-up) and retained.
Card-aware analysis is not lost — it moves to the scorer tier, where it belongs
(see *Ground-truth scorer*).

### `CollusionConfig` + CLI on `pkdealer_agent_rules`

Extends the existing decision-override flags (`apply_decision_overrides`
`crates/pkdealer_agent_rules/src/main.rs:232`), mirroring the
`--spectator-token`/`--exploit` precedent (`main.rs:151-165`):

```rust
struct CollusionConfig {
    partner: String,          // --collude-with <name>: partner's *arena-composed*
                              //   name (unique, e.g. `trudy` / `gto_2`),
                              //   resolved to a stable Uuid before use
    channel: CollusionChannel,// --collusion-channel spectator|peer
    style: CollusionStyle,    // --collusion-style soft|whipsaw|dump
}
enum CollusionChannel { Spectator, Peer }
enum CollusionStyle   { SoftPlay, Whipsaw, ChipDump }
```

Partner cards reach the decider by extending the existing snapshot path
(`snapshot_with_stats` `crates/pkdealer_agent_rules/src/main.rs:478`) with an
optional `partner_hole: Option<Cards>`; a thin collusion wrapper around
`RuleBasedDecider` (`main.rs:308-311`) reads it. When no collusion is configured
the field is `None` and behavior is byte-for-byte the current agent. Cheating is
strictly additive and off by default.

**Why a wrapper, not a new profile:** collusion is orthogonal to archetype — a
`gto` colluder and a `maniac` colluder are both interesting. Wrapping keeps every
existing `BotProfile` (`load_profile`, `main.rs:585-608`) usable as the
underlying honest strategy, so soft-play is measured as a *delta* from that
bot's honest baseline.

**Identity & hand sequence — shared Phase-0 plumbing for both vectors.** Two facts
the agent does not carry today must be threaded in first:

- **Stable partner identity (UUID, not name).** `HandState`/`SeatSnapshot` expose
  only seat *names* (`crates/pkdealer_agent_core/src/hand_state.rs:28`), but the whole
  detection pipeline keys on player `Uuid` (`RedactedHand.player_id`; the `Uuid`-keyed
  `StatsRegistry`). So `--collude-with` takes the **arena-composed** name — unique
  because `bin/arena:284` emits `${name}_${n}` — and the agent **resolves it to the
  partner's `Uuid`** from the status snapshot. Labels and peer shares store the
  **UUID**; the display name is kept only for human readability. This removes the
  name-collision ambiguity around duplicates like `gto_1` / `gto_2`.
- **Hand sequence.** A monotonic hand number already exists service-side as
  `TableStatus.round_number` (`crates/pkdealer_service/src/main.rs:646`, incremented
  at `:996`; also surfaced as `GetSessionInfo.hand_count`), but is **not** yet on
  `HandState`. Thread it onto `HandState` as `hand_no` so the snapshot and the
  Vector-B `CardShare` agree on which hand a shared card belongs to.

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
// One share per hand. `hand_no` = the dealer's `round_number` for that hand
// (threaded onto HandState in Phase 0), so two peers never mismatch hands.
// `player_id` is the sharer's resolved Uuid, not a display name.
struct CardShare { hand_no: u32, seat: u8, player_id: Uuid, hole_cards: String }

trait Backchannel {
    async fn publish(&self, share: CardShare);          // "here are my cards this hand"
    async fn partner_cards(&self, hand_no: u32) -> Option<CardShare>;
}
```

Each colluder publishes its own cards each hand and reads its partner's, matched by
`hand_no`. **Addressing must survive container isolation:** `bin/arena` puts every
agent in its own compose service/container (`bin/arena:284`), each with its own
network namespace, so a bare `127.0.0.1:PORT` cannot bridge two colluders. Two
workable designs:

- **Direct, via compose DNS** — one colluder binds a TCP listener; the partner dials
  it by **service hostname** (`PKDEALER_BACKCHANNEL=trudy:9099`), which docker's
  embedded DNS resolves on the shared compose network. Simplest, but asymmetric (one
  listens, one dials).
- **Broker service** *(recommended)* — a tiny `pkdealer_backchannel` compose service
  both colluders dial (`PKDEALER_BACKCHANNEL=backchannel:9099`); it fans `CardShare`s
  out by pair. Symmetric, scales past two seats, and keeps the agents identical — the
  channel becomes a first-class, inspectable component rather than hidden socket glue.

Either way it is **not** the dealer service — the service never learns cards are
shared. This is the "realistic" vector: partner-only information, no privileged token.

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
  re-raise to squeeze the victim out of pots the pair will contest. (EPIC-70
  called this "squeeze".) Detectable as coordinated raise→re-raise sequences
  that fold the field.
- **ChipDump** — fold strong hands *to the partner* when the partner is committed
  with a stronger holding (or is short and needs the chips). Detectable as chip
  flow that runs one direction between two specific players.

Each style is a pure function of `(base_decision, snapshot, partner_hole)` and is
unit-tested against fixtures asserting the *behavioral* signature it must leave.
The team can compare member hand strengths via the equity read already available
(`EquityMode::Fast{samples}`, `crates/pkdealer_agent_rules/src/main.rs:169-177`).

### `pkdealer_boss` — offline analyzer

New crate, modeled on `pkdealer_costsim` (`crates/pkdealer_costsim/src/{lib,app,report,main}.rs`):
a binary that reads exported sessions and emits a report. Its detection library
takes **only** `&[RedactedHand]` and computes, per unordered seat-pair, the
**signals**:

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
  above the base rate position justifies (catches Whipsaw).
- **Mutual VPIP/PFR conditioning** — how each player's `vpip()`/`pfr()`
  (`player_stats.rs:119,125`) shifts when the partner is in the pot.

Every metric above is computed from the **observed session alone** — no
counterfactual — which is exactly why the Boss can run them live. Win-rate *lift*
(did colluding actually **pay**?) is deliberately **absent here**: it needs a
collusion-off control run the Boss can never observe, so it is a *validation*
metric, not a detection signal, and lives in Phase 5 (Work Item **5b**), overlapping
exit criterion 1.

**Sequential verdict (`detector.rs`).** The signals feed a per-pair running
log-likelihood ratio: `LLR += log P(signal | collusion) − log P(signal | honest)`,
where the honest distribution is calibrated from an all-honest control run
(Phase 5). Flag the pair the first hand `LLR` crosses the upper threshold —
**Wald SPRT bounds** set from target false-positive/false-negative rates. The
flag-hand index is the **time-to-detection**, the headline metric. A
`pkcore::Confidence` (`player_stats.rs:220`) sample-size floor gates flagging —
the Boss says nothing with conviction after five hands regardless of signal.

**Why calibrated LLR, not a fixed heuristic threshold** (and not the earlier
weighted `SuspicionScore` composite): a nit folding marginal hands and a colluder
soft-playing look similar for a few hands; only the *accumulated* discrepancy
separates them, and the honest control gives the null distribution to measure
that accumulation against. A raw threshold either flags tight honest players
(false positive) or needs so much evidence it buries the time-to-detection
result — and a sequential test is the honest formalization of "how few hands."

The output is a per-pair `Verdict { pair, llr, confidence, flagged_at_hand: Option<u32> }`.

### Ground-truth scorer — grading, plus the card-aware oracle

`crates/pkdealer_boss/src/scorer.rs` — a sibling module that reads
`GroundTruthLabels` and the full (un-redacted) `HandCollection`, and grades a
Boss run: for each labeled colluding pair, the **hands-to-detection** (first
hand at which the SPRT flagged with the `Confidence` floor met) and the
**false-positive rate** (honest pairs flagged). It is the only code in the crate
allowed to touch hole cards, and it is import-isolated from the detection library.

It also hosts the **EV-sacrifice oracle** (absorbed from EPIC-70): for every
hand where labeled partners contest the same pot, compare the line a
perfect-information observer would rate optimal with the colluder's known cards
against the action actually taken. This answers EPIC-70's question — *"given
the cards, can you tell soft-play from a nit folding a marginal hand?"* — where
hole-card access is legitimate: as a **grading upper bound** (what an omniscient
auditor could detect, bounding what the blind Boss can be asked to achieve),
never as a detection input.

### `GroundTruthLabels`

`crates/pkdealer_boss/src/labels.rs` (new). Session-level metadata written by the
sim harness, keyed on **stable player `Uuid`s — not display names**, which collide
(`gto_1`/`gto_2`) and aren't what the pipeline keys on:
`{ colluding_pairs: [(Uuid, Uuid)], vector: Spectator|Peer, style }`, with the
human-facing arena names retained alongside purely for readability. The harness
resolves each `--collude-with` name → `Uuid` at write time — the same UUIDs the
recorder stamps on `PlayerEntry.player_id` (`hand_history.rs:1468`) and that
`RedactedHand` carries — so the scorer matches labels to hands with zero name
guesswork. Serialized as a sidecar YAML (the proto is untouched). Consumed only by
the scorer.

### Live boss binary (later phase)

`crates/pkdealer_agent_boss/` (new binary). An observer process that polls
`ExportSession` on the exploit-puller cadence (watermark throttle reuse,
`crates/pkdealer_agent_rules/src/main.rs:355`), **redacts at ingest**, maintains
rolling per-pair LLR, and flags via structured log + OTel. It sits in the arena
like any other agent but never takes a seat. An alternative low-latency tap —
subscribing to `stream_events` (`crates/pkdealer_service/src/main.rs:2669-2726`)
and reconstructing hands from per-event `TableStatus` — is viable and documented
here for future work, but the polling path is primary: it reuses proven plumbing
and feeds the same `redact()` choke point.

**OTel instruments** (absorbed from EPIC-70): the live boss initializes its own
`init_otel` (`crates/pkdealer_service/src/otel.rs:105`) under its own
`OTEL_SERVICE_NAME`, honoring `OTEL_SDK_DISABLED=true`, and exports
`pkdealer.boss.pair_llr` (gauge per pair), `pkdealer.boss.flag_hand`
(histogram), and `pkdealer.boss.false_positive` (counter) through the existing
OTLP pipeline (collector → Prometheus/Jaeger/Grafana).

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

### Phase 0 — Firewall, labels & team plumbing (prerequisites)

- [ ] **0a.** Create `crates/pkdealer_boss` crate skeleton (mirror
  `pkdealer_costsim`); add to workspace `Cargo.toml` members.
- [ ] **0b.** Implement `RedactedHand` + `redact()` in
  `crates/pkdealer_boss/src/redacted.rs`, consuming `HandCollection`.
- [ ] **0c.** Implement `GroundTruthLabels` (**UUID-keyed** pairs + names for
  readability) + sidecar YAML (de)serialization.
- [ ] **0d.** Thread `hand_no` (from `TableStatus.round_number`,
  `crates/pkdealer_service/src/main.rs:646`) onto `HandState`, and add partner
  name→`Uuid` resolution from the status snapshot. Both vectors depend on this.
- [ ] **0e.** Feature-gate: add a `collusion` feature to `pkdealer_agent_rules`
  and `pkdealer_agent_core`; confirm `cargo check` is green with and without it.
- [ ] **0f.** Add optional `team` to the `arena.toml` schema doc header
  (`arena.toml:11-15`) + two example colluder entries; in `bin/arena`, read it
  via `registry_field` (`bin/arena:52-64`) and expand team membership into
  `--collude-with`/`--collusion-channel`/`--collusion-style` flags in
  `emit_service` (`bin/arena:232-234`). No-op when absent. Test: a lineup with
  `mallory`+`trudy` on team `A` and `gto` honest emits partner flags for
  exactly the two teammates.

### Phase 1 — Vector A colluders + strategies

- [ ] **1a.** `CollusionConfig` + CLI/env flags on `pkdealer_agent_rules`
  (`--collude-with`, `--collusion-channel`, `--collusion-style`); a colluder
  requires a spectator token, reusing the `--exploit` validation pattern
  (`crates/pkdealer_agent_rules/src/main.rs:617-643`).
- [ ] **1b.** `SpectatorLeak` puller (partner-card extraction, honor filter),
  cloned from `ExploitPuller` (`crates/pkdealer_agent_rules/src/main.rs:325`).
- [ ] **1c.** `CollusionStyle` strategies (soft/whipsaw/dump) as decider wrappers.
- [ ] **1d.** Unit tests proving coordination is **partner-conditioned, not just
  passive**: colluder checks back a made hand vs. a live partner (soft-play) but
  bets the same hand vs. a lone non-teammate; colluder folds the *worse* of two
  known team hands to concentrate equity.
- [ ] **1e.** `arena.toml` colluding lineup entries ride Phase 0f expansion.
- [ ] **1f.** Sim smoke check: colluding pair's combined chips beat an honest
  control over N hands (mechanically confirms the cheat works — the *replicated,
  confidence-bounded* version is exit criterion 1 / Work Item 5b).

### Phase 2 — The Boss (offline) — *heart of the EPIC*

- [ ] **2a.** Pairwise signals over `&[RedactedHand]`, all observed-session-only
  (chip-flow, soft-play index, whipsaw count, conditioned VPIP/PFR). *No win-rate
  lift here — it needs a control run (Work Item 5b).*
- [ ] **2b.** `detector.rs` — per-pair SPRT LLR accumulator with Wald bounds +
  `Confidence` sample-size floor; `Verdict` with `flagged_at_hand`.
- [ ] **2c.** Ground-truth scorer: hands-to-detection + FP rate + the
  EV-sacrifice oracle (card-aware, scorer-tier only).
- [ ] **2d.** `pkdealer_boss` binary: read exported session + labels → report.

### Phase 3 — Vector B backchannel

- [ ] **3a.** `Backchannel` in `pkdealer_agent_core` (recommended: a small
  `pkdealer_backchannel` **broker** compose service both colluders dial by hostname;
  fall back to direct compose-DNS if simpler). NOT `127.0.0.1` — containers are
  network-isolated (`bin/arena:284`).
- [ ] **3b.** Wire `--collusion-channel peer` to publish/subscribe partner cards,
  matched by `hand_no`; `CardShare.player_id` is the resolved `Uuid`.
- [ ] **3c.** Re-run the Boss against a Vector-B session; assert the detection
  signature matches Vector A within tolerance (same behavior, different channel).

### Phase 4 — Live boss

- [ ] **4a.** `pkdealer_agent_boss` binary: `ExportSession` polling + redact-at-ingest.
- [ ] **4b.** OTel instruments (`pair_llr` gauge, `flag_hand` histogram,
  `false_positive` counter) via a boss-local `Metrics` + `init_otel`
  (`crates/pkdealer_service/src/otel.rs:105`); honor `OTEL_SDK_DISABLED`.
- [ ] **4c.** Arena wiring: a `boss` type in `emit_service` (`bin/arena:218`).

### Phase 5 — Calibration, validation & report

- [ ] **5a.** SPRT calibration over **K seeded runs** (not one high-variance
  sample — the gRPC arena is non-mirrored): fit the honest null distribution
  per signal from control runs; set Wald bounds from target FP/FN; sweep for
  best hands-to-detection vs FP.
- [ ] **5b.** Collusion-off **control run** (same agents, same seats, collusion
  disabled) → compute **win-rate lift** = pooled bb/100 (collusion) − pooled bb/100
  (control). This is the "did it pay" validation, kept out of the live detector.
- [ ] **5c.** Honest-lineup FP study over K runs → report a false-positive **rate
  with a confidence interval**, not a single "zero".
- [ ] **5d.** Statistical write-up: state "cheat pays" and detection speed as
  **replicated / confidence-bounded** results, including a table of
  `(team archetype, collusion style) → median hands-to-detection` and the
  oracle-vs-blind-Boss gap; note that EPIC-45's mirrored decks would shrink
  these intervals.
- [ ] **5e.** DEVLOG close-out section `## EPIC-70 — Collusion & Cheat Detection (YYYY-MM-DD)`.

---

## Test Plan

- `redact_drops_hole_cards` — `redact()` output serialized contains no card
  strings and no deck; a property test over recorded fixtures.
- `redacted_hand_has_no_card_field` — compile-time/structural: the detection API
  signature takes `&[RedactedHand]`; a doc test shows a `HandCollection` cannot be
  passed.
- `arena_team_expands_to_partner_flags` — a `team = "A"` pair in `arena.toml`
  emits `--collude-with` naming exactly the other teammate; honest seats emit
  no collusion flags.
- `soft_play_never_raises_partner_heads_up` — fixture where hero holds a raising
  hand heads-up vs partner → action is check/call, not raise.
- `colluder_softplays_partner_only` — the same made hand is *bet* vs. a lone
  non-teammate (proves partner-conditioning, not passivity).
- `colluder_folds_worse_team_hand` — with two known team hands preflop, the
  weaker folds to concentrate equity.
- `whipsaw_squeezes_third_party` — three-handed fixture, partner raises → hero
  re-raises the victim.
- `chip_dump_folds_strong_to_partner` — hero holds a strong hand, partner
  committed stronger → hero folds.
- `metric_chip_flow_flags_dump` / `metric_soft_play_index_flags_soft` /
  `metric_whipsaw_count_flags_whipsaw` — each signal on a synthetic session with a
  known planted signature scores the guilty pair above honest pairs.
- `chipflow_honest_nets_zero` — over a synthetic honest `HandCollection`, every
  pair's directed net ≈ 0 within the variance band.
- `sprt_flags_colluders` — on a labelled colluding corpus the SPRT flags the
  true pair and records a finite `flagged_at_hand`.
- `sprt_honest_under_fp_bound` — on the honest control corpus, flag rate ≤ bound.
- `suspicion_confidence_low_on_small_sample` — < ~50 hands → `Confidence` below
  the flagging floor regardless of signal.
- `scorer_reports_hands_to_detection` — labeled colluding session → scorer returns
  a finite hands-to-detection for the true pair.
- `oracle_ev_sacrifice_scores_softplay` — hand-crafted soft-play spot scores
  above an honest fold of the same cards (scorer tier).
- `vector_a_and_b_same_signature` — the same style over both channels yields
  detection scores within tolerance (Phase 3).
- `collude_with_resolves_composed_name_to_uuid` — `--collude-with gto_2` resolves to
  the correct partner `Uuid` from the status snapshot; ambiguous/duplicate base names
  never leak into labels or shares.
- `backchannel_matches_shares_by_hand_no` — a share published for hand N is returned
  to the partner only for hand N (no cross-hand contamination), using
  `round_number`-derived `hand_no`.

## Key Files

| File | Role |
|---|---|
| `arena.toml`, `bin/arena` | `team` field + expansion into collusion flags; boss + broker lineups |
| `crates/pkdealer_boss/src/redacted.rs` | `RedactedHand` + `redact()` firewall |
| `crates/pkdealer_boss/src/{signals,detector,verdict}.rs` | pairwise signals + SPRT verdict |
| `crates/pkdealer_boss/src/scorer.rs` | ground-truth grading + EV oracle (may read hole cards) |
| `crates/pkdealer_boss/src/labels.rs` | `GroundTruthLabels` sidecar |
| `crates/pkdealer_agent_rules/src/collude/` | `CollusionConfig`, `SpectatorLeak`, strategies |
| `crates/pkdealer_agent_core/src/backchannel.rs` | Vector B peer-channel client |
| `crates/pkdealer_backchannel/` | Vector B broker compose service (recommended) |
| `crates/pkdealer_agent_core/src/hand_state.rs:106` | add `hand_no` (from `round_number`) |
| `crates/pkdealer_agent_boss/` | live observer binary (Phase 4) |
| `Cargo.toml` | + `pkdealer_boss` (+ later crates) workspace members |

## Reuse (do NOT recreate)

- `crates/pkdealer_agent_rules/src/main.rs:367,617` — `ExploitPuller`'s
  **second-connection + spectator-token-into-metadata** plumbing: this is the part
  Vector A copies (it reads *live* state per decision, so it does NOT reuse the
  refresh cadence). The **watermark throttle** (`crates/pkdealer_agent_rules/src/main.rs:355`),
  which re-pulls only on a completed hand, is instead reused by the *live Boss*,
  whose `ExportSession` polling is completed-hand-driven.
- `crates/pkdealer_agent_rules/src/main.rs:478` — `snapshot_with_stats`: the snapshot
  threading point to extend with `partner_hole` (precedent: how `opponent_stats`
  reached the decider, commit `762b7d5`).
- `crates/pkdealer_agent_rules/src/main.rs:308-311,585-608,169-177` —
  `RuleBasedDecider`, `load_profile`/`BotProfile`, `EquityMode::Fast`: the honest
  strategy machinery the collusion wrapper composes over.
- `crates/pkdealer_agent_core/src/runner.rs:492-509` — `finalize_decision`
  legality clamp: unchanged; guarantees colluders cheat in choice, not protocol.
- `crates/pkdealer_service/src/main.rs:646,996` — `TableStatus.round_number`: the
  existing monotonic hand sequence to thread onto `HandState` as `hand_no` (Vector B
  hand-matching); also surfaced as `GetSessionInfo.hand_count`.
- `crates/pkdealer_service/src/main.rs:1037-1056,655-672,2404` —
  `card_visibility_from_metadata` / `filter_cards` / ExportSession gating: the
  visibility contract the sim depends on (read-only; unchanged).
- `crates/pkdealer_service/src/main.rs:2669-2726` — `stream_events` per-subscriber
  filtering: the documented alternative live tap for the boss.
- `player_stats.rs:55,163,265,342` — `PlayerStats` / `StatsRegistry` /
  `aggression_factor` / `ingest_collection`: the public-stat engine the Boss reuses.
- `player_stats.rs:220,233` — `Confidence` / `from_sample_size`: sample-size gating.
- `hand_history.rs:128,1468,1477,2286` — `HandHistory` fields `redact()` consumes.
- `crates/pkdealer_costsim/src/{lib,app,report}.rs` — the offline-analyzer crate
  shape `pkdealer_boss` mirrors.
- `crates/pkdealer_service/src/otel.rs:105` — `init_otel`: the boss-local OTel
  bootstrap (own `OTEL_SERVICE_NAME`, honors `OTEL_SDK_DISABLED`).

## Compatibility

- **Preserves** the dealer service, proto, pkcore, and default agent behavior —
  collusion is behind a `collusion` feature and off unless flags are set; `team`
  absent ⇒ byte-identical compose output. Vector A uses the existing spectator
  token; Vector B never touches the service.
- **Adds** the `pkdealer_boss` + `pkdealer_agent_boss` crates, collusion flags on
  the rules agent, a `team` field in `arena.toml`, a peer backchannel in
  agent-core, and a labels sidecar.
- **Breaks** nothing. No existing test should change behavior; new behavior only
  appears under new flags/features.

## Dependencies

- **Built on:** EPIC-23 (bot agents + `PokerAgent`/runner), EPIC-25 (recorder +
  `ExportSession`), EPIC-42 (Dynamic Arena Runner / `arena.toml` + `bin/arena`),
  and the `sbot` exploit wiring (`ExploitPuller`, spectator-token puller pattern,
  commit `762b7d5`).
- **Related:** EPIC-45 (Bot Evaluation Format) — its planned in-process,
  deterministic-deck arena would let the Boss be benchmarked under variance
  control (same deck, colluding vs honest). Not required; the gRPC path suffices
  now. When EPIC-45 lands, a Phase 6 could re-run the FP/speed study on mirrored
  decks. EPIC-41 (Reproducible Scenarios) is unstarted — no seeded replay, so
  time-to-detection is over hands dealt, not a reproducible corpus. **pkcore
  EPIC-79 (Mental Poker)** is the *fix* for the spectator-token hole this EPIC
  exploits — explicitly out of scope here.
- **Blocks:** nothing yet.

## Verification

```bash
cargo build --workspace --features collusion
cargo clippy --workspace --features collusion -- -D warnings
OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss --all-features
OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion
cargo test --doc -p pkdealer_boss

# End-to-end sim: mallory + trudy secretly on team A vs four honest bots.
# Team membership lives in arena.toml; bin/arena expands it into partner flags.
./bin/arena mallory trudy gto lag tag tp
PKDEALER_RECORD_DIR=./out docker compose ... up           # capture the session to disk
cargo run -p pkdealer_boss -- --session ./out/session-*.yaml --labels ./out/labels.yaml

# Honest control — same lineup, no teams — must stay under the FP bound
./bin/arena gto lag tag tp lp
cargo run -p pkdealer_boss -- --session ./out/control-*.yaml
```

Exit criteria:

1. Across **K seeded runs**, a configured colluding pair's pooled chip result
   **beats** the collusion-off control at **p < 0.05** — the cheat pays as a
   *replicated* result, not a single high-variance sample.
2. `./bin/arena mallory trudy gto …` emits a compose override where the two
   teammates carry `--collude-with` naming each other and honest seats carry no
   collusion flags.
3. The Boss, reading **only** `RedactedHand`, flags the true colluding pair via
   the SPRT with a finite, reported **hands-to-detection** (reported as a
   distribution over the K runs), and the report includes the
   `(archetype, style) → median hands-to-detection` table.
4. Over **K all-honest control runs**, the Boss's **false-positive rate** stays below
   the calibrated bound (target ≈ 0), reported **with a confidence interval** — not
   asserted as an absolute zero on a single run.
5. Vectors A and B produce detection scores within tolerance of each other — the
   Boss catches the *behavior*, not the channel.
6. `redact()` provably emits no hole cards or deck (test `redact_drops_hole_cards`),
   and the detection API cannot accept a `HandCollection`.
7. No **existing** crate's tests change behavior: the pre-existing crates' test
   results (run without the `collusion` feature) are identical to HEAD. New
   behavior appears only in the added `pkdealer_boss`/`pkdealer_agent_boss` crates
   and behind the `collusion` feature. (Note: `cargo test --workspace` itself now
   *runs more* — the new crates — so compare per-crate, not by raw workspace pass
   count.)
