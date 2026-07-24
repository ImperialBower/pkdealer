#!/usr/bin/env bash
# EPIC-70 Phase 4: the `boss` type emits a live-detector container — no
# --name/--profile, with the spectator token in its environment. Dry-run only.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() { echo "FAIL: $*" >&2; exit 1; }

out="$(./bin/arena --dry-run gto lag boss)"
override="$(sed -n 's/^Override file: //p' <<<"$out")"
[[ -f "$override" ]] || fail "no override file emitted"

grep -q -- '^  agent_boss_1:' "$override"                 || fail "boss service missing"
grep -q -- 'BIN_NAME: pkdealer_agent_boss' "$override"    || fail "boss BIN_NAME missing"
grep -q -- 'PKDEALER_SPECTATOR_TOKEN' "$override"         || fail "boss lacks spectator token env"

# The boss carries no player command (no --name / --profile).
boss_block="$(awk '/^  agent_boss_1:/{f=1;next} f&&/^  [a-z]/{f=0} f' "$override")"
[[ "$boss_block" != *'--name'* ]]    || fail "boss carries a --name flag"
[[ "$boss_block" != *'--profile'* ]] || fail "boss carries a --profile flag"

# A seated bot in the same lineup still gets its normal command.
grep -q -- '"--name", "gto_1", "--profile", "gto"' "$override" || fail "seated bot lost its command"

echo "OK: arena boss wiring"
