# EPIC-41: Reproducible Agent-vs-Bots Arena Scenarios

## Status

| Component | Status |
|---|---|
| `pkcore` deck-injection primitive (`set_next_deck` + seeded deck gen) | Not started |
| Proto: `ScenarioConfig` / `ScenarioMode` / `ConfigureScenario` RPC | Not started |
| Service: scenario engine (in-process bots, deck injection, stack reset) | Not started |
| Tournament mode | Not started |
| Duplicate mode (reset stacks each hand) | Not started |
| Deck source: seed-generated | Not started |
| Deck source: replayed from recorded session | Not started |
| `run_scenario` driver example | Not started |
| `compare` reporting example | Not started |
| Example scenario files under `scenarios/` | Not started |
| Mirrored/seat-rotated duplicate (positional-luck cancellation) | Future |

---

## Context

We want to run arena sessions where **one LLM agent plays against a table of bots**, and
to **replay the exact same series of hands across different sessions** so different agents
(or models) can be compared head-to-head on an identical environment. This is the
"Backend comparison harness (same hand, multiple models)" item flagged as *Future* in
EPIC-40.

The pieces exist but the loop doesn't close:

- Agents (random, rules, LLM/claude/ollama) all implement
  `PokerAgent::decide(&HandState) -> Decision` and connect over gRPC
  (`SeatPlayer` → `StreamEvents` → `Act`).
- EPIC-25 records every hand including the full 52-card `shuffled_deck` string, plus
  per-action `AgentFidelity` (model id, tokens, intended-vs-applied), to memory and disk.
- **The gap:** the captured deck is *forensic only*. `pkcore::replay()` rebuilds from
  recorded hole cards/board — it never re-deals from a deck, and there is **no way to seed
  the shuffle or inject a deck** at `start_hand()`. So two sessions cannot be made to face
  the same cards.

**Decisions:**

- **Comparison model:** support *both* a continuous **Tournament** mode and a **Duplicate**
  mode (reset stacks each hand → N independent, cleanly-comparable samples), selectable via
  a mode flag.
- **Architecture:** extend the live **gRPC service** with a deterministic *scenario* mode
  (rather than a separate in-process harness).
- **Deck source:** support *both* seed-generated decks and decks replayed from a previously
  recorded session file.

### Key design choice: the service owns the bots

In scenario mode the **service seats and drives the bot opponents in-process** (using
`pkcore::bot::profile::BotProfile` + the already-seedable `RuleBasedDecider`, with a single
scenario-seeded RNG), and **reserves one seat for the external agent-under-test**. The LLM
agent connects exactly as it does today — no agent-side changes. This is what makes the
environment deterministic and lets the service reset stacks (Duplicate) and inject decks.

The intended outcome: commit a scenario file, point any agent binary at it, and get a
recorded session directly comparable to any other agent's run of the same scenario.

---

## Architecture & Phases

### Phase 1 — pkcore: deterministic deck injection (the one core primitive)

`pkcore` is developed locally at `../pkcore`. Bump locally only — **never `cargo publish`**;
do a version bump (0.1.3 → 0.1.4) + CHANGELOG entry.

In `src/casino/session.rs`:

- Add field `pending_deck: Option<Cards>` to `PokerSession` (init `None` in `new`).
- Add `pub fn set_next_deck(&mut self, deck: Cards)` — sets `pending_deck`.
- In `start_hand` (`session.rs:323`, line 328 `self.table.deck.shuffle_in_place();`): if
  `pending_deck` is `Some`, **replace** `self.table.deck` with it instead of shuffling;
  otherwise shuffle as today. The `shuffled_deck_str` capture at line 329 is unchanged, so a
  forced hand still records the deck it played.
- Reuse existing primitives — no new card logic needed:
  - `Cards::deck()` (`cards.rs:72`) builds a fresh 52-card deck.
  - `Cards::shuffle_in_place_with(rng)` (`cards.rs:476`) does a seeded shuffle.
  - `Cards: FromStr` (`cards.rs:824`) parses a recorded deck string back into `Cards`.
