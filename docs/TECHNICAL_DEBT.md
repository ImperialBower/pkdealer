# Technical Debt

> Maintained by the `/backlog` skill. Items tagged 🤖 were proposed by automated
> review — review and edit them; they are suggestions, not facts. Promote the ones
> worth doing up to **Tracked debt**, delete the rest.

## Tracked debt
<!-- Human-authored and code-comment-sourced items. -->

_No `TODO`/`FIXME`/`HACK`/`XXX` markers exist in the Rust source — the codebase
follows the strict CLAUDE.md discipline. The items below are machine-proposed; move
any you accept into this section._

## 🤖 Automated review findings
<!-- Machine-proposed (one-time review, 2026-06-20). Measured against CLAUDE.md
     house rules (no unwrap/expect/panic in lib; enums over strings; doctests+tests
     on public APIs; no unchecked `as` casts). Promote good ones up; delete the rest. -->

### High
- [ ] 🤖 **File-wide `#![allow(clippy::cast_possible_truncation)]` masks real truncation** — naked `usize → u32` casts in `build_table_status` (`chips`, `withdrawn`, `chips_in_play`, `pot`, …) and `now_unix_ms` (`as_millis() as u64`) silently wrap; the global allow hides them. Suggested: replace with `u32::try_from(..).unwrap_or(u32::MAX)` (pattern already in `compute_profit_loss`) and scope the allow narrowly. (`crates/pkdealer_service/src/main.rs:2,608-650,876`)
- [ ] 🤖 **`req.seat as u8` can truncate proto `u32` seat to wrong seat** — seat comes from a `u32`; values >255 wrap. Suggested: `u8::try_from(req.seat).map_err(|_| Status::invalid_argument("seat out of range"))?`. (`crates/pkdealer_service/src/main.rs:1412,1483`)
- [ ] 🤖 **O(n) eviction in bounded recorder** — `recorder.hands.remove(0)` in a `while` loop is O(max²) per hand for large `record_max_hands`. Suggested: `VecDeque`, or a single `drain(..count)`. (`crates/pkdealer_service/src/main.rs:2132-2134`)

### Medium
- [ ] 🤖 **`round_number as u32` / `recorder.len() as u32` unchecked casts** — wrap silently past 4G. Use `u32::try_from(..).unwrap_or(u32::MAX)`. (`crates/pkdealer_service/src/main.rs:650,2425,2457`)
- [ ] 🤖 **`HandState::street` is stringly-typed** — `"preflop"/"flop"/…` compared by string equality across the codebase; CLAUDE.md says use enums for fixed sets. Suggested: a `Street`/`StreetKind` enum with a `Display`. (`crates/pkdealer_agent_core/src/hand_state.rs:49`, e.g. compared at `runner.rs:303`)
- [ ] 🤖 **`DealerConfig::from_env` has no direct test** — the primary boot path is only covered via individual helpers; a `PKDEALER_PRICING` + `PKDEALER_PRICE_AS` interaction bug wouldn't be caught. Suggested: a from_env integration test with a known env combo. (`crates/pkdealer_service/src/main.rs:177-198`)
- [ ] 🤖 **`hand_agent_fidelity` unbounded within a hand** — accumulates per accepted `act`; cleared on `start_hand`/consumed at `HandComplete`, but a never-completing hand grows it without cap. Suggested: defensive cap or documented lifetime bound. (`crates/pkdealer_service/src/main.rs:1876`)
- [ ] 🤖 **Silent error-body swallow in backends** — `resp.text().await.unwrap_or_default()` on the error path hides a network read failure behind the outer error. (`crates/pkdealer_agent_claude/src/lib.rs:148`, `crates/pkdealer_agent_ollama/src/lib.rs:121`)
- [ ] 🤖 **`parse_action_opt` undocumented coercion + no edge tests** — `"raise to 250."` / `"I'll raise to 250"` silently fall back to Fold/Check (`was_coerced=true`, no diagnostic). Suggested: document the behavior and add a trailing-punctuation test. (`crates/pkdealer_agent_llm/src/parse.rs:56-86`)
- [ ] 🤖 **Unknown proto event codes map to `Unspecified` with no log** — makes a proto mismatch silent. Add a `tracing::warn!` on the fallback. (`crates/pkdealer_agent_core/src/runner.rs:145`)
- [ ] 🤖 **`cost_usd` precision ceiling undocumented** — `u64 → f64` loses precision above 2^53 tokens; safe today but the `#[allow(cast_precision_loss)]` carries no justification. Suggested: note the ceiling in the doc comment. (`crates/pkdealer_pricing/src/lib.rs:56-57`)

### Low
- [ ] 🤖 **`build_prompt` needless clones** — clones `board` and `join`s `action_history` each call. Pass `&state.board`; build the history string more cheaply. (`crates/pkdealer_agent_llm/src/prompt.rs:39-41`)
- [ ] 🤖 **costsim `temp_file` test helper theoretically collides** — PID + nanos is near-unique but not collision-proof under parallel runs. Low risk (files are cleaned up). (`crates/pkdealer_costsim/src/app.rs:463-471`)

### Cross-cutting / known
- [ ] 🤖 **DRY `parse_price_as`** — duplicated in `pkdealer_costsim::app` (CLI) and `pkdealer_service` (env); lift into `pkdealer_pricing`. (EPIC-44 backlog B4)
- [ ] 🤖 **e2e test port-bind flakiness** — `crates/pkdealer_service/tests/e2e_two_players.rs` binds ephemeral ports and occasionally fails under contention; intermittent only.
