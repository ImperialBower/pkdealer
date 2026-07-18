# Guide: Configuring Bot Play with pkcore Decision Capabilities

*Applies to pkcore 0.3.0 (EPIC-36) and `pkdealer_agent_rules` ≥ 0.1.19.*

pkcore 0.3.0 adds a `decision:` section to every `BotProfile` — a set of **graded
decision-capability knobs** that dial how a rule-based bot thinks, independently
of its aggression / bluff / sizing personality. This guide shows how to drive
those knobs from within pkdealer, which knobs actually change play *today* over
the gRPC wire, and gives copy-pasteable example configurations.

Every knob defaults to the **historical** decider behavior, so a profile with no
`decision:` block (or an all-default one) plays exactly as it did before 0.3.0.

---

## The six knobs

| Knob (`decision.*`) | Values | What it changes |
|---|---|---|
| `equity` | `off` \| `fast` \| `exact` | Postflop hand-strength source: hand-rank proxy vs. real multi-way equity (seeded Monte Carlo / exact enumeration). |
| `ranges` | `flat` \| `position_aware` | Preflop open-raise range source: flat `range_strategy.open_raise` vs. the profile's position-aware `playbook`. |
| `pot_odds.discipline` | `0.0`–`1.0` | Call-threshold strictness. `1.0` = strict break-even (tight calling); `0.0` = ignore pot odds entirely (calls everything the equity path considers). |
| `outs` | `off` \| `on` | Draw/outs equity augmentation on the flop and turn. |
| `exploit` | `off` \| `light` \| `heavy` | Opponent-adjusted exploitation — engages only when the snapshot carries opponent stats. |
| `preflop_charts` | `off` \| `hup` \| `solver` | Preflop decision-chart source. |

---

## What actually changes play *today* (effectiveness matrix)

This is the part that matters when you configure a bot in pkdealer. The shipped
`RuleBasedDecider` and pkdealer's gRPC→snapshot conversion
(`hand_state_to_snapshot` in `crates/pkdealer_agent_rules/src/main.rs`) only
feed some of these knobs the data they need. Set the others if you like — they
are wire-format-safe — but know that they are inert on the current path.

| Knob | Consumed by `RuleBasedDecider`? | Effective over the pkdealer gRPC wire? |
|---|---|---|
| `equity` | ✅ yes | ✅ **Yes.** Hole cards + board + active stacks are all supplied, so `fast`/`exact` route through the real equity engine. |
| `pot_odds.discipline` | ✅ yes | ✅ **Yes.** Applied on every equity-based call decision (i.e. when hole cards are known). |
| `ranges` | ✅ yes | ⚠️ **Not yet.** `position_aware` needs `TableSnapshot::position()`, which needs `dealer_button` + `logical_seat`. pkdealer currently sends both as `None`, so it falls back to `flat`. Also requires the profile to have a `playbook`. |
| `exploit` | ✅ yes | ⚠️ **Not yet.** Needs `opponent_stats` on the snapshot; pkdealer sends `None`, so any setting no-ops. |
| `outs` | ❌ no | ❌ **Inert.** Config-only in 0.3.0; the shipped decider does not read it (EPIC-36 follow-on). |
| `preflop_charts` | ❌ no | ❌ **Inert.** Config-only in 0.3.0 (`hup`/`solver` are follow-on; see the pkcore EPIC-36 corrigendum). |

