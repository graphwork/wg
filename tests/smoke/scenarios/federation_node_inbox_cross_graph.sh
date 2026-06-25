#!/usr/bin/env bash
# Scenario: federation_node_inbox_cross_graph (WG-Fed Wave 4 — cross-graph transport)
#
# The Wave-4 deliverable (docs/federation-study/06 §5 Wave 4, ADR-fed-002): signed
# cross-WG messaging at email-speed over a REAL NETWORK — two WG node inboxes
# (`wg fed-node serve`, HTTP store-and-forward) exchanging signed/sealed
# `SignedEvent`s addressed purely by `wgid:`, offline-tolerant, plus the S-3 freshness
# attestation fail-closed-on-stale guard. Each step is a falsifiable assertion:
#
#   1. Two daemons: `wg fed-node serve` nodes A and B bind ephemeral HTTP ports and
#      answer /wgfed/v1/health.
#   2. Publish over HTTP: alice publishes her bundle (+freshness attestation) to node
#      A, bob to node B, via the HttpStore client rung (no shared filesystem).
#   3. Key-based peers + cascade: `wg peer add --wgid --endpoint` registers the
#      cross-graph peers; `wg msg --to wgid:` resolves the delivery node via the
#      ADR-fed-001 §D5 cascade (cached endpoint record).
#   4. Cross-graph send to an OFFLINE recipient: alice `wg msg send --to bob --seal`
#      is accepted for store-and-forward at node B while bob is not polling.
#   5. Authenticated poll: bob `wg msg poll --as bob` over node B verifies the sender
#      by key and decrypts the sealed body; a FORGED "from alice" event is REJECTED.
#   6. Freshness (S-3): a fresh attestation passes a high-value check; once alice's
#      attestation goes stale, a high-value `check-fresh` and a `--require-fresh
#      high-value` poll both FAIL CLOSED.
#   7. Offline tolerance: with alice's origin node A DOWN, bob still authenticates her
#      message from his node B inbox using the cached, already-verified sigchain.
#
# Two isolated $HOME keystores (the custody boundary is $HOME-relative) + two project
# dirs + two real HTTP nodes. The transport is untrusted: every byte is self-verifying.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for JSON assertions"

scratch=$(make_scratch)

# Per-actor isolated HOME (=> isolated wg-secret keystore) + project dir (=> --dir).
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"
B_HOME="$scratch/B_home"; B_DIR="$scratch/B/.wg"
M_HOME="$scratch/M_home"; M_DIR="$scratch/M/.wg" # mallory, the forger
STORE_A="$scratch/nodeA-store"; STORE_B="$scratch/nodeB-store"
mkdir -p "$A_HOME/.config" "$B_HOME/.config" "$M_HOME/.config" \
    "$A_DIR" "$B_DIR" "$M_DIR" "$STORE_A" "$STORE_B"

# Track + reap the fed-node processes we spawn (they are not `wg service daemon`,
# so the helper sweep won't catch them — register an explicit cleanup hook).
FED_PIDS_FILE="$scratch/fed_pids"; : >"$FED_PIDS_FILE"
# A backgrounded shell function runs in a subshell; the real `wg fed-node` is its
# child (env exec's into wg). Kill the child too, or the server keeps serving.
kill_node() {
    local pid="$1"
    pkill -P "$pid" 2>/dev/null
    kill "$pid" 2>/dev/null
}
kill_fed_nodes() {
    while read -r p; do kill_node "$p"; done <"$FED_PIDS_FILE"
}
add_cleanup_hook kill_fed_nodes

# Run wg as a given actor: isolated HOME (keystore) + explicit --dir (local state).
wgrun() { # wgrun <home> <wgdir> args...
    local home="$1" wgdir="$2"
    shift 2
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$home" XDG_CONFIG_HOME="$home/.config" \
        wg --dir "$wgdir" "$@"
}

jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
san() { printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'; }

# Start a fed-node on an ephemeral port; record url/pid into <prefix>.{url,pid,log}.
start_node() { # start_node <home> <wgdir> <store> <prefix>
    local home="$1" wgdir="$2" store="$3" prefix="$4"
    wgrun "$home" "$wgdir" fed-node serve --addr 127.0.0.1:0 --store "$store" \
        >"$prefix.log" 2>&1 &
    local pid=$!
    echo "$pid" >>"$FED_PIDS_FILE"
    echo "$pid" >"$prefix.pid"
    local i base
    for i in $(seq 1 100); do
        base=$(grep -oE 'http://127\.0\.0\.1:[0-9]+' "$prefix.log" | head -1)
        [ -n "$base" ] && {
            echo "$base" >"$prefix.url"
            return 0
        }
        kill -0 "$pid" 2>/dev/null || return 1
        sleep 0.1
    done
    return 1
}

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — Two daemons: bind the node inboxes.
# ───────────────────────────────────────────────────────────────────────────────
start_node "$A_HOME" "$A_DIR" "$STORE_A" "$scratch/nodeA" ||
    loud_fail "node A failed to start: $(cat "$scratch/nodeA.log" 2>/dev/null)"
start_node "$B_HOME" "$B_DIR" "$STORE_B" "$scratch/nodeB" ||
    loud_fail "node B failed to start: $(cat "$scratch/nodeB.log" 2>/dev/null)"
NODE_A=$(cat "$scratch/nodeA.url"); NODE_A_PID=$(cat "$scratch/nodeA.pid")
NODE_B=$(cat "$scratch/nodeB.url"); NODE_B_PID=$(cat "$scratch/nodeB.pid")

if command -v curl >/dev/null 2>&1; then
    endpoint_reachable "$NODE_A/wgfed/v1/health" || loud_fail "node A health unreachable ($NODE_A)"
    endpoint_reachable "$NODE_B/wgfed/v1/health" || loud_fail "node B health unreachable ($NODE_B)"
fi
echo "STEP 1 ok: two wg fed-node inboxes listening ($NODE_A, $NODE_B)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — Mint + publish each identity to ITS OWN node over HTTP (the default rung).
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$A_HOME" "$A_DIR" --json identity new alice >"$scratch/alice.json" 2>"$scratch/alice.err" ||
    loud_fail "mint alice: $(cat "$scratch/alice.err")"
ALICE=$(jfield "['wgid']" <"$scratch/alice.json")
wgrun "$B_HOME" "$B_DIR" --json identity new bob >"$scratch/bob.json" 2>&1 ||
    loud_fail "mint bob failed"
BOB=$(jfield "['wgid']" <"$scratch/bob.json")
case "$ALICE" in wgid:z*) ;; *) loud_fail "alice wgid malformed: $ALICE" ;; esac

wgrun "$A_HOME" "$A_DIR" --json identity publish alice --store "$NODE_A" \
    >"$scratch/pubA.json" 2>"$scratch/pubA.err" ||
    loud_fail "publish alice to node A over HTTP: $(cat "$scratch/pubA.err")"
[ "$(jfield "['attestation_cid']" <"$scratch/pubA.json")" != "None" ] ||
    loud_fail "STEP 2 FAILED: publish did not emit a freshness attestation"
wgrun "$B_HOME" "$B_DIR" identity publish bob --store "$NODE_B" >/dev/null 2>"$scratch/pubB.err" ||
    loud_fail "publish bob to node B over HTTP: $(cat "$scratch/pubB.err")"
