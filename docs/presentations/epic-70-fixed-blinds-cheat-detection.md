# Demo: Cheat Detection on a Stable Table (`make detect`)

> One command seats a peer-colluding pair, honest bots, and the blind Boss on a
> table whose blinds never move — so the Boss's evidence is signal, not blind-
> level noise — and watch it flag the cheats.

## Audience & framing

Engineering / research. The angle: the *runner*. The full EPIC-70 validation is
covered in `epic-70-live-validation.md`; this runbook is the fast path — one
`make detect` — and the one thing it changes: **fixed blinds**. Lead with *why*
that matters (below), then let it run.

> The docker steps were not smoked from the authoring session (no live arena
> there). Lines marked _(from code)_ are derived from the implementation;
> `bin/detect` / `make detect` argument wiring and the compose interpolation are
> _(verified)_ by dry-run + `docker compose config`.

## Why fixed blinds

The base arena runs a **tournament blind schedule** (escalate every 20 hands,
recycle stacks at the top) — great for an endless demo, bad for detection. The
Boss's signals are chip-flow asymmetry and bb/100 conditioning; escalating
blinds and periodic stack resets inject swings that have nothing to do with
collusion, muddying the SPRT. `bin/detect` sets
`PKDEALER_BLIND_SCHEDULE_ENABLED=false` for this run only (via a compose
interpolation — the shared `docker-compose.yml` is untouched), giving a stable
table where a chip-flow anomaly means what it says.

## Prerequisites

- Repo root, branch `boss`. Docker + `docker compose` v2 running (~4 GB free).
- Ports free: `50051`, `16686` (Jaeger), `9090` (Prometheus), `3001` (Grafana).
- One terminal to launch, one to tail the Boss. Optional Prometheus browser tab.
- No config edits needed — `arena.toml` already defines the colluders
  (`carol`+`dave`, team B, `channel = "peer"`, `style = "dump"`) and the `boss`.

## Setup (~1 min + first-build time)

1. **Sanity-check the run without launching**
   ```bash
   ./bin/detect --dry-run
   ```
   _Expected:_ `[detect] blinds frozen …`, lineup `carol dave gto lag boss`, and
   an override containing `--collusion-channel peer`, a `pkdealer_backchannel:`
   service, and `agent_boss_1:`. _(verified)_
   _Talking point:_ the runner is just `bin/arena` with a default lineup and the
   blinds pinned.

## The demo (~15 min — the Boss needs ≥ 50 hands before it will flag)

### 1. Launch the stable-table cheat scenario

1. **One command**
   ```bash
   make detect
   ```
   (equivalently `./bin/detect`, or a custom table:
   `make detect DETECT_PLAYERS="carol dave gto lag tag boss"`.)
   _Expected:_ builds images, "Arena is live"; `docker compose ps` shows
   `agent_carol_1`, `agent_dave_1`, `agent_boss_1`, `pkdealer_backchannel` `Up`. _(from code)_
   _Talking point:_ peer colluders, honest bots, and a blind observer — one line.

2. **Confirm blinds are actually frozen**
   ```bash
   PKDEALER_BLIND_SCHEDULE_ENABLED=false docker compose -f docker-compose.yml config \
     | grep BLIND_SCHEDULE_ENABLED
   ```
   _Expected:_ `PKDEALER_BLIND_SCHEDULE_ENABLED: "false"`. _(verified)_
   _Talking point:_ default runs render `"true"`; only this runner flips it.

### 2. Watch the Boss flag the pair

3. **Tail the Boss**
   ```bash
   OVERRIDE=$(ls -t "${TMPDIR:-/tmp}"/docker-compose.arena.*.yml | head -1)
   docker compose -f docker-compose.yml -f "$OVERRIDE" logs -f agent_boss_1
   ```
   _Expected:_ `boss online — polling…`, then, after the pair clears the 50-hand
   `Confidence` floor, `WARN … FLAG: suspected collusion pair=<carol>+<dave>
   hand=NN`. _(from code — this run validates it)_
   _Talking point:_ the headline number is *which hand* it first crossed —
   time-to-detection, on a table where blinds didn't move it.

4. **(Optional) the LLR climb in Prometheus** — query `pkdealer_boss_pair_llr`.
   _Expected:_ a rising series for the carol+dave pair. _(from code)_

## What to highlight verbally

- Fixed blinds isolate the *collusion* signal from tournament variance — the
  same detector, a cleaner measurement.
- The runner freezes blinds via a per-run env export against a compose
  interpolation; the shared `docker-compose.yml` default is unchanged (`"true"`).
- Everything downstream is the ordinary arena: same broker, same colluders, same
  blind Boss reading only public information.

## Likely questions & answers

- **Q: Why not just always disable blinds?** A: The escalating schedule keeps
  open-ended demos alive; detection wants a controlled table, so it's opt-in per
  run rather than a global default.
