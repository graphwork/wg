#!/usr/bin/env bash
# Scenario: harden_node_network (task harden-node — lock down the one exposed surface)
#
# The WG-Fed network node (`wg fed-node serve`) is the only genuinely-exposed surface
# (docs/prod-audit/00 B1/B2/M3/M4). This scenario proves the abuse-hardening holds over
# the wire against a hostile client, while a legitimate owner still publishes/polls:
#
#   1. Legit baseline: `wg identity publish` to the node is ACCEPTED (owner-signed
#      head + attestation + content-addressed objects all pass).
#   2. Write-auth (B1): the owner's real head with its signature STRIPPED is REJECTED
#      (403); the original owner-signed head is re-accepted (200).
#   3. CID integrity (M3): an object PUT under a chosen CID that is NOT its hash is
#      REJECTED (409) — no squatting a victim's content address with junk.
#   4. Bounded reads (B2): a request declaring a 4 GiB body (with none sent) is
#      REJECTED (413) immediately — the node never pre-allocates the lied length.
#   5. Inbox flood quota (B1/M4): with the per-inbox event cap tightened, delivery is
#      refused (507) past the cap.
#   6. Inbox GC / delete-after-ack (M4): a delivered event is listed, then DELETEd
#      (acked) and is gone — the inbox cannot grow without bound.
#   7. Slow-loris (M4): a client that opens a connection and never finishes its request
#      is closed by the read timeout (bounded), not pinned forever.
#
# One isolated $HOME keystore + one project dir + one real HTTP node with tightened
# limits (env-overridable). The transport is untrusted: every legit byte is
# self-verifying; every hostile write is refused at the boundary.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING python3" "needed for raw-socket + JSON assertions"
command -v curl >/dev/null 2>&1 || loud_skip "MISSING curl" "needed for adversarial HTTP writes"

scratch=$(make_scratch)
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"; STORE_A="$scratch/nodeA-store"
mkdir -p "$A_HOME/.config" "$A_DIR" "$STORE_A"

FED_PIDS_FILE="$scratch/fed_pids"; : >"$FED_PIDS_FILE"
kill_node() { local pid="$1"; pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; }
kill_fed_nodes() { while read -r p; do kill_node "$p"; done <"$FED_PIDS_FILE"; }
add_cleanup_hook kill_fed_nodes

wgrun() { # wgrun args...
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$A_HOME" XDG_CONFIG_HOME="$A_HOME/.config" \
        wg --dir "$A_DIR" "$@"
}
jfield() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
san() { printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'; }

# Tighten the node's limits so the flood/slow-loris bounds are exercisable in a test.
NODE_ENV=(WG_FED_NODE_INBOX_MAX_EVENTS=2 WG_FED_NODE_READ_TIMEOUT_MS=1000)
env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
    -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
    HOME="$A_HOME" XDG_CONFIG_HOME="$A_HOME/.config" "${NODE_ENV[@]}" \
    wg --dir "$A_DIR" fed-node serve --addr 127.0.0.1:0 --store "$STORE_A" \
    >"$scratch/nodeA.log" 2>&1 &
NODE_PID=$!; echo "$NODE_PID" >>"$FED_PIDS_FILE"
NODE=""
for i in $(seq 1 100); do
    NODE=$(grep -oE 'http://127\.0\.0\.1:[0-9]+' "$scratch/nodeA.log" | head -1)
    [ -n "$NODE" ] && break
    kill -0 "$NODE_PID" 2>/dev/null || loud_fail "node failed to start: $(cat "$scratch/nodeA.log")"
    sleep 0.1
done
[ -n "$NODE" ] || loud_fail "node did not report a listening address"
endpoint_reachable "$NODE/wgfed/v1/health" || loud_fail "node health unreachable ($NODE)"
echo "STEP 0 ok: hardened fed-node listening ($NODE)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — Legit baseline: a real owner publish is ACCEPTED through the hardened node.
# ───────────────────────────────────────────────────────────────────────────────
wgrun --json identity new alice >"$scratch/alice.json" 2>"$scratch/alice.err" ||
    loud_fail "mint alice: $(cat "$scratch/alice.err")"
ALICE=$(jfield "['wgid']" <"$scratch/alice.json")
wgrun --json identity publish alice --store "$NODE" >"$scratch/pub.json" 2>"$scratch/pub.err" ||
    loud_fail "STEP 1 FAILED: legit publish was rejected by the hardened node: $(cat "$scratch/pub.err")"
echo "STEP 1 ok: legitimate owner-signed publish ACCEPTED"

SAN_ALICE=$(san "$ALICE")
HEADS_URL="$NODE/wgfed/v1/heads/$SAN_ALICE"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — Write-auth (B1): unsigned head REJECTED; owner-signed head accepted.
# ───────────────────────────────────────────────────────────────────────────────
curl -s "$HEADS_URL" -o "$scratch/head.json" || loud_fail "could not GET alice's head"
python3 -c "import json; d=json.load(open('$scratch/head.json')); d['sig']=''; json.dump(d, open('$scratch/head_unsigned.json','w'))"
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary @"$scratch/head_unsigned.json" "$HEADS_URL")
[ "$CODE" = "403" ] || loud_fail "STEP 2 FAILED: unsigned head write returned $CODE (expected 403)"
# The original, owner-signed head IS re-accepted (proves we reject the forgery, not all writes).
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary @"$scratch/head.json" "$HEADS_URL")
[ "$CODE" = "200" ] || loud_fail "STEP 2 FAILED: re-publishing the owner-signed head returned $CODE (expected 200)"
echo "STEP 2 ok: unsigned head REJECTED (403); owner-signed head accepted (200)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — CID integrity (M3): junk under a chosen CID is REJECTED.
# ───────────────────────────────────────────────────────────────────────────────
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary "totally unrelated bytes" \
    "$NODE/wgfed/v1/objects/b3_deadbeefdeadbeefdeadbeefdeadbeef")
