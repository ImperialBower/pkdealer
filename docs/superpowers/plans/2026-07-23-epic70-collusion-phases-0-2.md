# EPIC-70 Collusion & Cheat Detection — Phases 0–2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement EPIC-70 Phases 0–2: the `RedactedHand` typed firewall + ground-truth labels + `team` arena plumbing (Phase 0), Vector-A spectator-leak colluders with soft-play/whipsaw/chip-dump strategies (Phase 1), and the offline Boss — pairwise public-information signals, a sequential SPRT detector, and a card-aware ground-truth scorer with an EV-sacrifice oracle (Phase 2).

**Architecture:** A new `pkdealer_boss` crate (mirroring `pkdealer_costsim`'s lib/app/report/main shape) holds the detection pipeline; its detection API accepts only `&[RedactedHand]`, so hole cards are unrepresentable by construction. Collusion lives in `pkdealer_agent_rules` behind a `collusion` cargo feature as a pure wrapper over the existing `RuleBasedDecider`; Vector A reuses the proven `ExploitPuller` second-connection + spectator-token pattern. `bin/arena` expands an `arena.toml` `team` field into explicit pairwise CLI flags.

**Tech Stack:** Rust 2024 (workspace), pkcore 0.3.1 (`HandCollection`, `PlayerStats`, `Confidence`, `HandRanker`), tonic/prost gRPC, clap 4, serde + serde_yaml_bw 2.5, bash (bin/arena).

**Spec:** `docs/EPIC-70_Collusion_and_Cheat_Detection.md` (Phases 0–2 only; Phases 3–5 — peer backchannel, live boss, calibration — are explicitly out of scope here).

## Global Constraints

- **No git commands are run by the implementing agent — ever.** At each commit point, print the exact `git add … && git commit -m "…"` command for the user (Christoph) to run themselves, and wait. This is the user's global rule and overrides all skill defaults.
- Do not modify `pkcore` (external, 0.3.1), `proto/dealer.proto`, or `crates/pkdealer_service` behavior. The service stays honest.
- All new agent-side collusion code is behind a `collusion` cargo feature on `pkdealer_agent_rules` and `pkdealer_agent_core`; with the feature off, every crate builds and all existing tests pass byte-identically.
- `pkdealer_boss` is a new unconditional workspace crate (no feature gate — detection is not a cheat).
- House rules (CLAUDE.md): no `unwrap()`/`expect()`/`panic!()` in library code (tests OK); every public item gets doc comments **with doc tests**; unit test names never start with `test_`; `#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]` at each crate root, matching existing crates.
- Run tests with `OTEL_SDK_DISABLED=true` (no collector running).
- `cargo --features` does not work with `--workspace` in a virtual workspace — always use `-p <crate> --features collusion` (the EPIC's `cargo build --workspace --features collusion` line is wrong; note this when updating docs in Task 14).
- Card-notation convention: recorded YAML uses suit symbols (`"A♠ K♠"`); the gRPC wire and `HandState` use index notation (`"Ah Kd"`). `Five/Six/Seven/Two::from_str` parse both; `Cards::forgiving_from_str` parses index notation.
- pkcore hand-rank convention: **lower `HandRankValue` (u16) = stronger hand** (royal flush = 1, worst = 7462).

## Documented deviations from the EPIC design sketch

Record these in the EPIC doc in Task 14 — they are deliberate:

1. **Amounts are `f64`, not `u32`**, in `RedactedHand` — pkcore `HandHistory` stores all chip amounts as `f64`; converting back to `u32` would invent edge cases (NaN/fractional) for zero benefit.
2. **`RedactedSeat` carries `name`** — display names are public information (any observer sees them) and reports are unreadable without them.
3. **`hand_no` in redacted output = position in the collection (1-based)** — the recorder pushes hands in order; parsing `HandMeta.id` would be fragile.
4. **Partner name→UUID resolution rides the export path, not the status snapshot** — verified: proto `SeatInfo` carries no UUID field, so the EPIC's "from the status snapshot" is impossible as written. Resolution lives in `pkdealer_boss::labels::GroundTruthLabels::resolve()` against the recorded `HandCollection` (where `PlayerEntry` pairs `name` with `player_id`). Live agent-side UUID resolution is only needed by Vector B (Phase 3).
5. **`--collusion-channel peer` parses but is rejected at startup** with a clear "EPIC-70 Phase 3" message — the flag surface matches the spec now so compose files won't change later.
6. **SPRT likelihood models are explicit pre-calibration defaults** (documented on `SprtParams`); Phase 5 calibration replaces the numbers, not the shapes. The soft-play model uses each player's *own running baseline* aggression as the honest hypothesis (the EPIC's nit-vs-colluder argument), with a fixed fallback until the baseline has ≥ 20 actions.
7. **Classic SPRT stops on the lower bound; we keep accumulating** (session-long evidence, better reporting). Flagging still requires crossing the Wald upper bound *and* a ≥ 50-hand `Confidence` floor.

## File Structure

```
crates/pkdealer_boss/            NEW crate (workspace member)
  Cargo.toml
  src/lib.rs                     crate docs + module tree + firewall contract
  src/error.rs                   BossError
  src/redacted.rs                RedactedHand/Seat/Action/Street + redact()   ← ONLY module (besides scorer/labels/app) that may import HandCollection
  src/labels.rs                  GroundTruthLabels + YAML + resolve()
  src/signals.rs                 Pair, per-hand observations, aggregates      ← redacted-only
  src/detector.rs                SprtParams, assess(), Verdict                ← redacted-only
  src/scorer.rs                  score(), EV-sacrifice oracle                 ← card-aware (grading tier)
  src/report.rs                  render()                                     ← redacted-only
  src/app.rs                     RunConfig + run()
  src/main.rs                    CLI binary
  src/fixtures.rs                #[cfg(test)] synthetic HandHistory builder + corpora
Cargo.toml                       + "crates/pkdealer_boss" member
crates/pkdealer_agent_core/
  Cargo.toml                     + [features] collusion = []
  src/hand_state.rs              + HandState.hand_no: u32
  src/runner.rs                  thread status.round_number → hand_no
crates/pkdealer_agent_rules/
  Cargo.toml                     + [features] collusion = []
  src/main.rs                    + CLI flags, validate_collusion, RulesAgent wiring
  src/collude/mod.rs             CollusionConfig, CollusionChannel            (feature-gated)
  src/collude/spectator.rs       SpectatorLeak + honor filter                 (feature-gated)
  src/collude/strategy.rs        CollusionStyle + apply_style + strength      (feature-gated)
crates/pkdealer_agent_random/src/main.rs   test fixtures gain hand_no: 0
crates/pkdealer_agent_llm/src/{agent,prompt}.rs  same
Dockerfile.agent                 + ARG FEATURES build arg
arena.toml                       + team/style doc header + mallory/trudy
bin/arena                        team → pairwise flag expansion
tests/arena_team.sh              NEW shell test (dry-run override assertions)
```

---

### Task 1: `pkdealer_boss` crate skeleton + `RedactedHand` firewall

**Files:**
- Create: `crates/pkdealer_boss/Cargo.toml`, `crates/pkdealer_boss/src/lib.rs`, `crates/pkdealer_boss/src/redacted.rs`, `crates/pkdealer_boss/src/fixtures.rs`, `crates/pkdealer_boss/src/main.rs` (stub), `crates/pkdealer_boss/src/error.rs` (stub for later tasks — created in Task 2; do NOT create here)
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces (later tasks depend on these exact shapes):

```rust
pub enum RedactedStreet { Preflop, Flop, Turn, River }
pub struct RedactedSeat { pub player_id: Uuid, pub seat: u8, pub name: String, pub starting_stack: f64, pub net: f64 }
pub struct RedactedAction { pub player_id: Uuid, pub seat: u8, pub street: RedactedStreet, pub action: pkcore::hand_history::ActionType, pub amount: Option<f64>, pub all_in: bool }
pub struct RedactedHand { pub hand_no: u32, pub button_seat: Option<u8>, pub big_blind: f64, pub seats: Vec<RedactedSeat>, pub actions: Vec<RedactedAction>, pub board: Option<String> }
impl RedactedHand { pub fn seat_of(&self, player_id: Uuid) -> Option<&RedactedSeat>; pub fn player_ids(&self) -> Vec<Uuid>; }
pub fn redact(collection: &HandCollection) -> Vec<RedactedHand>;
```

- Fixture API (used by Tasks 10–13 tests; `#[cfg(test)] pub(crate)`):

```rust
pub(crate) const MALLORY: Uuid; pub(crate) const TRUDY: Uuid; pub(crate) const GTO: Uuid; pub(crate) const TAG: Uuid;
pub(crate) fn player(seat: u8, name: &str, id: Uuid, stack: f64, hole: Option<&str>) -> PlayerEntry;
pub(crate) fn act(seat: u8, id: Uuid, action: ActionType, amount: Option<f64>) -> Action;
pub(crate) struct HandSpec { pub no: usize, pub players: Vec<PlayerEntry>, pub preflop: Vec<Action>, pub flop: Option<(String, Vec<Action>)>, pub turn: Option<(String, Vec<Action>)>, pub river: Option<(String, Vec<Action>)>, pub nets: Vec<(u8, f64)> }
pub(crate) fn build_hand(spec: HandSpec) -> HandHistory;
pub(crate) fn collection(hands: Vec<HandHistory>) -> HandCollection;
```

- [ ] **Step 1: Create the crate skeleton**

`crates/pkdealer_boss/Cargo.toml`:

```toml
[package]
name = "pkdealer_boss"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
description = "The Boss: blind collusion detection over redacted pkdealer arena sessions (EPIC-70)"
keywords = ["poker", "collusion", "detection", "sprt", "analysis"]
categories = ["games", "command-line-utilities"]
rust-version = "1.85"

[lib]
path = "src/lib.rs"

[[bin]]
name = "pkdealer_boss"
path = "src/main.rs"

[dependencies]
pkcore = { version = "0.3.1", features = ["bot-profiles"] }
clap = { version = "4", features = ["derive", "env"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml_bw = "2.5"
uuid = { version = "1.22", features = ["v4"] }
```

`crates/pkdealer_boss/src/lib.rs`:

```rust
#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_boss`
//!
//! The **Boss** — a collusion detector that is *blind by construction*
//! (EPIC-70). It classifies colluding seat-pairs from public information
//! alone: seats, player UUIDs, per-street actions with amounts, blinds,
//! board, and chip deltas.
//!
//! ## The typed firewall
//!
//! The detection pipeline ([`signals`], [`detector`], [`report`]) accepts
//! only [`redacted::RedactedHand`], a type with **no field that can hold a
//! hole card or deck**. [`redacted::redact`] is the single choke point that
//! consumes a [`pkcore::hand_history::HandCollection`] and drops
//! `hole_cards` and `shuffled_deck` at the boundary. Only [`scorer`] (the
//! grading tier) and [`labels`] resolution may read the full collection —
//! their output never feeds back into detection inputs.

pub mod app;
pub mod detector;
pub mod error;
pub mod labels;
pub mod redacted;
pub mod report;
pub mod scorer;
pub mod signals;

#[cfg(test)]
pub(crate) mod fixtures;
```

For this task only, comment out the not-yet-existing modules (`app`, `detector`, `error`, `labels`, `report`, `scorer`) — each later task uncomments its own line. `src/main.rs` stub so the `[[bin]]` builds:

```rust
#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! `pkdealer_boss` CLI — full argument surface arrives with the app module (Task 13).

fn main() {
    eprintln!("pkdealer_boss: CLI lands in EPIC-70 Phase 2d");
}
```

Add `"crates/pkdealer_boss",` to the workspace `members` list in the root `Cargo.toml` (after `"crates/pkdealer_costsim",`).

- [ ] **Step 2: Write the fixture builder** (`src/fixtures.rs` — tests need it before redact exists)

```rust
//! Synthetic `HandHistory` fixtures for unit tests. Test-only.
#![allow(clippy::unwrap_used)]

use pkcore::hand_history::{
    Action, ActionType, FlopStreet, HandCollection, HandHistory, HandMeta, HandVariant, Outcome,
    PlayerEntry, PreflopStreet, ResultEntry, RiverStreet, Stakes, Streets, TableInfo, TurnStreet,
};
use uuid::Uuid;

pub(crate) const MALLORY: Uuid = Uuid::from_u128(0xA1);
pub(crate) const TRUDY: Uuid = Uuid::from_u128(0xA2);
pub(crate) const GTO: Uuid = Uuid::from_u128(0xB1);
pub(crate) const TAG: Uuid = Uuid::from_u128(0xB2);

pub(crate) fn player(seat: u8, name: &str, id: Uuid, stack: f64, hole: Option<&str>) -> PlayerEntry {
    PlayerEntry {
        seat,
        name: name.to_string(),
        stack,
        player_id: Some(id),
        hole_cards: hole.map(str::to_string),
        posted: None,
        hole_cards_visibility: None,
        withdrawn: None,
    }
}

pub(crate) fn act(seat: u8, id: Uuid, action: ActionType, amount: Option<f64>) -> Action {
    Action { seat, player_id: Some(id), action, amount, all_in: None, agent: None }
}

pub(crate) struct HandSpec {
    pub no: usize,
    pub players: Vec<PlayerEntry>,
    pub preflop: Vec<Action>,
    pub flop: Option<(String, Vec<Action>)>,
    pub turn: Option<(String, Vec<Action>)>,
    pub river: Option<(String, Vec<Action>)>,
    /// (seat, net chips won/lost) — every seated player should appear.
    pub nets: Vec<(u8, f64)>,
}

pub(crate) fn build_hand(spec: HandSpec) -> HandHistory {
    let folded: std::collections::HashSet<u8> = spec
        .preflop
        .iter()
        .chain(spec.flop.iter().flat_map(|(_, a)| a))
        .chain(spec.turn.iter().flat_map(|(_, a)| a))
        .chain(spec.river.iter().flat_map(|(_, a)| a))
        .filter(|a| a.action == ActionType::Fold)
        .map(|a| a.seat)
        .collect();
    let results = spec
        .nets
        .iter()
        .map(|(seat, net)| ResultEntry {
            seat: *seat,
            best_hand: None,
            hand_rank: None,
            outcome: if folded.contains(seat) {
                Outcome::Fold
            } else if *net > 0.0 {
                Outcome::Win
            } else {
                Outcome::Lose
            },
            net: Some(*net),
            pot_won: None,
            mucked: None,
        })
        .collect();
    let board = {
        let mut parts: Vec<&str> = Vec::new();
        if let Some((c, _)) = &spec.flop { parts.push(c); }
        if let Some((c, _)) = &spec.turn { parts.push(c); }
        if let Some((c, _)) = &spec.river { parts.push(c); }
        if parts.is_empty() { None } else { Some(parts.join(" ")) }
    };
    HandHistory {
        pkcore_version: None,
        format_version: pkcore::hand_history::FORMAT_VERSION,
        hand: HandMeta {
            id: format!("fixture-hand-{:03}", spec.no),
            game: HandVariant::Holdem,
            timestamp: None,
            source: Some("fixture".to_string()),
            description: None,
        },
        table: TableInfo {
            name: Some("fixture".to_string()),
            seats: Some(u8::try_from(spec.players.len()).unwrap()),
            button: Some(0),
            stakes: Stakes { small_blind: 50.0, big_blind: 100.0, ante: None, straddle: None, bring_in: None },
            betting_structure: pkcore::games::betting_structure::BettingStructure::NoLimit,
        },
        players: spec.players,
        board,
        streets: Some(Streets {
            preflop: Some(PreflopStreet { actions: spec.preflop, pot: None }),
            flop: spec.flop.map(|(cards, actions)| FlopStreet { cards, actions, pot: None }),
            turn: spec.turn.map(|(card, actions)| TurnStreet { card, actions, pot: None }),
            river: spec.river.map(|(card, actions)| RiverStreet { card, actions, pot: None }),
        }),
        results: Some(results),
        analysis: None,
        shuffled_deck: Some("XX-DECK-MARKER-XX".to_string()),
    }
}

pub(crate) fn collection(hands: Vec<HandHistory>) -> HandCollection {
    let mut c = HandCollection::new();
    for h in hands {
        c.push(h);
    }
    c
}
```

(If `RiverStreet`'s card field is named differently than `TurnStreet`'s — both are `card: String` per pkcore 0.3.1 — the compiler will say so; fix to match pkcore.)

- [ ] **Step 3: Write the failing tests** (in `src/redacted.rs`'s `#[cfg(test)] mod tests` — write the test module first with a stub-free file; the file won't compile until Step 5, which IS the failing state for a new module)

Tests to include (exact list):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, GTO, MALLORY, TAG, TRUDY};
    use pkcore::hand_history::ActionType;

    fn two_hand_collection() -> pkcore::hand_history::HandCollection {
        let h1 = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_000.0, Some("A♠ A♥")),
                fixtures::player(1, "trudy_1", TRUDY, 10_000.0, Some("K♠ K♥")),
                fixtures::player(2, "gto_1", GTO, 10_000.0, Some("7♦ 2♣")),
            ],
            preflop: vec![
                fixtures::act(2, GTO, ActionType::Fold, None),
                fixtures::act(0, MALLORY, ActionType::Raise, Some(300.0)),
                fixtures::act(1, TRUDY, ActionType::Call, Some(300.0)),
            ],
            flop: Some(("Q�centre 6♦ 5♥".replace("centre", "♣"), vec![
                fixtures::act(0, MALLORY, ActionType::Bet, Some(400.0)),
                fixtures::act(1, TRUDY, ActionType::Fold, None),
            ])),
            turn: None,
            river: None,
            nets: vec![(0, 450.0), (1, -400.0), (2, -50.0)],
        });
        let h2 = fixtures::build_hand(fixtures::HandSpec {
            no: 2,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_450.0, Some("9♠ 9♥")),
                fixtures::player(1, "trudy_1", TRUDY, 9_600.0, Some("8♠ 8♥")),
                fixtures::player(3, "tag_1", TAG, 10_000.0, Some("A♦ K♦")),
            ],
            preflop: vec![
                fixtures::act(3, TAG, ActionType::Raise, Some(300.0)),
                fixtures::act(0, MALLORY, ActionType::Fold, None),
                fixtures::act(1, TRUDY, ActionType::Call, Some(300.0)),
            ],
            flop: None,
            turn: None,
            river: None,
            nets: vec![(0, 0.0), (1, -300.0), (3, 300.0)],
        });
        fixtures::collection(vec![h1, h2])
    }

    #[test]
    fn redact_drops_hole_cards() {
        let hands = redact(&two_hand_collection());
        let json = serde_json::to_string(&hands).unwrap();
        // Every planted secret must be gone; suits appear only via the board.
        for secret in ["A♠ A♥", "K♠ K♥", "7♦ 2♣", "9♠ 9♥", "8♠ 8♥", "A♦ K♦", "XX-DECK-MARKER-XX", "hole", "deck"] {
            assert!(!json.contains(secret), "leaked {secret:?} in {json}");
        }
    }

    #[test]
    fn redact_keeps_public_board_and_actions() {
        let hands = redact(&two_hand_collection());
        assert_eq!(hands.len(), 2);
        assert_eq!(hands[0].hand_no, 1);
        assert_eq!(hands[1].hand_no, 2);
        assert_eq!(hands[0].board.as_deref(), Some("Q♣ 6♦ 5♥"));
        assert_eq!(hands[0].actions.len(), 5);
        assert_eq!(hands[0].actions[3].street, RedactedStreet::Flop);
        assert_eq!(hands[0].actions[3].action, ActionType::Bet);
        assert!((hands[0].actions[3].amount.unwrap() - 400.0).abs() < f64::EPSILON);
        assert!((hands[0].big_blind - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn redact_maps_seats_nets_and_ids() {
        let hands = redact(&two_hand_collection());
        let mallory = hands[0].seat_of(MALLORY).unwrap();
        assert_eq!(mallory.seat, 0);
        assert_eq!(mallory.name, "mallory_1");
        assert!((mallory.net - 450.0).abs() < f64::EPSILON);
        assert_eq!(hands[1].player_ids().len(), 3);
    }

    #[test]
    fn redact_skips_hands_without_player_identity() {
        let mut anonymous = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![fixtures::player(0, "a", MALLORY, 1_000.0, None)],
            preflop: vec![],
            flop: None,
            turn: None,
            river: None,
            nets: vec![(0, 0.0)],
        });
        anonymous.players[0].player_id = None;
        let hands = redact(&fixtures::collection(vec![anonymous]));
        assert!(hands.is_empty(), "identity-less hands cannot be attributed pairwise");
    }

    #[test]
    fn redact_empty_collection_is_empty() {
        assert!(redact(&pkcore::hand_history::HandCollection::new()).is_empty());
    }
}
```

(Fix the deliberate `"Q♣ 6♦ 5♥"` construction to a plain string literal — write it directly as `"Q♣ 6♦ 5♥".to_string()` in the fixture call.)

- [ ] **Step 4: Run tests to verify they fail**

Run: `OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss 2>&1 | tail -20`
Expected: compile error — `redact`, `RedactedStreet`, etc. not found.

- [ ] **Step 5: Implement `src/redacted.rs`**

```rust
//! The typed hole-card firewall: what an honest observer may see.