- **Q: Does this touch the shared compose file?** A: No — `bin/detect` exports
  the override; the file reads `${PKDEALER_BLIND_SCHEDULE_ENABLED:-true}`, so
  every other caller still gets the tournament schedule.
- **Q: Can I pick the lineup?** A: Yes — `make detect DETECT_PLAYERS="…"` or
  `./bin/detect <lineup>`; the fixed-blind behavior applies to whatever you run.
- **Q: Still bust-proof?** A: Rebuy-on-bust stays on, so a chip-dump that busts
  the dumper doesn't wedge the table — only the *blind escalation* is disabled.

## Cleanup

```bash
make arena-down          # force-tear-down all arena containers + volumes
```
You stayed on branch `boss`; nothing to reset, and no files were edited to run
the demo.

## Troubleshooting

- **Boss never flags:** it needs ≥ 50 completed hands *and* the dump to fire.
  Deal faster with `PKDEALER_ACTION_DELAY_SECS=0 PKDEALER_HAND_END_DELAY_SECS=1
  make detect`.
- **Blinds still escalating:** you launched with plain `bin/arena` or `make
  arena`, not `make detect` — only the detect runner exports the override.
- **No `pkdealer_backchannel` / `agent_boss_1`:** the default lineup includes
  both; if you passed a custom `DETECT_PLAYERS`, make sure it has a
  `channel = "peer"` pair and `boss`.

## Recalibration checklist (one-pass, when the arena is up)

The first live run (2026-07-24, `docs/notes/EPIC-70_calibration.md`) exposed a
**detector miss**: on 927 hands the blind Boss scored the true colluders `carol+
dave` at LLR **−114.56** (High confidence) while the card-aware oracle proved
real cheating (4 EV-sacrifices). Root cause is *structural*, not just a bad
constant:

> The SPRT **sums independent per-signal LLRs**, but each collusion *style*
> shows up in only one signal. A `dump` colluder produces **zero whipsaws**, and
> with `whipsaw_colluding = 0.15` every whipsaw-free hand contributes
> `ln((1−0.15)/(1−0.02)) ≈ −0.14` LLR — about **−126 over 900 hands** — swamping
> the genuine soft-play signal (`soft_play_index` 0.51, actually closer to the
> colluding model 0.25 than to honest ~1.0). The detector penalizes a pair on
> signals irrelevant to their style.

Run this end-to-end to close it:

1. **Enable session capture** — the `PKDEALER_RECORD_DIR` + `./out` mount from
   `epic-70-live-validation.md` (Setup step 2). Skip if already done.
2. **Capture an honest control run** (no team ⇒ the fitted null is honest-only):
   ```bash
   make detect DETECT_PLAYERS="gto lag tag tp"   # ~200+ hands, then: make arena-down
   ```
3. **Capture a colluding run** (the default lineup):
   ```bash
   make detect                                   # ~200+ hands, then: make arena-down
   ```
4. **Fit the honest null** from the control session (any honest pair; the null
   pools all pairs, all honest here):
   ```bash
   cargo run -p pkdealer_boss --example calibrate_run -- out/<honest-session>.yaml gto_1 lag_1
   ```
   Record the per-signal `mean`/`std` (`net_flow`, `soft_play_index`, `whipsaw_count`).
5. **Diagnose the colluders** and note which signals separate vs. penalize:
   ```bash
   cargo run -p pkdealer_boss --example calibrate_run -- out/<colluding-session>.yaml carol_1 dave_1
   ```
6. **Fix `SprtParams` in `crates/pkdealer_boss/src/detector.rs`** — address the
   structural trap first, then set honest models from the null:
   - Prefer a **per-signal max (or gated OR)** over the global sum, so a pair
     colluding in *one* style flags without being dragged down by the others;
     **or** move `whipsaw_colluding` / `flow_colluding` toward their honest twins
     so *absence* of a signal stops being strong "honest" evidence.
   - Set `default_honest_aggr` and the honest-side probabilities from the fitted
     null; keep `soft_play_discount` (0.25) only if the null supports it.
7. **Confirm**:
   ```bash
   cargo test -p pkdealer_boss                                   # unit + doctests still green
   cargo run -p pkdealer_boss --example calibrate_run -- out/<colluding-session>.yaml carol_1 dave_1
   #   → carol+dave now FLAGGED with a finite hands-to-detection
   cargo run -p pkdealer_boss --example calibrate_run -- out/<honest-session>.yaml gto_1 lag_1
   #   → honest pair still NOT flagged (FP stays 0)
   ```
8. **Record** the corrected numbers in `docs/notes/EPIC-70_calibration.md`,
   replacing the `n=1` miss row with the post-calibration result (and add the
   honest-control FP row).

Guardrail: a `dump` run is a *faint* target (few pair-only pots, unprofitable
here at −19.91 bb/100). Also capture a `soft`-style run (a `channel = "peer"`,
`style = "soft"` team) for a denser signal to calibrate against.