[ "$CODE" = "409" ] || loud_fail "STEP 3 FAILED: bad-cid object write returned $CODE (expected 409)"
echo "STEP 3 ok: object whose CID != hash(bytes) REJECTED (409)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4 — Bounded reads (B2): a 4 GiB declared body (none sent) is REJECTED fast.
# ───────────────────────────────────────────────────────────────────────────────
RESP=$(python3 - "$NODE" <<'PY'
import socket, sys, urllib.parse as up
u = up.urlparse(sys.argv[1])
s = socket.create_connection((u.hostname, u.port), timeout=5)
req = ("PUT /wgfed/v1/objects/x HTTP/1.1\r\n"
       "Host: %s\r\nContent-Length: 4294967296\r\n\r\n" % u.hostname)
s.sendall(req.encode())
data = s.recv(256).decode(errors="replace")
print(data.splitlines()[0] if data else "")
PY
)
case "$RESP" in
    *413*) ;;
    *) loud_fail "STEP 4 FAILED: oversize-declared request did not get 413 (got: '$RESP')" ;;
esac
echo "STEP 4 ok: 4 GiB declared body REJECTED (413) without pre-allocation"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5 — Inbox flood quota (B1/M4): delivery refused past the per-inbox cap (=2).
# ───────────────────────────────────────────────────────────────────────────────
QBOX="wgid_zQuotaTarget"
c1=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary "e1" "$NODE/wgfed/v1/inbox/$QBOX/e1")
c2=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary "e2" "$NODE/wgfed/v1/inbox/$QBOX/e2")
c3=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary "e3" "$NODE/wgfed/v1/inbox/$QBOX/e3")
[ "$c1" = "200" ] && [ "$c2" = "200" ] || loud_fail "STEP 5 FAILED: deliveries under the cap returned $c1/$c2 (expected 200)"
[ "$c3" = "507" ] || loud_fail "STEP 5 FAILED: delivery over the cap returned $c3 (expected 507)"
echo "STEP 5 ok: inbox flood bounded — 3rd delivery over the cap REFUSED (507)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 6 — Inbox GC / delete-after-ack (M4): a delivered event is reclaimed on ack.
# ───────────────────────────────────────────────────────────────────────────────
ABOX="wgid_zAckTarget"
curl -s -o /dev/null -X PUT --data-binary "payload" "$NODE/wgfed/v1/inbox/$ABOX/m1"
N1=$(curl -s "$NODE/wgfed/v1/inbox/$ABOX" | jfield "['events'].__len__()")
[ "$N1" = "1" ] || loud_fail "STEP 6 FAILED: delivered event not listed (count=$N1)"
DCODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "$NODE/wgfed/v1/inbox/$ABOX/m1")
[ "$DCODE" = "200" ] || loud_fail "STEP 6 FAILED: ack-delete returned $DCODE (expected 200)"
N2=$(curl -s "$NODE/wgfed/v1/inbox/$ABOX" | jfield "['events'].__len__()")
[ "$N2" = "0" ] || loud_fail "STEP 6 FAILED: acked event not reclaimed (count=$N2)"
echo "STEP 6 ok: delete-after-ack reclaims a consumed event (inbox bounded)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 7 — Slow-loris (M4): a stalled connection is closed by the read timeout.
# ───────────────────────────────────────────────────────────────────────────────
BOUNDED=$(python3 - "$NODE" <<'PY'
import socket, sys, time, urllib.parse as up
u = up.urlparse(sys.argv[1])
s = socket.create_connection((u.hostname, u.port), timeout=5)
s.sendall(b"PUT /wgfed/v1/objects/x HTTP/1.1")  # partial line; then stall forever
s.settimeout(5)
t0 = time.time()
try:
    while True:
        b = s.recv(256)         # returns b"" when the server closes us (EOF)
        if not b:
            break
except socket.timeout:
    print("PINNED"); sys.exit(0)
print("BOUNDED" if time.time() - t0 < 5 else "PINNED")
PY
)
[ "$BOUNDED" = "BOUNDED" ] || loud_fail "STEP 7 FAILED: slow-loris connection was not bounded by the read timeout"
echo "STEP 7 ok: slow-loris connection closed by the read timeout (bounded)"

echo "ALL STEPS PASSED — WG-Fed node hardened: write-auth, bounded reads, CID-verify, quota+GC, timeouts"
exit 0
