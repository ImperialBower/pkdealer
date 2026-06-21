# EPIC-45: 6-Handed NLHE Bot-Evaluation Format ("arena-eval")

## Status

| Component | Status |
|---|---|
| `pkcore` deck-replay primitive (`PokerSession::set_next_deck`) | ⏳ Planned — Phase 0 (blocks all mirroring) |
| `pkdealer_arena` crate skeleton + `FormatConfig` TOML | ⏳ Planned — Phase 1 |
| In-process agent factory (rules/random/ollama/claude) | ⏳ Planned — Phase 2 |
| Single-hand in-process engine (`play_one_hand`) | ⏳ Planned — Phase 3 |
| Mirror + cash orchestration (duplicate poker, position-equalized) | ⏳ Planned — Phase 4 |
| Tournament/ICM orchestration (per-seed mirroring) | ⏳ Planned — Phase 5 |
| LLM token-cost budget gate | ⏳ Planned — Phase 6 |
| Scoring (leaderboard+CI, behavior, H2H matrix, export) + `bin/arena_eval` | ⏳ Planned — Phase 7 |

> **Not yet started.** This EPIC supersedes the ad-hoc evaluation done via
> `bin/arena` (EPIC-42) for the specific purpose of *measuring* bot strength. It
> is a separate, headless path: it drives `pkcore::PokerSession` in-process and
> does **not** touch the docker/gRPC stack. `bin/arena` remains the tool for
> live, observable, OTel-traced demo tables.

---

## Context

Today bots are pitted against each other via `bin/arena` (docker + gRPC,
EPIC-42) or pktui's local arena, looping an **escalating blind ladder**
(`pkdealer_service/src/blind_schedule.rs`) with **infinite rebuys**
(`run_auto_rebuy`). That setup is built for *watching* bots play — it is a poor
instrument for *measuring* which bot plays better, for two reasons:

1. **It conflates two different skills.** Looping 50/100 → 3000/6000 with rebuys
   mixes cash skill (playing a fixed stack depth well) with tournament/ICM skill
   (adjusting to shrinking stacks). A bot strong at 100bb and weak at 10bb gets
   averaged into mush.
2. **It does nothing to fight card variance.** Poker needs thousands of hands to
   separate edge from luck; `pkcore`'s own `Confidence` band tops out at
   "High = ≥200 hands," which is still tiny. The framework already *captures*
   `shuffled_deck_str` and supports seeding, so it is one small step from the
   gold-standard variance killer: **duplicate (mirror) poker**.

The goal of this EPIC is a dedicated, **headless** 6-max NLHE evaluation format
whose only job is to **discriminate between bots as sharply and fairly as
possible**, then emit a leaderboard, behavioral profiles, a head-to-head matrix,
and a replayable hand corpus — all while keeping LLM seats within a cost budget.

**6-max is deliberate:** enough seats for positional/multiway complexity
(BTN/SB/BB/CO/HJ/LJ all covered in `pkcore/src/casino/table/position.rs`), but
each bot is in far more hands per orbit than 9-max, so edges accumulate faster
per hand dealt.

### Decisions

- **Two selectable modes** (`mode = "cash" | "tournament"`):
  - **Cash, fixed depth** — fixed blinds; every hand resets all 6 seats to a
    constant stack cap (e.g. 100bb); hands are independent. The clean,
    fast-converging measure of raw NLHE skill.
  - **Tournament / ICM** — escalating blinds (reuse `blind_schedule`), no rebuy,
    play down to a winner. Measures survival + short-stack + ICM skill.

- **Variance reduction = mirror (duplicate) poker + position equalization**, at
  mode-dependent granularity:
  - **Cash → mirror unit = one hand.** Deal a deck, replay that *exact* deck 6×
    rotating the 6 bots through all seats; stacks reset each replay. Card luck
    cancels; positions are exactly equalized per group of 6. (ACPC-style.)
  - **Tournament → mirror unit = one whole tournament.** Fix the full deck
    sequence and run it 6× rotating the starting lineup through the 6 seats.
    Each bot plays the identical run of cards from each starting seat. Variance
    reduction is weaker (a tournament is one big correlated sample), so many
    seeds are needed.

- **Four outputs, all derived from one captured `HandCollection`:**
  mode-aware leaderboard with confidence intervals; rich behavioral profile per
  bot; head-to-head matrix; per-hand history export.

