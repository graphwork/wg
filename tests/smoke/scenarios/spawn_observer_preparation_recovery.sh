#!/usr/bin/env bash
# Real-daemon regression for verify-spawn-recovery. A tracked escaping symlink
# makes the isolated-worktree observer baseline fail before launch permission.
# The daemon must roll back one allocation without charging the spawn breaker or
# touching ready siblings, then reuse the same uncommitted agent ID and launch
# exactly once after the checkout is repaired.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "candidate binary build requires cargo"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "graph assertions require python3"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)" \
  || loud_fail "cannot locate repository root"
(cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) \
  || loud_fail "candidate wg build failed"
WG_BIN="$REPO_ROOT/target/debug/wg"
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
# The daemon and spawned wrapper must both resolve the unmerged candidate.
export PATH="$(dirname "$WG_BIN"):$PATH"

unset WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
global="$scratch/global"
fakebin="$scratch/fakebin"
sync="$scratch/sync"
mkdir -p "$project" "$home" "$global" "$fakebin" "$sync"
export HOME="$home"
export WG_GLOBAL_DIR="$global"
export XDG_CONFIG_HOME="$home/.config"
export OPENROUTER_API_KEY=fake
export FAKE_SYNC="$sync"

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
printf '%s\t%s\t%s\n' "${WG_TASK_ID:?}" "${WG_AGENT_ID:?}" "$PWD" >>"${FAKE_SYNC:?}/launches"
printf 'launched\n' >"$FAKE_SYNC/launched"
sleep 60
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"

cd "$project"
git init -q
git config user.email spawn-recovery@test.invalid
git config user.name 'Spawn Recovery Smoke'
printf 'base\n' >source.txt
ln -s ../../outside bad-link
git add source.txt bad-link
git commit -qm 'observer baseline failure fixture'

"$WG_BIN" init -m pi:openrouter:test/model --no-agency >init.log 2>&1 \
  || loud_fail "wg init failed: $(tail -40 init.log)"
wg_dir="$project/.wg"
"$WG_BIN" --dir "$wg_dir" config --auto-assign false --auto-evaluate false --no-reload >/dev/null \
  || loud_fail "could not disable agency automation"
"$WG_BIN" --dir "$wg_dir" config set dispatcher.poll_interval 1 >/dev/null \
  || loud_fail "could not set bounded daemon tick"

for spec in \
  'observer-retry|Observer retry target|critical' \
  'ready-sibling-a|Ready sibling A|high' \
  'ready-sibling-b|Ready sibling B|normal'
do
  IFS='|' read -r id title priority <<<"$spec"
  "$WG_BIN" --dir "$wg_dir" add "$title" --id "$id" --priority "$priority" \
    --model pi:openrouter:test/model \
    -d $'Exercise transactional spawn recovery.\n\n## Validation\n- daemon recovery remains exact' >/dev/null \
    || loud_fail "wg add failed for $id"
  "$WG_BIN" --dir "$wg_dir" publish "$id" --only >/dev/null \
    || loud_fail "wg publish failed for $id"
done