echo "STEP 2 ok: alice→node A, bob→node B over HTTP; freshness attestation present"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — Register key-based peers (the cascade's cached endpoint record).
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$A_HOME" "$A_DIR" peer add bob --wgid "$BOB" --endpoint "$NODE_B" >/dev/null 2>"$scratch/peerA.err" ||
    loud_fail "peer add bob (key-based): $(cat "$scratch/peerA.err")"
wgrun "$B_HOME" "$B_DIR" peer add alice --wgid "$ALICE" --endpoint "$NODE_A" >/dev/null 2>"$scratch/peerB.err" ||
    loud_fail "peer add alice (key-based): $(cat "$scratch/peerB.err")"

# Old path-based peers MUST keep working alongside key-based ones: add one and list.
wgrun "$A_HOME" "$A_DIR" peer add legacy "$scratch/B" >/dev/null 2>&1 ||
    loud_fail "path-based peer add should still work"
wgrun "$A_HOME" "$A_DIR" peer list 2>/dev/null | grep -q "legacy" ||
    loud_fail "STEP 3 FAILED: path-based peer 'legacy' did not resolve in peer list"
echo "STEP 3 ok: key-based peers added; path-based peer still resolves alongside"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4 — Cross-graph send to an OFFLINE recipient, addressed by wgid via cascade.
# ───────────────────────────────────────────────────────────────────────────────
SECRET="hello bob from alice over the node inbox"
wgrun "$A_HOME" "$A_DIR" --json msg send --to bob --from alice --body "$SECRET" --seal \
    >"$scratch/send.json" 2>"$scratch/send.err" ||
    loud_fail "cross-graph msg send --to bob failed: $(cat "$scratch/send.err")"
[ "$(jfield "['accepted']" <"$scratch/send.json")" = "True" ] ||
    loud_fail "STEP 4 FAILED: event not accepted for delivery"
[ "$(jfield "['sealed']" <"$scratch/send.json")" = "True" ] ||
    loud_fail "STEP 4 FAILED: event was not sealed"
echo "STEP 4 ok: signed+sealed wg msg --to wgid: accepted at node B for offline bob"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5 — Bob establishes contact (fetch+verify+cache alice), polls, verifies; a
#          forged "from alice" event is REJECTED.
# ───────────────────────────────────────────────────────────────────────────────
wgrun "$B_HOME" "$B_DIR" --json identity fetch "$ALICE" --store "$NODE_A" --save alice \
    >"$scratch/fetchA.json" 2>"$scratch/fetchA.err" ||
    loud_fail "bob fetch+verify alice over HTTP: $(cat "$scratch/fetchA.err")"
[ "$(jfield "['verified']" <"$scratch/fetchA.json")" = "True" ] ||
    loud_fail "STEP 5 FAILED: alice not verified offline"
[ "$(jfield "['has_attestation']" <"$scratch/fetchA.json")" = "True" ] ||
    loud_fail "STEP 5 FAILED: fetched bundle reports no freshness attestation"

wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$NODE_B" \
    >"$scratch/poll1.json" 2>"$scratch/poll1.err" ||
    loud_fail "bob cross-graph poll failed: $(cat "$scratch/poll1.err")"
[ "$(jfield "['accepted']" <"$scratch/poll1.json")" = "1" ] ||
    loud_fail "STEP 5 FAILED: genuine event did not verify (accepted != 1)"
[ "$(jfield "['rejected']" <"$scratch/poll1.json")" = "0" ] ||
    loud_fail "STEP 5 FAILED: genuine poll had unexpected rejections"
GOT=$(jfield "['events'][0]['body']" <"$scratch/poll1.json")
[ "$GOT" = "$SECRET" ] ||
    loud_fail "STEP 5 FAILED: sealed body did not decrypt to the sent message (got: $GOT)"
GOTFROM=$(jfield "['events'][0]['from']" <"$scratch/poll1.json")
[ "$GOTFROM" = "$ALICE" ] ||
    loud_fail "STEP 5 FAILED: authenticated sender mismatch (got: $GOTFROM)"

# Forge a "from alice" event: mallory authors a genuine event to bob, then we rewrite
# its `from` to alice. Mallory's signature does not verify against alice's key set.
wgrun "$M_HOME" "$M_DIR" --json identity new mallory >"$scratch/mallory.json" 2>&1 ||
    loud_fail "mint mallory failed"
wgrun "$M_HOME" "$M_DIR" identity send --from mallory --to "$BOB" --store "$NODE_B" \
    --body "i am totally alice" >/dev/null 2>"$scratch/msend.err" ||
    loud_fail "mallory failed to send her own (genuine) event: $(cat "$scratch/msend.err")"
inbox="$STORE_B/inbox/$(san "$BOB")"
python3 - "$inbox" "$ALICE" <<'PY'
import glob, json, os, sys
inbox, alice = sys.argv[1], sys.argv[2]
# Find mallory's event (the one NOT from alice) and forge its `from` to alice.
for p in glob.glob(os.path.join(inbox, "*.json")):
    ev = json.load(open(p))
    if ev.get("from") != alice:
        ev["from"] = alice
        json.dump(ev, open(p, "w"))
        break
PY
wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$NODE_B" \
    >"$scratch/poll2.json" 2>&1 || loud_fail "bob re-poll (forge test) errored"
[ "$(jfield "['rejected']" <"$scratch/poll2.json")" -ge 1 ] ||
    loud_fail "STEP 5 FAILED: forged 'from alice' event was NOT rejected"
echo "STEP 5 ok: genuine event verified+decrypted; forged 'from alice' REJECTED"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 6 — Freshness (S-3): fresh high-value check passes; stale fails CLOSED.
# ───────────────────────────────────────────────────────────────────────────────
# A genuinely fresh attestation satisfies a tight high-value freshness check.
wgrun "$B_HOME" "$B_DIR" identity check-fresh "$ALICE" --store "$NODE_A" --class high-value \
    >/dev/null 2>"$scratch/cf1.err" ||
    loud_fail "STEP 6 FAILED: fresh high-value check-fresh should PASS: $(cat "$scratch/cf1.err")"

# Alice re-issues an already-EXPIRED attestation (back-dated TTL). bump seq, stale time.
wgrun "$A_HOME" "$A_DIR" identity attest alice --store "$NODE_A" --fresh-ttl=-600 \
    >/dev/null 2>"$scratch/attest.err" ||
    loud_fail "alice attest (stale) failed: $(cat "$scratch/attest.err")"

# High-value check now FAILS CLOSED (exit non-zero) on the stale attestation.
if wgrun "$B_HOME" "$B_DIR" identity check-fresh "$ALICE" --store "$NODE_A" --class high-value \
    >/dev/null 2>&1; then
    loud_fail "STEP 6 FAILED: high-value check-fresh PASSED on a stale attestation (must fail closed)"
fi

# And the end-to-end gate: a --require-fresh high-value poll refuses the event while
# alice's attestation is stale (re-fetched from her node A via the cascade).
wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$NODE_B" --require-fresh high-value \
    >"$scratch/poll_fresh.json" 2>&1 || loud_fail "freshness-gated poll errored"
[ "$(jfield "['accepted']" <"$scratch/poll_fresh.json")" = "0" ] ||
    loud_fail "STEP 6 FAILED: stale-gated high-value event was accepted (must fail closed)"
echo "STEP 6 ok: fresh high-value check passes; stale attestation fails CLOSED (check + poll gate)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 7 — Offline tolerance: take alice's origin node A DOWN; bob still authenticates
#          her message from his node B inbox via the cached, verified sigchain.
# ───────────────────────────────────────────────────────────────────────────────
kill_node "$NODE_A_PID"
sleep 0.5
# Sanity: node A really is down.
if command -v curl >/dev/null 2>&1; then
    endpoint_reachable "$NODE_A/wgfed/v1/health" &&
        loud_fail "STEP 7 setup: node A should be DOWN but still answers"
fi
wgrun "$B_HOME" "$B_DIR" --json msg poll --as bob --store "$NODE_B" \
    >"$scratch/poll_offline.json" 2>"$scratch/poll_offline.err" ||
    loud_fail "offline re-poll errored: $(cat "$scratch/poll_offline.err")"
[ "$(jfield "['accepted']" <"$scratch/poll_offline.json")" -ge 1 ] ||
    loud_fail "STEP 7 FAILED: with alice's origin down, her message no longer verifies (offline tolerance broken)"
echo "STEP 7 ok: alice's origin node A down — bob STILL verifies her message from his inbox (cache)"

echo "ALL STEPS PASSED — WG-Fed Wave 4 cross-graph node-inbox messaging verified"
exit 0
