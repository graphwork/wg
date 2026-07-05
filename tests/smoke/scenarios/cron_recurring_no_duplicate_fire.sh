#!/usr/bin/env bash
# Scenario: cron_recurring_no_duplicate_fire
#
# Pins duplicate-prevention / idempotency for recurring cron wakeups
# (impl-recurring-wakeup-reliability; research note §4.1 / §5.2).
#
# CONTRACT under test (src/cron.rs::reset_cron_task +
# src/commands/service/coordinator.rs Phase 2.95):
#
#  1. CATCH-UP IS ONE-SHOT: a Done cron task whose `next_cron_fire` is in
#     the PAST (a fire that already ran, or a missed window the daemon is
#     catching up on) is reset to Open EXACTLY ONCE by a coordinator tick.
#     `reset_cron_task` only acts on `status == Done`, so once it reopens
#     the task a subsequent tick does NOT reset it again.
#
#  2. NO DUPLICATE FIRE FOR THE SAME WINDOW: after the reset,
#     `next_cron_fire` is advanced to the NEXT schedule slot (with jitter)
#     — i.e. the FUTURE — so the task is NOT due again immediately. A
#     single missed/due window therefore produces exactly ONE catch-up
#     dispatch, not a tight loop of re-fires. The task must NOT appear in
#     `wg ready` until the next real window arrives.
#
#  3. IDEMPOTENT ACROSS TICKS: a second tick leaves the task Open with the
#     same advanced `next_cron_fire` (no re-reset, no churn). This is the
#     "recurring/lagged loops avoid duplicate execution while still
#     catching up" guarantee.
#
# No LLM credentials. No worker spawn (the task is never due after reset,
# and `wg service tick --max-agents 0` short-circuits spawning). Pure
# graph-state + single coordinator ticks via `wg service tick`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 required to rewrite graph.jsonl"
fi

scratch=$(make_scratch)
cd "$scratch"

# Isolate HOME + XDG so the host global ~/.wg/config.toml cannot leak a
# stale executor/model or poll_interval into the merged config.
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"
export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# Weekly Monday 09:00 UTC cron. dow=2 is Monday in the `cron` crate
# (0.12.x), which uses a NON-STANDARD 1=Sunday mapping — see
# src/cron.rs::cron_dow_mapping_is_nonstandard_one_indexed_sunday.
cron_expr="0 0 9 * * 2"
add_out=$(wg add "weekly-no-dup" --cron "$cron_expr" --no-place 2>&1) || \
    loud_fail "wg add --cron failed: $add_out"
task_id=$(echo "$add_out" | grep "^Added task:" | grep -oP '\(\K[^)]+')
if [[ -z "$task_id" ]]; then
    loud_fail "could not parse task id from: $add_out"
fi

wg_dir="$scratch/.wg"
graph="$wg_dir/graph.jsonl"

# Helper: rewrite fields on $task_id in graph.jsonl.
set_field() {
    local field="$1" val="$2"
    python3 - "$graph" "$task_id" "$field" "$val" <<'PY'
import json, sys, os
path, tid, field, val = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
tmp = path + ".tmp.%d" % os.getpid()
with open(path) as f:
    lines = f.readlines()
out = []
for line in lines:
    line = line.rstrip("\n")
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == tid:
        if val == "__DELETE__":
            obj.pop(field, None)
        elif val == "true":
            obj[field] = True
        elif val == "false":
            obj[field] = False
        else:
            obj[field] = val
    out.append(json.dumps(obj, separators=(",", ":")))
with open(tmp, "w") as f:
    for o in out:
        f.write(o + "\n")
os.replace(tmp, path)
PY
}

# Helper: read a single field from $task_id (or "<none>").
get_field() {
    local field="$1"
    python3 - "$graph" "$task_id" "$field" <<'PY'
import json, sys
path, tid, field = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        obj = json.loads(line)
        if obj.get("kind") == "task" and obj.get("id") == tid:
            v = obj.get(field)
            print(v if v is not None else "<none>")
            break
PY
}

# Helper: is the task currently listed in `wg ready`?
is_ready() {
    wg --dir "$wg_dir" ready 2>&1 | grep -q "$task_id"
}

now_ts() {
    python3 -c "import datetime; print(datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'))"
}

