#!/usr/bin/env bash
# Scenario: cron_diagnostics_surface_missed_and_paused_state
#
# Pins the recurring-wakeup DIAGNOSTICS surface (impl-recurring-heartbeat-
# diagnostics) — `wg cron`, `wg list`, `wg show`, and `wg status` must expose
# enough scheduling state to debug a weekly recurring workflow that did not
# wake up on time.
#
# CONTRACT under test:
#
#  1. `wg cron` (a.k.a. `wg cron doctor`) lists every cron-enabled task with:
#     schedule, resolved weekday + UTC time-of-day (naming the cron crate's
#     non-standard 1=Sunday mapping so a Monday-intent `1` shows as Sunday),
#     next-fire, last-fire, due/overdue/paused state, and the missed-fire
#     count across daemon downtime.
#  2. `wg list` renders the resolved weekday/time inside the `[cron: …]` tag
#     (so a user spots the wrong-day bug at list time, not after a missed
#     week) — and does NOT duplicate-spam the note per row.
#  3. `wg show <cron-task>` prints a "Recurring (cron):" block with schedule,
#     resolved summary, next/last fire, state (DUE / OVERDUE / paused), and
#     the missed-fire count when the task has fallen behind.
#  4. `wg status` prints a compact "Recurring (cron):" summary (count, due /
#     overdue / paused, soonest next-fire) — exactly one section, no spam.
#  5. `wg heartbeat <non-agent-id>` REJECTS the external-trigger attempt with
#     a diagnostic pointing at the safe path (`wg heartbeat agent-N`, `wg cron`)
#     so a host cron does not silently fight the dispatcher.
#
# No LLM credentials. No worker spawn (max-agents=0 / no daemon needed for
# the diagnostics surfaces — they read graph state directly).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 required to rewrite cron fire fields in graph.jsonl"
fi

scratch=$(make_scratch)
cd "$scratch"

# Isolate HOME + XDG so the host global ~/.wg/config.toml cannot leak a stale
# executor/model or poll_interval into the merged config.
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"
export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# dow=1 is SUNDAY in the cron crate (non-standard mapping). A user intending
# "Monday" who writes `1` should see "Sun" surfaced everywhere — this is the
# headline wrong-day finding from repro-weekly-wakeup-heartbeat.
cron_expr="0 0 9 * * 1"
add_out=$(wg add "weekly-monday-intent" --cron "$cron_expr" --no-place 2>&1) || \
    loud_fail "wg add --cron failed: $add_out"
task_id=$(echo "$add_out" | grep "^Added task:" | grep -oP '\(\K[^)]+')
if [[ -z "$task_id" ]]; then
    loud_fail "could not parse task id from: $add_out"
fi

wg_dir="$scratch/.wg"
graph="$wg_dir/graph.jsonl"

# Helper: rewrite cron fields on $task_id in graph.jsonl.
set_cron_field() {
    local field="$1"
    local ts="$2"
    python3 - "$graph" "$task_id" "$field" "$ts" <<'PY'
import json, sys, os
path, tid, field, ts = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
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
        obj[field] = ts
    out.append(json.dumps(obj, separators=(",", ":")))
with open(tmp, "w") as f:
    for o in out:
        f.write(o + "\n")
os.replace(tmp, path)
PY
}

now_ts=$(python3 -c "import datetime; print(datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'))")
# 20 days ago — a weekly (Sunday) cron would have >=2 missed windows behind now.
stale_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=20)).strftime('%Y-%m-%dT%H:%M:%SZ'))")
# 1 hour ago — overdue (due but not yet dispatched past its fire time).
past_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=1)).strftime('%Y-%m-%dT%H:%M:%SZ'))")

# ── Test 1: `wg list` surfaces the resolved weekday (Sun, not Mon) ────────
list_out=$(wg list 2>&1)
if ! echo "$list_out" | grep -q "$task_id"; then
    loud_fail "wg list did not show the cron task:\n$list_out"
fi
if ! echo "$list_out" | grep -qi "Sun"; then
    loud_fail "wg list cron tag does not name the resolved weekday (Sun) — the wrong-day bug is invisible at list time:\n$list_out"
fi
# The non-standard mapping note must NOT be duplicated per row (no spam).
# Count occurrences of "cron dow" in the list output — should be at most 1
# (the summary carries the note; we do not repeat it per task here).
note_count=$(echo "$list_out" | grep -c "cron dow" || true)
if [[ "$note_count" -gt 1 ]]; then
    loud_fail "wg list spams the dow-mapping note per row ($note_count times) — should be at most 1:\n$list_out"
fi
echo "PASS (1/5): wg list names the resolved weekday (Sun) in the [cron: …] tag, no per-row spam"

# ── Test 2: `wg show` prints the Recurring (cron) diagnostics block ───────
# Seed a stale last_cron_fire (5 days ago) + a past next_cron_fire (1h ago)
# so the task is DUE + OVERDUE and has missed windows.
set_cron_field "last_cron_fire" "$stale_ts"
set_cron_field "next_cron_fire" "$past_ts"

show_out=$(wg show "$task_id" 2>&1)
if ! echo "$show_out" | grep -q "Recurring (cron):"; then
    loud_fail "wg show did not print a 'Recurring (cron):' diagnostics block:\n$show_out"
