# Demo: EPIC-70 Live Validation — Vector-B Collusion + the Live Boss

> Two bots secretly share cards over a peer backchannel, and a blind "Boss"
> observer catches them in real time — knowing only what any spectator sees.

## Audience & framing

Engineering / research. The angle: this is a *validation run*, not a polished
feature demo — the code is built and unit-tested, and this session proves the
Vector-B broker path and the live Boss work end-to-end on a real arena, then
harvests the numbers that fill the calibration write-up. Be upfront that Phases
4–5 were authored "unvalidated"; this run is what turns that into "validated."

> Note: the docker steps below could not be smoked from the authoring session
> (no running arena there). Every `Expected:` line marked _(from code)_ is
> derived from the implementation, not a live run — treat this as a dress
> rehearsal script and confirm each as you go.

## Prerequisites

- Repo checked out at the root, on branch `boss`.
- Docker + `docker compose` v2 running, ~4 GB free for images.
- `cargo` toolchain (only if you also want the offline analysis step).
- Ports free on the host: `50051` (dealer), `16686` (Jaeger), `9090`
  (Prometheus), `3001` (Grafana).
- Terminal layout: **two panes** — pane A drives the arena, pane B tails the
  Boss. Optionally a browser with tabs for Jaeger (`localhost:16686`) and
  Prometheus (`localhost:9090`).
- The colluding pair and Boss already exist in `arena.toml`: `carol`+`dave`
  (team B, `channel = "peer"`, `style = "dump"`) and `boss` (type `boss`). No
  edits needed for the core demo.

## Setup (~6 min, first build longer)

1. **Confirm the lineup expands correctly (no containers yet)**
   ```bash
   ./bin/arena --dry-run carol dave gto lag boss
   ```
   _Expected:_ prints an override path; it contains `--collusion-channel peer`,
   a `pkdealer_backchannel:` service, and an `agent_boss_1:` service. _(verified)_
   _Talking point:_ team membership in one TOML field fans out to all the wiring.

2. **(Optional, for the offline step) capture sessions to disk.** The service
   only persists hands when `PKDEALER_RECORD_DIR` is set with a host mount. Edit
   the **repo-root** `docker-compose.yml` (not a file under `docs/`) — the
   `pkdealer_service:` block already has an `environment:` map and a `volumes:`
   list, so add lines *into those existing maps* (a second `environment:`/
   `volumes:` key is invalid YAML). Add one line under `environment:`:
   ```yaml
         PKDEALER_RECORD_DIR: /out
   ```
   and one entry under the existing `volumes:` (next to the `pricing.toml` mount):
   ```yaml
         - ./out:/out
   ```
   ```bash
   mkdir -p ./out
   ```
   _Expected:_ `./out` exists; after a run it holds a `session-*.yaml`. _(from code — EPIC-25 sink)_
   _Talking point:_ the offline Boss needs a recorded file; the live Boss doesn't.

3. **Bring up the arena: peer colluders + honest bots + live Boss**
   ```bash
   ./bin/arena carol dave gto lag boss
   ```
   _Expected:_ builds images, then "Arena is live" with Jaeger/Grafana/Prom URLs;
   `docker compose ps` shows `agent_carol_1`, `agent_dave_1`, `agent_boss_1`, and
   `pkdealer_backchannel` all `Up`. _(from code)_
   _Talking point:_ the broker is a first-class service the colluders dial by
   hostname — the dealer never sees a card cross.

4. **Capture the override path** (the log/teardown commands below reuse it). Use
   the `Override file:` line `bin/arena` just printed, or grab the newest:
   ```bash
   OVERRIDE=$(ls -t "${TMPDIR:-/tmp}"/docker-compose.arena.*.yml | head -1)
   ```
   _Expected:_ `echo "$OVERRIDE"` prints a real `.yml` path. _(verified)_

## The demo (~15 min — the Boss needs ≥ 50 hands before it will flag)

### 1. Show the broker relaying cards (Vector B is live)

1. **Tail the backchannel broker**
   ```bash
   docker compose -f docker-compose.yml -f "$OVERRIDE" logs -f pkdealer_backchannel
   ```
   _Expected:_ `relaying on 0.0.0.0:9099`, then connection lines as carol/dave
   dial in. _(from code)_
   _Talking point:_ this traffic is entirely outside the dealer — no spectator
   token, no privileged read.

2. **Confirm the colluders run the collusion build**
   ```bash
   docker compose -f docker-compose.yml -f "$OVERRIDE" logs agent_carol_1 | head
   ```
   _Expected:_ startup banner naming partner `dave_1`, channel `peer`, style
   `dump`. _(from code)_
   _Talking point:_ `config.channel` appears only at startup — the decide path
   is channel-blind by construction.

### 2. Watch the Boss accumulate evidence (pane B)

3. **Tail the live Boss**
   ```bash
   docker compose -f docker-compose.yml -f "$OVERRIDE" logs -f agent_boss_1
   ```
   _Expected:_ `boss online — polling for completed hands`, then per-cycle
   ingest as hands complete. Silent on flags until the pair clears the 50-hand
   `Confidence` floor. _(from code)_
   _Talking point:_ the Boss says nothing with conviction early — a nit and a
   colluder look alike for a few hands; only accumulated LLR separates them.

4. **The flag** (after ~50+ completed hands)
   _Expected:_ a `WARN … FLAG: suspected collusion pair=<carol-uuid>+<dave-uuid>
   hand=NN` line. _(from code — unvalidated; this run confirms it)_
   _Talking point:_ the headline metric is *which hand* it first crossed — the
   time-to-detection.