# ── Setup: simulate a cron fire that already RAN and is awaiting reset. ──
# A weekly Monday cron fired (next_cron_fire was last Monday 09:00), the
# worker completed it (status=done), and the daemon is now about to tick
# to reset it for the next window. We plant a PAST next_cron_fire so the
# fire is unambiguously "due/just-fired".
past_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=2)).strftime('%Y-%m-%dT%H:%M:%SZ'))")
set_field "status" "done"
set_field "completed_at" "$(now_ts)"
set_field "next_cron_fire" "$past_ts"
set_field "last_cron_fire" "$past_ts"

# ── Test 1: a single tick resets the Done cron task exactly once. ────────
# NOTE: we deliberately do NOT pass --max-agents 0 here. With max-agents 0
# the coordinator early-returns at Phase 1 (alive_count >= max_agents)
# BEFORE the maintenance phases (2.5–2.95) run, so Phase 2.95 (cron reset)
# would never execute. --max-agents 1 lets the maintenance phases run while
# still never spawning a worker: the Done cron task is not ready, and after
# reset it is Open with a FUTURE next_cron_fire (not due) — so nothing is
# ever ready and no credentials are needed.
wg --dir "$wg_dir" service tick --max-agents 1 >tick1.log 2>&1 || \
    loud_fail "first service tick failed: $(tail -10 tick1.log)"

status_after1=$(get_field "status")
next_after1=$(get_field "next_cron_fire")
if [[ "$status_after1" != "open" ]]; then
    loud_fail "after tick 1 the Done cron task should be reset to Open, got status=$status_after1 (next_cron_fire=$next_after1). tick log:\n$(tail -15 tick1.log)"
fi
if [[ "$next_after1" == "<none>" || "$next_after1" == "" ]]; then
    loud_fail "after tick 1 next_cron_fire should be advanced (set), got <none>. tick log:\n$(tail -15 tick1.log)"
fi
# next_cron_fire must now be in the FUTURE (advanced past `now` to the next
# Monday window + jitter). If it were still in the past, the same window
# would re-fire on the next tick — the duplicate-fire bug.
now=$(date -u +%s)
next_epoch=$(python3 -c "import datetime,sys; print(int(datetime.datetime.fromisoformat(sys.argv[1].replace('Z','+00:00')).timestamp()))" "$next_after1" 2>/dev/null || echo 0)
if [[ "$next_epoch" -le "$now" ]]; then
    loud_fail "after tick 1 next_cron_fire ($next_after1) must be in the FUTURE (advanced past now) so the same window is not re-fired — duplicate-prevention broken. now=$now next_epoch=$next_epoch"
fi
echo "PASS (1/3): Done cron task with past next_cron_fire reset to Open once; next_cron_fire advanced to future ($next_after1)"

# ── Test 2: the reset task is NOT due → NOT in `wg ready` (no dup fire). ──
if is_ready; then
    loud_fail "after reset the cron task should NOT be ready (next_cron_fire is future), but it appeared in wg ready — same window would re-fire (duplicate dispatch). next_cron_fire=$next_after1"
fi
echo "PASS (2/3): reset cron task is NOT ready (next_cron_fire future) — no duplicate dispatch for the same window"

# ── Test 3: a second tick is idempotent — no re-reset, no churn. ──────────
wg --dir "$wg_dir" service tick --max-agents 1 >tick2.log 2>&1 || \
    loud_fail "second service tick failed: $(tail -10 tick2.log)"

status_after2=$(get_field "status")
next_after2=$(get_field "next_cron_fire")
if [[ "$status_after2" != "open" ]]; then
    loud_fail "after tick 2 the task should still be Open (idempotent — reset_cron_task only acts on Done), got status=$status_after2. tick log:\n$(tail -15 tick2.log)"
fi
if [[ "$next_after2" != "$next_after1" ]]; then
    loud_fail "after tick 2 next_cron_fire should be UNCHANGED (idempotent reset), got next_after1=$next_after1 next_after2=$next_after2 — the cron task was re-reset/duplicated across ticks"
fi
# Still not ready (still future).
if is_ready; then
    loud_fail "after tick 2 the cron task should still NOT be ready, but it appeared in wg ready — duplicate fire surfaced on second tick"
fi
echo "PASS (3/3): second tick is idempotent — task stays Open, next_cron_fire unchanged ($next_after2), still not ready"

echo "PASS: recurring cron wakeup is duplicate-prevented / idempotent (one catch-up fire per window, no re-fire across ticks)"
exit 0