- Add `Cards::shuffled_from_seed(seed: u64) -> Cards`
  = `Cards::deck()` then `shuffle_in_place_with(&mut SmallRng::seed_from_u64(seed))`.

A single "inject explicit deck" primitive covers *both* deck sources: seed-generated decks
and decks replayed from a recorded session are both just a `Cards` order handed to
`start_hand`.

### Phase 2 — proto: scenario configuration

In `proto/dealer.proto`:

- `enum ScenarioMode { SCENARIO_MODE_UNSPECIFIED = 0; SCENARIO_MODE_TOURNAMENT = 1; SCENARIO_MODE_DUPLICATE = 2; }`
- `message ScenarioSeat { uint32 seat = 1; string role = 2; uint32 chips = 3; }` — `role`
  is a `BotProfile` name (e.g. `"tight_aggressive"`) or the literal `"agent"` to reserve the
  seat for the external agent-under-test.
- `message ScenarioConfig { string id = 1; uint64 seed = 2; ScenarioMode mode = 3;
  repeated ScenarioSeat seats = 4; uint32 small_blind = 5; uint32 big_blind = 6;
  uint32 button_seat = 7; uint32 num_hands = 8; repeated string decks = 9; }` — `decks`
  empty → generate from `seed`; non-empty → replay these exact deck strings, `decks[i]` for
  hand `i`.
- `rpc ConfigureScenario(ConfigureScenarioRequest) returns (ConfigureScenarioResponse);`
  with `ConfigureScenarioRequest { ScenarioConfig config = 1; }`; the response carries
  resolved seat assignments / the agent seat number (so the agent client knows where to sit)
  or an `error`.

### Phase 3 — service: scenario engine

File: `crates/pkdealer_service/src/main.rs`.

1. **State on `TableState`** (struct at `main.rs:257`): add `scenario: Option<ScenarioState>`
   holding the parsed config, the seeded bot RNG (`SmallRng`), a `seat -> BotProfile` map,
   the agent seat(s), per-seat starting chips, the fixed button seat, and the deck source
   (precomputed `Vec<Cards>` for replay, or seed for generation).
2. **`ConfigureScenario` handler:** validate, seat the bot players in-process
   (`SeatsNoCell` + `PlayerNoCell::new_with_chips`, mirroring `demo.rs:51-60`), leave the
   `"agent"` seat empty for an external `SeatPlayerAt`, set blinds/button, store the state,
   reset the recorder.
3. **Deck injection on `StartHand`:** before driving the hand, if a scenario is active,
   compute the deck for the current hand index (`scenario.decks[i]` if provided, else
   `Cards::shuffled_from_seed(seed.wrapping_add(hand_index))`) and call
   `session.set_next_deck(deck)`. In **Duplicate** mode also reset every seat's chips to the
   starting stacks and pin the button to `button_seat`; in **Tournament** mode rotate the
   button and eliminate busted as `demo.rs` does.
4. **Auto-play bot seats:** in the existing auto-advance loop (`main.rs:~1716-1757`), when
   `SessionStep::PlayerToAct(seat)` is a **bot** seat, compute its action via
   `RuleBasedDecider.decide(&profile, &snapshot)` using the scenario RNG (the
   `HandState→TableSnapshot` conversion already exists in `pkdealer_agent_rules`), apply it,
   and continue the loop. Only **break** (wait for an external `Act`) when the seat is the
   **agent** seat; stop at `HandComplete`. The agent-under-test stays fully external and
   unchanged. When *all* seats are bots (no `"agent"` role), the hand runs to completion with
   no external input — doubling as a baseline-deck/recording generator.
5. **Recording** is unchanged — the existing hand-end hook (`main.rs:1768-1862`) captures the
   played deck, actions, stacks, and zips `hand_agent_fidelity`. Each scenario run produces a
   normal recorded `HandCollection` (disk via `PKDEALER_RECORD_DIR` or `ExportSession`).

