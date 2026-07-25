#!/usr/bin/env bash
# Scenario: evaluation_satellite_not_charged_against_open_task
#
# Pins the fix for the resolve-prophage-source incident: a stale
# `.evaluate-*` satellite must NEVER be scheduled (or fail-charged) against
# a source task that is still open/incomplete. Before the fix, a bulk-retry
# reset of the source left a zombie `.evaluate-*` behind (its `after` edge
# had been destructively stripped), so it respawned every tick and failed
# instantly with "task is open - can't evaluate" — and the loop drove
# spawn_failures up.
#
# Credential-free / daemon-free: tasks are materialised directly in
# graph.jsonl and the user-visible invariants are exercised through the
# `wg` CLI (`wg ready`, `wg show`, `wg reset`, `wg recover`):
#
#   1. `.evaluate-X` IS ready when X is failed-pending-eval (rescue bypass).
#   2. `.evaluate-X` IS ready when X is failed (§4.3 — failed tasks get
#      evaluated), with the `after` edge PRESERVED (no destructive strip).
#   3. ZOMBIE INVARIANT — when X reopens, `.evaluate-X` is NOT ready (the
#      preserved `after` edge re-blocks it). No respawn, no charge: both
#      source and satellite keep spawn_failures == 0.
#   4. `wg reset X` (even WITHOUT --also-strip-meta) cancels the stale
#      `.evaluate-X` / `.flip-X` / `.assign-X` so the agency pipeline
#      regenerates cleanly on the next completion.
#   5. `wg recover` on a failed source abandons its stale eval satellite;
#      the abandoned satellite does NOT respawn against the reopened source.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

graph_dir=""
for cand in .wg .workgraph; do
    if [[ -f "$scratch/$cand/graph.jsonl" ]]; then
        graph_dir="$scratch/$cand"
        break
    fi
done
if [[ -z "$graph_dir" ]]; then
    loud_fail "could not locate graph.jsonl under .wg/ after init"
fi
graph="$graph_dir/graph.jsonl"