use pkcore::hand_history::{Action, ActionType, HandCollection, HandHistory};
use serde::Serialize;
use uuid::Uuid;

/// Betting street a redacted action belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactedStreet {
    /// Before the flop.
    Preflop,
    /// Three community cards dealt.
    Flop,
    /// Fourth community card dealt.
    Turn,
    /// Fifth community card dealt.
    River,
}

/// One seated player's public state in a [`RedactedHand`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedSeat {
    /// Stable player identity (the recorder's `PlayerEntry.player_id`).
    pub player_id: Uuid,
    /// Seat number as recorded.
    pub seat: u8,
    /// Display name — public information at any table.
    pub name: String,
    /// Stack at hand start.
    pub starting_stack: f64,
    /// Net chips won (positive) or lost (negative) this hand.
    pub net: f64,
}

/// One public betting action in a [`RedactedHand`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedAction {
    /// Acting player's stable identity.
    pub player_id: Uuid,
    /// Acting player's seat.
    pub seat: u8,
    /// Street the action happened on.
    pub street: RedactedStreet,
    /// The action taken (fold/check/call/bet/raise/post/all-in).
    pub action: ActionType,
    /// Amount wagered, when the action carries one.
    pub amount: Option<f64>,
    /// Whether the actor was all-in after this action.
    pub all_in: bool,
}

/// A single completed hand as an honest observer may see it: public actions
/// and chip movements, with every hole card and the deck structurally
/// removed. **There is no field that can hold a hole card.**
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedHand {
    /// 1-based position of the hand within the recorded session.
    pub hand_no: u32,
    /// Dealer-button seat, when recorded.
    pub button_seat: Option<u8>,
    /// Big-blind amount for this hand.
    pub big_blind: f64,
    /// All seated players and their public per-hand outcomes.
    pub seats: Vec<RedactedSeat>,
    /// Every betting action across all streets, in order.
    pub actions: Vec<RedactedAction>,
    /// Community cards — dealt face-up, therefore public.
    pub board: Option<String>,
}

impl RedactedHand {
    /// Looks up the seat entry for `player_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::redacted::redact;
    /// use pkcore::hand_history::HandCollection;
    ///
    /// let hands = redact(&HandCollection::new());
    /// assert!(hands.is_empty());
    /// ```
    #[must_use]
    pub fn seat_of(&self, player_id: Uuid) -> Option<&RedactedSeat> {
        self.seats.iter().find(|s| s.player_id == player_id)
    }

    /// All player ids dealt into this hand.
    #[must_use]
    pub fn player_ids(&self) -> Vec<Uuid> {
        self.seats.iter().map(|s| s.player_id).collect()
    }
}

/// The ONLY constructor for redacted hands. Consumes a [`HandCollection`],
/// dropping `hole_cards`, `hole_cards_visibility`, `best_hand`, and
/// `shuffled_deck` at the boundary. Once redacted, the cards are gone.
///
/// Hands where any seat lacks a `player_id` (legacy/manual records) are
/// skipped entirely — pairwise detection is meaningless without stable
/// identity.
///
/// The detection API cannot accept the un-redacted collection:
///
/// ```compile_fail
/// use pkcore::hand_history::HandCollection;
/// // signals/detector take &[RedactedHand]; a HandCollection does not coerce.
/// let hands: &[pkdealer_boss::redacted::RedactedHand] = &HandCollection::new();
/// ```
///
/// # Examples
///
/// ```
/// use pkdealer_boss::redacted::redact;
/// use pkcore::hand_history::HandCollection;
///
/// assert!(redact(&HandCollection::new()).is_empty());
/// ```
#[must_use]
pub fn redact(collection: &HandCollection) -> Vec<RedactedHand> {
    collection
        .hands()
        .iter()
        .enumerate()
        .filter_map(|(index, hand)| redact_hand(index, hand))
        .collect()
}

