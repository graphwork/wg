#!/usr/bin/env bash
# Scenario: pilot_dry_run — the TURNKEY family-team pilot deploy, rehearsed locally.
#
# `wg pilot up --dry-run` is the one-command stand-up of the whole family team modelled
# on ONE machine as two FS-isolated dirs sharing a single relay node — no remote hosts, no
# OpenRouter key, no Telegram tokens. It runs the entire live family-team flow end to end
# (identity → cross-graph task → content-review gate → borrowed-box exec under a scoped
# UCAN → signed result back), the same flow `e2e_family_team.sh` proves, but driven as the
# DEPLOY WRAPPER. This scenario is the falsifiable proof that:
#
#   1. ONE command stands up the pilot and the live family-team check PASSES.
#   2. The SAFE defaults are applied (fail-closed gate, slack-bounded leash, split trust,
#      configured-peer, confidential-remote refused) — no unsafe knob on by default.
#   3. Teardown (`wg pilot down`) actually stops the node and is idempotent.
#   4. An explicitly-unsafe config is REFUSED before anything is stood up.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"
command -v curl >/dev/null 2>&1 || loud_skip "MISSING curl" "the pilot health-probes the relay node"

scratch=$(make_scratch)
SD="$scratch/state"          # the pilot's runtime state dir (node pid/url + identities)
GRAPH="$scratch/graph"       # a throwaway --dir so we never touch the global graph/daemon
mkdir -p "$GRAPH"

jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# The pilot spawns a fed-node the smoke framework does not track. Tear it down on ANY exit
# (including a mid-scenario loud_fail) — via `wg pilot down` AND a direct pid kill as
# belt-and-braces, mirroring federation_node_inbox's explicit fed-node reaping.
pilot_teardown() {
    wg --dir "$GRAPH" pilot down --state-dir "$SD" --wipe-identities >/dev/null 2>&1 || true
    if [ -f "$SD/pilot-state.json" ]; then
        local pid
        pid=$(python3 -c "import json;print(json.load(open('$SD/pilot-state.json')).get('node_pid') or '')" 2>/dev/null || true)
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    fi
}
add_cleanup_hook pilot_teardown

# ───────────────────────────────────────────────────────────────────────────────
# 1. ONE command stands up the pilot and the live family-team check PASSES.
# ───────────────────────────────────────────────────────────────────────────────
up_json="$scratch/up.json"
wg --dir "$GRAPH" --json pilot up --dry-run --state-dir "$SD" \
    >"$up_json" 2>"$scratch/up.err" ||
    loud_fail "one-command dry-run stand-up failed (exit $?): $(cat "$scratch/up.err")"

[ "$(jfield "['mode']" <"$up_json")" = "dry-run" ] ||
    loud_fail "stand-up mode is not dry-run"
[ "$(jfield "['check_passed']" <"$up_json")" = "True" ] ||
    loud_fail "the live family-team check did NOT pass ($(cat "$scratch/up.err"))"
[ "$(python3 -c "import json,sys;print(len(json.load(sys.stdin)['identities']))" <"$up_json")" = "4" ] ||
    loud_fail "expected the 4 family identities minted into custody"
NODE_URL=$(jfield "['node_url']" <"$up_json")
endpoint_reachable "$NODE_URL/wgfed/v1/health" ||
    loud_fail "relay node not reachable at $NODE_URL after stand-up"
echo "up ok: one command stood up the pilot; live family-team check PASSED; node at $NODE_URL"

