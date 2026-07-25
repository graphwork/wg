#!/usr/bin/env bash
# Scenario: spawn_circuit_breaker_self_heal
#
# Pins fix-spawn-failures: the per-task `spawn_failures` circuit breaker must
# NOT permanently brick a task. Before the fix, once a task hit the threshold
# (default 5) the dispatcher skipped it forever and NO `wg` CLI cleared the
# counter — the only remedy was surgical graph.jsonl surgery. This scenario
# proves the four fixes against a REAL `wg` binary:
#
#   1. VISIBILITY — a tripped breaker is surfaced in `wg show` AND `wg status`
#      (not just a silent "spawned=0" in the daemon log).
#   2. PER-TASK ISOLATION — a tripped breaker on one task never blocks another.
#   3. `wg retry` CLEARS IT — dispatch resumes with NO graph.jsonl edit.
#   4. SELF-HEAL — the breaker decays after a cooldown so a transient burst
#      (e.g. a registry/key outage) does not permanently brick a task.
#
# The live dispatch-resumption proof uses shell tasks (`exec = "true"`) which
# the dispatcher runs inline — no LLM endpoint or credential is required.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# The smoke harness may be launched from a worker. Pin every `wg` call to the
# scratch fixture by stripping the parent worker's WG_* context.
unset WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER

scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project"

# Resolve the wg binary explicitly (prefer a freshly-built target/debug so a
# developer's global install lag does not mask the fix under test).
WG_BIN="$(command -v wg)"
if [[ -x "$HERE/../../../target/debug/wg" ]]; then
    WG_BIN="$HERE/../../../target/debug/wg"
fi

# Edit one task in graph.jsonl: set spawn_failures + last_spawn_failure_at +
# status. This simulates the tripped state `record_spawn_failure` produces —
# exactly the incident state that previously required manual graph surgery.
seed_breaker() {
    local graph="$1" task="$2" failures="$3" ts="$4" status="$5"
    python3 - "$graph" "$task" "$failures" "$ts" "$status" <<'PY'
import json, sys
graph, task, failures, ts, status = sys.argv[1:6]
out = []
for ln in open(graph).read().splitlines():
    if not ln.strip():
        continue
    o = json.loads(ln)
    if o.get("id") == task:
        o["spawn_failures"] = int(failures)
        o["last_spawn_failure_at"] = ts
        o["status"] = status
        o["paused"] = False
    out.append(json.dumps(o))
open(graph, "w").write("\n".join(out) + "\n")
PY
}

read_field() {
    # read_field <graph> <task> <field>  → prints the JSON value (string minus quotes)
    python3 - "$1" "$2" "$3" <<'PY'
import json, sys
graph, task, field = sys.argv[1:4]
for ln in open(graph).read().splitlines():
    if not ln.strip():
        continue
    o = json.loads(ln)
    if o.get("id") == task:
        v = o.get(field)
        print("" if v is None else v)
        break
PY
}

cd "$project"
export HOME="$scratch/home"
mkdir -p "$HOME"

# Lower threshold for a fast, deterministic scenario. Short cooldown (2s) so
# the decay self-heal is observable without slowing the gate. `wg init`
# writes explicit defaults, so REPLACE rather than append (avoids a duplicate
# TOML key that would silently fall back to defaults).
if ! "$WG_BIN" init >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi
wg_dir="$project/.wg"
graph="$wg_dir/graph.jsonl"
python3 - "$wg_dir/config.toml" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
s = re.sub(r'^max_spawn_failures\s*=.*$', 'max_spawn_failures = 3', s, count=1, flags=re.M)
if re.search(r'^spawn_failure_cooldown\s*=', s, flags=re.M):
    s = re.sub(r'^spawn_failure_cooldown\s*=.*$', 'spawn_failure_cooldown = "2s"', s, count=1, flags=re.M)
else:
    s = s.replace('[dispatcher]\n', '[dispatcher]\nspawn_failure_cooldown = "2s"\n', 1)
open(p, 'w').write(s)
PY