#[allow(clippy::cast_possible_truncation)]
fn redact_hand(index: usize, hand: &HandHistory) -> Option<RedactedHand> {
    let seats: Option<Vec<RedactedSeat>> = hand
        .players
        .iter()
        .map(|p| {
            let net = hand
                .results
                .as_ref()
                .and_then(|rs| rs.iter().find(|r| r.seat == p.seat))
                .and_then(|r| r.net)
                .unwrap_or(0.0);
            p.player_id.map(|player_id| RedactedSeat {
                player_id,
                seat: p.seat,
                name: p.name.clone(),
                starting_stack: p.stack,
                net,
            })
        })
        .collect();
    let seats = seats?; // any missing identity ⇒ skip the hand
    let seat_ids: std::collections::HashMap<u8, Uuid> =
        seats.iter().map(|s| (s.seat, s.player_id)).collect();

    let mut actions = Vec::new();
    if let Some(streets) = &hand.streets {
        let buckets: [(RedactedStreet, Option<&Vec<Action>>); 4] = [
            (RedactedStreet::Preflop, streets.preflop.as_ref().map(|s| &s.actions)),
            (RedactedStreet::Flop, streets.flop.as_ref().map(|s| &s.actions)),
            (RedactedStreet::Turn, streets.turn.as_ref().map(|s| &s.actions)),
            (RedactedStreet::River, streets.river.as_ref().map(|s| &s.actions)),
        ];
        for (street, bucket) in buckets {
            for action in bucket.into_iter().flatten() {
                let Some(player_id) = action.player_id.or_else(|| seat_ids.get(&action.seat).copied())
                else {
                    continue;
                };
                actions.push(RedactedAction {
                    player_id,
                    seat: action.seat,
                    street,
                    action: action.action.clone(),
                    amount: action.amount,
                    all_in: action.all_in.unwrap_or(false),
                });
            }
        }
    }

    Some(RedactedHand {
        hand_no: (index + 1) as u32,
        button_seat: hand.table.button,
        big_blind: hand.table.stakes.big_blind,
        seats,
        actions,
        board: hand.board.clone(),
    })
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss 2>&1 | tail -10`
Expected: all `redacted::tests` PASS + the doc tests (`cargo test --doc -p pkdealer_boss` — run this too).

- [ ] **Step 7: Clippy + workspace sanity**

Run: `cargo clippy -p pkdealer_boss -- -D warnings && cargo test --workspace 2>&1 | tail -5`
Expected: clean; existing crates unaffected.

- [ ] **Step 8: Hand off commit to the user**

Print and wait:

```bash
git add Cargo.toml crates/pkdealer_boss
git commit -m "feat(epic-70): pkdealer_boss crate + RedactedHand typed firewall (Phase 0a/0b)"
```

---

### Task 2: `BossError` + `GroundTruthLabels` sidecar

**Files:**
- Create: `crates/pkdealer_boss/src/error.rs`, `crates/pkdealer_boss/src/labels.rs`
- Modify: `crates/pkdealer_boss/src/lib.rs` (uncomment `pub mod error;` / `pub mod labels;`)

**Interfaces:**
- Consumes: fixtures from Task 1.
- Produces:

```rust
pub enum BossError { Io(std::io::Error), Parse(String), Empty }   // Display + std::error::Error + From<std::io::Error>
pub enum LabelVector { Spectator, Peer }                          // Clone, Copy, serde snake_case
pub enum LabelStyle { SoftPlay, Whipsaw, ChipDump }               // Clone, Copy, serde snake_case
pub struct LabeledPair { pub a: Uuid, pub b: Uuid, pub a_name: String, pub b_name: String, pub vector: LabelVector, pub style: LabelStyle }
pub struct GroundTruthLabels { pub colluding_pairs: Vec<LabeledPair> }
impl GroundTruthLabels {
    pub fn from_yaml(yaml: &str) -> Result<Self, BossError>;
    pub fn to_yaml(&self) -> Result<String, BossError>;
    pub fn is_colluding(&self, x: Uuid, y: Uuid) -> bool;         // order-insensitive
    pub fn resolve(collection: &HandCollection, pairs: &[(String, String, LabelVector, LabelStyle)]) -> Result<Self, BossError>;
}
```

- [ ] **Step 1: Write failing tests** (inside `labels.rs`; error.rs gets construction/Display tests)

```rust
#[test]
fn labels_yaml_roundtrip() {
    let labels = GroundTruthLabels {
        colluding_pairs: vec![LabeledPair {
            a: fixtures::MALLORY, b: fixtures::TRUDY,
            a_name: "mallory_1".into(), b_name: "trudy_1".into(),
            vector: LabelVector::Spectator, style: LabelStyle::ChipDump,
        }],
    };
    let yaml = labels.to_yaml().unwrap();
    assert!(yaml.contains("chip_dump") && yaml.contains("spectator"));
    let back = GroundTruthLabels::from_yaml(&yaml).unwrap();
    assert_eq!(back.colluding_pairs.len(), 1);
    assert_eq!(back.colluding_pairs[0].a, fixtures::MALLORY);
}

#[test]
fn is_colluding_is_order_insensitive() { /* both (a,b) and (b,a) true; (a, GTO) false */ }

#[test]
fn collude_with_resolves_composed_name_to_uuid() {
    // A collection whose latest hand seats gto_1 and gto_2 with distinct ids:
    // resolve [("gto_1","gto_2",…)] maps each composed name to the right Uuid.
    let c = fixtures::collection(vec![fixtures::build_hand(fixtures::HandSpec {
        no: 1,
        players: vec![
            fixtures::player(0, "gto_1", fixtures::GTO, 1_000.0, None),
            fixtures::player(1, "gto_2", fixtures::TAG, 1_000.0, None),
        ],
        preflop: vec![], flop: None, turn: None, river: None,
        nets: vec![(0, 0.0), (1, 0.0)],
    })]);
    let labels = GroundTruthLabels::resolve(
        &c,
        &[("gto_1".into(), "gto_2".into(), LabelVector::Spectator, LabelStyle::SoftPlay)],
    ).unwrap();
    assert_eq!(labels.colluding_pairs[0].a, fixtures::GTO);
    assert_eq!(labels.colluding_pairs[0].b, fixtures::TAG);
}

#[test]
fn resolve_unknown_name_errors() { /* resolve with name "nobody" → Err(BossError::Parse(_)) */ }

#[test]
fn from_yaml_garbage_errors() { assert!(GroundTruthLabels::from_yaml(": not yaml [").is_err()); }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_boss labels` → compile error (module missing).

- [ ] **Step 3: Implement**

`src/error.rs`:

```rust
//! Error type for the Boss pipeline.

/// Errors surfaced by the `pkdealer_boss` library and CLI.
#[derive(Debug)]
pub enum BossError {
    /// Reading a session or labels file failed.
    Io(std::io::Error),
    /// A session or labels payload failed to parse.
    Parse(String),
    /// The session contained no attributable hands.
    Empty,
}

impl std::fmt::Display for BossError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BossError::Io(e) => write!(f, "io error: {e}"),
            BossError::Parse(msg) => write!(f, "parse error: {msg}"),
            BossError::Empty => write!(f, "session contains no attributable hands"),
        }
    }
}

impl std::error::Error for BossError {}

impl From<std::io::Error> for BossError {
    fn from(e: std::io::Error) -> Self {
        BossError::Io(e)
    }
}
```

(Plus doc comments with a small doc test constructing `BossError::Empty` and asserting its Display string, and unit tests per house rules.)

`src/labels.rs` — implement per the interface. Key bodies:

```rust
pub fn from_yaml(yaml: &str) -> Result<Self, BossError> {
    serde_yaml_bw::from_str(yaml).map_err(|e| BossError::Parse(e.to_string()))
}

pub fn to_yaml(&self) -> Result<String, BossError> {
    serde_yaml_bw::to_string(self).map_err(|e| BossError::Parse(e.to_string()))
}

#[must_use]
pub fn is_colluding(&self, x: Uuid, y: Uuid) -> bool {
    self.colluding_pairs
        .iter()
        .any(|p| (p.a == x && p.b == y) || (p.a == y && p.b == x))
}

pub fn resolve(
    collection: &HandCollection,
    pairs: &[(String, String, LabelVector, LabelStyle)],
) -> Result<Self, BossError> {
    // Latest-hand-wins name → id map (mirrors seat_ids_from_collection in agent_rules).
    let mut by_name: std::collections::HashMap<&str, Uuid> = std::collections::HashMap::new();
    for hand in collection.hands() {
        for p in &hand.players {
            if let Some(id) = p.player_id {
                by_name.insert(p.name.as_str(), id);
            }
        }
    }
    let mut colluding_pairs = Vec::with_capacity(pairs.len());
    for (a_name, b_name, vector, style) in pairs {
        let a = *by_name.get(a_name.as_str())
            .ok_or_else(|| BossError::Parse(format!("unknown player name: {a_name}")))?;
        let b = *by_name.get(b_name.as_str())
            .ok_or_else(|| BossError::Parse(format!("unknown player name: {b_name}")))?;
        colluding_pairs.push(LabeledPair {
            a, b, a_name: a_name.clone(), b_name: b_name.clone(),
            vector: *vector, style: *style,
        });
    }
    Ok(Self { colluding_pairs })
}
```

Derives: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]` on structs, `+ Copy, Eq` on the two enums, `#[serde(rename_all = "snake_case")]` on the enums. Doc comments + doc tests on every public item (YAML roundtrip is the natural doc test).

- [ ] **Step 4: Run tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss && cargo test --doc -p pkdealer_boss` → PASS.
- [ ] **Step 5: Clippy** — `cargo clippy -p pkdealer_boss -- -D warnings` → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_boss
git commit -m "feat(epic-70): GroundTruthLabels UUID-keyed sidecar + BossError (Phase 0c)"
```

---

### Task 3: Thread `hand_no` onto `HandState` (Phase 0d)

**Files:**
- Modify: `crates/pkdealer_agent_core/src/hand_state.rs` (struct + doc example + tests), `crates/pkdealer_agent_core/src/runner.rs:294-306` (`decide_and_act`), `crates/pkdealer_agent_rules/src/main.rs` (`sample_state`), `crates/pkdealer_agent_random/src/main.rs:119-150` (two fixtures), `crates/pkdealer_agent_llm/src/agent.rs` (doc example line ~48 + `sample_state`), `crates/pkdealer_agent_llm/src/prompt.rs` (doc examples lines ~19/~92 + `sample_state` + any full literals the compiler flags)

**Interfaces:**
- Produces: `HandState.hand_no: u32` — the dealer's `TableStatus.round_number` for the hand in play; `0` means unknown (e.g. hand-built fixtures).

- [ ] **Step 1: Write the failing test** (append to `hand_state.rs` tests)

```rust
#[test]
fn hand_state_carries_hand_no() {
    let state = HandState { hand_no: 42, ..sample_state() };
    assert_eq!(state.hand_no, 42);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_agent_core hand_state_carries_hand_no` → "no field `hand_no`".

- [ ] **Step 3: Add the field**

In `hand_state.rs`, after `button_seat` in the struct:

```rust
    /// Monotonic hand number from the dealer (`TableStatus.round_number`,
    /// 1-based). `0` when unknown — e.g. states built outside a live hand.
    /// Lets colluding peers and recorders agree on *which* hand an
    /// observation belongs to (EPIC-70 Phase 0d).
    pub hand_no: u32,
```

Update the struct's doc example and `sample_state()` in the same file with `hand_no: 0,`.

In `runner.rs` `decide_and_act`, the `HandState` literal gains:

```rust
        hand_no: status.round_number,
```

- [ ] **Step 4: Fix every other construction site**

Run: `cargo check --workspace 2>&1 | grep -B2 "missing field"`
Add `hand_no: 0,` to each flagged literal — known sites: `pkdealer_agent_rules/src/main.rs` `sample_state`, `pkdealer_agent_random/src/main.rs` `state_with_call`/`state_no_call`, `pkdealer_agent_llm/src/agent.rs` (doc example + `sample_state`), `pkdealer_agent_llm/src/prompt.rs` (2 doc examples + `sample_state` + any others the compiler names). Fixtures that use `..sample_state()` need no change.

- [ ] **Step 5: Run the full suite**

Run: `OTEL_SDK_DISABLED=true cargo test --workspace 2>&1 | tail -5`
Expected: PASS everywhere — behavior identical, only the new field exists.

- [ ] **Step 6: Clippy** — `cargo clippy --workspace -- -D warnings` → clean.
- [ ] **Step 7: Hand off commit**

```bash
git add crates/pkdealer_agent_core crates/pkdealer_agent_rules crates/pkdealer_agent_random crates/pkdealer_agent_llm
git commit -m "feat(epic-70): thread dealer round_number onto HandState as hand_no (Phase 0d)"
```

---

### Task 4: `collusion` feature gates + Dockerfile `FEATURES` build-arg (Phase 0e)

**Files:**
- Modify: `crates/pkdealer_agent_rules/Cargo.toml`, `crates/pkdealer_agent_core/Cargo.toml`, `Dockerfile.agent`

- [ ] **Step 1: Declare the features**

Append to both Cargo.tomls:

```toml
[features]
# EPIC-70: colluding-agent machinery. Off by default — a default build is
# byte-for-byte the honest agent.
collusion = []
```

- [ ] **Step 2: Add the Docker build-arg** (so `bin/arena` can build colluding images in Task 5)

In `Dockerfile.agent`, builder stage — change:

```dockerfile
ARG BIN_NAME
RUN test -n "$BIN_NAME" || (echo "BIN_NAME build arg is required" && exit 1)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --bin "$BIN_NAME" --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin "$BIN_NAME"
```

to:

```dockerfile
ARG BIN_NAME
# Optional cargo feature list (e.g. "collusion" for EPIC-70 colluding agents).
ARG FEATURES=""
RUN test -n "$BIN_NAME" || (echo "BIN_NAME build arg is required" && exit 1)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --bin "$BIN_NAME" ${FEATURES:+--features "$FEATURES"} --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin "$BIN_NAME" ${FEATURES:+--features "$FEATURES"}
```

- [ ] **Step 3: Verify both build flavors**

Run: `cargo check -p pkdealer_agent_rules && cargo check -p pkdealer_agent_rules --features collusion && cargo check -p pkdealer_agent_core --features collusion`
Expected: all clean (the feature is empty for now).

- [ ] **Step 4: Hand off commit**

```bash
git add crates/pkdealer_agent_rules/Cargo.toml crates/pkdealer_agent_core/Cargo.toml Dockerfile.agent
git commit -m "feat(epic-70): collusion cargo feature + Dockerfile FEATURES build-arg (Phase 0e)"
```

---

### Task 5: `arena.toml` `team` field + `bin/arena` pairwise expansion (Phase 0f)

**Files:**
- Modify: `arena.toml`, `bin/arena`
- Create: `tests/arena_team.sh` (executable)

**Interfaces:**
- Produces: colluding compose services whose command is
  `["--name", "<id>", "--profile", "<p>", "--collude-with", "<partner_id>", "--collusion-channel", "spectator", "--collusion-style", "<soft|whipsaw|dump>"]`,
  image `pkdealer/agent_rules_collusion:latest`, build arg `FEATURES: collusion`. Honest seats and team-less lineups emit byte-identical output to today.

- [ ] **Step 1: Write the failing shell test** — `tests/arena_team.sh`:

```bash
#!/usr/bin/env bash
# EPIC-70 Phase 0f: team → pairwise collusion flag expansion (dry-run only).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "FAIL: $*" >&2; exit 1; }

out="$(./bin/arena --dry-run mallory trudy gto)"
override="$(sed -n 's/^Override file: //p' <<<"$out")"
[[ -f "$override" ]] || fail "no override file emitted"

grep -q -- '"--collude-with", "trudy_1"' "$override"   || fail "mallory_1 lacks partner flag"
grep -q -- '"--collude-with", "mallory_1"' "$override" || fail "trudy_1 lacks partner flag"
grep -q -- '"--collusion-channel", "spectator"' "$override" || fail "channel flag missing"
grep -q -- '"--collusion-style", "soft"' "$override"   || fail "style flag missing"
grep -q -- 'agent_rules_collusion' "$override"         || fail "colluding image not used"
grep -q -- 'FEATURES: collusion' "$override"           || fail "FEATURES build arg missing"

gto_cmd="$(awk '/^  agent_gto_1:/{f=1} f && /command:/{print; exit}' "$override")"
[[ "$gto_cmd" != *collude* ]] || fail "honest seat gto_1 carries collusion flags"

out2="$(./bin/arena --dry-run gto lag)"
override2="$(sed -n 's/^Override file: //p' <<<"$out2")"
grep -q -- '--collude-with' "$override2" && fail "team-less lineup emitted collusion flags"

echo "OK: arena team expansion"
```