> **Bottom line:** over the live docker/gRPC table, `equity` and
> `pot_odds.discipline` are the two knobs that change how your bot plays right
> now. The rest round-trip safely and are ready for when the wire is widened —
> see [Closing the wire gap](#closing-the-wire-gap).

The `equity` feature is compiled in because `pkdealer_agent_rules` depends on
pkcore with default features on (`equity` is a default), plus `bot-profiles`.

---

## Two ways to configure a bot

### 1. In the profile YAML (`data/bots/*.yaml`)

Add a `decision:` block to any profile. Absent keys keep their default:

```yaml
name: my_grinder
description: gto base, real equity + disciplined calling
style: gto
range_strategy:
  open_raise: QQ+, AKs, AKo
  three_bet: QQ+, AKs
  call_three_bet: JJ+, AQs+
  postflop_cbet_frequency: 50
betting_strategy:
  aggression_factor: 50
  bluff_frequency: 33
  check_raise_frequency: 15
  preferred_bet_sizes:
    - 1/3
    - 1/1
decision:
  equity:
    mode: fast
    samples: 2000
  pot_odds:
    discipline: 1.0
```

Run it (local, no mount needed for a path):

```bash
cargo run --bin pkdealer_agent_rules -- --name grinder --profile data/bots/my_grinder.yaml
```

In the docker demo, drop the file in `./data/bots/` and pass the mounted path,
e.g. `--profile /data/bots/my_grinder.yaml` (see `DEMO.md` → *Different rules
profiles*).

### 2. As CLI overrides (no YAML edit)

`pkdealer_agent_rules` exposes one flag per knob. Each flag overrides just that
knob on top of whatever profile you loaded; omit a flag to leave the profile's
value untouched:

| Flag | Values |
|---|---|
| `--equity` | `off`, `fast`, `exact` |
| `--equity-samples <N>` | Monte-Carlo budget for `--equity fast` (default `2000`) |
| `--ranges` | `flat`, `position-aware` |
| `--pot-odds-discipline <f>` | `0.0`–`1.0` (out-of-range values are clamped) |
| `--outs` | `off`, `on` |
| `--exploit` | `off`, `light`, `heavy` |
| `--preflop-charts` | `off`, `hup`, `solver` |

```bash
# gto personality, but force Monte-Carlo equity + looser calling
cargo run --bin pkdealer_agent_rules -- \
    --name loosey --profile gto \
    --equity fast --equity-samples 4000 --pot-odds-discipline 0.4
```

When any override fires, the agent logs the resulting `decision` config at
startup so you can confirm what took effect.

---

## Example configurations

### A. Calling station — ignores pot odds

Loosest possible calling. Same personality as `gto`, but the pot-odds gate is
switched off, so the bot calls on raw equity alone.

```bash
cargo run --bin pkdealer_agent_rules -- --name station --profile gto \
    --equity fast --pot-odds-discipline 0.0
```

YAML equivalent (`decision:` block only):

```yaml
decision:
  equity:
    mode: fast
  pot_odds:
    discipline: 0.0
```

### B. Equity-driven TAG — disciplined, real Monte-Carlo equity

Tight-aggressive personality, upgraded from the hand-rank proxy to true
multi-way equity with strict pot-odds discipline.

```bash
cargo run --bin pkdealer_agent_rules -- --name tag --profile tight_aggressive \
    --equity fast --equity-samples 3000 --pot-odds-discipline 1.0
```

### C. Exact-equity nit — maximum postflop accuracy

`exact` enumerates the remaining runouts instead of sampling. Costs more CPU per
decision; use it for a small table or an analysis run rather than a large fast
arena.

```bash
cargo run --bin pkdealer_agent_rules -- --name professor --profile abc --equity exact
```

### D. The reference pair — `strong_all_on` vs. `weak_all_off`

These two ship in `data/bots/` and are resolvable as built-in names (`strong` /
`weak` for short). They share the `gto` base and differ only in `decision:` —
every knob dialed up vs. every knob dialed down — which makes them the canonical
A/B for "do the capabilities help?".

```bash
# All knobs on: Monte-Carlo equity, position-aware ranges, strict pot odds
cargo run --bin pkdealer_agent_rules -- --name strong --profile strong_all_on

# All knobs off: hand-rank proxy, flat ranges, pot odds ignored
cargo run --bin pkdealer_agent_rules -- --name weak --profile weak_all_off
```

Their `decision:` blocks:

```yaml
# strong_all_on
decision:
  equity: { mode: fast, samples: 1000 }
  ranges: position_aware
  pot_odds: { discipline: 1.0 }
  outs: off
  exploit: { mode: off }
  preflop_charts: off

# weak_all_off
decision:
  equity: { mode: off }
  ranges: flat
  pot_odds: { discipline: 0.0 }
  outs: off
  exploit: { mode: off }
  preflop_charts: off
```

> Over the live gRPC wire the measurable gap between these two comes from
> `equity` and `pot_odds.discipline` (see the matrix above). `strong_all_on`
> also sets `ranges: position_aware`, which needs the button/logical-seat wire
> extension to bite — until then it degrades to `flat` for both bots.

### E. Toggling a profile's knob back off

Overrides work in both directions — you can *disable* a knob a profile turned on:

```bash
# strong_all_on ships with Monte-Carlo equity; force the proxy instead
cargo run --bin pkdealer_agent_rules -- --profile strong_all_on --equity off --ranges flat
```

---

## Verifying behavior

- **In pkcore (fastest signal).** pkcore ships a seeded cash-game bench that
  ranks YAML profiles by chips per 100 hands. From the pkcore checkout:

  ```bash
  cargo run --example bot_capability_bench            # built-in strong vs. weak
  ```

  Point it at your own `decision:` variants to A/B them head-to-head before you
  wire them into a table.

- **In pkdealer (live table).** Launch two rules agents with different knobs into
  the same arena (`bin/aiarena` / the docker demo in `DEMO.md`) and watch — or
  record the hand histories and compare. Because `equity` uses a *seeded* RNG,
  repeated runs of the same seed are reproducible.

- **Confirm what took effect.** Any active override is echoed at agent startup:
  `[name] decision overrides applied: DecisionConfig { … }`.

---

## Closing the wire gap

To make `ranges: position_aware` and `exploit` effective over gRPC, widen
`hand_state_to_snapshot` in `crates/pkdealer_agent_rules/src/main.rs` so the
`TableSnapshot` it builds carries the data those knobs consult:

- **`ranges: position_aware`** needs `dealer_button` and `logical_seat` (so
  `TableSnapshot::position()` returns `Some`) plus a `playbook` on the profile.
  All the built-in profiles (`gto`, `tight_aggressive`, …) already carry
  playbooks; the missing piece is button/seat info from the `HandState`.
- **`exploit`** needs `opponent_stats: Some(&StatsRegistry)` — i.e. per-player
  aggregates threaded through from the service.

Both fields are currently hard-coded to `None`. Nothing in the profile format
changes when they land: existing `decision:` blocks already name these knobs, so
a bot configured for them today simply starts honoring them once the wire
carries the data.

---

## Wire-format compatibility

The `decision:` field is `#[serde(default, skip_serializing_if = "…is_default")]`.
A profile that never opts in serializes with **no** `decision:` key, and an
all-default `decision:` block round-trips to nothing — so every pre-0.3.0
profile YAML in `data/bots/` loads unchanged. You only ever see `decision:` in a
file when at least one knob is non-default.

---

## See also

- `crates/pkdealer_agent_rules/src/main.rs` — the CLI flags, override logic
  (`apply_decision_overrides`), and snapshot conversion.
- `data/bots/strong_all_on.yaml`, `data/bots/weak_all_off.yaml` — the reference pair.
- pkcore `src/bot/decision_config.rs` — the knob definitions and defaults.
- pkcore `docs/EPIC-36_Configurable_Bot_Capabilities.md` — upstream design and corrigendum.
- `DEMO.md` — running agents in the docker arena.
