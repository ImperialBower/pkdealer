#!/usr/bin/env bash
# EPIC-70 Phase 3: a peer-channel team expands into a `pkdealer_backchannel`
# broker service + PKDEALER_BACKCHANNEL wiring; a spectator team and an honest
# lineup emit neither. Dry-run only — no containers are started.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "FAIL: $*" >&2; exit 1; }

# ── peer team: carol + dave on team B, channel = peer ────────────────────────
out="$(./bin/arena --dry-run carol dave gto)"
override="$(sed -n 's/^Override file: //p' <<<"$out")"
[[ -f "$override" ]] || fail "no override file emitted"

grep -q -- '"--collusion-channel", "peer"' "$override"   || fail "peer channel flag missing"
grep -q -- '"--collude-with", "dave_1"' "$override"      || fail "carol_1 lacks partner flag"
grep -q -- '"--collude-with", "carol_1"' "$override"     || fail "dave_1 lacks partner flag"

# The broker service is emitted exactly once when a peer colluder is present.
brokers="$(grep -c -- '^  pkdealer_backchannel:' "$override" || true)"
[[ "$brokers" == 1 ]] || fail "expected exactly one broker service, got $brokers"
grep -q -- 'PKDEALER_BACKCHANNEL: pkdealer_backchannel:9099' "$override" \
  || fail "PKDEALER_BACKCHANNEL env missing on peer seats"
# Both peer seats declare a dependency on the broker.
deps="$(grep -c -- '- pkdealer_backchannel' "$override" || true)"
[[ "$deps" -ge 2 ]] || fail "peer seats do not depend_on the broker (got $deps)"
# The honest third seat carries none of the collusion wiring.
gto_cmd="$(awk '/^  agent_gto_1:/{f=1} f && /command:/{print; exit}' "$override")"
[[ "$gto_cmd" != *collude* ]] || fail "honest seat gto_1 carries collusion flags"

# ── spectator team must NOT emit a broker ────────────────────────────────────
out2="$(./bin/arena --dry-run mallory trudy gto)"
override2="$(sed -n 's/^Override file: //p' <<<"$out2")"
grep -q -- '"--collusion-channel", "spectator"' "$override2" \
  || fail "spectator team lost its channel flag"
grep -q -- 'pkdealer_backchannel' "$override2" && fail "spectator team emitted a broker"

# ── honest lineup: no broker, no collusion ───────────────────────────────────
out3="$(./bin/arena --dry-run gto lag)"
override3="$(sed -n 's/^Override file: //p' <<<"$out3")"
grep -q -- 'pkdealer_backchannel' "$override3" && fail "honest lineup emitted a broker"
grep -q -- '--collude-with' "$override3" && fail "honest lineup emitted collusion flags"

echo "OK: arena peer-channel expansion"
