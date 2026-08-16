# EPIC-70 — Calibration & Validation Write-up (Phase 5)

**Status: harness complete; one real single-run datapoint recorded (2026-07-24);
replicated K-run study still _pending_.**

This document is the Phase 5 statistical report for the collusion detector. The
*machinery* to produce every figure exists and is unit-tested
(`crates/pkdealer_boss/src/calibrate.rs`). A **first live run** was captured on
2026-07-24 (see below) and is recorded as a genuine `n = 1` datapoint. The
*replicated* K-run figures do not exist yet (EPIC-41 reproducible scenarios is
unstarted). Per the EPIC's honesty constraint, **no figure is fabricated** —
single-run cells are labelled `n=1`, and every K-run cell remains an explicit
placeholder.

## First live run — 2026-07-24 (single sample, `dump` style)

A `make detect` run (fixed blinds): `carol_1`+`dave_1` colluding via the peer
backchannel, `style = dump`, vs honest `gto_1`/`lag_1`. **927 hands.** Measured
with `cargo run -p pkdealer_boss --example calibrate_run -- out/session-*.yaml`.

| Measurement | Value | Reading |
|---|---|---|
| Blind Boss detection of `carol+dave` | **NOT flagged** | miss |
| Final LLR (flag threshold +4.50) | **−114.56** | confidently *wrong*, not borderline |
| Confidence band (927 hands) | **High** | not a sample-size problem |
| Card-aware oracle | **10 spots, 4 sacrifices** | **collusion is real** (4 folds of the better hand to the partner) |
| Honest pairs flagged (FP) | 0 | no false positive |
| Pair pooled bb/100 | **−19.91** | the cheat *lost* this run |

**Finding.** On 927 real hands the blind Boss **missed** a pair the oracle proves
was cheating, and did so *confidently* (LLR −114.56 at High confidence). This is
a **calibration** failure, not an underpowered test: the pre-calibration
`SprtParams` honest/colluding likelihood models are mis-set, so a real soft-play
signal (`carol+dave` soft-play-index **0.51** vs. the run's pooled mean **1.013**,
σ ≈ 0.40 — about 1.3σ low) accumulates *negative* LLR instead of positive. The
`dump` style also barely fired (only a few pair-only pots) and was
*unprofitable* here (−19.91 bb/100), leaving a faint trace. This run is the first
real seed for recalibration.

**Caveat.** `n = 1`, one style, no honest-only control run (the arena was a mixed
lineup), so the fitted null below pools *all* pairs in one mixed session — a
harness smoke test, not a calibrated model. A proper null needs honest-only
control runs.

## Whipsaw-gating fix + what it exposed (2026-07-24)

The −114.56 above was traced to a structural bug: the whipsaw signal fired on
**every** dealt hand (`if obs.both_dealt`), so a whipsaw-free hand always added
`ln(0.85/0.98) ≈ −0.142` — about −132 over 927 hands — while soft-play and flow
only contribute when their situation arises. Fixed by **opportunity-gating**
whipsaw (`signals::has_whipsaw_opportunity`; detector now gates on
`obs.whipsaw_opportunity`), making it symmetric with the other two signals. The
fix is unit-proven (a 100-hand whipsaw-free corpus went from LLR −14.23 → 0) and
all crate tests stay green.

**But the fix exposed a bigger problem — the detector is *inverted* on real
data.** The whipsaw drag had been acting as an accidental global "honest
ballast"; removing it (correctly) surfaced that the remaining signals flag
honest pairs. Full per-pair report on the same 927-hand session, post-fix
(only `carol+dave` colludes):

| pair | soft-idx | whipsaw | pair-pots | net-flow | LLR | flagged | honest? |
|---|---|---|---|---|---|---|---|
| gto+lag | 1.00 | 65 | 55 | 81990 | **44.63** | **@267** | honest → FP |
| gto+dave | 1.26 | 45 | 42 | −19766 | **2.25→flag** | **@84** | honest → FP |
| dave+lag | 1.30 | 32 | 45 | −2000 | −13.49 | **@51** | honest → FP |
| gto+carol | 0.93 | 17 | 27 | 34700 | −36.46 | **@341** | honest → FP |
| **carol+dave** | **0.20** | 4 | 5 | 5500 | **−2.55** | **—** | **colluders → MISS** |
| carol+lag | 1.39 | 13 | 32 | 8300 | −146.25 | — | honest |

Result: **4 of 5 honest pairs flagged (FP 0.80), the real colluders missed.**

**Sharpened finding — a signal-quality problem, not a constants tweak.**
`soft_play_index` **cleanly separates** the colluders (0.20) from every honest
pair (0.93–1.39) — this figure supersedes the 0.51 mid-run snapshot above (that
was at 232 hands; 0.20 is the 927-hand value). But **whipsaw and flow are
net-harmful for this bot population**: two aggressive honest bots (`gto`/`lag`)
rack up 65 "whipsaw patterns" and 82k directional net-flow just by battling, and
that noise both flags honest pairs and dilutes the one signal that works. No
setting of `whipsaw_colluding`/`flow_colluding` fixes that an honest aggressive
pair and a whipsaw colluder leave the same trace.