- **Scale control:** rules/random bots scale freely (10k+ cash hands / many
  tournament seeds); LLM seats bounded by a configurable token-cost cap (reuse
  `pkdealer_pricing`). Configurable hand/seed budget + a hard LLM cost cap.

- **Interface:** new headless batch runner (new crate + `bin/arena_eval`), not
  the docker/gRPC stack. The harness holds `Box<dyn PokerAgent>` per seat and
  drives `pkcore::PokerSession` directly — for determinism, speed, and trivial
  seat rotation.

### Feasibility (verified against source)

- **Deck replay is the one hard dependency.** `shuffled_deck_str`
  (`pkcore/src/casino/session.rs:329`) is a complete, ordered, replayable
  description of a hand's deal: the deck is an ordered `IndexSet`, dealing draws
  from index 0, and `Display`/`FromStr` round-trip
  (`pkcore/src/cards.rs:35,276,621,824`). The only blocker is the unconditional
  shuffle at `session.rs:328`. Fix is a ~10-line, backward-compatible pkcore
  addition (Phase 0).
- **In-process agents all work as a library.** Rules via
  `pkcore::bot::{RuleBasedDecider, BotProfile}`; LLMs via
  `LlmPokerAgent::with_model` + `OllamaBackend::new` / `ClaudeBackend::new`
  (the gRPC binaries are thin wrappers over these libs).
- **Mirror determinism** is guaranteed by rebuilding a fresh 6-seat 100bb table
  + new `PokerSession` per cash replay — no surgical reset. `banked_profit` is a
  service-only ledger and is *not* needed; cash scoring uses per-hand
  `ResultEntry.net`.
- **Agent determinism:** rules deciders MUST use `decide_seeded`
  (`pkcore/src/bot/decider.rs:151`) with a per-(bot,group,rotation) `SmallRng`;
  the random agent must be re-implemented locally with an injected seed (the bin
  uses `rand::rng()`). LLMs are inherently nondeterministic — mirroring still
  cancels their *card* variance.

---

## Architecture & Phases

New crate **`crates/pkdealer_arena/`** (library + bin). Module layout:

```
src/
  lib.rs            public harness API
  config.rs         FormatConfig, Mode, LineupEntry, Budget; from_toml + validate (seats == 6)
  lineup.rs         AgentSpec -> Box<dyn PokerAgent> factory (in-process)
  seat_agent.rs     local seedable RulesSeat / RandomSeat wrappers
  handstate.rs      build HandState from &TableNoCell + finalize_decision (ported)
  mirror.rs         MirrorUnit, RotationSchedule (6 cyclic perms), deck capture/replay
  engine.rs         play_one_hand(session, agents, rng) -> HandHistory + token tally
  orchestrator.rs   mode-aware run loops + budget gate
  corpus.rs         HandCollection wrapper + per-hand net extraction
  scoring/
    mod.rs
    leaderboard.rs  bb/100 (cash) | ROI/placement (tourney) + grouped CI
    behavior.rs     StatsRegistry passthrough
    h2h.rs          pairwise net matrix
    report.rs       text / JSON rendering
  bin/arena_eval.rs clap entrypoint -> orchestrator::run(config)
```

### Phase 0 — pkcore deck-replay primitive (unblocks everything)

The make-or-break change. Additive and backward-compatible.

In `pkcore/src/casino/session.rs`:

```rust
// new field on PokerSession
pending_deck: Option<Cards>,   // None => shuffle as today

pub fn set_next_deck(&mut self, deck: Cards) { self.pending_deck = Some(deck); }
```

Then branch `start_hand` (currently the unconditional shuffle at `session.rs:328`):

```rust
match self.pending_deck.take() {
    Some(deck) => { self.table.deck = deck; }
    None       => { self.table.deck.shuffle_in_place(); }
}
self.shuffled_deck_str = Some(self.table.deck.to_string());   // capture unchanged
```

The `None` path is byte-identical to today. This is the only mandatory pkcore
change for cash mirroring; tournament replay reuses the same primitive.

### Phase 1 — crate skeleton + config

New `pkdealer_arena` crate, added to workspace `members` in `Cargo.toml`.
`config.rs`:

