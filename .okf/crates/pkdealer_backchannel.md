---
type: Rust Crate
title: pkdealer_backchannel
description: EPIC-70 Vector-B collusion backchannel broker — relays CardShare messages between colluding agent processes, out of the dealer's sight.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_backchannel
tags: [binary, library, collusion, broker, tcp, epic-70]
timestamp: 2026-08-30T12:00:00Z
---

The adversary side of EPIC-70. A deliberately dumb TCP fan-out relay that lets
two or more colluding agents exchange their hole cards during a hand, so the
detectors in [pkdealer_boss](/crates/pkdealer_boss.md) and
[pkdealer_agent_boss](/crates/pkdealer_agent_boss.md) have a real,
ground-truthed cheat to catch.

The broker keeps **no state**: every line it receives is broadcast to every
*other* connected client, and clients filter for their partner themselves. The
dealer service never sees these messages — that is the point. It is a test
instrument for the detection work, not a feature of normal play.

# Schema

`CardShare` is the one wire type, newline-delimited JSON:

| Field | Type | Meaning |
|-------|------|---------|
| `hand_no` | `u32` | Dealer hand number the cards belong to |
| `seat` | `u8` | Sharer's seat |
| `player_id` | `Uuid` | Sharer's stable player UUID |
| `hole_cards` | `String` | Hole cards in index notation, e.g. `"Ah Kd"` |

Only the tests and the doc-test serialize; the broker itself relays raw lines,
which keeps `serde_json` a dev-dependency. See
[EPIC docs](/references/epic-docs.md) and the
[arena runbook](/runbooks/arena.md) for how Vector-B runs are launched.
