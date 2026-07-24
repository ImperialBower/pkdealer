# EPIC-70 Phase 3 — Vector B Peer Backchannel — Design

**Status:** approved design, pre-implementation (2026-07-23).
**Parent:** `docs/EPIC-70_Collusion_and_Cheat_Detection.md` (Phases 0–2 landed on
branch `boss`, commit `ef33abf`).
**Scope of this pass:** the `Backchannel` abstraction, the `pkdealer_backchannel`
broker crate, agent peer-wiring, and the proto/service identity change — all
validated **in-process** (including a real broker over a loopback socket).
The full live-`docker compose` A/B *signature* equivalence run is deferred to a
manual checklist, consistent with how Phases 0–2 handled live verification.

## Context

Phase 1 shipped Vector A: a colluder reads its partner's hole cards live over
the spectator token (`SpectatorLeak`, `crates/pkdealer_agent_rules/src/collude/spectator.rs`),
applies an `apply_style` adjustment (`collude/strategy.rs`), and plays otherwise
honestly. Vector B must be **behaviorally indistinguishable at the table** — same
strategy, same decisions — differing only in *how* the partner's cards arrive: a
bot-to-bot side-channel the dealer never sees, using **partner-only** information
(no privileged spectator token).

Phase 0 surfaced the blocking wrinkle: `proto/dealer.proto` `SeatInfo` carries no
player UUID, so a *live* agent cannot learn its own (or its partner's)
recorder-assigned identity from the wire. Phase 0 worked around this offline
(`GroundTruthLabels::resolve` maps name→UUID from the recorded `HandCollection`),
but Vector B needs a live identity for the peer exchange.

## Decision: amend the "proto untouched" non-goal

EPIC-70's Context section states the EPIC does **not** change the dealer service
or proto. **Phase 3 deliberately reopens that non-goal**, narrowly:

- `SeatInfo` gains one **additive** field: a player UUID string. A UUID is public
  identity (names are already on the wire), **not** a hole card — so this does
  **not** touch the fog-of-war card redaction (`filter_cards`,
  `card_visibility_from_metadata`). The service stays honest about *cards*; it
  merely stops hiding an *identity* it already tracks.
- The field is populated from state the server already holds
  (`ServerState.seat_to_token: HashMap<u8, Uuid>`, `crates/pkdealer_service/src/main.rs:355`).

This amendment is documented in the EPIC's non-goals and Phase-3 corrigendum.

## Design

### 1. Proto + service — identity on the wire

- `proto/dealer.proto`: add `string player_id = 13;` to `SeatInfo` (UUID string;
  empty when unknown). Additive, backward-compatible (proto3).
- `crates/pkdealer_service/src/main.rs`: at the `SeatInfo` construction site
  (`main.rs:601`), populate `player_id` from `seat_to_token.get(&seat)`,
  stringified. **Not** redacted — identity is public. `filter_cards` continues to
  redact `cards` only.
- **Verify at implementation:** whether `seat_to_token`'s UUID equals the
  recorder's `PlayerEntry.player_id`. If it does, wire and recorder identities
  unify (a nice-to-have that also lets a later pass retire the Phase 0
  name→UUID offline deviation). If it does **not**, Vector B still works: the
  `CardShare` UUID is used only peer-to-peer (both colluders read the same
  `TableStatus`, so they agree), never compared against `RedactedHand` — the Boss
  still matches labels by recorder UUID offline.

### 2. Agent-side live UUID resolution

- `crates/pkdealer_agent_core/src/hand_state.rs`: `SeatSnapshot` gains
  `player_id: Option<Uuid>` (parsed from `SeatInfo.player_id`; `None` when the
  field is empty or unparseable — e.g. an older service).
- `crates/pkdealer_agent_core/src/runner.rs`: the `seat_snapshot` builder parses
  and threads it. A colluder then reads its own UUID (its seat's snapshot) and
  its partner's UUID (the partner-seat snapshot, located by the existing
  `--collude-with` name match) live, with no proto round-trip beyond the status
  it already fetches.

### 3. Backchannel + broker — the Vector B transport

New crate `crates/pkdealer_backchannel/` (unconditional workspace member — a
relay is not itself a cheat, mirroring how `pkdealer_boss` is unconditional):

- A TCP broker binary. On each connection it registers the client; every
  `CardShare` line received is **broadcast to all other connected clients**
  (never echoed to the sender). No pair configuration — clients filter for their
  partner. Line-delimited JSON, one `CardShare` per line.
- `CardShare { hand_no: u32, seat: u8, player_id: Uuid, hole_cards: String }`
  (`serde` Serialize/Deserialize). One share per publisher per hand.

`crates/pkdealer_agent_core/src/backchannel.rs` (feature-gated `collusion`):

```rust
pub struct BackchannelClient { /* write half + shared buffer */ }

impl BackchannelClient {
    /// Dials the broker and spawns a background reader that buffers incoming
    /// shares by (publisher_uuid, hand_no).
    pub async fn connect(addr: &str) -> Result<Self, String>;
    /// "Here are my cards this hand."
    pub async fn publish(&self, share: CardShare);
    /// The partner's cards for `hand_no`, or `None` if not yet received.
    pub async fn partner_cards(&self, partner_id: Uuid, hand_no: u32) -> Option<Cards>;
}
```

Buffer: `Arc<Mutex<HashMap<(Uuid, u32), Cards>>>`. Best-effort — a missed/late
partner share yields `None`, and the colluder decides honestly that turn (the
same graceful degradation as Vector A's `SpectatorLeak`).

### 4. Unify Vectors A and B behind one interface

The core of "catch the behavior, not the channel" becomes a **type-level**
guarantee. In `crates/pkdealer_agent_rules/src/collude/`:

```rust
#[async_trait]
pub trait PartnerCardSource {
    /// The partner's hole cards for this hand, or `None` (decide honestly).
    async fn partner_hole(
        &self,
        hand_no: u32,
        my_cards: &Cards,
        partner_id: Uuid,
    ) -> Option<Cards>;
}
```

- `SpectatorLeak` implements it: ignores `hand_no`/`my_cards`/`partner_id`, reads
  the partner's live cards via the status snapshot (existing honor-filter path).
- `BackchannelClient` implements it: `publish(CardShare { hand_no, my_cards, .. })`
  then `partner_cards(partner_id, hand_no)`.

`Colluder.leak` changes from `SpectatorLeak` to `Box<dyn PartnerCardSource>`.
`RulesAgent::choose` resolves `partner_seat` + `partner_id` from the snapshot and
calls `partner_hole(...)` — **identical code for both vectors**. `apply_style`
already takes `partner_hole` and is channel-agnostic, so the decision path is
byte-for-byte the same. `validate_collusion` drops the peer rejection; a peer
colluder constructs a `BackchannelClient` from `PKDEALER_BACKCHANNEL`
(env/flag). Spectator still requires `--spectator-token`.

### 5. Arena / compose wiring

- `arena.toml`: teamed players gain an optional `channel = spectator | peer`
  (default `spectator`), alongside `style`.
- `bin/arena`: when a team's `channel` is `peer`, `emit_service` adds
  `--collusion-channel peer` and `PKDEALER_BACKCHANNEL=backchannel:9099` to those
  agents, and the generator emits the `pkdealer_backchannel` broker compose
  service **once** if any peer team is present. Spectator teams and team-less
  lineups are byte-identical to today.
- New `tests/arena_peer.sh` (dry-run, mirroring `tests/arena_team.sh`): a
  `channel = peer` team emits the peer flags + the env + the broker service;
  spectator teams emit none of these.

### 6. Testing — in-process, no docker

- `backchannel_matches_shares_by_hand_no`: bind the broker to an ephemeral
  loopback port, connect two clients, publish from A, assert B reads A's cards
  for hand N and **only** hand N (no cross-hand contamination).
- `broker_broadcasts_to_others_not_sender`: a publisher never receives its own
  share back.
- `partner_cards_absent_hand_returns_none`: graceful degradation.
- `vector_a_and_b_same_decision`: given identical `HandState` + the *same* partner
  cards delivered through each `PartnerCardSource`, `choose()` returns the same
  action. This is the in-process A/B equivalence.
- Service: a unit/integration test asserting `SeatInfo.player_id` is populated
  for seated players and empty for empty seats.
- Regression: every existing test passes unchanged with the feature off; the new
  proto field defaults empty so no existing behavior changes.

### 7. Deferred to a manual checklist

The full **live-docker A/B signature equivalence** (EPIC test
`vector_a_and_b_same_signature`, exit criterion 5): stand up the broker + two
peer colluders + the dealer, capture a Vector-B session, run `pkdealer_boss`, and
assert it flags the pair with a hands-to-detection within tolerance of a Vector-A
run. Requires a working local docker environment; not runnable under plain
`cargo test`.

### 8. Docs

- `docs/EPIC-70_...md`: flip the Vector-B Status row to **Complete**, check work
  items 3a–3c, amend the Context non-goal (proto/service now touched, with the
  identity-not-cards rationale), and add a Phase-3 entry to the Implementation
  corrigendum.
- OKF: add the `pkdealer_backchannel` crate concept + index entry; note the
  `SeatInfo.player_id` addition in the Dealer gRPC API concept; append a dated
  `log.md` line; re-validate `--strict`.

## File structure

```
proto/dealer.proto                         + SeatInfo.player_id (field 13)
crates/pkdealer_service/src/main.rs        populate player_id at SeatInfo build (:601)
crates/pkdealer_backchannel/               NEW unconditional crate (broker binary)
  Cargo.toml
  src/main.rs                              TCP broker: broadcast CardShare to others
  src/lib.rs                               CardShare type + broker core (testable)
crates/pkdealer_agent_core/
  src/hand_state.rs                        + SeatSnapshot.player_id: Option<Uuid>
  src/runner.rs                            parse + thread seat player_id
  src/backchannel.rs                       NEW (feature collusion): BackchannelClient
  Cargo.toml                               + serde/serde_json (feature-gated as needed)
crates/pkdealer_agent_rules/
  src/collude/mod.rs                       PartnerCardSource trait; Colluder.leak: Box<dyn>
  src/collude/spectator.rs                 impl PartnerCardSource for SpectatorLeak
  src/collude/backchannel_source.rs        NEW: impl PartnerCardSource for BackchannelClient
  src/main.rs                              validate_collusion: accept peer; wire client
arena.toml                                 + channel field on teamed players
bin/arena                                  peer → broker service + env + flags
tests/arena_peer.sh                        NEW dry-run shell test
Cargo.toml                                 + crates/pkdealer_backchannel member
```

## Open verification points (resolve during implementation)

1. Does `seat_to_token`'s UUID equal the recorder's `PlayerEntry.player_id`?
   (Determines whether wire/recorder identities unify — nice-to-have, not
   required.)
2. Exact `SeatInfo` construction site and whether it runs per-subscriber (it must
   populate `player_id` for all callers, including the redacted path).
3. Whether `pkdealer_agent_core` already depends on `serde`/`serde_json` or needs
   them added for `CardShare`.
