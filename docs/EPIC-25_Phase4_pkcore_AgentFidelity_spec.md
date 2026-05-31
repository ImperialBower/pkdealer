# Spec: `pkcore` agent-fidelity action metadata

**Target repo:** `ImperialBower/pkcore` (a release after `0.1.2`)
**Driven by:** `ImperialBower/pkdealer` EPIC-25 Phase 4 (arena recorder —
agent-fidelity annotations). See `docs/EPIC-25_Arena_Recorder_and_Export.md`.
**Type:** additive schema + API change. No breaking changes; existing YAML/JSON
hand histories must round-trip byte-for-byte when no agent metadata is present.

## Context

pkdealer records every arena hand as a `pkcore::hand_history::HandHistory`
(EPIC-25 Phases 1–3, shipped). Phase 4 wants each *voluntary* action to also
carry **agent fidelity**: what the model actually produced versus what the table
applied — raw response text, whether the action was coerced, the originally
intended action, and (for LLM agents) token usage and model id.

Today there is nowhere to put this. `pkcore::hand_history::Action` is purely
mechanical (`seat`, `player_id`, `action`, `amount`, `all_in`) and the per-street
`Action` lists are **built internally** by
`HandHistory::from_table_state[_with_ids]` from the `TableAction` event log — a
caller cannot attach extra per-action data without a pkcore API for it. This spec
adds the field and a safe way to populate it after construction.

This metadata is **descriptive only**: `HandHistory::replay()` must continue to
ignore it entirely (replay already reconstructs from actions + hole cards +
board). It exists for analysis/eval, like `shuffled_deck`.

## Schema additions

### 1. New `AgentFidelity` struct

```rust
/// Per-action provenance describing what an agent *produced* versus what the
/// table *applied*. Optional and analysis-only: [`HandHistory::replay`] ignores
/// it. Populated by arena recorders (pkdealer EPIC-25); absent for hand
/// histories imported from other sources.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AgentFidelity {
    /// Raw, unparsed model/agent response text (LLM agents). `None` for agents
    /// that produce a structured decision directly (rules/random).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<String>,

    /// True when the applied action differs from what the agent intended —
    /// e.g. unparseable model output, a bet/raise clamped to a legal size, or a
    /// server-rejected action replaced by a safe fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_coerced: Option<bool>,

    /// The action the agent originally intended, when it differs from the
    /// applied `Action::action`. Pairs with `intended_amount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended_action: Option<ActionType>,

    /// Intended wager amount for an intended bet/raise/call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended_amount: Option<f64>,

    /// Prompt/input tokens reported by the backend (LLM agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,

    /// Completion/output tokens reported by the backend (LLM agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,

    /// Model / agent identifier (e.g. `"claude-..."`, `"rules-v1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
```

Decision rationale (from the EPIC's Phase 4 open decisions):
- **Nested, not flattened** (decision A) — keeps `Action` clean; one
  `skip_serializing_if = "Option::is_none"` hides the whole block when absent.
- **Intended + applied + flag** (decision B) — `was_coerced` alone loses the
  most useful signal (what the model *wanted*). Applied action stays in the
  existing `Action` fields; intended lives here.

### 2. New optional field on `Action`

```rust
pub struct Action {
    pub seat: u8,
    // ... existing fields (player_id, action, amount, all_in) ...

