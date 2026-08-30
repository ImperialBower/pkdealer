# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.26] - 2026-08-30

### Changed

- Upgraded to `pkcore` 0.11.0 (from 0.7.0). None of the four releases'
  breaking changes reach pkdealer, which consumes `hand_history`, `bot`,
  `cards` and `casino::equity` and nothing else:
  - 0.8.0 removed the `TableCelled` family and re-based `Dealer` on
    `casino::table::Table` — pkdealer never used either.
  - 0.10.0 added Pluribus-format export; additive.
  - 0.11.0 dropped `store` and `terminal` from the default features, moved the
    combinatorics signatures to `impl Iterator`, removed
    `FIVE_CARD_COMBOS`/`Deck::to_par_iter`, and deprecated
    `TableManager`/`TableEvent` — pkdealer requests none of those features and
    calls none of those items. (`pkdealer`'s own `TableEvent` is the proto
    type, unrelated.)
  - 0.11.0 also cut the `EquityOptions::max_samples` default from 100,000 to
    25,000. Silent for most consumers, but not for pkdealer:
    `pkdealer_agent_rules` always passes an explicit `--equity-samples`
    budget (default 2,000), so behaviour is unchanged.

  No code changes were required; `cargo check --all-targets`,
  `cargo clippy --all-targets -- -D warnings`, and the full test suite stayed
  green on the manifest bump alone.

- `AI-BOM.md` header now records pkcore v0.11.0.

## [0.1.25] - 2026-08-22

### Changed

- Upgraded to `pkcore` 0.7.0. The headline break —
  `PokerSession::next_actor` returning `Result<Option<u8>, PKError>` instead
  of collapsing a failed street advance to `None` — does not reach pkdealer,
  which drives the hand loop through `next_step()`/`SessionStep`, not
  `next_actor()`. No code changes were required; build, clippy, and the full
  test suite stayed green on the manifest bump alone.

## [0.1.24] - 2026-08-21

### Changed

