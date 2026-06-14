# Spec: `pkcore` PokerBench scenario model + scoring

**Target repo:** `ImperialBower/pkcore` (a release after the current `0.1.x`)
**Driven by:** `ImperialBower/pkdealer` EPIC-43 Phase 1 (PokerBench integration —
offline benchmark of arena LLMs). See
[`docs/EPIC-43_PokerBench_Integration.md`](EPIC-43_PokerBench_Integration.md).
**Type:** additive, self-contained library module behind a feature flag. No changes
to existing types; no breaking changes.

## Context

pkdealer wants to benchmark its arena LLM backends against a solver-optimal ground
truth ([PokerBench](https://github.com/pokerllm/pokerbench), HuggingFace
`RZ412/PokerBench` — No-Limit Texas Hold'em, 6-handed). The poker-domain pieces —
parsing the dataset, normalizing the solver `label` into a canonical action, mapping
a scenario onto a canonical game state, and **scoring** a predicted action against
the label — are reusable library logic and belong in pkcore alongside its existing
equity/GTO/range machinery. pkdealer's harness (EPIC-43 Phase 2) then converts the
canonical state into its own `HandState` and runs it through the live prompt/parse
pipeline.

This module is **descriptive/analytical only**: it adds no behavior to existing
poker engine types and is gated behind a `pokerbench` cargo feature so it pulls in
no parsing/serde cost for consumers that don't use it.

## Dataset shape (informative)

Each item: an **instruction** (natural-language 6-max state — positions
UTG/HJ/CO/BTN/SB/BB, board, action line, hole cards, pot, legal moves) and a
**label** (optimal action: fold / check / call, or bet/raise with a size). Two
splits (pre-flop, post-flop), each in JSON (prompt+label) and CSV (structured
columns). pkcore must read both forms; the CSV columns are the easier structured
source and the JSON is the canonical prompt text.

## Module additions (feature `pokerbench`)

### 1. `PokerBenchScenario`

```rust
/// One PokerBench item: a parsed 6-max No-Limit Hold'em decision point plus the
/// solver-optimal action. Analysis-only; constructed by the loaders below.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PokerBenchScenario {
    /// Original natural-language instruction text (the LLM prompt).
    pub instruction: String,
    /// Hero position (UTG, HJ, CO, BTN, SB, BB).
    pub hero: Position,
    /// Community cards as parsed pkcore cards (empty pre-flop).
    pub board: Vec<Card>,
    /// Hero hole cards.
    pub hole: Vec<Card>,
    /// Pot before the hero acts (chips).
    pub pot: u32,
    /// Chips the hero must call (0 = check available).
    pub to_call: u32,
    /// Big blind (chip unit baseline).
    pub big_blind: u32,
    /// Per-position stacks at the decision point.
    pub stacks: Vec<(Position, u32)>,
    /// Action line leading to the decision, in order.
    pub history: Vec<PokerBenchAction>,
    /// Legal moves offered at the decision point.
    pub legal: Vec<PokerBenchAction>,
    /// The solver-optimal action (the label being predicted).
    pub optimal: PokerBenchAction,
    /// Which split this came from.
    pub split: PokerBenchSplit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokerBenchSplit { Preflop, Postflop }
```

`Position`, `Card` reuse existing pkcore types. If pkcore has no `Position` enum
yet, add a minimal one in this module (UTG/HJ/CO/BTN/SB/BB) rather than touching
engine code.

### 2. `PokerBenchAction`

A self-describing action mirroring PokerBench's label vocabulary. Kept separate from
any engine action type so this module stays additive:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokerBenchAction {
    Fold,
    Check,
    Call,
    Bet(u32),    // size in chips
    Raise(u32),  // total-to amount in chips
    AllIn,
}
```

### 3. Loaders

```rust
impl PokerBenchScenario {
    /// Parse the structured CSV form of a split into scenarios.
    ///
    /// # Errors
    /// Returns `Err` if a row is malformed or a card/position fails to parse.
    pub fn load_csv(path: &Path, split: PokerBenchSplit)
        -> Result<Vec<PokerBenchScenario>, PokerBenchError>;

    /// Parse the JSON (prompt+label) form of a split into scenarios.
    ///
    /// # Errors
    /// Returns `Err` if the JSON is malformed or a label fails to parse.
    pub fn load_json(path: &Path, split: PokerBenchSplit)
        -> Result<Vec<PokerBenchScenario>, PokerBenchError>;
}
```

`PokerBenchError` is a custom enum implementing `std::error::Error` + `Display`
(per pkcore conventions — no `unwrap`/`expect`/`panic!` in library code).

### 4. Canonical-state conversion

pkdealer builds its own `HandState`, but it should not re-parse prose. Expose a
canonical, name/seat-friendly view so the pkdealer adapter is a trivial field map:

```rust
impl PokerBenchScenario {
    /// Canonical seating for this scenario: `(seat, position, name, chips)` in
    /// 6-max seat order, with the hero's seat identified. Names are synthesized
    /// from positions. Resolves PokerBench's position labels to 0-based seats so
    /// downstream seat-indexed state (e.g. pkdealer `HandState`) maps directly.
    pub fn canonical_seating(&self) -> CanonicalSeating;
}

pub struct CanonicalSeating {
    pub hero_seat: u8,
    pub seats: Vec<CanonicalSeat>, // (seat, position, name, chips)
}
```

The two mapping concerns the pkdealer epic calls out — **position→seat** and
**this-street-only action history** — are resolved here so they are decided once,
in the library, and unit-tested against the canonical model.

### 5. Scoring

```rust
/// A predicted action's score against the solver-optimal label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionScore {
    /// Action *types* match (fold/check/call/bet/raise/all-in).
    pub type_match: bool,
    /// For bet/raise: |predicted - label| as a fraction of the pot. `None` when
    /// the optimal action carries no size.
    pub size_error: Option<f64>,
    /// Optional solver-equity EV-loss (filled when the equity path is enabled).
    pub ev_loss: Option<f64>,
}

/// Scores a predicted `PokerBenchAction` against a scenario's optimal label.
///
/// # Examples
/// ```
/// # use pkcore::pokerbench::{score_action, PokerBenchAction};
/// // ... construct a scenario whose optimal is Call ...
/// // let s = score_action(&scenario, PokerBenchAction::Call);
/// // assert!(s.type_match);
/// ```
pub fn score_action(scenario: &PokerBenchScenario, predicted: PokerBenchAction)
    -> ActionScore;
```

`ev_loss` starts as `None`; a follow-up wires it to pkcore's existing equity
machinery once the EV-loss definition is signed off (EPIC-43 open decision #3).

## Testing requirements (pkcore conventions)

Per pkcore's standard (doc test + unit tests for every public item; no
`unwrap`/`expect`/`panic!` in library code):

- **Doc tests** on `PokerBenchScenario` (construct + a field assert), each loader
  (parse a tiny inline/fixture sample), `canonical_seating`, and `score_action`.
- **Loader fixtures**: a handful of vendored CSV + JSON rows covering preflop and
  postflop; assert counts, a parsed board, and a parsed `optimal` label.
- **Conversion**: assert `canonical_seating` resolves each of UTG/HJ/CO/BTN/SB/BB to
  the expected seat and identifies the hero seat; assert stacks/pot/to_call survive.
- **Scoring matrix**: type match vs mismatch; bet/raise size-error fraction; a
  size-less optimal (fold/check/call) yields `size_error: None`.
- **Round-trip**: `PokerBenchScenario` survives serde JSON round-trip equal.
- **Error paths**: malformed CSV row and malformed JSON each return `Err`, not panic.

## Acceptance criteria

1. `pokerbench` module added behind a cargo feature; no existing types or tests
   change; default build is unaffected.
2. `PokerBenchScenario`, `PokerBenchAction`, `PokerBenchSplit`, loaders,
   `canonical_seating`, `ActionScore`, and `score_action` implemented, documented,
   and tested per the matrix above.
3. CSV and JSON loaders both produce equivalent scenarios for the same items.
4. No `unwrap`/`expect`/`panic!` in the module; all fallible paths return
   `PokerBenchError`.
5. New crate version published; changelog notes the additive feature.

## Downstream consumer (pkdealer EPIC-43 Phase 2, for reference — not in this PR)

After this lands and the pin is bumped, `crates/pkdealer_pokerbench` will:
1. Load scenarios via `PokerBenchScenario::load_{csv,json}`.
2. Build a `pkdealer_agent_core::HandState` from `canonical_seating` + scenario
   fields.
3. Run each `HandState` through `build_prompt` → `LlmBackend::complete` →
   `parse_action_opt` (the live arena path), mapping the resulting `Decision` to a
   `PokerBenchAction`.
4. Call `score_action` per scenario and aggregate a per-model, per-split leaderboard,
   exported via the EPIC-25 recorder format.

No pkcore changes beyond this spec are required for pkdealer Phase 2.