    /// Optional agent-fidelity provenance (analysis-only; ignored by replay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentFidelity>,
}
```

**Back-compat:** because the field is `Option` with `serde(default,
skip_serializing_if)`, (a) older YAML/JSON deserializes with `agent: None`, and
(b) records without agent metadata serialize identically to today. Any code that
constructs `Action { .. }` literally inside pkcore (and its doctests/tests) must
add `agent: None`.

## Injection API

Actions are derived internally, so callers need a post-build way to attach
metadata. Provide **both** a high-level zip and a low-level escape hatch.

### Primary: seat-checked positional zip (decision D — setter, smallest surface)

```rust
impl HandHistory {
    /// Attaches agent-fidelity metadata to this hand's **voluntary** actions in
    /// canonical order (preflop → flop → turn → river, in recorded order),
    /// skipping forced `Post` actions (blinds/antes).
    ///
    /// `entries` must be in that same canonical voluntary-action order; each is
    /// `(expected_seat, fidelity)`. For every voluntary action, if the next
    /// unconsumed entry's `expected_seat` matches the action's seat, its
    /// fidelity is assigned. On any seat mismatch the entry is **skipped**
    /// (action left `None`) rather than misattributed.
    ///
    /// Returns the number of actions successfully annotated. Never panics and
    /// never reorders or drops actions.
    pub fn attach_agent_fidelity(&mut self, entries: &[(u8, AgentFidelity)]) -> usize;
}
```

Why seat-checked positional rather than `(street, seat, ordinal)` keys: the arena
recorder buffers metadata in `Act` arrival order, which is exactly the event-log
voluntary-action order pkcore replays into the per-street lists — so a positional
zip aligns naturally, and the seat check guards against any drift (e.g. a
server-rejection retry that produced an extra applied action).

### Secondary: low-level accessor

```rust
impl HandHistory {
    /// Mutable references to every voluntary (`!= Post`) action across all
    /// streets, in canonical order. Lets callers implement bespoke matching.
    pub fn voluntary_actions_mut(&mut self) -> Vec<&mut Action>;
}
```

## Testing requirements (pkcore conventions)

Per pkcore's testing standard (doc test + unit tests for every public item):

- **Doc tests** on `AgentFidelity` (construct + a field assert) and on
  `attach_agent_fidelity` / `voluntary_actions_mut` (small `HandHistory` with two
  streets; attach; assert annotated count and that a chosen `Action.agent` is
  `Some`).
- **Round-trip:** a `HandHistory` with `agent` metadata survives `to_yaml →
  from_yaml` and `serde_json` round-trips equal; a hand **without** it serializes
  with **no** `agent:` key (assert the substring is absent), and a legacy file
  lacking the key deserializes to `agent: None`.
- **Matching matrix** for `attach_agent_fidelity`: all-fold hand, all-in,
  multi-raise street, dead-button hand, and a seat-mismatch case (assert the
  mismatched entry is skipped and the count reflects it).
- **Replay invariance:** a hand replays to the identical `ReplayResult`
  (`is_consistent`, final stacks) with and without `agent` metadata attached —
  proving replay ignores the field.

## Acceptance criteria

1. `AgentFidelity` + `Action.agent` added; all existing pkcore tests/doctests
   updated to compile (`agent: None` in literals).
2. Existing hand-history YAML/JSON fixtures round-trip unchanged (no `agent:` key
   emitted when absent).
3. `attach_agent_fidelity` and `voluntary_actions_mut` implemented, documented,
   and tested per the matrix above.
4. `replay()` behavior is provably unchanged by the presence of `agent` data.
5. New crate version published; changelog notes the additive field.

## Downstream consumer (pkdealer Phase 4, for reference — not in this PR)

After this lands and the pin is bumped, the pkdealer service will:
1. Add optional agent fields to the `Act` proto (`raw_response`, `was_coerced`,
   `intended_action_type`/`intended_amount`, token counts, `model`).
2. Buffer per-`Act` metadata per hand in `TableState`, in arrival order.
3. After building the `HandHistory` in the hand-end hook, call
   `hh.attach_agent_fidelity(&entries)` (entries in arrival order as
   `(seat, AgentFidelity)`).
4. The `agent_core` runner / `agent_llm` populate the new `Act` fields, surfacing
   raw text and the parse/clamp/retry coercions already present in
   `pkdealer_agent_*`.

No pkcore changes beyond this spec are required for pkdealer Phase 4.