```rust
pub enum Mode { CashFixedDepth, Tournament }

pub struct FormatConfig {
    pub mode: Mode,
    pub seats: usize,              // fixed 6 (validated)
    pub lineup: Vec<LineupEntry>,  // exactly 6 distinct bots
    pub stack_cap_bb: u32,         // cash, e.g. 100
    pub small_blind: usize,
    pub big_blind: usize,
    pub starting_stack_bb: u32,    // tournament start stack
    pub hands_per_level: usize,    // tournament ladder cadence
    pub budget: Budget,
}
pub struct Budget {
    pub max_mirror_groups: u64,    // cash: groups of 6 hands; tourney: seeds
    pub llm_cost_cap_usd: f64,
    pub pricing_toml: Option<String>,
}
pub struct LineupEntry { pub name: String, pub spec: AgentSpec }
pub enum AgentSpec {
    Rules  { profile: String },
    Random,
    Ollama { host: String, model: String },
    Claude { model: String, max_tokens: u32 },  // key from env
}
impl FormatConfig { pub fn from_toml(s: &str) -> Result<Self, ConfigError>; }
```

### Phase 2 — in-process agents

`lineup.rs` factory `AgentSpec -> Box<dyn PokerAgent>`:
- **Rules** via local `RulesSeat` wrapping `RuleBasedDecider` + `BotProfile`
  (port the snapshot build from `crates/pkdealer_agent_rules/src/main.rs:90-169`,
  but call `BotDecider::decide_seeded`, not the default `decide`).
- **Random** via local seedable `RandomSeat` (port
  `crates/pkdealer_agent_random/src/main.rs:68-95`, replace `rand::rng()` with an
  injected `SmallRng`).
- **Ollama/Claude** via `LlmPokerAgent::with_model(OllamaBackend::new(...) /
  ClaudeBackend::new(...))`.

### Phase 3 — single-hand engine

`handstate.rs` + `engine.rs::play_one_hand`:

```rust
pub async fn play_one_hand(
    session: &mut PokerSession,
    agents:  &[Box<dyn PokerAgent>; 6],   // indexed by seat
    rng:     &mut [SmallRng; 6],          // per-seat seed for rules/random
) -> Result<(HandHistory, Vec<SeatTokens>), PKError>;
```

Mirrors `pkdealer_agent_core/src/runner.rs:237-310,473-503` against
`TableNoCell`:
1. `session.set_next_deck(...)` (cash) then `session.start_hand()`.
2. `while let Some(seat) = session.next_actor()`: build `HandState` from
   `&session.table`; `agent.decide_with_fidelity(&hs).await`; apply
   `finalize_decision` (ported) + floor-raise; map `Decision -> PlayerAction`;
   `session.apply_action(seat, action)` with the same reject→fold fallback as
   `runner.rs:316-353`; accumulate LLM tokens from the fidelity.
3. `session.end_hand()`; build the `HandHistory` via
   `HandHistory::from_table_state_with_ids` (`pkcore/src/hand_history.rs:311`),
   passing stable per-bot `Uuid`s (one per lineup bot, reused across all
   rotations) so stats and the H2H matrix can correlate a bot across seats.

### Phase 4 — mirror + cash orchestration

`mirror.rs` (deck capture, `RotationSchedule` yielding the 6 cyclic perms) +
`orchestrator.rs` cash loop:

```
for group in 0..max_groups {
    deck_str = Cards::deck().shuffle_in_place_with(group_rng).to_string()
    for rotation in schedule.permutations() {     // 6 rotations
        build fresh 6-seat 100bb table -> new PokerSession
        session.set_next_deck(Cards::from_str(&deck_str))
        assign the 6 agents to seats per rotation
        play_one_hand -> collection.push(hh)
    }
}
```

Fresh-table-per-replay guarantees a true duplicate. Invariant to test: each
group is zero-sum and each bot occupies each of the 6 positions exactly once.

### Phase 5 — tournament orchestration

Reuse `blind_schedule::{blind_update_for, BLIND_LEVELS}`
(`crates/pkdealer_service/src/blind_schedule.rs:73,15`) +
`session.{set_blinds, eliminate_busted, count_funded}`:

```
for seed in 0..max_groups {
    for rotation in schedule.permutations() {
        run a full tournament with rotated starting lineup:
          escalate blinds via blind_update_for(hands_completed, hands_per_level)
          eliminate busted; play until count_funded() == 1
        record placement order; push each hand's HandHistory
    }
}
```