Run: `chmod +x tests/arena_team.sh && ./tests/arena_team.sh`
Expected: FAIL — `mallory` is an unknown player.

- [ ] **Step 2: Add the registry entries + schema docs to `arena.toml`**

Extend the header comment block (after the `type` explanation, line ~15):

```toml
# `team` (optional, rules-only) — EPIC-70 collusion: the two seats sharing a
#   team id secretly collude. bin/arena expands membership into explicit
#   --collude-with / --collusion-channel / --collusion-style flags on each
#   member (pairs only; a team with ≠2 seats is rejected).
# `style` (optional, teamed seats) — collusion strategy: soft | whipsaw | dump
#   (default soft).
```

Append the colluder entries (after `[players.joker]`):

```toml
# EPIC-70 colluding pair — mallory & trudy secretly share hole cards via the
# spectator token. Seat them with honest bots: ./bin/arena mallory trudy gto lag
[players.mallory]
type    = "rules"
profile = "tight_aggressive"
team    = "A"

[players.trudy]
type    = "rules"
profile = "loose_aggressive"
team    = "A"
```

- [ ] **Step 3: Implement the expansion in `bin/arena`**

3a. Replace the emission loop (lines 273-286, the `uniq_names` dedup + nested emit loop) with a two-pass structure. Keep the dedup; insert between it and emission:

```bash
# ── EPIC-70: expand `team` membership into pairwise collusion flags ──────────
# First pass: composed instance ids in emission order, with team + registry name.
composed_ids=(); composed_regnames=(); composed_teams=()
for name in "${uniq_names[@]}"; do
  k="$(printf '%s\n' "${players[@]}" | grep -cx -- "$name" || true)"
  team="$(registry_field "$name" team)"
  if [[ -n "$team" && "$(registry_field "$name" type)" != rules ]]; then
    echo "❌ '$name' declares team '$team' but is not a rules agent — colluders are rules-only (EPIC-70)." >&2
    exit 1
  fi
  for ((n = 1; n <= k; n++)); do
    composed_ids+=("${name}_${n}")
    composed_regnames+=("$name")
    composed_teams+=("${team:-}")
  done
done

# Every declared team must field exactly two seats (pairs only in EPIC-70).
while IFS= read -r team; do
  [[ -n "$team" ]] || continue
  count=0
  for tm in "${composed_teams[@]}"; do [[ "$tm" == "$team" ]] && count=$((count + 1)); done
  if (( count != 2 )); then
    echo "❌ team '$team' has $count seat(s) in this lineup; collusion teams are pairs (exactly 2)." >&2
    exit 1
  fi
done < <(printf '%s\n' "${composed_teams[@]}" | awk 'NF && !seen[$0]++')

# collusion_partner_for <id> — prints "<partner_id> <style>" when <id> is teamed.
collusion_partner_for() {
  local id="$1" i j style
  for ((i = 0; i < ${#composed_ids[@]}; i++)); do
    [[ "${composed_ids[$i]}" == "$id" && -n "${composed_teams[$i]}" ]] || continue
    for ((j = 0; j < ${#composed_ids[@]}; j++)); do
      if (( j != i )) && [[ "${composed_teams[$j]}" == "${composed_teams[$i]}" ]]; then
        style="$(registry_field "${composed_regnames[$i]}" style)"
        printf '%s %s\n' "${composed_ids[$j]}" "${style:-soft}"
        return 0
      fi
    done
  done
  return 1
}

for ((idx = 0; idx < ${#composed_ids[@]}; idx++)); do
  emit_service "${composed_ids[$idx]}" "${composed_regnames[$idx]}"
done
```

(The old `for name in "${uniq_names[@]}" … emit_service` loop is deleted — emission order is unchanged.)

3b. In `emit_service`, extend the `rules` arm. At the top of the function, after `t="$(registry_field "$name" type)"`, resolve collusion before the image is printed:

```bash
  local collude_partner="" collude_style=""
  if [[ "$t" == rules ]]; then
    local partner_style
    if partner_style="$(collusion_partner_for "$id")"; then
      collude_partner="${partner_style%% *}"
      collude_style="${partner_style##* }"
    fi
  fi
```

Change the `rules` image assignment to honor collusion:

```bash
    rules)
      bin=pkdealer_agent_rules
      if [[ -n "$collude_partner" ]]; then
        image=pkdealer/agent_rules_collusion:latest
      else
        image=pkdealer/agent_rules:latest
      fi
      ;;
```

In the build-args block, after `printf '        BIN_NAME: %s\n' "$bin"`:

```bash
    if [[ -n "$collude_partner" ]]; then
      printf '        FEATURES: collusion\n'
    fi
```

And the command line for rules:

```bash
    if [[ "$t" == rules ]]; then
      profile="$(registry_field "$name" profile)"
      if [[ -n "$collude_partner" ]]; then
        printf '    command: ["--name", "%s", "--profile", "%s", "--collude-with", "%s", "--collusion-channel", "spectator", "--collusion-style", "%s"]\n' \
          "$id" "$profile" "$collude_partner" "$collude_style"
      else
        printf '    command: ["--name", "%s", "--profile", "%s"]\n' "$id" "$profile"
      fi
    else
```