cleanup_fixture() {
  "$WG_BIN" --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_fixture

start_wg_daemon "$project" --max-agents 1 --interval 1 --no-coordinator-agent
[[ -n "${WG_SMOKE_DAEMON_PID:-}" ]] || loud_fail "real daemon PID unavailable"

graph="$wg_dir/graph.jsonl"
daemon_log="$wg_dir/service/daemon.log"

# Wait for the first observer-baseline refusal to be durably diagnosed.
seen=false
for _ in $(seq 1 120); do
  if python3 - "$graph" <<'PY' >/dev/null 2>&1
import json,sys
for line in open(sys.argv[1]):
    row=json.loads(line)
    if row.get('id')=='observer-retry':
        assert any(e.get('actor')=='spawn-preparation' for e in row.get('log',[]))
        raise SystemExit(0)
raise SystemExit(1)
PY
  then
    seen=true
    break
  fi
  sleep 0.1
done
$seen || loud_fail "daemon never recorded observer preparation refusal:\n$(tail -80 "$daemon_log" 2>/dev/null || true)"

# Let multiple additional ticks run. The identical persistent cause must remain
# one diagnostic, zero breaker charges, and must stop each tick before siblings.
sleep 2.5
python3 - "$graph" "$wg_dir/service/registry.json" "$sync/first-attempt.json" <<'PY'
import json,os,sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
by={row['id']:row for row in rows}
t=by['observer-retry']
assert t['status']=='open',t
assert t.get('assigned') is None,t
assert t.get('spawn_failures',0)==0,t
assert t.get('last_spawn_failure_at') is None,t
assert t.get('dispatch_count',0)==0,t
logs=t.get('log',[])
assert sum(e.get('actor')=='spawn-preparation' for e in logs)==1,logs
assert not any('Spawn failed (attempt' in e.get('message','') for e in logs),logs
audit=t.get('lifecycle',{}).get('audit',[])
prep=[e for e in audit if e.get('reason_code')=='spawn_preparation_deferred']
assert len(prep)==1,prep
reserved=[e for e in audit if e.get('event_kind')=='attempt-reserved']
cancelled=[e for e in audit if e.get('event_kind')=='reservation-cancelled']
assert reserved and cancelled,(reserved,cancelled)
first=reserved[0]['projection']['current_attempt']
assert first['actor_id']=='agent-1',first
json.dump(first,open(sys.argv[3],'w'))
for sibling in ('ready-sibling-a','ready-sibling-b'):
    row=by[sibling]
    assert row['status']=='open',row
    assert row.get('assigned') is None,row
    assert row.get('spawn_failures',0)==0,row
    assert row.get('dispatch_count',0)==0,row
    assert row.get('lifecycle',{}).get('attempt_sequence',0)==0,row
if os.path.exists(sys.argv[2]):
    registry=json.load(open(sys.argv[2]))
    assert not registry.get('agents'),registry
PY
[[ ! -e "$sync/launches" ]] || loud_fail "handler launched before observer baseline repair"
if [[ -d "$wg_dir/agents" ]] && find "$wg_dir/agents" -mindepth 1 -print -quit | grep -q .; then
  loud_fail "rolled-back spawn left an agent output phantom: $(find "$wg_dir/agents" -mindepth 1 -maxdepth 2 -print)"
fi
if [[ -d "$project/.wg-worktrees" ]] && find "$project/.wg-worktrees" -mindepth 1 -print -quit | grep -q .; then
  loud_fail "rolled-back spawn left a worktree phantom: $(find "$project/.wg-worktrees" -mindepth 1 -maxdepth 2 -print)"
fi
[[ "$(grep -c 'pre-launch preparation rolled back cleanly' "$daemon_log" 2>/dev/null || true)" -eq 1 ]] \
  || loud_fail "preparation diagnostic repeated instead of coalescing:\n$(tail -100 "$daemon_log")"
kill -0 "$WG_SMOKE_DAEMON_PID" 2>/dev/null || loud_fail "daemon died after observer preparation failure"
echo 'PASS (1/2): repeated real-daemon ticks left Open/unassigned, zero phantoms/breaker charges, one diagnostic, siblings untouched'

# Repair the shared checkout. The next bounded daemon tick must create a fresh
# reservation event while reusing the uncommitted agent-1 allocation.
rm bad-link
printf 'safe materialized file\n' >bad-link
git add bad-link
git commit -qm 'repair observer baseline cause'

launched=false
for _ in $(seq 1 160); do
  if [[ -s "$sync/launched" ]]; then
    launched=true
    break
  fi
  sleep 0.1
done
$launched || loud_fail "repaired allocation never launched:\n$(tail -120 "$daemon_log" 2>/dev/null || true)"
# Keep ticking while the one live worker occupies max-agents=1; no duplicate.
sleep 2

python3 - "$graph" "$wg_dir/service/registry.json" "$sync/first-attempt.json" "$sync/launches" <<'PY'
import json,os,sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
by={row['id']:row for row in rows}
t=by['observer-retry']
first=json.load(open(sys.argv[3]))
assert t['status']=='in-progress',t
assert t.get('assigned')=='agent-1',t
assert t.get('spawn_failures',0)==0,t
assert t.get('last_spawn_failure_at') is None,t
audit=t['lifecycle']['audit']
reserved=[e for e in audit if e.get('event_kind')=='attempt-reserved']
assert len(reserved)>=2,reserved
assert len({e['idempotency_key'] for e in reserved})==len(reserved),reserved
current=t['lifecycle']['current_attempt']
assert current['actor_id']=='agent-1',current
assert current['id']!=first['id'],(first,current)
assert sum(e.get('reason_code')=='spawn_preparation_deferred' for e in audit)==1,audit
assert sum(e.get('actor')=='spawn-preparation' for e in t.get('log',[]))==1,t.get('log',[])
assert sum('Spawned by coordinator' in e.get('message','') for e in t.get('log',[]))==1,t.get('log',[])
for sibling in ('ready-sibling-a','ready-sibling-b'):
    row=by[sibling]
    assert row['status']=='open' and row.get('assigned') is None,row
    assert row.get('spawn_failures',0)==0,row
    assert row.get('lifecycle',{}).get('attempt_sequence',0)==0,row
registry=json.load(open(sys.argv[2]))
agents=registry.get('agents',{})
assert list(agents)==['agent-1'],agents
assert agents['agent-1']['task_id']=='observer-retry',agents
launches=[line for line in open(sys.argv[4]).read().splitlines() if line]
assert len(launches)==1,launches
parts=launches[0].split('\t')
assert parts[0:2]==['observer-retry','agent-1'],parts
assert '.wg-worktrees/agent-1' in parts[2],parts
PY
[[ "$(find "$wg_dir/agents" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 1 ]] \
  || loud_fail "expected exactly one committed agent output directory"
[[ "$(find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 1 ]] \
  || loud_fail "expected exactly one committed worktree"
kill -0 "$WG_SMOKE_DAEMON_PID" 2>/dev/null || loud_fail "daemon died during repaired retry"

echo 'PASS (2/2): repaired cause appended a fresh reservation, reused agent-1, and spawned exactly once'
echo 'PASS: observer preparation rollback is exact, breaker-neutral, coalesced, and retry-safe'
