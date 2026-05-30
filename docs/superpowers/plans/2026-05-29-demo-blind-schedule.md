# Demo Blind Schedule Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the `aiarena` and `botarena` demos an escalating tournament blind structure that rises one level every N hands (default 20) and recycles — capping over-stacked players back to the default rebuy value and resetting blinds — once the top level finishes.

**Architecture:** A new pure module `blind_schedule` (in the service's `lib.rs` so its doc tests run) decides blinds for the upcoming hand from a running `hands_completed` counter. `pkdealer_service` reads two new `DealerConfig` env vars, maintains the counter, and — when enabled and no hand is in progress — applies the decision in `start_hand` (cap stacks on cycle wrap, then `PokerSession::set_blinds`). The feature is turned on for the demos via `docker-compose.yml`, off by default everywhere else.

**Tech Stack:** Rust, pkcore 0.1.2 (`PokerSession`, `ForcedBets`, `PlayerNoCell`), tonic/gRPC, Docker Compose.

---

## File Structure

- **Create** `crates/pkdealer_service/src/blind_schedule.rs` — `BLIND_LEVELS`, `BlindUpdate`, `blind_update_for`. Pure, no I/O. Unit + doc tested.
- **Modify** `crates/pkdealer_service/src/lib.rs` — export `pub mod blind_schedule;`.
- **Modify** `crates/pkdealer_service/src/main.rs` —
  - new constant `DEFAULT_HANDS_PER_LEVEL`;
  - new `DealerConfig` fields + `from_env` + `Default` + a `parse_hands_per_level` helper;
  - `TableState.hands_completed` field + init;
  - increment `hands_completed` after `end_hand`;
  - a `cap_stacks_to` free fn;
  - schedule wiring in `start_hand`;
  - module-level env-var doc table rows.
- **Modify** `docker-compose.yml` — two env vars on `pkdealer_service`.
- **Modify** `crates/pkdealer_service/README.md` — env-var rows + behaviour subsection.

Note on test naming: this project forbids the `test_` prefix on Rust test fn names. Use descriptive names like `blind_update_for_first_hand`.

---

## Task 1: `blind_schedule` module (pure schedule logic)

**Files:**
- Create: `crates/pkdealer_service/src/blind_schedule.rs`
- Modify: `crates/pkdealer_service/src/lib.rs`

- [ ] **Step 1: Export the module**

In `crates/pkdealer_service/src/lib.rs`, after the `pub mod otel;` line, add:

```rust
pub mod blind_schedule;
```

- [ ] **Step 2: Write the module with the failing tests**

Create `crates/pkdealer_service/src/blind_schedule.rs` with the full content below. (Implementation and tests are written together here because the doc test on the public fn must compile; the unit tests are what we run to verify behaviour.)

```rust
//! Tournament blind schedule for the demo arenas.
//!
//! Blinds escalate one level every `hands_per_level` hands, following the
//! 12-level [`BLIND_LEVELS`] table (identical to the `pkarena0-web`
//! schedule). The top level is **not** terminal: once it plays out its
//! `hands_per_level` hands the cycle wraps back to level 0, and the wrap is
//! signalled by [`BlindUpdate::reset_stacks`] so the caller can cap
//! over-large stacks back to the starting amount.
//!
//! This module is pure — it performs no I/O and holds no state. The caller
//! owns the `hands_completed` counter.

/// `(small_blind, big_blind)` for each level. Values match the
/// `pkarena0-web` `BLIND_LEVELS` array.
pub const BLIND_LEVELS: [(usize, usize); 12] = [
    (50, 100),
    (100, 200),
    (150, 300),
    (200, 400),
    (300, 600),
    (400, 800),
    (500, 1000),
    (750, 1500),
    (1000, 2000),
    (1500, 3000),
    (2000, 4000),
    (3000, 6000),
];

/// The blind decision for the hand that is about to start.
///
/// Produced by [`blind_update_for`]. `small_blind` / `big_blind` are the
/// blinds to post for the upcoming hand; `level` is its 0-based level index
/// into [`BLIND_LEVELS`]; `reset_stacks` is true exactly on the hand that
/// begins a fresh cycle (every player over the starting stack should be
/// capped back down before this hand is dealt).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlindUpdate {
    pub small_blind: usize,
    pub big_blind: usize,
    pub level: usize,
    pub reset_stacks: bool,
}

/// Decides the blinds for the upcoming hand from the number of hands already
/// completed.
///
/// `hands_completed` is the count of hands that have already finished, so the
/// hand about to start is hand number `hands_completed + 1`. Level 0 therefore
/// covers completed-counts `0..hands_per_level`.
///
/// A `hands_per_level` of 0 is treated as 1 so the function never divides by
/// zero.
///
/// # Examples
///
/// ```
/// use pkdealer_service::blind_schedule::blind_update_for;
///
/// // First hand of the tournament: level 0, 50/100, no reset.
/// let upd = blind_update_for(0, 20);
/// assert_eq!(upd.small_blind, 50);
/// assert_eq!(upd.big_blind, 100);
/// assert_eq!(upd.level, 0);
/// assert!(!upd.reset_stacks);
///
/// // Hand 241 (240 completed) wraps the cycle back to level 0 and resets.
/// let wrap = blind_update_for(240, 20);
/// assert_eq!(wrap.level, 0);
/// assert!(wrap.reset_stacks);
/// ```
#[must_use]
pub fn blind_update_for(hands_completed: u64, hands_per_level: usize) -> BlindUpdate {
    let per = hands_per_level.max(1);
    let cycle = BLIND_LEVELS.len() * per;
    let pos = (hands_completed % cycle as u64) as usize;
    let level = pos / per;
    let (small_blind, big_blind) = BLIND_LEVELS[level];
    BlindUpdate {
        small_blind,
        big_blind,
        level,
        reset_stacks: hands_completed > 0 && pos == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blind_update_for_first_hand() {
        let upd = blind_update_for(0, 20);
        assert_eq!(upd.level, 0);
        assert_eq!((upd.small_blind, upd.big_blind), (50, 100));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_mid_level_no_reset() {
        let upd = blind_update_for(15, 20);
        assert_eq!(upd.level, 0);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_level_boundary() {
        let upd = blind_update_for(20, 20);
        assert_eq!(upd.level, 1);
        assert_eq!((upd.small_blind, upd.big_blind), (100, 200));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_last_level() {
        // 11 * 20 = 220 completed → level 11 (top), 3000/6000.
        let upd = blind_update_for(220, 20);
        assert_eq!(upd.level, 11);
        assert_eq!((upd.small_blind, upd.big_blind), (3000, 6000));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_top_level_internal_hand() {
        // Still inside the top level (no wrap yet).
        let upd = blind_update_for(239, 20);
        assert_eq!(upd.level, 11);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_cycle_wrap_resets() {
        let upd = blind_update_for(240, 20);
        assert_eq!(upd.level, 0);
        assert_eq!((upd.small_blind, upd.big_blind), (50, 100));
        assert!(upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_second_cycle_boundary() {
        // 260 completed = 240 (one full cycle) + 20 → level 1, no reset.
        let upd = blind_update_for(260, 20);
        assert_eq!(upd.level, 1);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_zero_per_level_does_not_panic() {
        // per normalises to 1: cycle = 12, pos = 5 % 12 = 5, level 5.
        let upd = blind_update_for(5, 0);
        assert_eq!(upd.level, 5);
        assert!(!upd.reset_stacks);
    }
}
```

- [ ] **Step 3: Run the unit tests and verify they pass**

Run: `cargo test -p pkdealer_service blind_schedule`
Expected: all `blind_update_for_*` tests PASS.

- [ ] **Step 4: Run the doc test and verify it passes**

Run: `cargo test -p pkdealer_service --doc blind_schedule`
Expected: 1 doc test for `blind_update_for` PASSES.

- [ ] **Step 5: Commit**

```bash
git add crates/pkdealer_service/src/blind_schedule.rs crates/pkdealer_service/src/lib.rs
git commit -m "feat(service): add tournament blind_schedule module"
```

---

## Task 2: `DealerConfig` env wiring

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs`

- [ ] **Step 1: Add the default constant**

In `main.rs`, immediately after the existing line `const DEFAULT_REBUY_AMOUNT: usize = 10_000;` (~line 98), add:

```rust
/// Default number of hands played at each blind level before the schedule
/// advances. Used when the blind schedule is enabled but
/// `PKDEALER_HANDS_PER_LEVEL` is unset, unparseable, or zero.
const DEFAULT_HANDS_PER_LEVEL: usize = 20;
```

- [ ] **Step 2: Add the two fields to `DealerConfig`**

In the `struct DealerConfig { ... }` block (~lines 118-128), after the `topup_enabled: bool,` field, add:

```rust
    /// When true, the service escalates blinds on a fixed schedule
    /// (see [`pkdealer_service::blind_schedule`]) and recycles the table at
    /// the top of the schedule. Off by default; the demos enable it via
    /// `docker-compose.yml`.
    blind_schedule_enabled: bool,
    /// Number of hands played at each blind level before advancing. Only
    /// consulted when `blind_schedule_enabled` is true.
    hands_per_level: usize,
```

- [ ] **Step 3: Add the parse helper**

In `main.rs`, immediately after the `fn parse_env_bool(key: &str) -> bool { ... }` function (~line 165), add:

```rust
/// Parses `PKDEALER_HANDS_PER_LEVEL`-style input into a positive
/// hands-per-level value. Unset, unparseable, or zero all fall back to
/// [`DEFAULT_HANDS_PER_LEVEL`] so a typo can't divide the schedule by zero.
///
/// Split out from env reading so it can be unit-tested without env races.
fn parse_hands_per_level(raw: Option<String>) -> usize {
    raw.and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_HANDS_PER_LEVEL)
}
```

- [ ] **Step 4: Populate the fields in `from_env`**

In `DealerConfig::from_env` (~lines 133-142), inside the returned `DealerConfig { ... }`, after the `topup_enabled: parse_env_bool("PKDEALER_TOPUP_ENABLED"),` line, add:

```rust
            blind_schedule_enabled: parse_env_bool("PKDEALER_BLIND_SCHEDULE_ENABLED"),
            hands_per_level: parse_hands_per_level(env::var("PKDEALER_HANDS_PER_LEVEL").ok()),
```

- [ ] **Step 5: Populate the fields in `Default`**

In `impl Default for DealerConfig` (~lines 145-152), inside `DealerConfig { ... }`, after `topup_enabled: false,`, add:

```rust
            blind_schedule_enabled: false,
            hands_per_level: DEFAULT_HANDS_PER_LEVEL,
```

- [ ] **Step 6: Add unit tests for the helper and defaults**

In `main.rs`, locate the `#[cfg(test)] mod tests { ... }` block (starts ~line 1834). Add these tests inside it (place them near the top of the module body, right after its `use` statements):

```rust
    #[test]
    fn parse_hands_per_level_defaults_when_absent() {
        assert_eq!(parse_hands_per_level(None), DEFAULT_HANDS_PER_LEVEL);
    }

    #[test]
    fn parse_hands_per_level_defaults_on_garbage() {
        assert_eq!(parse_hands_per_level(Some("nope".to_owned())), DEFAULT_HANDS_PER_LEVEL);
    }

    #[test]
    fn parse_hands_per_level_defaults_on_zero() {
        assert_eq!(parse_hands_per_level(Some("0".to_owned())), DEFAULT_HANDS_PER_LEVEL);
    }

    #[test]
    fn parse_hands_per_level_accepts_positive() {
        assert_eq!(parse_hands_per_level(Some("30".to_owned())), 30);
    }

    #[test]
    fn dealer_config_default_disables_blind_schedule() {
        let cfg = DealerConfig::default();
        assert!(!cfg.blind_schedule_enabled);
        assert_eq!(cfg.hands_per_level, DEFAULT_HANDS_PER_LEVEL);
    }
```

- [ ] **Step 7: Run the tests and verify they pass**

Run: `cargo test -p pkdealer_service parse_hands_per_level`
Then: `cargo test -p pkdealer_service dealer_config_default_disables_blind_schedule`
Expected: all PASS. (Compilation also confirms the new struct fields are consistent.)

- [ ] **Step 8: Commit**

```bash
git add crates/pkdealer_service/src/main.rs
git commit -m "feat(service): add blind-schedule config (PKDEALER_BLIND_SCHEDULE_ENABLED, PKDEALER_HANDS_PER_LEVEL)"
```

---

## Task 3: `hands_completed` counter on `TableState`

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs`

- [ ] **Step 1: Add the field**

In `struct TableState { ... }` (~lines 186-207), after the `last_prompt_at: Option<std::time::Instant>,` field, add:

```rust
    /// Count of hands that have fully completed (`end_hand` succeeded). Drives
    /// the blind schedule when `blind_schedule_enabled` is set. Monotonic for
    /// the life of the process.
    hands_completed: u64,
```

- [ ] **Step 2: Initialise it**

In `new_with_config`, inside the `TableState { ... }` literal (~lines 300-309), after `last_prompt_at: None,`, add:

```rust
            hands_completed: 0,
```

- [ ] **Step 3: Increment it after a completed hand**

In `act`, in the `SessionStep::HandComplete` arm, find the line `self.metrics.hands_played.add(1, &[]);` (~line 1314). Immediately after it, add:

```rust
                                    guard.hands_completed += 1;
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p pkdealer_service`
Expected: builds clean (no unused-field warning — the field is written in Task 3 and read in Task 4; if running Task 3 standalone, an unused-read warning is acceptable and resolved by Task 4).

- [ ] **Step 5: Commit**

```bash
git add crates/pkdealer_service/src/main.rs
git commit -m "feat(service): track hands_completed on TableState"
```

---

## Task 4: Stack-cap helper + `start_hand` wiring

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs`

- [ ] **Step 1: Add the `cap_stacks_to` free function with a failing test**

In `main.rs`, add this free function just before `fn compute_profit_loss` (~line 688, the `// ─ Computes a player's cumulative profit/loss` comment block — place `cap_stacks_to` immediately above that comment):

```rust
/// Caps every occupied seat's stack to `cap`, leaving stacks already at or
/// below `cap` untouched. Returns `(seat_index, handle, old_chips)` for each
/// seat that was reduced, so the caller can log/emit the change.
///
/// Touches only `chips` (not `withdrawn`), so per-seat profit/loss tracking
/// stays cumulative across cycles — matching the demo's documented behaviour.
///
/// # Examples
///
/// ```
/// # // illustrative — `cap_stacks_to` is private to the service binary.
/// // A seat with 300_000 chips and a cap of 10_000 ends at 10_000;
/// // a seat with 1_000 chips is left at 1_000.
/// ```
fn cap_stacks_to(session: &mut PokerSession, cap: usize) -> Vec<(u8, String, usize)> {
    let mut capped = Vec::new();
    let table = &mut session.table;
    let size = table.seats.size();
    for i in 0..size {
        if let Some(seat) = table.seats.get_seat_mut(i)
            && !seat.is_empty()
            && seat.player.chips > cap
        {
            let old = seat.player.chips;
            seat.player.chips = cap;
            capped.push((i, seat.player.handle.clone(), old));
        }
    }
    capped
}
```

Then add this test inside the `#[cfg(test)] mod tests { ... }` block (near the other new unit tests from Task 2):

```rust
    #[test]
    fn cap_stacks_to_reduces_only_oversized_stacks() {
        use pkcore::casino::game::ForcedBets;
        use pkcore::casino::session::PokerSession;
        use pkcore::casino::table_no_cell::{
            PlayerNoCell, SeatNoCell, SeatsNoCell, TableNoCell,
        };

        let seats = SeatsNoCell::new(vec![
            SeatNoCell::new(PlayerNoCell::new_with_chips("rich".to_string(), 300_000)),
            SeatNoCell::new(PlayerNoCell::new_with_chips("poor".to_string(), 1_000)),
            SeatNoCell::new(PlayerNoCell::new_with_chips("exact".to_string(), 10_000)),
        ]);
        let mut session =
            PokerSession::new(TableNoCell::nlh_from_seats(seats, ForcedBets::new(50, 100)));

        let capped = cap_stacks_to(&mut session, 10_000);

        // Only the 300k stack is reduced.
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].0, 0);
        assert_eq!(capped[0].2, 300_000);
        assert_eq!(session.table.seats.get_seat(0).unwrap().player.chips, 10_000);
        assert_eq!(session.table.seats.get_seat(1).unwrap().player.chips, 1_000);
        assert_eq!(session.table.seats.get_seat(2).unwrap().player.chips, 10_000);
    }
```

- [ ] **Step 2: Run the test to verify it fails, then passes after Step 1**

Run: `cargo test -p pkdealer_service cap_stacks_to_reduces_only_oversized_stacks`
Expected: PASS (the helper and test are added together; a red-first run is unnecessary since the helper is new code with its own test).

- [ ] **Step 3: Add the schedule import**

At the top of `main.rs`, with the other `use pkdealer_service::` imports (~line 40, near `use pkdealer_service::otel;`), add:

```rust
use pkdealer_service::blind_schedule::blind_update_for;
```

- [ ] **Step 4: Apply the schedule in `start_hand`**

In `start_hand`, inside the `let mut guard = self.lock()?;` block, AFTER the `if guard.session.count_funded() < 2 { ... return ... }` guard (which ends ~line 1008) and BEFORE `match guard.session.start_hand() {` (~line 1009), insert:

```rust
            // Tournament blind schedule (demo-only; off by default). Apply
            // only when no hand is in progress so a losing multi-agent
            // start_hand race — which is about to return the benign "already
            // in progress" error below — cannot cap stacks or change blinds.
            if self.config.blind_schedule_enabled && !guard.session.is_hand_in_progress() {
                let upd = blind_update_for(guard.hands_completed, self.config.hands_per_level);
                if upd.reset_stacks {
                    let cap = self.config.default_rebuy_amount;
                    let capped = cap_stacks_to(&mut guard.session, cap);
                    if !capped.is_empty() {
                        let names = capped
                            .iter()
                            .map(|(_, h, _)| h.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let status = Self::build_table_status(
                            &guard.session,
                            CardVisibility::Spectator,
                        );
                        self.emit_event(
                            EventType::HandStarted,
                            format!(
                                "Blind cycle reset — stacks capped to {cap}, blinds back to \
                                 {}/{} (capped: {names})",
                                upd.small_blind, upd.big_blind
                            ),
                            status,
                        );
                    }
                }
                guard
                    .session
                    .set_blinds(ForcedBets::new(upd.small_blind, upd.big_blind));
            }
```

- [ ] **Step 5: Fold the level into the "Hand started" event description**

Still in `start_hand`, in the `Ok(())` arm of `match guard.session.start_hand()`, find the event tuple (~lines 1033-1037):

```rust
                    let event = (
                        EventType::HandStarted,
                        "Hand started".to_owned(),
                        event_status,
                    );
```

Replace the middle (description) element so the blinds/level appear when the schedule is on:

```rust
                    let hand_desc = if self.config.blind_schedule_enabled {
                        let fb = guard.session.table.forced;
                        format!(
                            "Hand started — blinds {}/{}",
                            fb.small_blind, fb.big_blind
                        )
                    } else {
                        "Hand started".to_owned()
                    };
                    let event = (EventType::HandStarted, hand_desc, event_status);
```

- [ ] **Step 6: Run the full service test suite and verify it passes**

Run: `cargo test -p pkdealer_service`
Expected: all tests PASS, including the new `cap_stacks_to_*`, `blind_update_for_*`, `parse_hands_per_level_*`, and existing tests. No unused-field/import warnings remain.

- [ ] **Step 7: Clippy clean**

Run: `cargo clippy -p pkdealer_service --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/pkdealer_service/src/main.rs
git commit -m "feat(service): apply tournament blind schedule and stack reset in start_hand"
```

---

## Task 5: Enable for demos + documentation

**Files:**
- Modify: `docker-compose.yml`
- Modify: `crates/pkdealer_service/README.md`
- Modify: `crates/pkdealer_service/src/main.rs` (module doc env table)

- [ ] **Step 1: Turn the feature on in compose**

In `docker-compose.yml`, in the `pkdealer_service:` service's `environment:` block, after the `PKDEALER_REBUY_AMOUNT: "10000"` line (~line 24), add:

```yaml
      # Demo tournament blinds: escalate one level every 20 hands through the
      # 12-level schedule, then cap stacks back to PKDEALER_REBUY_AMOUNT and
      # restart. Shared service runs only under the aiarena/botarena profiles,
      # so this scopes the feature to those demos.
      PKDEALER_BLIND_SCHEDULE_ENABLED: "true"
      PKDEALER_HANDS_PER_LEVEL: "20"
```

- [ ] **Step 2: Validate the compose file parses**

Run: `docker compose config >/dev/null`
Expected: exits 0, no YAML/schema error. (If `docker` is unavailable in the environment, instead confirm the indentation matches the surrounding `environment:` keys by eye.)

- [ ] **Step 3: Add the env vars to the `main.rs` module doc table**

In `main.rs`, in the module-level env-var table (the `//! | Variable ... |` rows, ~lines 17-23), after the `PKDEALER_TOPUP_ENABLED` row, add two rows (align the columns with the existing rows):

```rust
//! | `PKDEALER_BLIND_SCHEDULE_ENABLED` | false             | Escalate blinds every N hands and recycle stacks at the top (demo)     |
//! | `PKDEALER_HANDS_PER_LEVEL`        | 20                | Hands per blind level when the schedule is enabled                      |
```

- [ ] **Step 4: Document behaviour in the service README**

In `crates/pkdealer_service/README.md`, add the two variables to its env-var table (matching that table's column layout) and add a short subsection. Use this prose:

```markdown
### Tournament blind schedule

When `PKDEALER_BLIND_SCHEDULE_ENABLED=true`, the service escalates blinds on a
fixed 12-level schedule (50/100 up to 3,000/6,000 — the same values as
pkarena0-web), advancing one level every `PKDEALER_HANDS_PER_LEVEL` hands
(default 20). The top level is not terminal: after it plays out its hands the
table recycles — every stack above `PKDEALER_REBUY_AMOUNT` is capped back down
to it (smaller stacks are left alone), blinds drop to 50/100, and escalation
starts over. A full cycle is `12 × PKDEALER_HANDS_PER_LEVEL` hands (240 by
default).

The flag is off by default, so plain `cargo run` and the test suite keep the
fixed 50/100 blinds. The `aiarena` and `botarena` demos enable it via
`docker-compose.yml`. Stack caps touch only the chip stack, so the per-seat
profit/loss metric stays cumulative across cycles (it steps down at each
reset).
```

- [ ] **Step 5: Verify docs build**

Run: `cargo doc -p pkdealer_service --no-deps`
Expected: builds without warnings about the module doc comment.

- [ ] **Step 6: Commit**

```bash
git add docker-compose.yml crates/pkdealer_service/README.md crates/pkdealer_service/src/main.rs
git commit -m "feat(demo): enable tournament blind schedule for aiarena/botarena + docs"
```

---

## Final verification

- [ ] **Run the whole workspace test suite**

Run: `cargo test`
Expected: all tests PASS.

- [ ] **Run doc tests**

Run: `cargo test --doc`
Expected: `blind_update_for` doc test PASSES along with the rest.

- [ ] **Clippy across the workspace**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Manual demo smoke check (optional, requires Docker + ollama for aiarena)**

Run: `PKDEALER_HANDS_PER_LEVEL=3 ./bin/botarena` then `docker compose logs -f pkdealer_service`
Expected: "Hand started — blinds X/Y" descriptions escalate every 3 hands; after `12 × 3 = 36` hands a "Blind cycle reset" event appears and blinds return to 50/100. Tear down with `docker compose down -v`.
```