# ───────────────────────────────────────────────────────────────────────────────
# 2. SAFE defaults applied — no unsafe knob on by default.
# ───────────────────────────────────────────────────────────────────────────────
python3 - "$up_json" <<'PY'
import json, sys
sd = json.load(open(sys.argv[1]))["safe_defaults"]
assert sd["review_gate"] == "enforcing", f"gate not fail-closed: {sd}"
assert sd["confidential_remote"] == "refuse", f"confidential not refused: {sd}"
assert sd["peer_discovery"] == "configured", f"peer discovery not configured-only: {sd}"
assert sd["split_trust"] is True, f"split-trust off: {sd}"
assert sd["leash_max_ttl_secs"] > 0, f"leash unbounded/invalid: {sd}"
print("OK")
PY
[ $? -eq 0 ] || loud_fail "SAFE defaults were not applied"
echo "defaults ok: fail-closed gate + slack-bounded leash + split-trust + configured-peer + confidential-refused"

# The live check (which just passed) internally asserts the two safety-critical behaviors:
# a hostile inbound is BLOCKED before consumption, and a confidential task to the
# non-attested box is REFUSED (context never shipped). Re-run --no-check would skip them;
# check_passed=True above means both held. Confirm status agrees.
wg --dir "$GRAPH" --json pilot status --state-dir "$SD" >"$scratch/status.json" 2>&1 ||
    loud_fail "pilot status errored"
[ "$(jfield "['up']" <"$scratch/status.json")" = "True" ] ||
    loud_fail "status does not report the node UP"
[ "$(jfield "['check_passed']" <"$scratch/status.json")" = "True" ] ||
    loud_fail "status does not report the check PASSED"
echo "status ok: node UP, check PASSED, 4 identities recorded"

# ───────────────────────────────────────────────────────────────────────────────
# 3. Teardown actually stops the node and is idempotent.
# ───────────────────────────────────────────────────────────────────────────────
NODE_PID=$(jfield "['node_pid']" <"$up_json")
kill -0 "$NODE_PID" 2>/dev/null || loud_fail "node pid $NODE_PID not alive before teardown"
wg --dir "$GRAPH" pilot down --state-dir "$SD" >"$scratch/down.out" 2>&1 ||
    loud_fail "pilot down errored: $(cat "$scratch/down.out")"
# Give the SIGTERM a moment to land, then the node MUST be gone.
for _ in $(seq 1 20); do kill -0 "$NODE_PID" 2>/dev/null || break; sleep 0.1; done
kill -0 "$NODE_PID" 2>/dev/null && loud_fail "node pid $NODE_PID still alive after 'wg pilot down'"
[ -f "$SD/pilot-state.json" ] && loud_fail "down did not clear the state file"
# Idempotent: a second down with nothing up is a clean no-op (exit 0).
wg --dir "$GRAPH" pilot down --state-dir "$SD" >"$scratch/down2.out" 2>&1 ||
    loud_fail "idempotent second down returned non-zero"
grep -qi "nothing to tear down" "$scratch/down2.out" ||
    loud_fail "second down was not a clean no-op: $(cat "$scratch/down2.out")"
echo "teardown ok: down stopped the node (pid $NODE_PID gone) and is idempotent"

# ───────────────────────────────────────────────────────────────────────────────
# 4. An explicitly-unsafe config is REFUSED before anything is stood up.
# ───────────────────────────────────────────────────────────────────────────────
bad_cfg="$scratch/unsafe.toml"
cat >"$bad_cfg" <<'EOF'
[defaults]
confidential_remote = "allow"
EOF
if wg --dir "$GRAPH" pilot up --config "$bad_cfg" --state-dir "$scratch/state_bad" \
    >"$scratch/bad.out" 2>&1; then
    loud_fail "an unsafe [defaults].confidential_remote='allow' was NOT refused"
fi
grep -qi "unsafe default refused" "$scratch/bad.out" ||
    loud_fail "unsafe config refused without the expected loud reason: $(cat "$scratch/bad.out")"
[ -f "$scratch/state_bad/pilot-state.json" ] &&
    loud_fail "an unsafe config still wrote pilot state (should refuse before stand-up)"
echo "safety ok: an unsafe config is refused loudly before anything is stood up"

echo "PASS: pilot_dry_run — one command stands up the family team, live check passes, safe defaults applied, teardown clean+idempotent, unsafe config refused"
exit 0
