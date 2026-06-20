# EPIC-44 Phase 2 — Live Notional Cost Column

**Date:** 2026-06-19
**Branch:** epic-44
**Epic:** [EPIC-44 Token Accounting & External Cost Simulation](../../EPIC-44_Token_Accounting_and_Cost_Simulation.md)
**Builds on:** Phase 1 (per-seat token accumulator) ✅, `pkdealer_pricing` leaf crate ✅

## Goal

Surface a per-seat **notional USD cost** on `SeatInfo` (`cost_micro_usd`), computed
live in `pkdealer_service` from each seat's accumulated tokens and a configurable
pricing table, so external spectators can render a cost column. Live and post-hoc
(`pkdealer_costsim`) dollar figures must agree by construction.

## Scope decisions (settled in brainstorming)

- **Per-seat model capture:** extend the Phase-1 accumulator value to a struct
  `SeatTokens { input, output, model }` — one map, single source of truth.
- **Notional pricing:** support a model→notional-model override mapping (mirrors
  costsim's `--price-as`), so local-Ollama model ids that aren't in `pricing.toml`
  can be priced as a commercial model.
- **Config source:** env vars via `DealerConfig::from_env` — `PKDEALER_PRICING`
  (table path) and `PKDEALER_PRICE_AS` (override map). Keeps the shared/static rate
  table separate from per-deployment notional choices.
- **Absent pricing:** `cost_micro_usd = 0` when unpriceable (no file, model absent,
  or bot). Renderers treat 0 as blank — same contract as the Phase-1 token fields.

## Out of scope

- The rendered cost column itself (external pktui repo).
- A cost OTel gauge (YAGNI; not in the EPIC's Phase 2 — trivial to add later).
- Tokenizer-variance correction (Phase 3); cached/batch/reasoning pricing (Phase 4).

## Design

### 1. Shared cost logic — `pkdealer_pricing`

Lift the model→override→price resolution and the micro-USD conversion into the
shared leaf crate so both the service and costsim use one code path:

```rust
// pkdealer_pricing
/// Resolve a (possibly overridden) model id to its Price, or None.
pub fn resolve_price<'a>(
    pricing: &'a Pricing,
    overrides: &HashMap<String, String>,
    model: Option<&str>,
) -> Option<&'a Price>;

/// Notional cost in integer micro-USD (1e-6 USD), rounded. 0 when no price.
#[must_use]
pub fn cost_micro_usd(price: &Price, input_tokens: u64, output_tokens: u64) -> u64;
```

`cost_micro_usd` rounds `cost_usd(price, in, out) * 1e6` to the nearest integer
`u64` (saturating at `u64::MAX`, clamped at 0 for any negative — rates are
non-negative so this is defensive).

**Refactor `pkdealer_costsim::report::cost_seats`** to call `resolve_price`
(it currently inlines the override+lookup). Behavior is unchanged — costsim's
existing `cost_seats` tests are the proof — and the two consumers now share the
resolution path, so figures agree by construction.

### 2. Per-seat model capture — `TableState`

Replace the Phase-1 tuple value with a named struct:

```rust
/// Per-seat cumulative LLM token usage and the model that produced it.
/// Mirrors `banked_profit`; bots never create an entry. Surfaced on `SeatInfo`.
session_tokens: std::collections::HashMap<u8, SeatTokens>,

#[derive(Clone, Debug, Default)]
struct SeatTokens {
    input: u64,
    output: u64,
    model: Option<String>,
}
```

`accumulate_session_tokens` gains a `model: Option<&str>` parameter and records it
(last-write-wins) alongside the token increment, only on the accepted-action path:

```rust
fn accumulate_session_tokens(
    acc: &mut HashMap<u8, SeatTokens>,
    seat: u8,
    input: Option<u32>,
    output: Option<u32>,
    model: Option<&str>,
);
```

At the `act` site the model comes from `agent_fidelity.model.as_deref()`. The
Phase-1 OTel token gauges read `entry.input`/`entry.output` (struct fields instead
of tuple positions). Seat-departure clear is unchanged (removes the whole entry).

### 3. Config — `DealerConfig::from_env`

Two new env vars, parsed once at service start:

```
PKDEALER_PRICING   = pricing.toml
PKDEALER_PRICE_AS  = "gemma=claude-opus-4-8,llama=deepseek-v3.2"
```

`DealerConfig` gains:

```rust
pricing: pkdealer_pricing::Pricing,            // loaded from PKDEALER_PRICING; empty if unset
price_as: std::collections::HashMap<String, String>,  // parsed from PKDEALER_PRICE_AS
```

- Missing `PKDEALER_PRICING`, an unreadable file, or a TOML parse error → log a
  warning and fall back to `Pricing::default()` (empty). The service never fails to
  start over pricing config.
- `PKDEALER_PRICE_AS` parses comma-separated `key=value` pairs; malformed pairs are
  skipped with a logged warning. Unset → empty map.

### 4. Proto + population

Add to `SeatInfo` (last field is `output_tokens = 11`):

```protobuf
// Notional cost of this seat's tokens in integer micro-USD (1e-6 USD); 0 when
// unpriceable (no pricing table, model not priced, or bot). Divide by 1e6 to display.
uint64 cost_micro_usd = 12;
```

`build_table_status` computes it per seat from `SeatTokens` + the pricing config:

```rust
let cost = session_tokens.get(&i).map_or(0, |t| {
    pkdealer_pricing::resolve_price(pricing, price_as, t.model.as_deref())
        .map_or(0, |price| pkdealer_pricing::cost_micro_usd(price, t.input, t.output))
});
```

**Threading the pricing config into `build_table_status`:** it is a static
`Self::` associated fn, and one of its call sites (`fresh_seat_at_inner`) is itself
a static fn (no `self`), so converting `build_table_status` to a `&self` method does
not work cleanly. Instead, thread a single borrowed context:

```rust
struct PricingCtx<'a> {
    pricing: &'a pkdealer_pricing::Pricing,
    price_as: &'a std::collections::HashMap<String, String>,
}
```

`build_table_status` takes one extra `pricing: &PricingCtx<'_>` parameter
(immediately after `session_tokens`), and `fresh_seat_at_inner` takes the same
param and forwards it. Every call site sources it from `self.config`
(`&PricingCtx { pricing: &self.config.pricing, price_as: &self.config.price_as }`);
`fresh_seat_at_inner`'s two callers (`seat_player_at`, which has `&self`) pass it
down. This is the Phase-1 threading pattern, bundled into a single param to avoid
adding two arguments at ~12 sites.

### 5. Testing (per CLAUDE.md: happy + edge + error, doc comments + doctests on public items)

- **`pkdealer_pricing`:** `resolve_price` — override hit, passthrough (no override),
  bot (`model: None`) → `None`, model absent from table → `None`. `cost_micro_usd` —
  known rate rounds correctly, zero tokens → 0, large counts don't overflow. Doctests
  on both public fns.
- **`pkdealer_costsim`:** existing `cost_seats` tests stay green after the
  `resolve_price` refactor (behavior-preservation proof).
- **`pkdealer_service`:** `accumulate_session_tokens` records the model; a second
  action updates tokens and keeps/refreshes the model; bot action creates no entry;
  cost computed in `build_table_status` for a priced seat, an overridden seat, and an
  unpriced seat (→ 0). Proto round-trip for `cost_micro_usd`. `DealerConfig::from_env`
  parses `PKDEALER_PRICE_AS` pairs and tolerates malformed input.

## Verification gates

- `OTEL_SDK_DISABLED=true cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
- `cargo test --doc -p pkdealer_pricing -p pkdealer_service`

## Files to modify

| File | Action |
|---|---|
| `crates/pkdealer_pricing/src/lib.rs` | Add `resolve_price`, `cost_micro_usd` + tests/doctests |
| `crates/pkdealer_costsim/src/report.rs` | Refactor `cost_seats` to call `resolve_price` |
| `proto/dealer.proto` | Add `cost_micro_usd = 12` to `SeatInfo` |
| `crates/pkdealer_service/Cargo.toml` | Add `pkdealer_pricing` dependency |
| `crates/pkdealer_service/src/main.rs` | `SeatTokens` struct; `session_tokens` value type; `accumulate_session_tokens` model param; `DealerConfig` pricing/price_as + `from_env` parsing; `PricingCtx` threaded through `build_table_status` + `fresh_seat_at_inner`; populate `cost_micro_usd`; gauge field-access fixups; tests |
| `crates/pkdealer_service/README.md` | Document `PKDEALER_PRICING` / `PKDEALER_PRICE_AS` |
| EPIC-44 doc | Flip Phase 2 rows to ✅ |

No new crates. The cost column render stays in external pktui.