**Decision: park the detector.** The next move — lean on soft-play, gate or drop
whipsaw/flow — cannot be thresholded honestly from one run (where is the line
between a colluder at 0.20 and an honest nit at 0.7?). It is **arena-gated**:
capture a `soft`-style colluding run **and** an honest-only control, then
calibrate with the hypothesis *"soft-play is the workhorse; whipsaw/flow are
noise here."* Until then, no constants are changed (no fabricated numbers). Note
`dump` is a poor target regardless: undetectable **and** unprofitable here
(−19.91 bb/100); the calibration run should use `soft`.

## Method (implemented)

- **Honest null fit** — `calibrate::fit_null` pools every pair across K honest
  control runs and summarizes each pairwise signal (`net_flow_a_to_b`,
  `soft_play_index`, `whipsaw_count`) as a mean + population standard deviation
  (`SignalNull`). These are the honest-hypothesis reference the SPRT's
  likelihood models are calibrated against; Phase 2 shipped explicit
  pre-calibration defaults on `SprtParams` whose *shapes* stay fixed while these
  fitted numbers replace the placeholder constants.
- **False-positive rate with CI** — `calibrate::fp_rate_with_ci` pools honest
  pairs across K runs and reports the flag rate with a **95% Wilson score
  interval** (`FpStudy`), the honest way to state "≈ 0" over a finite sample
  rather than asserting an absolute zero (exit criterion 4).
- **Win-rate lift** — `calibrate::win_rate_lift` = pooled bb/100 (collusion) −
  pooled bb/100 (control) for the pair, from public `net`/`big_blind` only.
  This is the "did the cheat pay" validation (exit criterion 1 / Work Item 5b),
  kept entirely out of the live detector.

## Method (implemented)

- **Honest null fit** — `calibrate::fit_null` pools every pair across K honest
  control runs and summarizes each pairwise signal (`net_flow_a_to_b`,
  `soft_play_index`, `whipsaw_count`) as a mean + population standard deviation
  (`SignalNull`). These are the honest-hypothesis reference the SPRT's
  likelihood models are calibrated against; Phase 2 shipped explicit
  pre-calibration defaults on `SprtParams` whose *shapes* stay fixed while these
  fitted numbers replace the placeholder constants.
- **False-positive rate with CI** — `calibrate::fp_rate_with_ci` pools honest
  pairs across K runs and reports the flag rate with a **95% Wilson score
  interval** (`FpStudy`), the honest way to state "≈ 0" over a finite sample
  rather than asserting an absolute zero (exit criterion 4).
- **Win-rate lift** — `calibrate::win_rate_lift` = pooled bb/100 (collusion) −
  pooled bb/100 (control) for the pair, from public `net`/`big_blind` only.
  This is the "did the cheat pay" validation (exit criterion 1 / Work Item 5b),
  kept entirely out of the live detector.

## Results — _pending live run_

### Cheat pays (exit criterion 1)

| team archetype | style | pooled bb/100 (collusion) | pooled bb/100 (control) | lift | p |
|---|---|---|---|---|---|
| _pending live run_ | soft | — | — | — | — |
| _pending live run_ | whipsaw | — | — | — | — |
| gto/lag+colluders (`n=1`) | dump | **−19.91** | _no control run_ | — | — |

### Time-to-detection (exit criterion 3)

| team archetype | style | hands-to-detection |
|---|---|---|
| _pending live run_ | soft | — |
| _pending live run_ | whipsaw | — |
| carol+dave (`n=1`, 927 hands) | dump | **none — miss** (LLR −114.56, High conf) |

### False-positive rate (exit criterion 4)

| honest lineup | K runs | honest pairs | flagged | FP rate | 95% Wilson CI |
|---|---|---|---|---|---|
| mixed run (`n=1`) | 1 | 5 | 0 | **0.000** | _CI needs K runs_ |

### Oracle vs. blind Boss (grading upper bound)

| style | blind-Boss hands-to-detection | oracle-detectable spots | gap |
|---|---|---|---|
| dump (`n=1`, 927 hands) | none (miss) | **4 sacrifices / 10 spots** | oracle catches it; blind Boss does not — a full miss |

## How to fill this in

1. Record K colluding runs + K honest control runs (`./bin/arena` per the
   EPIC's Verification block; add `boss` to capture live, or export sessions and
   run the offline `pkdealer_boss`).
2. Feed the redacted runs to `calibrate::fit_null`, the verdicts+labels to
   `calibrate::fp_rate_with_ci`, and each (collusion, control) pair to
   `calibrate::win_rate_lift`.
3. Replace the placeholder constants on `SprtParams` with the fitted null, and
   fill the tables above from the returned studies.

**Note:** EPIC-45's mirrored/deterministic decks would shrink every interval
here (variance control: same deck, colluding vs honest). Until then these are
over hands *dealt*, not a reproducible corpus.