# Two tasks: one we brick, one that stays healthy (per-task isolation proof).
"$WG_BIN" add "bricked by transient burst" --id bricked >add.log 2>&1 || loud_fail "add bricked failed"
"$WG_BIN" add "healthy sibling" --id healthy >add2.log 2>&1 || loud_fail "add healthy failed"
"$WG_BIN" unpause bricked >/dev/null 2>&1 || true
"$WG_BIN" unpause healthy >/dev/null 2>&1 || true

NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
OLD="$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ)"
seed_breaker "$graph" "bricked" 3 "$NOW" "Incomplete"

# ── (1) VISIBILITY ─────────────────────────────────────────────────────────
show_out="$("$WG_BIN" show bricked 2>&1)"
if ! grep -q "Spawn circuit breaker TRIPPED" <<<"$show_out"; then
    loud_fail "(1) wg show did not surface the tripped breaker:\n$show_out"
fi
if ! grep -q "wg retry bricked" <<<"$show_out"; then
    loud_fail "(1) wg show did not name the recovery action:\n$show_out"
fi
status_out="$("$WG_BIN" status 2>&1)"
if ! grep -q "SPAWN BREAKER" <<<"$status_out"; then
    loud_fail "(1) wg status did not surface the tripped breaker:\n$status_out"
fi
if ! grep -q "bricked" <<<"$status_out"; then
    loud_fail "(1) wg status breaker line missing the task id:\n$status_out"
fi
# JSON surface must carry the counter.
sf_json="$("$WG_BIN" show bricked --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('spawn_failures'))")"
[[ "$sf_json" == "3" ]] || loud_fail "(1) wg show --json spawn_failures=$sf_json, expected 3"
echo "PASS (1/4): tripped breaker visible in wg show + wg status (json + human)"

# ── (2) PER-TASK ISOLATION ─────────────────────────────────────────────────
# The healthy task must NOT be reported as tripped; only the bricked one.
healthy_breakers="$(grep -c "SPAWN BREAKER" <<<"$status_out")"
[[ "$healthy_breakers" -eq 1 ]] || loud_fail "(2) expected exactly 1 tripped breaker in status, got $healthy_breakers:\n$status_out"
if grep -q "healthy" <<<"$(grep 'SPAWN BREAKER' <<<"$status_out")"; then
    loud_fail "(2) healthy task wrongly listed as tripped:\n$status_out"
fi
show_healthy="$("$WG_BIN" show healthy 2>&1)"
if grep -q "Spawn circuit breaker" <<<"$show_healthy"; then
    loud_fail "(2) healthy task shows a breaker:\n$show_healthy"
fi
echo "PASS (2/4): breaker is per-task — healthy sibling unaffected"

# ── (3) `wg retry` CLEARS IT (NO GRAPH EDIT) ───────────────────────────────
retry_out="$("$WG_BIN" retry bricked 2>&1)"
if ! grep -q "Spawn circuit breaker cleared" <<<"$retry_out"; then
    loud_fail "(3) wg retry did not report the breaker cleared:\n$retry_out"
fi
after_sf="$(read_field "$graph" "bricked" "spawn_failures")"
after_lsf="$(read_field "$graph" "bricked" "last_spawn_failure_at")"
after_status="$(read_field "$graph" "bricked" "status")"
# spawn_failures serializes with skip_if_zero, so an absent field == 0.
[[ "$after_sf" == "0" || -z "$after_sf" ]] || loud_fail "(3) spawn_failures=$after_sf after retry, expected 0 (no graph edit should be needed)"
[[ -z "$after_lsf" ]] || loud_fail "(3) last_spawn_failure_at=$after_lsf after retry, expected cleared"
[[ "$after_status" == "open" ]] || loud_fail "(3) status=$after_status after retry, expected open"
# And the breaker is no longer surfaced.
status_after="$("$WG_BIN" status 2>&1)"
if grep -q "SPAWN BREAKER" <<<"$status_after"; then
    loud_fail "(3) breaker still surfaced after retry:\n$status_after"
fi
echo "PASS (3/4): wg retry cleared spawn_failures→0 + status→open, no graph.jsonl edit"

