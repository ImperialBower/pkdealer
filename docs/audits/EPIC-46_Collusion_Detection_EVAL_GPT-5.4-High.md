# EPIC-46 Evaluation

**Date:** 2026-07-21

**Epic:** `docs/EPIC-46_Collusion_Detection.md`

**Verdict:** Strong epic, close to implementation-ready, but not quite. It is well grounded in the current repo, has a credible trust boundary, and reuses the right machinery, but it still has a few spec gaps that would block or distort implementation.

## What’s strong

- The `RedactedHand` firewall is the right centerpiece; it makes “the Boss never sees hole cards” structurally true.
- It reuses real surfaces already in this repo (`ExploitPuller`, `ExportSession`, `StatsRegistry`, `bin/arena`, `pkdealer_costsim`-style offline analysis) instead of inventing parallel abstractions.
- The phase order is good: offline detector first, live observer later.
- The exit criteria are concrete and measurable.

## What needs tightening

1. **Vector B networking is underspecified.** `127.0.0.1:PORT` will not connect two colluding `bin/arena` containers; the spec needs a compose-hostname or broker design.
2. **`CardShare.hand_no` has no clear source.** Current agent state does not expose a hand sequence, so peer shares can be matched to the wrong hand unless the spec adds one.
3. **Partner identity is too brittle.** `--collude-with` and `GroundTruthLabels` should use arena-composed names or, better, stable player UUIDs; plain names get messy with duplicates like `gto_1` / `gto_2`.
4. **`win-rate lift` is in the wrong phase.** It depends on a collusion-off control run, so it belongs in calibration/reporting, not the core Phase 2 runtime detector.
5. **The statistical bar is too absolute for the current arena.** “Cheat pays” and “zero false positives” should be stated as replicated or confidence-bounded thresholds, especially since this epic explicitly stays on the non-mirrored gRPC arena instead of EPIC-45’s deterministic format.

## Recommendation

Approve after those edits. As written, it is one of the better specs in the repo, but it still needs those clarifications before it is ready for handoff.
