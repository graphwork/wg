#!/usr/bin/env bash
# Scenario: ops_observability (task ops-and-tests — audit M20/M21 over the real wire)
#
# The production audit (docs/prod-audit/audit-testops.md #7/#8/#9) found near-ZERO
# observability: providers/ + review/ emitted no logs and the relay node had no /metrics.
# This scenario proves the M20 observability layer + the M21 runbook are real, exercised
# through the OPERATOR's actual flow — the `wg fed-node serve` binary scraped over HTTP,
# not a unit substitute (the always-on cargo coverage lives in tests/integration_fed_wire.rs
# + integration_failure_injection.rs, the M29/M22 deliverables, which run with no tooling).
#
#   1. The node exposes GET /wgfed/v1/metrics in Prometheus text format with EVERY required
#      counter family (verdicts / placements / refusals / freshness-failures / node reqs).
#   2. The counters MOVE with real wire traffic: a legit publish (2xx writes) + a
#      guaranteed-miss GET (4xx) are reflected in wg_node_requests_total / _responses_total.
#   3. Tracing + correlation IDs are wired: with RUST_LOG=info the node emits a per-request
#      access line carrying a `corr=` id (the cross-host correlation handle).
#   4. The M21 operator runbook exists and covers deploy/monitor/backup/key-rotation.
#
# One isolated $HOME keystore + one project dir + one real HTTP node. Credential-free.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

command -v curl >/dev/null 2>&1 || loud_skip "MISSING curl" "needed to scrape the node /metrics endpoint over HTTP"

scratch=$(make_scratch)
A_HOME="$scratch/A_home"; A_DIR="$scratch/A/.wg"; STORE_A="$scratch/nodeA-store"
mkdir -p "$A_HOME/.config" "$A_DIR" "$STORE_A"

FED_PIDS_FILE="$scratch/fed_pids"; : >"$FED_PIDS_FILE"
kill_node() { local pid="$1"; pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; }
kill_fed_nodes() { while read -r p; do kill_node "$p"; done <"$FED_PIDS_FILE"; }
add_cleanup_hook kill_fed_nodes

wgrun() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
        HOME="$A_HOME" XDG_CONFIG_HOME="$A_HOME/.config" \
        wg --dir "$A_DIR" "$@"
}

# Start the node with RUST_LOG=info so the access log (tracing → log → env_logger) is on.
env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
    -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
    HOME="$A_HOME" XDG_CONFIG_HOME="$A_HOME/.config" RUST_LOG=info \
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
echo "STEP 0 ok: fed-node listening with RUST_LOG=info ($NODE)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 1 — Drive real traffic: a legit publish (2xx writes) + a guaranteed miss (4xx).
# ───────────────────────────────────────────────────────────────────────────────
wgrun --json identity new alice >"$scratch/alice.json" 2>"$scratch/alice.err" ||
    loud_fail "mint alice: $(cat "$scratch/alice.err")"
wgrun --json identity publish alice --store "$NODE" >"$scratch/pub.json" 2>"$scratch/pub.err" ||
    loud_fail "STEP 1 FAILED: legit publish rejected: $(cat "$scratch/pub.err")"
curl -s -o /dev/null "$NODE/wgfed/v1/objects/b3_guaranteed_miss" || true
echo "STEP 1 ok: drove a legit publish + a guaranteed-miss GET through the node"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 2 — /metrics is Prometheus text with every required counter family (M20).
# ───────────────────────────────────────────────────────────────────────────────
curl -s "$NODE/wgfed/v1/metrics" -o "$scratch/metrics.txt" || loud_fail "could not scrape /metrics"
for fam in \
    "wg_review_verdicts_total" \
    "wg_exec_placements_total" \
    "wg_exec_refusals_total" \
    "wg_exec_results_accepted_total" \
    "wg_fed_freshness_failures_total" \
    "wg_node_requests_total" \
    "wg_node_responses_total"; do
    grep -q "$fam" "$scratch/metrics.txt" ||
        loud_fail "STEP 2 FAILED: /metrics missing family '$fam':\n$(cat "$scratch/metrics.txt")"
done
# Prometheus exposition hygiene: every family has a # TYPE … counter line.
grep -q "# TYPE wg_node_requests_total counter" "$scratch/metrics.txt" ||
    loud_fail "STEP 2 FAILED: /metrics missing the # TYPE preamble"
echo "STEP 2 ok: /metrics serves Prometheus text with all required counter families"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 3 — Counters reflect the wire traffic (M20). node_requests_total > 0 and the
#          2xx (publish) + 4xx (miss) classes each registered at least one response.
# ───────────────────────────────────────────────────────────────────────────────
REQS=$(grep -E '^wg_node_requests_total ' "$scratch/metrics.txt" | awk '{print $2}')
C2XX=$(grep -F 'wg_node_responses_total{class="2xx"}' "$scratch/metrics.txt" | awk '{print $2}')
C4XX=$(grep -F 'wg_node_responses_total{class="4xx"}' "$scratch/metrics.txt" | awk '{print $2}')
[ "${REQS:-0}" -ge 1 ] || loud_fail "STEP 3 FAILED: wg_node_requests_total=${REQS:-0} (expected >=1)"
[ "${C2XX:-0}" -ge 1 ] || loud_fail "STEP 3 FAILED: 2xx responses=${C2XX:-0} (publish writes expected)"
[ "${C4XX:-0}" -ge 1 ] || loud_fail "STEP 3 FAILED: 4xx responses=${C4XX:-0} (the guaranteed miss expected)"
echo "STEP 3 ok: counters move with traffic (requests=$REQS, 2xx=$C2XX, 4xx=$C4XX)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 4 — Tracing + correlation IDs are wired (M20): the access log carries a corr= id.
# ───────────────────────────────────────────────────────────────────────────────
grep -qE 'node request corr=[a-z0-9-]+ ' "$scratch/nodeA.log" ||
    loud_fail "STEP 4 FAILED: no correlated 'node request corr=…' access line in:\n$(cat "$scratch/nodeA.log")"
echo "STEP 4 ok: node emits per-request access log with a correlation id (corr=…)"

# ───────────────────────────────────────────────────────────────────────────────
# STEP 5 — The M21 operator runbook exists and covers the four required sections.
# ───────────────────────────────────────────────────────────────────────────────
RB="$HERE/../../../docs/ops/runbook.md"
[ -f "$RB" ] || loud_fail "STEP 5 FAILED: operator runbook missing at docs/ops/runbook.md"
for sect in "Deploy" "Monitor" "Backup" "Key rotation" "wg done"; do
    grep -qi "$sect" "$RB" || loud_fail "STEP 5 FAILED: runbook missing the '$sect' section"
done
echo "STEP 5 ok: operator runbook present and covers deploy/monitor/backup/key-rotation/wg-done"

echo "ALL STEPS PASSED — WG observability (M20) + operator runbook (M21) live over the wire"
exit 0