- [ ] **Step 4: Run the shell test** — `./tests/arena_team.sh` → `OK: arena team expansion`.
- [ ] **Step 5: Regression-check honest output** — `./bin/arena --dry-run gto lag` and diff the override against one generated from `git stash`-free HEAD is not possible without git; instead verify by eye that a team-less override contains no `collude`, no `FEATURES:`, no `agent_rules_collusion` (the shell test's second half already asserts this).
- [ ] **Step 6: Hand off commit**

```bash
git add arena.toml bin/arena tests/arena_team.sh
git commit -m "feat(epic-70): arena.toml team field + bin/arena pairwise collusion expansion (Phase 0f)"
```

> Note: `bin/arena` may be gitignored (EPIC-42 needed `git add -f bin/arena` historically) — if `git add` is a no-op, tell the user to use `git add -f bin/arena`.

---

### Task 6: `CollusionConfig` + CLI flags on `pkdealer_agent_rules` (Phase 1a)

**Files:**
- Create: `crates/pkdealer_agent_rules/src/collude/mod.rs`
- Modify: `crates/pkdealer_agent_rules/src/main.rs` (Args + arg enums + `validate_collusion`)

**Interfaces:**
- Produces (feature `collusion` only):

```rust
// collude/mod.rs
pub struct CollusionConfig { pub partner: String, pub channel: CollusionChannel, pub style: CollusionStyle }
pub enum CollusionChannel { Spectator, Peer }
pub use strategy::CollusionStyle;            // added in Task 8; for now declare CollusionStyle here and move it in Task 8 — NO: to avoid churn, create collude/strategy.rs in THIS task containing only the enum, and mod.rs re-exports it.
// main.rs
fn validate_collusion(args: &Args) -> Result<Option<CollusionConfig>, String>;   // #[cfg(feature = "collusion")]
```

- [ ] **Step 1: Write failing tests** (append to `main.rs` tests, all `#[cfg(feature = "collusion")]`)

```rust
#[cfg(feature = "collusion")]
mod collusion_args {
    use super::super::*;

    #[test]
    fn args_without_collude_with_yield_no_config() {
        let args = Args::try_parse_from(["pkdealer_agent_rules"]).expect("parse");
        assert!(validate_collusion(&args).expect("valid").is_none());
    }

    #[test]
    fn args_parse_collusion_flags() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules", "--name", "mallory_1",
            "--collude-with", "trudy_1", "--collusion-style", "dump",
        ]).expect("parse");
        let config = validate_collusion(&args).expect("valid").expect("config");
        assert_eq!(config.partner, "trudy_1");
        assert_eq!(config.channel, CollusionChannel::Spectator);
        assert_eq!(config.style, CollusionStyle::Dump);
    }

    #[test]
    fn peer_channel_is_rejected_until_phase_3() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules", "--collude-with", "trudy_1", "--collusion-channel", "peer",
        ]).expect("parse");
        assert!(validate_collusion(&args).is_err());
    }

    #[test]
    fn colluding_with_yourself_is_rejected() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules", "--name", "x", "--collude-with", "x",
        ]).expect("parse");
        assert!(validate_collusion(&args).is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_agent_rules --features collusion collusion_args` → compile error.

- [ ] **Step 3: Implement**

`src/collude/mod.rs`:

```rust
//! EPIC-70 collusion machinery (feature `collusion`): configuration, the
//! Vector-A spectator leak, and the decision-adjusting strategies. Cheating
//! is strictly additive — with no [`CollusionConfig`], the agent is
//! byte-for-byte the honest bot.

pub mod strategy;

pub use strategy::CollusionStyle;

/// How partner hole cards reach this agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollusionChannel {
    /// Vector A: read the partner's cards live via the spectator token.
    Spectator,
    /// Vector B: peer backchannel (EPIC-70 Phase 3 — not yet implemented).
    Peer,
}

/// A resolved, validated collusion assignment for this agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollusionConfig {
    /// Partner's arena-composed display name (unique, e.g. `trudy_1`).
    pub partner: String,
    /// Card-leak channel.
    pub channel: CollusionChannel,
    /// Decision-adjustment strategy.
    pub style: CollusionStyle,
}
```

`src/collude/strategy.rs` (this task: enum only; Task 8 adds behavior):

```rust
//! Collusion styles — pure adjustments over the honest decider (Task 8).

/// Ways a colluding pair exploits shared hole cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollusionStyle {
    /// Never bet/raise into the partner heads-up; check/call down.
    Soft,
    /// Re-raise behind the partner's raise to squeeze third parties out.
    Whipsaw,
    /// Fold the weaker team hand to concentrate chips on the partner.
    Dump,
}
```

`src/main.rs` additions:

```rust
#[cfg(feature = "collusion")]
mod collude;
#[cfg(feature = "collusion")]
use collude::{CollusionChannel, CollusionConfig, CollusionStyle};
```

Args fields (after `spectator_token`):

```rust
    /// Collude with the named partner (arena-composed name, e.g. `trudy_1`).
    /// EPIC-70: enables the cheating wrapper; requires a spectator token for
    /// the `spectator` channel. Absent ⇒ the agent is fully honest.
    #[cfg(feature = "collusion")]
    #[arg(long)]
    collude_with: Option<String>,

    /// Card-leak channel: `spectator` (Vector A) or `peer` (Vector B, Phase 3).
    #[cfg(feature = "collusion")]
    #[arg(long, value_enum, default_value = "spectator")]
    collusion_channel: CollusionChannelArg,

    /// Collusion strategy: `soft`, `whipsaw`, or `dump`.
    #[cfg(feature = "collusion")]
    #[arg(long, value_enum, default_value = "soft")]
    collusion_style: CollusionStyleArg,
```

CLI mirrors (same pattern as `EquityArg`):

```rust
/// CLI mirror of [`CollusionChannel`].
#[cfg(feature = "collusion")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CollusionChannelArg { Spectator, Peer }

/// CLI mirror of [`CollusionStyle`].
#[cfg(feature = "collusion")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CollusionStyleArg { Soft, Whipsaw, Dump }
```

Validation:

```rust
/// Resolves and validates the collusion flags into a [`CollusionConfig`].
///
/// Mirrors the `--exploit` validation posture: configuration errors are
/// fatal at startup (a colluder that cannot leak is a broken experiment,
/// not a degraded bot).
#[cfg(feature = "collusion")]
fn validate_collusion(args: &Args) -> Result<Option<CollusionConfig>, String> {
    let Some(partner) = args.collude_with.clone() else {
        return Ok(None);
    };
    if partner == args.name {
        return Err("--collude-with must name a different player".to_string());
    }
    let channel = match args.collusion_channel {
        CollusionChannelArg::Spectator => CollusionChannel::Spectator,
        CollusionChannelArg::Peer => {
            return Err(
                "--collusion-channel peer is the EPIC-70 Phase 3 backchannel — not yet implemented"
                    .to_string(),
            );
        }
    };
    if args.spectator_token.is_empty() {
        return Err("the spectator collusion channel requires --spectator-token".to_string());
    }
    let style = match args.collusion_style {
        CollusionStyleArg::Soft => CollusionStyle::Soft,
        CollusionStyleArg::Whipsaw => CollusionStyle::Whipsaw,
        CollusionStyleArg::Dump => CollusionStyle::Dump,
    };
    Ok(Some(CollusionConfig { partner, channel, style }))
}
```

- [ ] **Step 4: Run tests both ways**

Run: `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion 2>&1 | tail -5` → PASS (new + existing).
Run: `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules 2>&1 | tail -5` → PASS (feature off, untouched).

- [ ] **Step 5: Clippy both ways** — `cargo clippy -p pkdealer_agent_rules -- -D warnings && cargo clippy -p pkdealer_agent_rules --features collusion -- -D warnings` → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_agent_rules
git commit -m "feat(epic-70): CollusionConfig + collusion CLI flags on rules agent (Phase 1a)"
```

---

### Task 7: `SpectatorLeak` — Vector A partner-card puller (Phase 1b)

**Files:**
- Create: `crates/pkdealer_agent_rules/src/collude/spectator.rs`
- Modify: `crates/pkdealer_agent_rules/src/collude/mod.rs` (`pub mod spectator;`)

**Interfaces:**
- Consumes: `PLAYER_TOKEN_METADATA_KEY` (`main.rs:88`), `DealerServiceClient`, `GetStatusRequest`/`TableStatus` from `pkdealer_proto::dealer`.
- Produces:

```rust
pub struct SpectatorLeak { /* client: Mutex<DealerServiceClient<Channel>>, token: String, partner: String */ }
impl SpectatorLeak {
    pub async fn connect(endpoint: String, token: String, partner: String) -> Result<Self, String>;
    pub async fn partner_hole(&self) -> Option<Cards>;   // live read, per decision
}
pub(crate) fn extract_partner_cards(status: &TableStatus, partner: &str) -> Option<String>;  // the honor filter, pure
```

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pkdealer_proto::dealer::{SeatInfo, TableStatus};

    fn seat(name: &str, cards: &str) -> SeatInfo {
        SeatInfo {
            seat_number: 0,
            player_name: name.to_string(),
            cards: cards.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn extracts_partner_cards_only() {
        let status = TableStatus {
            seats: vec![seat("mallory_1", "Ah Kd"), seat("trudy_1", "Qs Qc"), seat("gto_1", "7d 2c")],
            ..Default::default()
        };
        // Honor filter: only the partner's cards come out, ever.
        assert_eq!(extract_partner_cards(&status, "trudy_1").as_deref(), Some("Qs Qc"));
    }

    #[test]
    fn absent_partner_yields_none() {
        let status = TableStatus { seats: vec![seat("gto_1", "7d 2c")], ..Default::default() };
        assert!(extract_partner_cards(&status, "trudy_1").is_none());
    }

    #[test]
    fn empty_cards_yield_none() {
        // Between hands (or if the token was rejected and cards were redacted)
        // the partner's seat carries no cards — never fabricate a read.
        let status = TableStatus { seats: vec![seat("trudy_1", "")], ..Default::default() };
        assert!(extract_partner_cards(&status, "trudy_1").is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_agent_rules --features collusion extract` → compile error.

- [ ] **Step 3: Implement `spectator.rs`**

```rust
//! Vector A (`SpectatorLeak`): reads the partner's live hole cards through
//! the spectator token, on a dedicated second connection — the same
//! connection + token-injection pattern as `ExploitPuller`, but reading
//! *live* card state on every decision instead of completed-hand history.
//!
//! **Honor filter (load-bearing for A/B equivalence):** the spectator view
//! exposes *every* seat's cards; [`extract_partner_cards`] discards all but
//! the partner's at ingest, collapsing Vector A's information position to
//! Vector B's. See EPIC-70 → Scope.

use pkcore::cards::Cards;
use pkdealer_proto::dealer::dealer_service_client::DealerServiceClient;
use pkdealer_proto::dealer::{GetStatusRequest, TableStatus};
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::PLAYER_TOKEN_METADATA_KEY;

/// Live partner-card reader over the spectator token (Vector A).
pub struct SpectatorLeak {
    /// Dedicated connection, separate from the play connection.
    client: Mutex<DealerServiceClient<Channel>>,
    /// Spectator token injected into request metadata.
    token: String,
    /// Partner's arena-composed display name.
    partner: String,
}

impl SpectatorLeak {
    /// Opens the dedicated spectator connection.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the endpoint is unreachable —
    /// the caller exits: a colluder that cannot leak is a broken experiment.
    pub async fn connect(endpoint: String, token: String, partner: String) -> Result<Self, String> {
        match DealerServiceClient::connect(endpoint).await {
            Ok(client) => Ok(Self { client: Mutex::new(client), token, partner }),
            Err(e) => Err(format!("spectator-leak connection failed: {e}")),
        }
    }

    /// Reads the partner's current hole cards, or `None` when unavailable
    /// (between hands, transport error, redacted view). Best-effort per
    /// decision — a missed read means the agent decides honestly this turn.
    pub async fn partner_hole(&self) -> Option<Cards> {
        let mut request = tonic::Request::new(GetStatusRequest {});
        request
            .metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, self.token.parse().ok()?);
        let status = {
            let mut client = self.client.lock().await;
            client.get_status(request).await.ok()?.into_inner().status?
        };
        extract_partner_cards(&status, &self.partner).map(|s| Cards::forgiving_from_str(&s))
    }
}

/// The honor filter: pulls **only** the named partner's cards out of a
/// spectator-visible status, discarding every other seat's at ingest.
pub(crate) fn extract_partner_cards(status: &TableStatus, partner: &str) -> Option<String> {
    status
        .seats
        .iter()
        .find(|s| s.player_name == partner)
        .map(|s| s.cards.clone())
        .filter(|cards| !cards.is_empty())
}
```

In `main.rs`, make the metadata key visible to the module: change `const PLAYER_TOKEN_METADATA_KEY` to `pub(crate) const PLAYER_TOKEN_METADATA_KEY`.

- [ ] **Step 4: Run tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion 2>&1 | tail -5` → PASS.
- [ ] **Step 5: Clippy both ways** (same commands as Task 6 Step 5) → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_agent_rules
git commit -m "feat(epic-70): SpectatorLeak partner-card puller with honor filter (Phase 1b)"
```

---

### Task 8: Collusion strategies — soft/whipsaw/dump (Phase 1c/1d)

**Files:**
- Modify: `crates/pkdealer_agent_rules/src/collude/strategy.rs`

**Interfaces:**
- Consumes: `TableSnapshot`/`SeatInfo` (pkcore), `PlayerAction`, `Cards`, `Two`, `Five`/`Six`/`Seven` + `HandRanker`.
- Produces:

```rust
pub fn apply_style(style: CollusionStyle, base: PlayerAction, snap: &TableSnapshot<'_>, partner_seat: u8, partner_hole: &Cards) -> PlayerAction;
```

- [ ] **Step 1: Write failing tests** (in `strategy.rs`; snapshots built via `crate::hand_state_to_snapshot` and `HandState` fixtures — the collude module is part of the same binary crate)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hand_state_to_snapshot;
    use pkcore::bot::player_action::PlayerAction;
    use pkdealer_agent_core::{HandState, SeatSnapshot};

    fn seat(seat: u8, name: &str, chips: u32, bet: u32, active: bool) -> SeatSnapshot {
        SeatSnapshot { seat, name: name.to_string(), chips, bet, is_active: active }
    }

    /// Hero at seat 0 with `hole`; opponents as given.
    fn state(hole: &str, board: &str, to_call: u32, others: Vec<SeatSnapshot>) -> HandState {
        let mut stacks = vec![seat(0, "mallory_1", 10_000, 0, true)];
        stacks.extend(others);
        HandState {
            seat: 0,
            hole_cards: hole.to_string(),
            board: board.to_string(),
            pot: 600,
            to_call,
            my_chips: 10_000,
            stacks,
            big_blind: 100,
            street: if board.is_empty() { "preflop".into() } else { "flop".into() },
            action_history: vec![],
            button_seat: Some(0),
            hand_no: 7,
        }
    }

    #[test]
    fn soft_play_never_raises_partner_heads_up() {
        // Partner (seat 1) is the only live opponent; a raising hand checks back.
        let s = state("Ah Kd", "Ac Kc 2d", 0, vec![seat(1, "trudy_1", 9_000, 0, true)]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Soft, PlayerAction::Bet(400), &snap, 1, &pkcore::cards::Cards::forgiving_from_str("Qs Qc"));
        assert_eq!(adjusted, PlayerAction::Check);
    }

    #[test]
    fn soft_play_calls_when_facing_partner_bet() {
        let s = state("Ah Kd", "Ac Kc 2d", 300, vec![seat(1, "trudy_1", 9_000, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Soft, PlayerAction::Raise(900), &snap, 1, &pkcore::cards::Cards::forgiving_from_str("Qs Qc"));
        assert_eq!(adjusted, PlayerAction::Call);
    }

    #[test]
    fn colluder_softplays_partner_only() {
        // Same made hand, but the live opponent is NOT the partner → base stands.
        let s = state("Ah Kd", "Ac Kc 2d", 0, vec![
            seat(1, "trudy_1", 9_000, 0, false),  // partner folded
            seat(2, "gto_1", 9_000, 0, true),
        ]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Soft, PlayerAction::Bet(400), &snap, 1, &pkcore::cards::Cards::forgiving_from_str("Qs Qc"));
        assert_eq!(adjusted, PlayerAction::Bet(400));
    }

    #[test]
    fn whipsaw_squeezes_third_party() {
        // Partner leads the street, a victim is still live, base would call →
        // re-raise to squeeze.
        let s = state("9h 8h", "", 300, vec![
            seat(1, "trudy_1", 9_700, 300, true),  // partner raised to 300
            seat(2, "gto_1", 9_900, 100, true),    // victim in the middle
        ]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Whipsaw, PlayerAction::Call, &snap, 1, &pkcore::cards::Cards::forgiving_from_str("As Ac"));
        assert!(matches!(adjusted, PlayerAction::Raise(_)), "got {adjusted:?}");
    }

    #[test]
    fn whipsaw_without_third_party_leaves_base() {
        let s = state("9h 8h", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        assert_eq!(
            apply_style(CollusionStyle::Whipsaw, PlayerAction::Call, &snap, 1, &pkcore::cards::Cards::forgiving_from_str("As Ac")),
            PlayerAction::Call
        );
    }

    #[test]
    fn chip_dump_folds_strong_to_partner() {
        // Hero holds KK (strong) but partner's committed AA is stronger on a
        // full board → fold rather than pay off... rather than WIN off the partner.
        let s = state("Kh Kd", "2d 7c 9s Jd 3h", 400, vec![seat(1, "trudy_1", 9_000, 400, true)]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Dump, PlayerAction::Call, &snap, 1, &pkcore::cards::Cards::forgiving_from_str("As Ah"));
        assert_eq!(adjusted, PlayerAction::Fold);
    }

    #[test]
    fn colluder_folds_worse_team_hand() {
        // Preflop: hero 72o vs partner's committed AA → weaker team hand folds.
        let s = state("7d 2c", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(CollusionStyle::Dump, PlayerAction::Call, &snap, 1, &pkcore::cards::Cards::forgiving_from_str("As Ah"));
        assert_eq!(adjusted, PlayerAction::Fold);
    }

    #[test]
    fn chip_dump_keeps_base_when_hero_is_stronger() {
        let s = state("As Ah", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        assert_eq!(
            apply_style(CollusionStyle::Dump, PlayerAction::Call, &snap, 1, &pkcore::cards::Cards::forgiving_from_str("Kh Kd")),
            PlayerAction::Call
        );
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_agent_rules --features collusion strategy` → `apply_style` not found.

- [ ] **Step 3: Implement** (extend `strategy.rs`)

```rust
use pkcore::arrays::HandRanker;
use pkcore::arrays::five::Five;
use pkcore::arrays::seven::Seven;
use pkcore::arrays::six::Six;
use pkcore::arrays::two::Two;
use pkcore::bot::player_action::PlayerAction;
use pkcore::bot::table_snapshot::{SeatInfo, TableSnapshot};
use pkcore::cards::Cards;
use std::str::FromStr;

/// Applies a collusion style on top of the honest decider's `base` action.
/// Pure function of `(style, base, snapshot, partner_seat, partner_hole)` —
/// the channel that delivered `partner_hole` is irrelevant (A/B equivalence).
#[must_use]
pub fn apply_style(
    style: CollusionStyle,
    base: PlayerAction,
    snap: &TableSnapshot<'_>,
    partner_seat: u8,
    partner_hole: &Cards,
) -> PlayerAction {
    match style {
        CollusionStyle::Soft => soft_play(base, snap, partner_seat),
        CollusionStyle::Whipsaw => whipsaw(base, snap, partner_seat),
        CollusionStyle::Dump => chip_dump(base, snap, partner_seat, partner_hole),
    }
}

/// Live opponents (contesting the pot, not the hero).
fn live_opponents<'a>(snap: &'a TableSnapshot<'_>) -> impl Iterator<Item = &'a SeatInfo> {
    snap.stacks.iter().filter(|s| s.is_active && s.seat != snap.seat)
}

/// SoftPlay: never bet or raise into a pot where the partner is the only
/// other live player — check/call down instead.
fn soft_play(base: PlayerAction, snap: &TableSnapshot<'_>, partner_seat: u8) -> PlayerAction {
    let mut live = live_opponents(snap);
    let heads_up_with_partner =
        matches!((live.next(), live.next()), (Some(s), None) if s.seat == partner_seat);
    if !heads_up_with_partner {
        return base;
    }
    match base {
        PlayerAction::Bet(_) | PlayerAction::Raise(_) | PlayerAction::AllIn => {
            if snap.to_call > 0 { PlayerAction::Call } else { PlayerAction::Check }
        }
        other => other,
    }
}

/// Whipsaw: when the partner leads the betting this street and a third
/// party is still live, escalate a passive base action into a re-raise to
/// squeeze the victim out.
fn whipsaw(base: PlayerAction, snap: &TableSnapshot<'_>, partner_seat: u8) -> PlayerAction {
    let Some(partner) = snap.stacks.iter().find(|s| s.seat == partner_seat && s.is_active) else {
        return base;
    };
    let top_bet = snap.stacks.iter().map(|s| s.bet).max().unwrap_or(0);
    let partner_leads = partner.bet > 0 && partner.bet == top_bet && partner.bet > snap.big_blind;
    let third_party_live = live_opponents(snap).any(|s| s.seat != partner_seat);
    if partner_leads && third_party_live && matches!(base, PlayerAction::Check | PlayerAction::Call)
    {
        return PlayerAction::Raise(snap.current_bet + snap.min_raise);
    }
    base
}

/// ChipDump: when the partner is committed with the stronger team hand,
/// fold (or check when folding is free money) to concentrate the team's
/// equity on the partner.
fn chip_dump(
    base: PlayerAction,
    snap: &TableSnapshot<'_>,
    partner_seat: u8,
    partner_hole: &Cards,
) -> PlayerAction {
    let Some(partner) = snap.stacks.iter().find(|s| s.seat == partner_seat && s.is_active) else {
        return base;
    };
    let committed = partner.bet > 0 || (partner.chips == 0 && partner.is_active);
    if !committed {
        return base;
    }
    let (Some(hero), Some(villain)) = (
        strength(&snap.hole_cards, &snap.board),
        strength(partner_hole, &snap.board),
    ) else {
        return base;
    };
    if villain.beats(&hero) {
        if snap.to_call > 0 { PlayerAction::Fold } else { PlayerAction::Check }
    } else {
        base
    }
}

/// Comparable hand strength. Postflop uses pkcore's Cactus-Kev rank value
/// (LOWER wins); preflop uses a crude deterministic proxy (HIGHER wins,
/// pairs above unpaired hands). The two forms never compare across streets
/// — both team hands always share the same board.
enum Strength {
    Postflop(u16),
    Preflop(u32),
}

impl Strength {
    fn beats(&self, other: &Strength) -> bool {
        match (self, other) {
            (Strength::Postflop(a), Strength::Postflop(b)) => a < b,
            (Strength::Preflop(a), Strength::Preflop(b)) => a > b,
            _ => false,
        }
    }
}

fn strength(hole: &Cards, board: &Cards) -> Option<Strength> {
    if hole.len() < 2 {
        return None;
    }
    let joined = format!("{hole} {board}");
    match board.len() {
        0 => preflop_score(hole).map(Strength::Preflop),
        3 => Five::from_str(&joined).ok().map(|h| Strength::Postflop(h.hand_rank_value())),
        4 => Six::from_str(&joined).ok().map(|h| Strength::Postflop(h.hand_rank_value())),
        5 => Seven::from_str(&joined).ok().map(|h| Strength::Postflop(h.hand_rank_value())),
        _ => None,
    }
}

/// Crude preflop ordering: any pair beats any unpaired hand; within each
/// class, higher rank bits win; suited breaks ties. NOT equity-accurate
/// (22 outranks AKs here) — sufficient for deterministic dump decisions,
/// documented as a simulation constraint.
fn preflop_score(hole: &Cards) -> Option<u32> {
    let two = Two::from_str(&hole.to_string()).ok()?;
    let base = two.rank_binary();
    Some(if two.is_pair() {
        1_000_000 + base
    } else {
        base * 2 + u32::from(two.is_suited())
    })
}
```

Every public item gets full doc comments; `apply_style` gets a doc test (feature-gated doc tests run under `--features collusion`):

```rust
/// # Examples
///
/// ```
/// // Soft-play: heads-up with the partner, an aggressive base action
/// // degrades to a check. (Requires --features collusion.)
/// ```
```

(Write the doc test against the public `apply_style` with a hand-built `TableSnapshot` — copy the shape from the unit test; it must compile under `cargo test --doc -p pkdealer_agent_rules --features collusion`. Note: doc tests only run for library targets — `pkdealer_agent_rules` is a **binary** crate, so doc tests do NOT run; keep the example as ```text``` fenced instead, matching the existing `apply_decision_overrides` doc style.)

- [ ] **Step 4: Run tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion 2>&1 | tail -5` → PASS. If `Six` lacks `from_str` or `HandRanker`, fall back to evaluating the turn via best-of-6 `Five` combinations — but check pkcore first; `Six` exists in `pkcore::arrays::six`.
- [ ] **Step 5: Clippy both ways** → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_agent_rules
git commit -m "feat(epic-70): soft-play/whipsaw/chip-dump collusion strategies (Phase 1c/1d)"
```

---

### Task 9: Wire collusion into `RulesAgent` (Phase 1 integration)

**Files:**
- Modify: `crates/pkdealer_agent_rules/src/main.rs` (`RulesAgent`, `decide`, `main`)

**Interfaces:**
- Consumes: `validate_collusion` (Task 6), `SpectatorLeak` (Task 7), `apply_style` (Task 8).
- Produces: a `RulesAgent::new(profile, exploit)` constructor (collusion `None`) used by `main` and tests.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(feature = "collusion")]
#[tokio::test]
async fn rules_agent_without_collusion_behaves_honest() {
    // The wrapper is strictly additive: no config ⇒ the exact honest path.
    let agent = RulesAgent::new(BotProfile::gto(), None);
    let decision = agent.decide(&sample_state()).await;
    assert!(matches!(
        decision,
        Decision::Fold | Decision::Call | Decision::Raise(_) | Decision::AllIn
    ));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pkdealer_agent_rules --features collusion rules_agent_without_collusion` → `new` not found.

- [ ] **Step 3: Implement**

Struct + constructor:

```rust
struct RulesAgent {
    profile: BotProfile,
    exploit: Option<ExploitPuller>,
    /// EPIC-70: active collusion runtime — partner assignment plus the
    /// Vector-A card leak. `None` ⇒ the agent is byte-for-byte honest.
    #[cfg(feature = "collusion")]
    collusion: Option<Colluder>,
}

/// A validated collusion assignment bound to its live card channel.
#[cfg(feature = "collusion")]
struct Colluder {
    config: CollusionConfig,
    leak: collude::spectator::SpectatorLeak,
}

impl RulesAgent {
    /// Honest constructor — collusion (when compiled in) starts disabled.
    fn new(profile: BotProfile, exploit: Option<ExploitPuller>) -> Self {
        Self {
            profile,
            exploit,
            #[cfg(feature = "collusion")]
            collusion: None,
        }
    }
}
```

`decide` — factor the base decision and adjustment:

```rust
    async fn decide(&self, state: &HandState) -> Decision {
        let action = if let Some(puller) = &self.exploit {
            puller.refresh().await;
            let guard = puller.state.lock().await;
            let snapshot = snapshot_with_stats(state, Some(&guard.registry), &guard.seat_ids);
            self.choose(state, &snapshot).await
        } else {
            let snapshot = hand_state_to_snapshot(state);
            self.choose(state, &snapshot).await
        };
        pkcore_action_to_decision(action)
    }
```

```rust
impl RulesAgent {
    /// Base decision from the honest decider, then — when colluding and the
    /// partner's cards are in hand — the style adjustment. A failed leak
    /// read means this turn is decided honestly (best-effort, per decision).
    async fn choose(&self, state: &HandState, snapshot: &TableSnapshot<'_>) -> PkcoreAction {
        let base = RuleBasedDecider.decide(&self.profile, snapshot);
        #[cfg(feature = "collusion")]
        if let Some(colluder) = &self.collusion {
            let partner_seat = state
                .stacks
                .iter()
                .find(|s| s.name == colluder.config.partner)
                .map(|s| s.seat);
            if let Some(partner_seat) = partner_seat {
                if let Some(partner_hole) = colluder.leak.partner_hole().await {
                    return collude::strategy::apply_style(
                        colluder.config.style,
                        base,
                        snapshot,
                        partner_seat,
                        &partner_hole,
                    );
                }
            }
        }
        #[cfg(not(feature = "collusion"))]
        let _ = state;
        base
    }
}
```

`main()` — after `connect_exploit_puller`:

```rust
    #[cfg(feature = "collusion")]
    let collusion = match validate_collusion(&args) {
        Ok(None) => None,
        Ok(Some(config)) => {
            match collude::spectator::SpectatorLeak::connect(
                args.endpoint.clone(),
                args.spectator_token.clone(),
                config.partner.clone(),
            )
            .await
            {
                Ok(leak) => {
                    eprintln!(
                        "[{}] COLLUSION ACTIVE: partner={} channel=spectator style={:?}",
                        args.name, config.partner, config.style
                    );
                    Some(Colluder { config, leak })
                }
                Err(e) => {
                    eprintln!("[{}] collusion requested but {e}", args.name);
                    process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("[{}] invalid collusion flags: {e}", args.name);
            process::exit(1);
        }
    };
```

Note: `args.spectator_token` is consumed later by `connect_exploit_puller` — reorder so collusion validation runs first, or clone the token. Simplest: clone where needed (`args.spectator_token.clone()` in both call sites).

Then:

```rust
    let mut agent = RulesAgent::new(profile, exploit);
    #[cfg(feature = "collusion")]
    {
        agent.collusion = collusion;
    }
    if let Err(e) = run_agent(agent, config).await {
```

Update the four existing test literals `RulesAgent { profile: …, exploit: None }` → `RulesAgent::new(…, None)`.

- [ ] **Step 4: Run tests both ways** — with and without `--features collusion`; all PASS.
- [ ] **Step 5: Clippy both ways** → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_agent_rules
git commit -m "feat(epic-70): wire collusion wrapper into RulesAgent decide path (Phase 1)"
```

> Phase 1f (live sim smoke: colluding pair beats honest control) requires a docker arena run — deferred to the manual verification checklist in Task 14; it is NOT a blocker for Phase 2.

---

### Task 10: Pairwise signals over `&[RedactedHand]` (Phase 2a)

**Files:**
- Create: `crates/pkdealer_boss/src/signals.rs`
- Modify: `crates/pkdealer_boss/src/lib.rs` (uncomment), `crates/pkdealer_boss/src/fixtures.rs` (add corpora)

**Interfaces:**
- Consumes: `RedactedHand` (Task 1) ONLY — this module must not import `pkcore::hand_history`.
- Produces:

```rust
pub struct Pair { pub a: Uuid, pub b: Uuid }                       // a < b normalized; Clone Copy Eq Hash Ord
impl Pair { pub fn new(x: Uuid, y: Uuid) -> Self; pub fn contains(&self, id: Uuid) -> bool; }
pub enum PairPotOutcome { FlowAtoB(f64), FlowBtoA(f64), Neutral }
pub struct PairHandObs { pub hand_no: u32, pub both_dealt: bool, pub hu_actions: Vec<(Uuid, bool)>, pub baseline_actions: Vec<(Uuid, bool)>, pub whipsaw_events: u32, pub pair_pot: Option<PairPotOutcome> }
pub fn observe_hand(hand: &RedactedHand, pair: &Pair) -> PairHandObs;
pub fn pairs_in(hands: &[RedactedHand]) -> Vec<Pair>;
pub struct PairSignals { pub pair: Pair, pub hands_together: u32, pub pair_pots: u32, pub net_flow_a_to_b: f64, pub soft_play_index: Option<f64>, pub whipsaw_count: u32, pub vpip_with_partner: Option<f64>, pub vpip_without_partner: Option<f64> }
pub fn aggregate(hands: &[RedactedHand], pair: &Pair) -> PairSignals;
pub fn public_stats(hands: &[RedactedHand]) -> HashMap<Uuid, PlayerStats>;   // pkcore PlayerStats, fields populated from public actions only
pub fn names_from(hands: &[RedactedHand]) -> HashMap<Uuid, String>;
```

Semantics (implement exactly):
- **Voluntary action**: any `ActionType` except `Post`. **Aggressive**: `Bet | Raise | AllIn`.
- **Fold tracking**: walk `hand.actions` in order, maintaining a folded set; "live set" at an action = dealt players − folded-so-far.
- **hu_actions**: voluntary actions by a pair member taken while the live set == exactly the two pair members. **baseline_actions**: voluntary actions by a pair member in any other live-set configuration.
- **whipsaw_events**: per street, 1 if ∃ i<j in that street's actions with: `actions[i]` by member X is `Bet|Raise`; `actions[j]` by member Y (the other) is `Raise`; at j ≥ 1 third party is live; and after j no third party takes any voluntary action other than `Fold` for the remainder of the hand. Sum over streets.
- **pair_pot**: at hand end, non-folders == exactly the pair, and their nets have opposite signs → `FlowAtoB(min(-net_a, net_b))` when a lost / b won, symmetric otherwise; `Neutral` when signs don't oppose. `None` when the pot wasn't pair-only.
- **soft_play_index** (aggregate): `(hu aggressive rate) / (baseline aggressive rate)`; `None` when either bucket is empty (aggregate over both members combined).
- **vpip conditioning**: member A's VPIP (any voluntary preflop chips) over hands where B also VPIP'd vs hands where B didn't — aggregate over both members: `vpip_with_partner` / `vpip_without_partner`.
- **public_stats**: per player fill `hands_dealt`, `hands_voluntarily_played`, `pfr_opportunities` (= hands dealt), `pfr_count` (preflop `Raise`), and `by_street` `ActionCounts` (`folds/checks/calls/bets/raises/all_ins` — `pkcore::bot::sim::ActionCounts`, street index 0-3) so `vpip()`, `pfr()`, `aggression_factor()` resolve. Document that only these fields are populated.
- **names_from**: last-seen name per `player_id`.

- [ ] **Step 1: Add corpora to `fixtures.rs`** (used by this task's tests and Tasks 11-13). Four players, seats 0-3: `mallory_1`(MALLORY), `trudy_1`(TRUDY), `gto_1`(GTO), `tag_1`(TAG), stacks 10 000. Implement:

```rust
/// n hands of balanced play: hand i's opener is seat (i % 4), caller is the
/// next seat; the other two fold preflop. Winner alternates by (i / 4) % 2,
/// net ±200. Every adjacent pair meets, aggression flows both ways, and over
/// any multiple of 8 hands every pair's directed chip flow nets to zero.
pub(crate) fn honest_corpus(n: usize) -> Vec<HandHistory>;

/// Like honest_corpus, but whenever mallory & trudy would contest a pot they
/// check/call it down instead (no bet/raise between them, tiny alternating
/// nets ±100); against gto/tag both stay normally aggressive.
pub(crate) fn soft_play_corpus(n: usize) -> Vec<HandHistory>;

/// gto & tag fold preflop every hand; mallory raises, trudy calls, then
/// mallory folds the flop to trudy's bet: net −300 mallory / +300 trudy in
/// 9 of 10 hands (+ every 10th reversed for realism).
pub(crate) fn dump_corpus(n: usize) -> Vec<HandHistory>;

/// trudy raises preflop (300), gto calls (victim), mallory re-raises (900),
/// gto folds, trudy calls; flop checks down; nets ±small alternating between
/// mallory/trudy, gto −300 — refunded every 4th hand where gto wins a
/// mallory-opened pot so gto isn't felted. One whipsaw event per hand.
pub(crate) fn whipsaw_corpus(n: usize) -> Vec<HandHistory>;
```

Write these concretely with `build_hand` — deterministic, no randomness. Keep each generator under ~40 lines; give all four seats hole cards (`Some("A♠ A♥")`-style constants) so scorer fixtures work later; boards only when a street is played.

- [ ] **Step 2: Write failing tests** (in `signals.rs`)

```rust
#[test] fn pair_new_normalizes_order() { assert_eq!(Pair::new(TRUDY, MALLORY), Pair::new(MALLORY, TRUDY)); }
#[test] fn pairs_in_enumerates_unordered_pairs() { /* 4 players → 6 pairs on honest_corpus(8) */ }
#[test] fn metric_chip_flow_flags_dump() {
    let hands = redact(&fixtures::collection(fixtures::dump_corpus(100)));
    let guilty = aggregate(&hands, &Pair::new(MALLORY, TRUDY));
    let honest = aggregate(&hands, &Pair::new(GTO, TAG));
    assert!(guilty.net_flow_a_to_b.abs() > 20.0 * 100.0, "planted dump flow");
    assert!(honest.pair_pots == 0 || honest.net_flow_a_to_b.abs() < 500.0);
}
#[test] fn chipflow_honest_nets_zero() {
    let hands = redact(&fixtures::collection(fixtures::honest_corpus(96)));
    for pair in pairs_in(&hands) {
        let s = aggregate(&hands, &pair);
        assert!(s.net_flow_a_to_b.abs() < 300.0, "pair {pair:?} drifted: {}", s.net_flow_a_to_b);
    }
}
#[test] fn metric_soft_play_index_flags_soft() {
    let hands = redact(&fixtures::collection(fixtures::soft_play_corpus(100)));
    let guilty = aggregate(&hands, &Pair::new(MALLORY, TRUDY)).soft_play_index.unwrap();
    let honest = aggregate(&hands, &Pair::new(GTO, TAG)).soft_play_index.unwrap_or(1.0);
    assert!(guilty < 0.5 * honest, "guilty {guilty} vs honest {honest}");
}
#[test] fn metric_whipsaw_count_flags_whipsaw() {
    let hands = redact(&fixtures::collection(fixtures::whipsaw_corpus(80)));
    assert!(aggregate(&hands, &Pair::new(MALLORY, TRUDY)).whipsaw_count >= 50);
    assert_eq!(aggregate(&hands, &Pair::new(GTO, TAG)).whipsaw_count, 0);
}
#[test] fn public_stats_resolve_vpip_pfr() {
    let hands = redact(&fixtures::collection(fixtures::honest_corpus(40)));
    let stats = public_stats(&hands);
    let m = &stats[&MALLORY];
    assert!(m.vpip().is_some() && m.pfr().is_some());
}
```

- [ ] **Step 3: Run to verify failure** — module missing → compile error.
- [ ] **Step 4: Implement `signals.rs`** per the semantics block. ~250 lines. Structure:

```rust
//! Pairwise public-information signals over redacted hands. This module
//! never sees a hole card: it imports only [`crate::redacted`] types.

use std::collections::{HashMap, HashSet};
use pkcore::analysis::player_stats::PlayerStats;
use pkcore::bot::sim::ActionCounts;
use pkcore::hand_history::ActionType;   // the action *enum* is card-free
use uuid::Uuid;
use crate::redacted::{RedactedHand, RedactedStreet};
```

(`ActionType` import is allowed — it is a wire enum with no card content; the firewall bars `HandCollection`/`HandHistory`/`PlayerEntry`.) Implement helpers `fn is_voluntary(a: &ActionType) -> bool`, `fn is_aggressive(a: &ActionType) -> bool`, `fn street_index(s: RedactedStreet) -> usize`, then `observe_hand`, `pairs_in`, `aggregate`, `public_stats`, `names_from` exactly per the Interfaces block. Doc comments + doc tests on all public items (doc tests can build a tiny `RedactedHand` literal — the struct is fully public).

- [ ] **Step 5: Run tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss 2>&1 | tail -6` → PASS (tune corpus generators, not thresholds, if a metric misses — the corpora are the spec of "planted signature").
- [ ] **Step 6: Clippy** → clean.
- [ ] **Step 7: Hand off commit**

```bash
git add crates/pkdealer_boss
git commit -m "feat(epic-70): pairwise chip-flow/soft-play/whipsaw/VPIP signals over RedactedHand (Phase 2a)"
```

---

### Task 11: SPRT sequential detector (Phase 2b)

**Files:**
- Create: `crates/pkdealer_boss/src/detector.rs`
- Modify: `crates/pkdealer_boss/src/lib.rs`

**Interfaces:**
- Consumes: `observe_hand`, `Pair`, `PairPotOutcome` (Task 10); `pkcore::analysis::player_stats::Confidence`.
- Produces:

```rust
pub struct SprtParams {
    pub alpha: f64,                 // 0.01  target false-positive rate
    pub beta: f64,                  // 0.10  target false-negative rate
    pub default_honest_aggr: f64,   // 0.40  honest HU aggression until baseline warms
    pub min_baseline_actions: u32,  // 20
    pub soft_play_discount: f64,    // 0.25  colluder HU aggression = discount × own baseline
    pub whipsaw_honest: f64,        // 0.02  P(hand has ≥1 whipsaw pattern | honest)
    pub whipsaw_colluding: f64,     // 0.15
    pub flow_honest: f64,           // 0.50  P(pair-pot flow matches running majority | honest)
    pub flow_colluding: f64,        // 0.85
    pub min_hands: u32,             // 50    pkcore Confidence floor (Low band ends at 49)
}
impl Default for SprtParams { /* the numbers above — documented as PRE-CALIBRATION defaults, Phase 5 replaces them */ }
impl SprtParams { pub fn upper_bound(&self) -> f64; pub fn lower_bound(&self) -> f64; }  // Wald: ln((1-β)/α), ln(β/(1-α))
pub struct Verdict { pub pair: Pair, pub llr: f64, pub hands_observed: u32, pub confidence: Confidence, pub flagged_at_hand: Option<u32> }
pub fn assess(hands: &[RedactedHand], params: &SprtParams) -> Vec<Verdict>;   // sorted by llr desc
```

LLR accumulation per hand per pair (sequential, in `hand_no` order):
1. Update each member's running baseline aggression from `baseline_actions` **first**.
2. Per `hu_action (member, aggressive)`: `p_h` = member's baseline rate if its sample ≥ `min_baseline_actions`, else `default_honest_aggr`; `p_c = (p_h * soft_play_discount)`; add `bernoulli_llr(aggressive, p_h, p_c)`.
3. Per hand: add `bernoulli_llr(whipsaw_events > 0, whipsaw_honest, whipsaw_colluding)` — only for hands where both members were dealt.
4. Per pair-pot event: match against the *running majority* direction from previous events (first event and ties contribute nothing, only update counts); add `bernoulli_llr(matches_majority, flow_honest, flow_colluding)`.
5. After the hand: `hands_observed += both_dealt`; flag once when `flagged_at_hand.is_none() && Confidence::from_sample_size(hands_observed) != Confidence::Low && llr >= upper_bound()` → `flagged_at_hand = Some(hand.hand_no)`.

```rust
const P_MIN: f64 = 0.01;
const P_MAX: f64 = 0.99;
fn bernoulli_llr(observed: bool, p_honest: f64, p_colluding: f64) -> f64 {
    let ph = p_honest.clamp(P_MIN, P_MAX);
    let pc = p_colluding.clamp(P_MIN, P_MAX);
    if observed { (pc / ph).ln() } else { ((1.0 - pc) / (1.0 - ph)).ln() }
}
```

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn sprt_flags_colluders() {
    let hands = redact(&fixtures::collection(fixtures::dump_corpus(120)));
    let verdicts = assess(&hands, &SprtParams::default());
    let guilty = verdicts.iter().find(|v| v.pair == Pair::new(MALLORY, TRUDY)).unwrap();
    let at = guilty.flagged_at_hand.expect("colluding pair must flag");
    assert!(at >= 50, "confidence floor holds: {at}");
    assert!(at <= 120, "flag within the session: {at}");
}

#[test]
fn sprt_flags_soft_play_and_whipsaw_too() {
    for corpus in [fixtures::soft_play_corpus(120), fixtures::whipsaw_corpus(120)] {
        let hands = redact(&fixtures::collection(corpus));
        let verdicts = assess(&hands, &SprtParams::default());
        let guilty = verdicts.iter().find(|v| v.pair == Pair::new(MALLORY, TRUDY)).unwrap();
        assert!(guilty.flagged_at_hand.is_some());
    }
}

#[test]
fn sprt_honest_under_fp_bound() {
    let hands = redact(&fixtures::collection(fixtures::honest_corpus(160)));
    let verdicts = assess(&hands, &SprtParams::default());
    assert!(verdicts.iter().all(|v| v.flagged_at_hand.is_none()),
        "honest lineup must not flag: {verdicts:?}");
}

#[test]
fn suspicion_confidence_low_on_small_sample() {
    // 30 hands of blatant dumping — still below the Confidence floor.
    let hands = redact(&fixtures::collection(fixtures::dump_corpus(30)));
    let verdicts = assess(&hands, &SprtParams::default());
    let guilty = verdicts.iter().find(|v| v.pair == Pair::new(MALLORY, TRUDY)).unwrap();
    assert!(guilty.flagged_at_hand.is_none());
    assert_eq!(guilty.confidence, pkcore::analysis::player_stats::Confidence::Low);
}

#[test]
fn wald_bounds_from_targets() {
    let p = SprtParams::default();
    assert!((p.upper_bound() - (0.90_f64 / 0.01).ln()).abs() < 1e-9);
    assert!((p.lower_bound() - (0.10_f64 / 0.99).ln()).abs() < 1e-9);
}
```

- [ ] **Step 2: Run to verify failure** — compile error.
- [ ] **Step 3: Implement `detector.rs`** per the algorithm block (~180 lines). Sort verdicts by `llr` descending (`sort_by` + `total_cmp`, reversed). Full doc comments; `SprtParams`'s docs must spell out that the numbers are pre-calibration defaults and cite EPIC-70 Phase 5a. Doc test: `SprtParams::default().upper_bound() > 0.0`.
- [ ] **Step 4: Run tests** — PASS. If `sprt_honest_under_fp_bound` fails, the honest corpus has a modeling asymmetry — fix the corpus (or the majority-direction tie handling), never widen the bounds to pass.
- [ ] **Step 5: Clippy** → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_boss
git commit -m "feat(epic-70): per-pair SPRT detector with Wald bounds + Confidence floor (Phase 2b)"
```

---

### Task 12: Ground-truth scorer + EV-sacrifice oracle (Phase 2c)

**Files:**
- Create: `crates/pkdealer_boss/src/scorer.rs`
- Modify: `crates/pkdealer_boss/src/lib.rs`

**Interfaces:**
- Consumes: `GroundTruthLabels` (Task 2), `Verdict`/`Pair` (Tasks 10-11), full `HandCollection` (allowed — grading tier only), `Seven` + `HandRanker`.
- Produces:

```rust
pub struct OracleScore { pub spots: u32, pub sacrifices: u32 }
pub struct PairScore { pub pair: Pair, pub names: (String, String), pub hands_to_detection: Option<u32>, pub oracle: OracleScore }
pub struct ScoreReport { pub labeled: Vec<PairScore>, pub false_positives: Vec<Pair>, pub honest_pairs: u32, pub fp_rate: f64 }
pub fn score(collection: &HandCollection, labels: &GroundTruthLabels, verdicts: &[Verdict]) -> ScoreReport;
```

Oracle rules (card-aware, per labeled pair, per hand where both members appear with known hole cards):
- **Fold-the-better-hand** (needs a complete 5-card board): a member folded (any `Fold` action) while the partner had committed chips (any partner action with `amount > 0`), and the folder's `Seven` rank value is strictly lower (stronger) than the partner's → `spots += 1; sacrifices += 1`. Same situation but folder weaker → `spots += 1` only.
- **Passive-strong** (pair-only pot, complete board): member holds two-pair-or-better (`Seven` rank value ≤ 3325) → `spots += 1`; if that member took zero aggressive actions in the hand → `sacrifices += 1`.
- Hands without a 5-card board or without both hole cards are skipped (documented limitation).
- `fp_rate` = flagged-but-unlabeled pairs ÷ total unlabeled pairs among the verdicts (0.0 when there are no honest pairs).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn scorer_reports_hands_to_detection() {
    let c = fixtures::collection(fixtures::dump_corpus(120));
    let hands = redact(&c);
    let verdicts = assess(&hands, &SprtParams::default());
    let labels = GroundTruthLabels::resolve(&c, &[
        ("mallory_1".into(), "trudy_1".into(), LabelVector::Spectator, LabelStyle::ChipDump),
    ]).unwrap();
    let report = score(&c, &labels, &verdicts);
    assert!(report.labeled[0].hands_to_detection.is_some());
    assert!(report.false_positives.is_empty());
    assert!(report.fp_rate.abs() < f64::EPSILON);
}

#[test]
fn oracle_ev_sacrifice_scores_softplay() {
    // Hand-crafted: pair-only pot, mallory checks down AA-on-KK-board
    // (two pair or better) vs trudy → 1 spot, 1 sacrifice.
    let hand = fixtures::build_hand(fixtures::HandSpec {
        no: 1,
        players: vec![
            fixtures::player(0, "mallory_1", MALLORY, 10_000.0, Some("A♠ A♥")),
            fixtures::player(1, "trudy_1", TRUDY, 10_000.0, Some("Q♠ Q♥")),
        ],
        preflop: vec![
            fixtures::act(0, MALLORY, ActionType::Call, Some(100.0)),
            fixtures::act(1, TRUDY, ActionType::Check, None),
        ],
        flop: Some(("K♠ K♥ 7♦".into(), vec![
            fixtures::act(0, MALLORY, ActionType::Check, None),
            fixtures::act(1, TRUDY, ActionType::Check, None),
        ])),
        turn: Some(("3♣".into(), vec![
            fixtures::act(0, MALLORY, ActionType::Check, None),
            fixtures::act(1, TRUDY, ActionType::Check, None),
        ])),
        river: Some(("2♦".into(), vec![
            fixtures::act(0, MALLORY, ActionType::Check, None),
            fixtures::act(1, TRUDY, ActionType::Check, None),
        ])),
        nets: vec![(0, 100.0), (1, -100.0)],
    });
    let c = fixtures::collection(vec![hand]);
    let labels = GroundTruthLabels::resolve(&c, &[
        ("mallory_1".into(), "trudy_1".into(), LabelVector::Spectator, LabelStyle::SoftPlay),
    ]).unwrap();
    let report = score(&c, &labels, &[]);
    assert!(report.labeled[0].oracle.spots >= 1);
    assert!(report.labeled[0].oracle.sacrifices >= 1);
}

#[test]
fn oracle_counts_fold_of_better_hand() {
    // mallory folds AA on the river while committed trudy holds QQ → sacrifice.
    // (Board K♠ 8♥ 7♦ 3♣ 2♦ — AA beats QQ; trudy bet 400 first.)
    /* build analogous fixture; assert sacrifices >= 1 */
}

#[test]
fn honest_flag_counts_as_false_positive() {
    // Fabricate a verdict flagging (GTO, TAG) with empty labels → fp_rate > 0.
    let c = fixtures::collection(fixtures::honest_corpus(8));
    let labels = GroundTruthLabels { colluding_pairs: vec![] };
    let fake = Verdict { pair: Pair::new(GTO, TAG), llr: 9.0, hands_observed: 60,
        confidence: pkcore::analysis::player_stats::Confidence::Medium, flagged_at_hand: Some(55) };
    let report = score(&c, &labels, &[fake]);
    assert_eq!(report.false_positives.len(), 1);
    assert!((report.fp_rate - 1.0).abs() < f64::EPSILON);
}
```

- [ ] **Step 2: Run to verify failure**, **Step 3: Implement** per the rules block (~170 lines; imports: `pkcore::arrays::{HandRanker, seven::Seven}`, `std::str::FromStr`; helper `fn rank7(hole: &str, board: &str) -> Option<u16>` mirroring pkcore's private `rank_seven`: `Seven::from_str(&format!("{hole} {board}")).ok().map(|s| s.hand_rank_value())` gated on a 5-card board). Document loudly at module top: *this is the only detection-adjacent code allowed to read hole cards; its output grades the Boss and never feeds detection.* `const TWO_PAIR_OR_BETTER: u16 = 3325;`
- [ ] **Step 4: Run tests** → PASS. **Step 5: Clippy** → clean.
- [ ] **Step 6: Hand off commit**

```bash
git add crates/pkdealer_boss
git commit -m "feat(epic-70): ground-truth scorer + card-aware EV-sacrifice oracle (Phase 2c)"
```

---

### Task 13: Report + app + CLI binary (Phase 2d)

**Files:**
- Create: `crates/pkdealer_boss/src/report.rs`, `crates/pkdealer_boss/src/app.rs`
- Modify: `crates/pkdealer_boss/src/main.rs` (real CLI), `crates/pkdealer_boss/src/lib.rs`

**Interfaces:**
- Produces:

```rust
// report.rs — redacted-tier only
pub fn render(verdicts: &[Verdict], signals: &[PairSignals], names: &HashMap<Uuid, String>, score: Option<&ScoreReport>, params: &SprtParams) -> String;
// app.rs
pub struct RunConfig { pub session: PathBuf, pub labels: Option<PathBuf> }
pub fn run(config: &RunConfig) -> Result<String, BossError>;
```

`run()` pipeline: read session file → parse (`HandCollection::from_yaml`, falling back to `serde_json::from_str` when the trimmed payload starts with `{`) → `redact` → `Empty` error if no hands → `pairs_in`/`aggregate`/`assess` → optional labels file → `score` → `render`. Report layout (plain text, one pair per line, sorted by LLR desc):

```
pkdealer_boss — blind collusion report (EPIC-70)
hands: 120   players: 4   pairs: 6

pair                          hands  soft-idx  whipsaw  pair-pots  net-flow  llr      flagged@
mallory_1 + trudy_1             118      0.21       14         36   -4200.0  12.40    61
gto_1 + tag_1                   115      0.96        0          8      50.0  -3.10    —
…
SPRT: alpha=0.010 beta=0.100 upper=4.50 lower=-2.29 confidence-floor=50 hands (pre-calibration defaults)

ground truth: 1 labeled pair (spectator / chip_dump)
  mallory_1 + trudy_1  DETECTED  hands-to-detection=61  oracle: 34 spots / 31 sacrifices
false positives: 0 / 5 honest pairs (rate 0.00)
```

`main.rs`:

```rust
#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! `pkdealer_boss` CLI (EPIC-70 Phase 2d): read a recorded session (+
//! optional ground-truth labels), run the blind detection pipeline, print
//! the per-pair report. A pure consumer of recorded output.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use pkdealer_boss::app::{RunConfig, run};

/// Command-line arguments for the Boss.
#[derive(Parser, Debug)]
#[command(
    name = "pkdealer_boss",
    version,
    about = "Blind collusion detection over recorded pkdealer sessions"
)]
struct Cli {
    /// Recorded `HandCollection` session file (YAML from the EPIC-25 sink,
    /// or JSON from `ExportSession`).
    #[arg(long, value_name = "FILE")]
    session: PathBuf,

    /// Ground-truth labels sidecar (YAML). Adds the scorer section:
    /// hands-to-detection, false-positive rate, and the EV-sacrifice oracle.
    #[arg(long, value_name = "FILE")]
    labels: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = RunConfig { session: cli.session, labels: cli.labels };
    match run(&config) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pkdealer_boss: {err}");
            ExitCode::FAILURE
        }
    }
}
```

(Note the EPIC's verification block uses `--session`/`--labels` flags — match it.)

- [ ] **Step 1: Write failing tests** (app.rs tests use the scratch temp dir via `std::env::temp_dir()`)

```rust
#[test]
fn run_end_to_end_on_recorded_yaml() {
    let c = fixtures::collection(fixtures::dump_corpus(120));
    let yaml = c.to_yaml().unwrap();
    let dir = std::env::temp_dir();
    let session = dir.join("boss_e2e_session.yaml");
    let labels_path = dir.join("boss_e2e_labels.yaml");
    std::fs::write(&session, yaml).unwrap();
    let labels = GroundTruthLabels::resolve(&c, &[
        ("mallory_1".into(), "trudy_1".into(), LabelVector::Spectator, LabelStyle::ChipDump),
    ]).unwrap();
    std::fs::write(&labels_path, labels.to_yaml().unwrap()).unwrap();
    let report = run(&RunConfig { session, labels: Some(labels_path) }).unwrap();
    assert!(report.contains("mallory_1 + trudy_1"));
    assert!(report.contains("DETECTED"));
    assert!(report.contains("false positives: 0"));
}