### Phase 4 — orchestration, comparison, scenario files

- **Scenario files:** add a `scenarios/` dir with example YAML scenarios (one LLM seat + a
  table of bot profiles, both modes) — the shareable/reproducible artifacts. A scenario can
  also be produced from a recorded session by extracting each hand's `shuffled_deck` into
  `decks`.
- **Driver:** `crates/pkdealer_client/examples/run_scenario.rs` — loads a scenario YAML,
  calls `ConfigureScenario`, waits for the external agent to `SeatPlayerAt` the agent seat,
  drives `num_hands` via `StartHand`, then `ExportSession` to a per-agent file. Run once per
  agent binary to compare.
- **Comparison/report:** `crates/pkdealer_client/examples/compare.rs` — loads N exported
  session files via `HandCollection::from_yaml`/`from_json`; for Duplicate mode aligns by
  hand index (same deck) to compute each agent's agent-seat net P&L per hand plus aggregate
  (total, mean bb/100); for Tournament mode reports cumulative P&L / finish. Uses
  `AgentFidelity.model` to label each run. Mirrors the read-only style of `examples/audit.rs`.

---

## Reproducibility caveat

A fixed deck fully determines the cards dealt regardless of actions. In **Duplicate** mode,
resetting stacks each hand keeps every hand an identical, independent situation → clean
hand-by-hand comparison. In **Tournament** mode, once an agent plays differently from
another, stacks and eliminations legitimately diverge — the cards per hand index are still
identical, but compare cumulative results, not hand-by-hand. LLM agents are themselves
stochastic, so exact bit-for-bit reproduction is only guaranteed for deterministic agents
(e.g. a seeded rules bot); the value here is a *controlled, identical environment*, which is
what enables fair comparison.

*Future enhancement: rotate the agent's seat across runs — "mirrored" duplicate — to cancel
positional luck.*

---

## Files to create / modify

- `pkcore/src/casino/session.rs`, `pkcore/src/cards.rs`, `pkcore/CHANGELOG.md`,
  `pkcore/Cargo.toml` (version bump) — Phase 1.
- `proto/dealer.proto` — Phase 2.
- `crates/pkdealer_service/src/main.rs` (+ bump `pkcore = "0.1.4"` in its `Cargo.toml`;
  same for `pkdealer_client`) — Phase 3.
- `scenarios/*.yaml`, `crates/pkdealer_client/examples/run_scenario.rs`,
  `crates/pkdealer_client/examples/compare.rs` — Phase 4.

---

## Verification

1. **pkcore:** `cargo test` in pkcore — deck-injection determinism unit/doctests pass (two
   `start_hand`s with the same injected deck produce identical hole cards and board;
   `shuffled_from_seed(s)` stable for fixed `s`). Test names omit the `test_` prefix per repo
   convention.
2. **Service unit tests** (`main.rs` test module): `ConfigureScenario` + `StartHand` deals
   the injected deck; Duplicate mode resets stacks and pins the button; Tournament rotates the
   button; bot seats auto-play without an external `Act`.
3. **Determinism integration test:** run a full scenario twice with the **same deterministic
   agent** (seeded rules bot in the agent seat) and assert the two exported `HandCollection`s
   are byte-identical.
4. **End-to-end (manual):**
   - `cargo build` workspace (proto regenerates).
   - Start service with `PKDEALER_RECORD_DIR=./out`.
   - `cargo run -p pkdealer_client --example run_scenario -- scenarios/heads_up_vs_bots.yaml`
     while pointing `pkdealer_agent_claude` (then a different model/agent) at the agent seat;
     confirm two session files are produced.
   - `cargo run -p pkdealer_client --example compare -- out/agentA.yaml out/agentB.yaml`
     prints a per-deck comparison table.
   - Confirm both runs faced identical decks (diff the `shuffled_deck` fields per hand index).
5. Every new public fn/struct gets doc + doctest + unit test per `CLAUDE.md`; run
   `cargo test --doc`.