# Append a minimal task line directly to graph.jsonl (bypasses `wg add`'s
# draft pause so the task is immediately eligible for `wg ready`).
add_task() {
    # args: id title status [after_id...]
    local id="$1" title="$2" status="$3"; shift 3
    local after_json="[]"
    if [[ $# -gt 0 ]]; then
        after_json=$(printf '%s\n' "$@" | python3 -c "import sys,json; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))")
    fi
    python3 - "$graph" "$id" "$title" "$status" "$after_json" <<'PY'
import json, sys
path, tid, title, status, after_json = sys.argv[1:6]
line = {"kind": "task", "id": tid, "title": title, "status": status,
        "after": json.loads(after_json), "spawn_failures": 0,
        "created_at": "2026-07-25T00:00:00+00:00"}
with open(path, "a") as f:
    f.write(json.dumps(line) + "\n")
PY
}

# Set a task field (e.g. status) in graph.jsonl by id.
set_field() {
    local id="$1" field="$2" value="$3"
    python3 - "$graph" "$id" "$field" "$value" <<'PY'
import json, sys
path, tid, field, value = sys.argv[1:5]
out = []
with open(path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        obj = json.loads(line)
        if obj.get("kind") == "task" and obj.get("id") == tid:
            obj[field] = value
        out.append(json.dumps(obj))
with open(path, "w") as f:
    for o in out:
        f.write(o + "\n")
PY
}

# Read any field straight from graph.jsonl for a given task id. `wg show
# --json` omits some fields (e.g. spawn_failures) and enriches others (e.g.
# `after` into [{id,status}] objects); graph.jsonl is the source of truth.
raw_field() {
    local id="$1" field="$2"
    python3 - "$graph" "$id" "$field" <<'PY'
import json, sys
path, tid, field = sys.argv[1:4]
with open(path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        obj = json.loads(line)
        if obj.get("kind") == "task" and obj.get("id") == tid:
            val = obj.get(field, "__missing__")
            print(json.dumps(val) if isinstance(val, (list, dict)) else val)
            sys.exit(0)
print("?")
PY
}

# Read the raw `after` edge (as a JSON list of id strings) straight from
# graph.jsonl. The raw stored edge is the source of truth for "the edge was
# preserved, not destructively stripped".
raw_after() {
    raw_field "$1" after
}

ready_has() {
    wg ready 2>&1 | grep -q "$1"
}

# ── Test 1: satellite IS ready when source is failed-pending-eval ────────────
add_task "src-fpe" "src failed-pending-eval" "failed-pending-eval"
add_task ".evaluate-src-fpe" "eval" "open" "src-fpe"
if ! ready_has ".evaluate-src-fpe"; then
    loud_fail "Test 1 FAIL: .evaluate-X must be ready when source is failed-pending-eval. ready: $(wg ready 2>&1)"
fi
echo "PASS (1/5): .evaluate-X ready when source is failed-pending-eval"

# ── Test 2: satellite IS ready when source is failed; after edge preserved ──
add_task "src-failed" "src failed" "failed"
add_task ".evaluate-src-failed" "eval" "open" "src-failed"
if ! ready_has ".evaluate-src-failed"; then
    loud_fail "Test 2 FAIL: .evaluate-X must be ready when source is failed (§4.3). ready: $(wg ready 2>&1)"
fi
after_edge=$(raw_after ".evaluate-src-failed")
if [[ "$after_edge" != '["src-failed"]' ]]; then
    loud_fail "Test 2 FAIL: after edge must be preserved as [\"src-failed\"], got: $after_edge"
fi
echo "PASS (2/5): .evaluate-X ready when source failed; after edge preserved ($after_edge)"

# ── Test 3: ZOMBIE INVARIANT — reopen source re-blocks satellite, no charge ─
# Simulate a bulk-retry that reopens the source (without going through
# `wg reset`, which would also cancel the satellite — tested separately).
# The preserved `after` edge must re-block the satellite.
set_field "src-failed" "status" "open"
if ready_has ".evaluate-src-failed"; then
    loud_fail "Test 3 FAIL: .evaluate-X must NOT be ready against an open source (zombie respawn). ready: $(wg ready 2>&1)"
fi
sf_src=$(raw_field "src-failed" spawn_failures)
sf_sat=$(raw_field ".evaluate-src-failed" spawn_failures)
if [[ "$sf_src" != "0" ]] || [[ "$sf_sat" != "0" ]]; then
    loud_fail "Test 3 FAIL: no spawn_failures should be charged. source=$sf_src satellite=$sf_sat"
fi
after_edge2=$(raw_after ".evaluate-src-failed")
if [[ "$after_edge2" != '["src-failed"]' ]]; then
    loud_fail "Test 3 FAIL: after edge must still be preserved after reopen, got: $after_edge2"
fi
echo "PASS (3/5): reopened source re-blocks satellite; spawn_failures source=$sf_src satellite=$sf_sat; after=$after_edge2"

# ── Test 4: wg reset cancels stale satellites (no --also-strip-meta) ─────────
add_task "src-reset" "src for reset" "failed"
add_task ".evaluate-src-reset" "eval" "open" "src-reset"
add_task ".flip-src-reset" "flip" "open" "src-reset"
add_task ".assign-src-reset" "assign" "open"

wg reset src-reset --direction forward --yes >reset.log 2>&1 \
    || loud_fail "Test 4 FAIL: wg reset failed: $(tail -5 reset.log)"

status_reset=$(raw_field "src-reset" status)
if [[ "$status_reset" != "open" ]]; then
    loud_fail "Test 4 FAIL: expected src-reset open after reset, got: $status_reset"
fi
for sat in ".evaluate-src-reset" ".flip-src-reset" ".assign-src-reset"; do
    if wg show "$sat" >/dev/null 2>&1; then
        loud_fail "Test 4 FAIL: stale $sat must be cancelled by wg reset (still present)"
    fi
done
echo "PASS (4/5): wg reset cancelled stale .evaluate-/.flip-/.assign- without --also-strip-meta"

# ── Test 5: wg recover abandons stale satellite; no respawn vs open source ──
add_task "src-recover" "src for recover" "failed"
add_task ".evaluate-src-recover" "eval" "failed" "src-recover"

wg recover --yes >recover.log 2>&1 \
    || loud_fail "Test 5 FAIL: wg recover failed: $(tail -5 recover.log)"

status_rec=$(raw_field "src-recover" status)
status_sat=$(raw_field ".evaluate-src-recover" status)
if [[ "$status_rec" != "open" ]]; then
    loud_fail "Test 5 FAIL: expected src-recover open after recover, got: $status_rec"
fi
if [[ "$status_sat" != "abandoned" ]]; then
    loud_fail "Test 5 FAIL: expected .evaluate-src-recover abandoned after recover, got: $status_sat"
fi
# The abandoned satellite must NOT respawn against the reopened source.
if ready_has ".evaluate-src-recover"; then
    loud_fail "Test 5 FAIL: abandoned .evaluate-X must NOT respawn against open source. ready: $(wg ready 2>&1)"
fi
echo "PASS (5/5): recover abandons stale satellite; source reopened; no respawn (source=$status_rec satellite=$status_sat)"

echo ""
echo "PASS: evaluation satellite is never scheduled/charged against an open task (resolve-prophage-source scenario)"
exit 0