#[test]
fn run_missing_file_is_io_error() {
    let err = run(&RunConfig { session: "/nonexistent/x.yaml".into(), labels: None }).unwrap_err();
    assert!(matches!(err, BossError::Io(_)));
}

#[test]
fn run_garbage_payload_is_parse_error() { /* write "not yaml [" → BossError::Parse */ }

#[test]
fn render_marks_unflagged_pairs_with_dash() { /* honest corpus → report contains "—" and no "DETECTED" */ }
```

- [ ] **Step 2: Run to verify failure**, **Step 3: Implement** `report.rs` (~90 lines, `write!` into a String — remember `use std::fmt::Write`; no unwraps: `let _ = writeln!(…)` is fine since writing to String cannot fail, or thread the fmt Result) and `app.rs` (~60 lines per the pipeline). Doc tests on `run` (`no_run`) and `render`.
- [ ] **Step 4: Run everything** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_boss && cargo test --doc -p pkdealer_boss` → PASS.
- [ ] **Step 5: Smoke the real binary**

Run: `cargo run -p pkdealer_boss -- --session /nonexistent.yaml; echo "exit=$?"`
Expected: `pkdealer_boss: io error: …` + `exit=1`.

- [ ] **Step 6: Clippy** → clean.
- [ ] **Step 7: Hand off commit**