### 3. See it in the telemetry (browser)

5. **Prometheus — the per-pair LLR gauge**
   Point at the Prometheus tab, query:
   ```
   pkdealer_boss_pair_llr
   ```
   _Expected:_ a rising series labelled with the carol+dave pair. _(from code — OTLP dots→underscores)_
   _Talking point:_ `pair_llr` gauge, `flag_hand` histogram, `false_positive`
   counter — all exported through the same collector as the dealer.

6. **Jaeger — the Boss service is present**
   In the Jaeger tab, service dropdown → `agent_boss_1`.
   _Expected:_ the Boss appears as its own service. _(from code)_

### 4. Offline analysis on a captured session (needs Setup step 2)

7. **Run the blind offline Boss on the recorded file** (detection only — no labels)
   ```bash
   cargo run -p pkdealer_boss -- --session ./out/session-*.yaml
   ```
   _Expected:_ a per-pair report; carol+dave rank top by LLR with a finite
   `flagged_at_hand`. _(from code)_
   _Talking point:_ same detector as the live Boss, but reading an
   already-redacted file — the provably-blind path, no token.

## Filling the calibration write-up (follow-up, partially manual today)

The detection story above needs **no labels**. The *grading* story — hands-to-
detection tables, false-positive rate with a CI, "did the cheat pay" — needs
ground-truth labels and a short driver, neither of which has a CLI yet. Be
honest about this in the room:

- **Labels:** `pkdealer_boss::labels::GroundTruthLabels::resolve(&collection,
  &[("carol_1","dave_1", LabelVector::Peer, LabelStyle::ChipDump)])` maps the
  composed names to the recorded UUIDs and `to_yaml()`s a sidecar. There is no
  binary for this — it's a ~10-line throwaway `examples/` program or a follow-up
  `pkdealer_boss --emit-labels` flag.
- **With labels**, the offline Boss adds the scorer/FP section:
  ```bash
  cargo run -p pkdealer_boss -- --session ./out/session-*.yaml --labels ./out/labels.yaml
  ```
- **Calibration (Phase 5):** `pkdealer_boss::calibrate::{fit_null,
  fp_rate_with_ci, win_rate_lift}` are library functions, fixture-tested, with
  no CLI. To fill `docs/notes/EPIC-70_calibration.md`, capture **K colluding**
  runs (`carol dave …`) and **K honest control** runs (same seats, no team) and
  feed the redacted sessions + verdicts + labels to those functions from a short
  driver. Until then every table in that write-up is a `pending live run`
  placeholder — by design, no fabricated numbers.

## What to highlight verbally

- The Boss catches the *behavior*, not the channel: `detector::assess` never
  receives the channel, and grading under `Spectator` vs `Peer` labels is
  byte-identical (pinned by a test that fails under an injected branch).
- The typed `RedactedHand` firewall means the detection library *structurally*
  cannot hold a hole card — the blind claim is enforced by types, not discipline.
- The live Boss holds the un-redacted export for one instant, then `redact()`s
  before any detector code runs — an honest trust-boundary the offline path
  sidesteps entirely.
- Sequential SPRT + a 50-hand confidence floor is why "how few hands" is a
  real, reportable number rather than a hand-wave.

## Likely questions & answers

- **Q: If the colluders can see all cards via the spectator token, isn't Vector
  B redundant?** A: Vector A rides the spectator token; Vector B uses no
  privileged read at all — the point is the Boss flags both *identically*, so
  it's catching coordination, not a token.
- **Q: Why did it take ~50 hands to flag?** A: A deliberate `Confidence` floor —
  below it the sample can't separate a colluder from a tight honest player, so
  the Boss withholds judgment.
- **Q: Does the dealer know cheating happened?** A: No. The broker is a separate
  service; the dealer's proto and logic are untouched. The sim measures
  detectability, it doesn't prevent collusion (that's pkcore EPIC-79).
- **Q: Are the Phase 4/5 numbers real?** A: The detection is real and live here.
  The calibration *tables* are placeholders until K seeded runs exist — we don't
  publish fabricated figures.
- **Q: Why no labels for the live demo?** A: Detection is blind and needs none;
  labels only grade the Boss after the fact (hands-to-detection, false positives).

## Cleanup

```bash
# Tear down the arena + volumes
docker compose -f docker-compose.yml -f "$OVERRIDE" down -v
```
- If you edited `docker-compose.yml` in Setup step 2, revert it:
  ```bash
  git checkout -- docker-compose.yml
  ```
- Remove captured sessions if not keeping them: `rm -rf ./out`.
- You stayed on branch `boss` throughout; nothing to reset.

## Troubleshooting

- **Boss never flags:** it needs ≥ 50 completed hands *and* the dump style to
  actually fire (a member must hold the stronger hand into a pair pot). Let it
  run longer, or lower `PKDEALER_ACTION_DELAY_SECS`/`PKDEALER_HAND_END_DELAY_SECS`
  on the host before `./bin/arena` to deal faster.
- **`agent_boss_1` exits / can't export:** the Boss must present the service's
  spectator token. It sends `${PKDEALER_SPECTATOR_TOKEN:-spectator}`; if you set
  a non-default token on the service, export the same value in the shell running
  `./bin/arena`.
- **No `session-*.yaml` in `./out`:** `PKDEALER_RECORD_DIR` wasn't set on the
  service or the `./out` mount is missing (Setup step 2). The live Boss still
  works without it; only the offline step needs the file.
- **`pkdealer_backchannel` not in `docker compose ps`:** it's only emitted when a
  `channel = "peer"` colluder is in the lineup. Confirm with the dry-run in
  Setup step 1.
