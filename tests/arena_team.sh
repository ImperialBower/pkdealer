#!/usr/bin/env bash
# EPIC-70 Phase 0f: team → pairwise collusion flag expansion (dry-run only).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "FAIL: $*" >&2; exit 1; }

out="$(./bin/arena --dry-run mallory trudy gto)"
override="$(sed -n 's/^Override file: //p' <<<"$out")"
[[ -f "$override" ]] || fail "no override file emitted"

grep -q -- '"--collude-with", "trudy_1"' "$override"   || fail "mallory_1 lacks partner flag"
grep -q -- '"--collude-with", "mallory_1"' "$override" || fail "trudy_1 lacks partner flag"
grep -q -- '"--collusion-channel", "spectator"' "$override" || fail "channel flag missing"
grep -q -- '"--collusion-style", "soft"' "$override"   || fail "style flag missing"
grep -q -- 'agent_rules_collusion' "$override"         || fail "colluding image not used"
grep -q -- 'FEATURES: collusion' "$override"           || fail "FEATURES build arg missing"

gto_cmd="$(awk '/^  agent_gto_1:/{f=1} f && /command:/{print; exit}' "$override")"
[[ "$gto_cmd" != *collude* ]] || fail "honest seat gto_1 carries collusion flags"

out2="$(./bin/arena --dry-run gto lag)"
override2="$(sed -n 's/^Override file: //p' <<<"$out2")"
grep -q -- '--collude-with' "$override2" && fail "team-less lineup emitted collusion flags"

echo "OK: arena team expansion"