- Upgraded to `pkcore` 0.6.0. `SessionStep` gained a `Failed(PKError)`
  variant, which broke the two exhaustive `match session.next_step()` sites
  (`pkdealer_service::main` and `pkdealer_client`'s `demo` example). Both now
  call `PokerSession::abort_hand()` on failure instead of `end_hand()`, since
  an aborted hand never reached showdown; the service reports it as an
  `EventType::HandEnded` event with an "Hand aborted:" description.

## [0.1.23] - 2026-08-18

### Changed

- Upgraded to `pkcore` 0.5.0 (#26).

## [0.1.22] - 2026-08-16

### Changed

- Upgraded to `pkcore` 0.4.0; set `TableSnapshot::raises_this_street`.

### Fixed

- Moved `serde_json` to dev-dependencies in `pkdealer_backchannel` (#24,
  "boss" branch).

## [0.1.21] - 2026-08-16

### Added

- **EPIC-70: collusion simulation + blind "Boss" detector (Phases 0-5)** — a
  cheat-detection harness that simulates colluding bots at the table and a
  detector that flags suspicious patterns without seeing the collusion
  signal directly ("blind" detection).
- Opportunity-gated the whipsaw collusion signal and recorded an
  inverted-detection finding from the evaluation pass.
- OKF (Open Knowledge Format) knowledge bundle added to the repo (`.okf/`).

## [0.1.20] - 2026-07-18

### Added

- `docs/GUIDE_Bot_Decision_Capabilities.md` — a pkdealer-specific guide to
  configuring bot play with pkcore 0.3.0's EPIC-36 decision knobs.
- Wired the `exploit` opponent-stats knob through `ExportSession` in
  `pkdealer_agent_rules` (option 1).

### Changed

- Upgraded `pkcore` 0.3.0 -> 0.3.1.
- Renamed test functions to drop the redundant `test_` prefix, matching the
  `#[test]`-attribute-already-marks-it convention.

## [0.1.19] - 2026-07-18

### Changed

- Migrated to `pkcore` 0.2.0, then 0.3.0 (Casino reorg: `NoCell` ->
  canonical types, moved paths). The 0.3.0 break — a new public
  `BotProfile.decision` field — didn't touch pkdealer, since it never
  constructs a `BotProfile` via struct literal and `TableSnapshot`'s shape
  was unchanged; the migration compiled clean with no code edits.
- Bumped `actions/checkout` 6 -> 7 and `actions/cache` 5 -> 6 in CI.

### Added

- `docs/EPIC-45` — 6-max NLHE bot-evaluation format.
- Improved LLM bot behavior in the arena.

## [0.1.18] - 2026-06-20

### Added

- **EPIC-43: PokerBench dataset integration** — dataset download plus
  PokerBench-guided Ollama models, priced and documented.
- **EPIC-44: priced arena LLM seats** so `pktui` shows live per-seat cost.

## [0.1.17] - 2026-06-20

### Added

- **EPIC-42, Phases 1-2 + 4: dynamic arena runner** — `arena.toml`-driven
  `bin/arena`, an agent registry, `arena-down`, and docs.
- **EPIC-43: PokerBench integration epic** + companion `pkcore` spec.
- **EPIC-44: cost simulation, Phases 0-3** — offline token/notional-cost
  analysis (`pkdealer_costsim`), a shared `pkdealer_pricing` leaf crate,
  per-seat `input_tokens`/`output_tokens` on `SeatInfo`, a per-seat token
  accumulator with OTel gauges, and a live notional-cost column
  (`cost_micro_usd`).

### Fixed

- Runner's minimum-raise floor computed `to_call + min_raise` where pkcore
  validates `raise_amount - current_bet >= min_raise_increment`; these only
  agree when the acting player's street bet is zero. Diverged for the small
  blind preflop and for any player re-raising after opening.

## [0.1.16] - 2026-06-01

### Added

- **EPIC-25, Phases 1-4: arena hand recorder** — in-memory recorder,
  `ExportSession` (YAML/JSON), a disk sink, deck capture, and per-action
  `AgentFidelity` provenance buffered from `Act` calls and attached to
  recorded hands.
- `bin/botarena` — an all-bots demo launched via Compose profiles.
- Tournament blind-schedule module: blinds escalate on a hand-count
  schedule with a stack reset at cycle wrap; live blinds and
  button/blind seats exposed on `TableStatus`.
- Round-reset tournament mode.
- Per-street bet exposed on `SeatInfo`; player name included in
  `PlayerAction` event descriptions; showdown message now shows the
  winning hand's cards, not just the rank name.

### Changed

- Demo Compose stack: dropped `agent_random`, renamed `agent_ollama` ->
  `agent_llama`, added `agent_mistral` and `agent_gemma`, each with an
  overridable model env var.
- `HandEnded` handler in the shared agent loop now pauses on every hand
  end, not just showdowns (previously gated on `live_seats >= 2`).
- `demo.sh` retired in favor of `bin/simpletmux`; stale doc references
  updated.

### Fixed

- Banked capped chips so blind-cycle resets keep P/L zero-sum (blind-wrap
  chip leak).
- Default blind escalation reverted to 20 hands per level after a brief
  10-hand experiment.

## [0.1.15] - 2026-05-29

### Added

- Self-heal for the shared agent loop (`pkdealer_agent_core::runner`): the
  blocking `event_stream.message()` call is wrapped in a 3-second
  `tokio::time::timeout`; on timeout the agent pulls authoritative status via
  `GetStatus` and acts if the table is silently waiting on its seat.
  Extracted `act_if_my_turn` so the event-driven and reconcile paths recover
  a stuck seat identically.

## [0.1.14] - 2026-05-28

### Fixed

- Demo no longer stalls.

## [0.1.13] - 2026-05-27

### Added

- **EPIC-24: demo packaging** — agent containers, launcher, presenter guide.
- Five-agent demo stack in `docker-compose.yml` (gto/lag/tag rules bots,
  random, Ollama) wired to the dealer and OTel; `Dockerfile.agent` — a
  shared cargo-chef multi-stage build parametrized by `BIN_NAME` for every
  agent binary; `demo.sh` one-command launcher with an Ollama preflight
  check and dealer-readiness wait.
- **EPIC-25** design work (later renumbered from EPIC-25 to EPIC-40 to avoid
  a collision with a pkcore epic number).
- `ISC` and `CDLA-Permissive-2.0` added to the `deny.toml` allow-list.

### Fixed

- Flaky tests; rebuy code-review findings addressed.

## [0.1.12] - 2026-05-24

### Changed

- **License migration: GPL-3.0 -> MIT OR Apache-2.0.** `LICENSE-GPL3.0`
  removed; `LICENSE-MIT`/`LICENSE-APACHE` added (copied from `pkcore`); root
  `Cargo.toml` license field and `deny.toml` allow-list updated to drop the
  copyleft framing; README and docs updated accordingly.
- EPIC-23 presentation and a Claude sub-project README added.

## [0.1.11] - 2026-05-24

### Added

- **EPIC-23: AI agent crates** — `pkdealer_agent_rules` (rule-based bot
  driven by a pkcore `BotProfile`) and `pkdealer_agent_claude` (Claude-backed
  agent), both wired up and ready.

## [0.1.10] - 2026-05-23

### Added

- **EPIC-20 closeout: seat resume via `client_secret`.**

## [0.1.9] - 2026-05-23

_No user-facing changes; workspace version bump only._

## [0.1.8] - 2026-05-23

### Added

- **EPIC-22: OpenTelemetry instrumentation** — `otel` module with OTLP gRPC
  exporters; hand/street/action span lifecycle with `traceparent`
  extraction; `hands_played`, `pot_size`, and `action_duration_ms` metrics;
  gRPC reflection (v1 + v1alpha) for dynamic clients; a cargo-chef
  multi-stage container build for `pkdealer_service`; OTel collector,
  Prometheus, and Grafana configs plus a Docker Compose observability stack
  and provisioned dashboard.

## [0.1.7] - 2026-05-03

### Changed

- Pinned `pkdealer_proto`'s git dependency to both a SemVer version and a
  precise commit rev (no behavior change).

## [0.1.6] - 2026-04-22

_Proto dependency update; no other user-facing changes._

## [0.1.5] - 2026-04-22

### Added

- `pkdealer_service::web` — an Axum HTTP server with an embedded spectator
  page (`GET /`) and a server-sent-events stream (`GET /events`), started
  alongside the gRPC service when `PKDEALER_WEB_ADDR` is set.
- `proto/dealer.proto` promoted to the repo root (previously nested under
  `crates/pkdealer_proto/proto/`).

## [0.1.4] - 2026-04-19

### Added

- **EPIC-20, Part 2: Act-only autonomous flow.** The service now uses
  `PokerSession`/`TableNoCell` (no `unsafe impl Send`), auto-advances
  streets and ends hands inside the `Act` handler via a `next_step()` loop,
  and emits `StreetAdvanced`/`HandEnded` events at each step. The
  `advance_street` and `end_hand` RPCs are deprecated with clear error
  messages; the demo client drives the whole hand — 9 players seated, hole
  cards dealt, streets auto-advancing preflop through showdown — purely via
  `Act` calls.

## [0.1.3] - 2026-04-06

### Changed

- Upgraded `pkcore` to 0.0.39.

## [0.1.2] - 2026-04-03

### Added

- Seat-authentication tokens: `SeatPlayer`/`SeatPlayerAt` issue a
  `player_token`; `Act` requires a token matching the acting seat
  (`PermissionDenied` otherwise); `GetStatus` filters hole cards by token
  (Hidden / Player(seat) / Spectator); `StreamEvents` broadcasts always hide
  hole cards; `remove_player` revokes the token; `PKDEALER_SPECTATOR_TOKEN`
  env var (defaults to `"spectator"`). Three new auth-specific tests
  (no-token, wrong-seat, spectator/player/hidden visibility).

## [0.1.1] - 2026-03-28

### Added

- Initial pkdealer workspace: `pkdealer_proto`, `pkdealer_service`,
  `pkdealer_client` crates; CI/CD pipeline (`cargo deny`, clippy, fmt);
  first playable phase-1 demo over tmux.

[0.1.25]: https://github.com/ImperialBower/pkdealer/compare/v0.1.24...v0.1.25
[0.1.24]: https://github.com/ImperialBower/pkdealer/compare/v0.1.23...v0.1.24
[0.1.23]: https://github.com/ImperialBower/pkdealer/compare/v0.1.22...v0.1.23
[0.1.22]: https://github.com/ImperialBower/pkdealer/compare/v0.1.21...v0.1.22
[0.1.21]: https://github.com/ImperialBower/pkdealer/compare/v0.1.20...v0.1.21
[0.1.20]: https://github.com/ImperialBower/pkdealer/compare/v0.1.19...v0.1.20
[0.1.19]: https://github.com/ImperialBower/pkdealer/compare/v0.1.18...v0.1.19
[0.1.18]: https://github.com/ImperialBower/pkdealer/compare/v0.1.17...v0.1.18
[0.1.17]: https://github.com/ImperialBower/pkdealer/compare/v0.1.16...v0.1.17
[0.1.16]: https://github.com/ImperialBower/pkdealer/compare/v0.1.15...v0.1.16
[0.1.15]: https://github.com/ImperialBower/pkdealer/compare/v0.1.14...v0.1.15
[0.1.14]: https://github.com/ImperialBower/pkdealer/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/ImperialBower/pkdealer/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/ImperialBower/pkdealer/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/ImperialBower/pkdealer/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/ImperialBower/pkdealer/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/ImperialBower/pkdealer/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/ImperialBower/pkdealer/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/ImperialBower/pkdealer/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/ImperialBower/pkdealer/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ImperialBower/pkdealer/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ImperialBower/pkdealer/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ImperialBower/pkdealer/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ImperialBower/pkdealer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ImperialBower/pkdealer/releases/tag/v0.1.1