fi
if ! echo "$show_out" | grep -qi "Schedule:"; then
    loud_fail "wg show cron block missing Schedule line:\n$show_out"
fi
if ! echo "$show_out" | grep -qi "Resolved:"; then
    loud_fail "wg show cron block missing Resolved (weekday/time) line:\n$show_out"
fi
if ! echo "$show_out" | grep -qi "Sun"; then
    loud_fail "wg show cron block does not name Sunday (resolved weekday) — wrong-day bug invisible:\n$show_out"
fi
if ! echo "$show_out" | grep -qiE "OVERDUE|DUE"; then
    loud_fail "wg show cron block does not surface DUE/OVERDUE state:\n$show_out"
fi
if ! echo "$show_out" | grep -qi "Missed fires"; then
    loud_fail "wg show cron block does not surface the missed-fire count:\n$show_out"
fi
echo "PASS (2/5): wg show prints the Recurring (cron) diagnostics block (schedule, resolved weekday, DUE/OVERDUE, missed fires)"

# ── Test 3: `wg cron` lists the task with state + missed-fire count ───────
cron_out=$(wg cron 2>&1)
if ! echo "$cron_out" | grep -q "$task_id"; then
    loud_fail "wg cron did not list the cron task:\n$cron_out"
fi
if ! echo "$cron_out" | grep -qi "Sun"; then
    loud_fail "wg cron does not name the resolved weekday (Sun):\n$cron_out"
fi
# Either OVERDUE or DUE tag must be present (the task's next_cron_fire is past).
if ! echo "$cron_out" | grep -qiE "OVERDUE|DUE"; then
    loud_fail "wg cron does not tag the due/overdue task:\n$cron_out"
fi
if ! echo "$cron_out" | grep -qi "missed"; then
    loud_fail "wg cron does not surface the missed-fire count:\n$cron_out"
fi
# The non-standard dow mapping note appears exactly once (grouped, not per-row).
cron_note_count=$(echo "$cron_out" | grep -c "1=Sunday" || true)
if [[ "$cron_note_count" -lt 1 ]]; then
    loud_fail "wg cron does not surface the non-standard dow mapping note:\n$cron_out"
fi
echo "PASS (3/5): wg cron lists the task with resolved weekday, DUE/OVERDUE state, and missed-fire count"

# ── Test 4: `wg cron --json` emits parseable JSON with the diagnostics ───
cron_json=$(wg cron --json 2>&1)
if ! echo "$cron_json" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert isinstance(d, list) and len(d)>=1' 2>/dev/null; then
    loud_fail "wg cron --json did not emit a non-empty JSON array:\n$cron_json"
fi
echo "$cron_json" | python3 -c '
import json, sys
d = json.load(sys.stdin)
row = next(r for r in d if r["id"] == sys.argv[1])
assert "summary" in row and "Sun" in row["summary"], "summary missing Sun: " + str(row.get("summary"))
assert row.get("due") is True, "due must be true for past next_cron_fire"
assert row.get("overdue_secs") is not None and row["overdue_secs"] > 0, "overdue_secs must be >0"
assert row.get("missed_fires") is not None and row["missed_fires"] >= 1, "missed_fires must be >=1"
' "$task_id" || loud_fail "wg cron --json row missing diagnostics fields (summary/due/overdue_secs/missed_fires):\n$cron_json"
echo "PASS (4/5): wg cron --json emits diagnostics fields (summary, due, overdue_secs, missed_fires)"

# ── Test 5: `wg status` prints a compact cron summary; heartbeat rejects ──
status_out=$(wg status 2>&1)
if ! echo "$status_out" | grep -q "Recurring (cron):"; then
    loud_fail "wg status did not print a 'Recurring (cron):' summary:\n$status_out"
fi
# Exactly ONE "Recurring (cron):" section (no duplicate spam).
status_cron_sections=$(echo "$status_out" | grep -c "Recurring (cron):" || true)
if [[ "$status_cron_sections" -ne 1 ]]; then
    loud_fail "wg status printed the cron summary $status_cron_sections times (expected 1):\n$status_out"
fi
if ! echo "$status_out" | grep -qiE "overdue"; then
    loud_fail "wg status cron summary does not surface the overdue task:\n$status_out"
fi

# External heartbeat rejection: a non-agent ID must be rejected with a
# diagnostic pointing at the safe path. (The external-trigger interop
# contract — received ≠ consumed; a host cron must NOT poke the graph.)
hb_out=$(wg heartbeat "weekly-monday-intent" 2>&1 || true)
if echo "$hb_out" | grep -qi "heartbeat recorded"; then
    loud_fail "wg heartbeat accepted a non-agent (task) ID — external-trigger interop broken:\n$hb_out"
fi
if ! echo "$hb_out" | grep -qi "agent"; then
    loud_fail "wg heartbeat rejection does not point at the agent-ID safe path:\n$hb_out"
fi
if ! echo "$hb_out" | grep -qi "cron"; then
    loud_fail "wg heartbeat rejection does not mention `wg cron` for recurring-task diagnostics:\n$hb_out"
fi
echo "PASS (5/5): wg status prints one compact cron summary; wg heartbeat rejects non-agent IDs with a safe-path diagnostic"

echo "PASS: recurring-wakeup diagnostics surface missed/paused/overdue state across wg cron/list/show/status + heartbeat interop"
exit 0