For robustness against RNG-draw-count drift, capture each hand's
`shuffled_deck_str` on rotation #1 of a seed and replay via `set_next_deck` on
rotations #2–6 (the same primitive), making replay independent of RNG
bookkeeping.

### Phase 6 — LLM cost budget

Build a cost tracker from `pkdealer_pricing::Pricing` + the LLM seats. Before
scheduling each new group/seed, stop if projected LLM spend would exceed
`llm_cost_cap_usd` or `max_mirror_groups` is reached. **Always finish a whole
group** (never a partial rotation) so positional balance is preserved. Default
`max_mirror_groups` low when any LLM seat is present (rotation multiplies cost
×6; two LLM seats ×6 rotations is expensive).

### Phase 7 — scoring + outputs + bin

All four outputs derive from the single `HandCollection`:

1. **Leaderboard (mode-aware, with CI).** Per-hand per-seat net =
   `ResultEntry.net` (`pkcore/src/hand_history.rs:2355`) keyed by `player_id`.
   - **Cash bb/100:** headline = *grouped estimator* — average the 6 rotations
     within a group into one group-mean, treat the `G` group-means as i.i.d.
     samples, 95% CI = `100 * (mean ± 1.96 * s/sqrt(G))`. This is the
     lower-variance number that mirroring earns. Also report the raw per-hand CI
     and `n` (hands), `G` (groups).
   - **Tournament:** placement points (6th..1st → 0..5) and/or ROI vs a payout
     curve; grouped CI over seed-groups.
   - Note: `pkcore::Confidence` (`player_stats.rs:220`) is only a 3-band
     sample-size label, **not** a numeric CI — the interval is new code here.
2. **Behavioral profile.** Pure passthrough: `StatsRegistry::ingest_collection`
   (`pkcore/src/analysis/player_stats.rs:297`) → VPIP/PFR/3bet/cbet/AF/W@SD per
   bot `Uuid`. Zero new stats code.
3. **Head-to-head matrix.** Per hand, attribute net flows into a
   `BTreeMap<(Uuid,Uuid), f64>`; reuse `HandCollection::hands_by_player`
   (`hand_history.rs:1027`).
4. **Per-hand history export.** `HandCollection::to_yaml` / `save`
   (`hand_history.rs:1196,1221`); JSON via serde.

`bin/arena_eval.rs` is a clap entrypoint over `orchestrator::run(config)`.

---

## Config (TOML)

`arena_eval.toml` reuses the `type`/`profile`/`model` vocabulary of `arena.toml`
so the registry concept transfers, and adds mode + budgets + a 6-bot lineup:

```toml
[format]
mode          = "cash"        # "cash" | "tournament"
seats         = 6
stack_cap_bb  = 100
small_blind   = 50
big_blind     = 100
# tournament-only:
starting_stack_bb = 100
hands_per_level   = 20        # feeds blind_schedule::blind_update_for

[budget]
max_mirror_groups = 2000      # cash: groups of 6 hands (=12k hands); tourney: seeds
llm_cost_cap_usd  = 5.00
pricing_toml      = "pricing.toml"

[[lineup]]                     # exactly 6
name = "gto";    type = "rules";  profile = "gto"
[[lineup]]
name = "lag";    type = "rules";  profile = "loose_aggressive"
[[lineup]]
name = "tag";    type = "rules";  profile = "tight_aggressive"
[[lineup]]
name = "rando";  type = "random"
[[lineup]]
name = "llama";  type = "ollama"; host = "http://127.0.0.1:11434"; model = "llama3.1"
[[lineup]]
name = "claude"; type = "claude"; model = "claude-haiku-4-5"; max_tokens = 256
```

---

## Files to create / modify