# ── (4) SELF-HEAL via cooldown decay + LIVE dispatch resumption ────────────
# Re-trip with an OLD timestamp: the breaker is past the cooldown, so it has
# DECAYED. `wg status` must NOT list it as tripped (the next dispatcher tick
# resets it). This is the same gather logic the dispatcher's decay branch uses.
seed_breaker "$graph" "bricked" 3 "$OLD" "Incomplete"
status_decayed="$("$WG_BIN" status 2>&1)"
if grep -q "SPAWN BREAKER" <<<"$status_decayed"; then
    loud_fail "(4) a decayed breaker (past cooldown) was still surfaced as tripped:\n$status_decayed"
fi
echo "PASS (4a/4): decayed breaker self-heals — not surfaced (cooldown elapsed)"

# LIVE dispatch-resumption proof: shell tasks run inline (no credential). The
# breaker check runs BEFORE the shell spawn branch, so a tripped shell task is
# SKIPPED while a healthy one spawns. After `wg retry`, it dispatches.
"$WG_BIN" add "shell bricked" --id shell-bricked --exec "true" >add3.log 2>&1 \
    || loud_fail "(4) add shell-bricked failed: $(tail -5 add3.log)"
"$WG_BIN" add "shell healthy" --id shell-healthy --exec "true" >add4.log 2>&1 \
    || loud_fail "(4) add shell-healthy failed: $(tail -5 add4.log)"
# The `--exec` flag may not set exec_mode=shell by itself on every version; set
# it explicitly so the dispatcher takes the inline shell path.
python3 - "$graph" <<'PY'
import json, sys
graph = sys.argv[1]
out = []
for ln in open(graph).read().splitlines():
    if not ln.strip():
        continue
    o = json.loads(ln)
    if o.get("id") in ("shell-bricked", "shell-healthy"):
        o["exec_mode"] = "shell"
        o["paused"] = False
    out.append(json.dumps(o))
open(graph, "w").write("\n".join(out) + "\n")
PY
seed_breaker "$graph" "shell-bricked" 3 "$NOW" "Incomplete"

# Start the daemon and let it tick. Expect: shell-healthy spawns, shell-bricked
# is skipped by the breaker (per-task isolation, LIVE). The daemon requires a
# selected route to start; a dummy Pi route is enough — shell tasks run inline
# and never invoke the LLM.
"$WG_BIN" config --local --model pi:openrouter:test/fake >/dev/null 2>&1 \
    || loud_fail "(4) could not set a dummy route for the daemon"
if ! start_wg_daemon "$project" --max-agents 2 --no-coordinator-agent --interval 1; then
    loud_fail "(4) daemon did not start for the dispatch-resumption proof"
fi
daemon_log="$WG_SMOKE_DAEMON_DIR/service/daemon.log"
skipped=false
for _ in $(seq 1 40); do
    if grep -q "Skipping 'shell-bricked'.*spawn circuit breaker" "$daemon_log" 2>/dev/null; then
        skipped=true
        break
    fi
    sleep 0.25
done
$skipped || loud_fail "(4) daemon never skipped the tripped shell-bricked task (breaker did not block):\n$(tail -40 "$daemon_log" 2>/dev/null || true)"

healthy_spawned=false
for _ in $(seq 1 40); do
    if grep -q "Spawning shell task inline for: shell-healthy" "$daemon_log" 2>/dev/null; then
        healthy_spawned=true
        break
    fi
    sleep 0.25
done
$healthy_spawned || loud_fail "(4) breaker blocked the healthy sibling too — NOT per-task:\n$(tail -40 "$daemon_log" 2>/dev/null || true)"

# Now retry the bricked shell task and prove dispatch RESUMES (no graph edit).
"$WG_BIN" retry shell-bricked >retry2.log 2>&1 \
    || loud_fail "(4) wg retry shell-bricked failed: $(cat retry2.log)"
bricked_spawned=false
for _ in $(seq 1 40); do
    if grep -q "Spawning shell task inline for: shell-bricked" "$daemon_log" 2>/dev/null; then
        bricked_spawned=true
        break
    fi
    sleep 0.25
done
$bricked_spawned || loud_fail "(4) after retry, the daemon did NOT dispatch shell-bricked (dispatch did not resume):\n$(tail -50 "$daemon_log" 2>/dev/null || true)"

echo "PASS (4b/4): tripped task was skipped while healthy dispatched; after wg retry dispatch resumed (no graph edit)"

echo "PASS: spawn circuit breaker is visible, per-task, retry-clearable, and self-healing"
exit 0