```bash
git add crates/pkdealer_boss
git commit -m "feat(epic-70): pkdealer_boss report + app pipeline + CLI (Phase 2d)"
```

---

### Task 14: Docs, EPIC status, OKF bundle, final verification

**Files:**
- Modify: `docs/EPIC-70_Collusion_and_Cheat_Detection.md` (Status table + work-item checkboxes + deviations note), `docs/BACKLOG.md` (EPIC-70 row → "Phases 0–2 done"), `.okf/log.md` (+1 dated line), `.okf/` crate-concept file that lists workspace crates (find it via `.okf/index.md`; add `pkdealer_boss`, bump its `timestamp`)

- [ ] **Step 1: Update the EPIC doc.** Check off work items 0a-0f, 1a-1e, 2a-2d (leave 1f unchecked with a note "needs live arena run"). Update the Status table rows for the delivered components to `✅ Done (2026-07-23)`. Replace the "Not yet started" blockquote with a dated progress note. Append a short `### Implementation deviations (Phases 0–2)` subsection copying the 7 deviations from this plan's header. Fix the Verification block's `cargo build --workspace --features collusion` → per-package commands (see Step 3).
- [ ] **Step 2: Update BACKLOG.md + OKF.** One-line status change in BACKLOG; add `pkdealer_boss` to the OKF crates concept + refresh its `timestamp:`; append to `.okf/log.md`: `- 2026-07-23: EPIC-70 Phases 0–2 implemented (pkdealer_boss crate, collusion feature on rules agent, arena team expansion).` Then validate: run the `/okf:validate .okf --strict` skill (or, if executing as a subagent without skills, run the deterministic checker it wraps and report any frontmatter errors).
- [ ] **Step 3: Full verification sweep** (the EPIC's block, corrected)

```bash
cargo build --workspace
cargo build -p pkdealer_agent_rules --features collusion
cargo clippy --workspace -- -D warnings
cargo clippy -p pkdealer_agent_rules --features collusion -- -D warnings
OTEL_SDK_DISABLED=true cargo test --workspace
OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion
cargo test --doc -p pkdealer_boss
./tests/arena_team.sh
./bin/arena --dry-run mallory trudy gto lag   # eyeball: exit criterion 2
```

Expected: all green; the dry-run override shows `--collude-with` on exactly the two teammates.

- [ ] **Step 4: Report exit-criteria status honestly** (in the final summary to the user):
  - ✅ Criterion 2 (arena expansion), 6 (redact provably card-free + compile_fail), 7 (existing tests unchanged; compare per-crate).
  - 🟡 Criterion 3 partially: SPRT flags true pairs with finite hands-to-detection **on synthetic corpora**; live-session numbers need arena runs.
  - ⬜ Criteria 1, 4, 5 (replicated chip-edge, K-run FP study, A/B equivalence): Phase 1f + Phases 3/5 — out of this plan's scope.
- [ ] **Step 5: Hand off the final commit**

```bash
git add docs/EPIC-70_Collusion_and_Cheat_Detection.md docs/BACKLOG.md .okf
git commit -m "docs(epic-70): mark Phases 0-2 delivered; OKF bundle + backlog refresh"
```

---

## Self-review checklist (run after drafting, before execution)

1. **Spec coverage:** 0a→T1, 0b→T1, 0c→T2, 0d→T3 (+deviation 4 via T2 `resolve`), 0e→T4, 0f→T5; 1a→T6, 1b→T7, 1c/1d→T8, 1e→T5, 1f→deferred (T14 notes); 2a→T10, 2b→T11, 2c→T12, 2d→T13. EPIC test-plan names all present except `vector_a_and_b_same_signature` (Phase 3) and `backchannel_matches_shares_by_hand_no` (Phase 3) — correctly out of scope; `arena_team_expands_to_partner_flags` realized as `tests/arena_team.sh`; `redacted_hand_has_no_card_field` realized as the `compile_fail` doc test; `collude_with_resolves_composed_name_to_uuid` realized in T2.
2. **Type consistency:** `Pair`/`Verdict`/`SprtParams`/`RedactedHand` signatures repeated identically in Tasks 10-13 interface blocks; `CollusionStyle::{Soft,Whipsaw,Dump}` (not SoftPlay/ChipDump) everywhere in agent code; labels use `LabelStyle::{SoftPlay,Whipsaw,ChipDump}` (serde snake_case) — distinct types by design (boss must not depend on the agent crate).
3. **Placeholders:** corpus generators in T10 Step 1 are specified by behavior contract rather than full code — the implementer writes them against the metric tests that consume them; every other code step is complete.