| File | Action |
|---|---|
| `pkcore` `src/casino/session.rs` | Modify — add `set_next_deck` + `pending_deck` branch (Phase 0); local version bump + CHANGELOG |
| `crates/pkdealer_arena/Cargo.toml` | Create |
| `crates/pkdealer_arena/src/lib.rs` | Create — public harness API |
| `crates/pkdealer_arena/src/config.rs` | Create — `FormatConfig` + TOML |
| `crates/pkdealer_arena/src/lineup.rs` | Create — in-process agent factory |
| `crates/pkdealer_arena/src/seat_agent.rs` | Create — seedable rules/random seats |
| `crates/pkdealer_arena/src/handstate.rs` | Create — HandState build + `finalize_decision` (ported) |
| `crates/pkdealer_arena/src/mirror.rs` | Create — rotation + deck duplication |
| `crates/pkdealer_arena/src/engine.rs` | Create — `play_one_hand` |
| `crates/pkdealer_arena/src/orchestrator.rs` | Create — mode-aware loops + budget gate |
| `crates/pkdealer_arena/src/corpus.rs` | Create — `HandCollection` wrapper |
| `crates/pkdealer_arena/src/scoring/*.rs` | Create — leaderboard/behavior/h2h/report |
| `crates/pkdealer_arena/src/bin/arena_eval.rs` | Create — clap entrypoint |
| `arena_eval.toml` | Create — example format spec |
| `Cargo.toml` | Modify — add `pkdealer_arena` to workspace members |
| `DEMO.md` | Modify — document `arena_eval` usage |

### Reuse vs. new

**Reuse as-is:** `HandHistory::from_table_state_with_ids`, `HandCollection`
(push/to_yaml/save), the entire `StatsRegistry`/`PlayerStats` pipeline,
`pkcore::bot::{RuleBasedDecider, BotProfile}`, `PokerSession` lifecycle,
`Cards::{deck, from_str, to_string, shuffle_in_place_with}`,
`pkdealer_agent_llm/_ollama/_claude` libs, `pkdealer_pricing`, `blind_schedule`.

**Port (copy + adapt):** `runner.rs` `decide_and_act`/`finalize_decision`/
HandState build (gRPC → in-process); the rules/random agent wrappers (bin → lib,
add seeding).

**Genuinely new:** the `set_next_deck` primitive (pkcore), `mirror.rs`,
the orchestrator loops, the bb/100 grouped-CI estimator, the H2H matrix, the
budget gate, the TOML format spec, and the bin.

---

## Risks

1. **Phase 0 gates cash mirroring.** Without `set_next_deck` you cannot replay a
   deck. Do it first, isolated, backward-compatible.
2. **Tournament RNG-draw drift.** "Same seed → same hands" is fragile if any
   RNG-draw count varies (all-in run-outs). Mitigate by capturing each hand's
   deck on rotation #1 and replaying via `set_next_deck` on #2–6.
3. **Rules decider RNG.** `RuleBasedDecider` rolls dice internally
   (`decider.rs:181…`); the harness must call `decide_seeded`, not the default
   `decide`, or "same deck" replays diverge in actions.
4. **Random agent has no seed** (`agent_random/main.rs:69`) — re-implement
   locally with an injected RNG.
5. **LLM nondeterminism + cost.** Mirroring cancels only *card* variance; LLM
   decision noise remains and consumes budget. The budget gate must finish whole
   groups or positional balance breaks. Surface projected cost prominently.
6. **Cross-repo ordering.** Phase 0 is in `pkcore` (separately published).
   Per project rules: local version bump + CHANGELOG only, **never**
   `cargo publish`. pkcore must bump before `pkdealer_arena` can consume
   `set_next_deck`.

---

## Verification

1. **Phase 0 unit test (pkcore):** capture `shuffled_deck_str`, feed back via
   `set_next_deck`, assert identical hole cards + board; assert the `None` path
   is unchanged.
2. **Mirror invariant test:** one cash group of 6 rotations sums to zero net and
   each bot occupies each of the 6 positions exactly once.
3. **Determinism test:** a rules/random-only run with fixed seeds produces a
   byte-identical `HandCollection` across two runs.
4. **End-to-end (rules only, no cost):**
   `cargo run -p pkdealer_arena --bin arena_eval -- --config arena_eval.toml`
   with a 6-rules cash lineup and `max_mirror_groups=2000` (12k hands); confirm a
   leaderboard with separating CIs, a populated behavioral table, a 6×6 H2H
   matrix, and a written corpus YAML.
5. **LLM smoke + budget:** a lineup with 1 Claude/Ollama seat and a tiny
   `llm_cost_cap_usd`; confirm the run stops on the cap at a group boundary and
   reported spend ≤ cap.
6. **Workspace build:** `OTEL_SDK_DISABLED=true cargo test --workspace` passes;
   `cargo clippy --workspace --all-targets -- -D warnings` clean.
7. Every new public fn/struct gets doc comment + doctest + unit test per
   `CLAUDE.md`; `cargo test --doc -p pkdealer_arena` passes.
