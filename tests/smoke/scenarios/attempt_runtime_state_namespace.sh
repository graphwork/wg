#!/usr/bin/env bash
# Regression: attempt IDs are task-local. Exercise two live attempt-0-1 Pi
# fixtures plus an actual shell spawn in the presence of 27 historical flat
# directories. Historical bytes must remain immutable and no stale state may be
# projected for the newly spawned task.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "candidate binary build requires cargo"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "tuple assertions require python3"
command -v sha256sum >/dev/null 2>&1 || loud_skip "MISSING SHA256SUM" "historical byte identity requires sha256sum"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)" || loud_fail "cannot locate repository root"
(cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate wg build failed"
WG_BIN="$REPO_ROOT/target/debug/wg"

unset WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
global="$scratch/global"
mkdir -p "$project" "$home" "$global"
export HOME="$home" WG_GLOBAL_DIR="$global" XDG_CONFIG_HOME="$home/.config"

cd "$project"
git init -q
git config user.email attempt-runtime@test.invalid
git config user.name 'Attempt Runtime Smoke'
printf 'base\n' >base.txt
git add base.txt
git commit -qm base
"$WG_BIN" init --executor shell --no-agency >/dev/null
wg_dir="$project/.wg"

# Two tasks concurrently own the same local attempt ID. Fixture-init writes real
# Pi watchdog/session evidence and must choose two distinct tuple namespaces.
for id in old-task peer-task; do
  "$WG_BIN" --dir "$wg_dir" add "$id" --id "$id" --exec 'sleep 30' >/dev/null
  "$WG_BIN" --dir "$wg_dir" publish "$id" --only >/dev/null
  "$WG_BIN" --dir "$wg_dir" claim "$id" --actor "actor-$id" >/dev/null
  "$WG_BIN" --dir "$wg_dir" pi-watchdog fixture-init "$id" --worktree "$project" --now 100 >/dev/null
done

python3 - "$wg_dir" "$WG_BIN" <<'PY'
import glob,json,os,subprocess,sys
wg,bin=sys.argv[1:]
roots=glob.glob(os.path.join(wg,'attempts','by-source-tuple','*'))
found={}
for root in roots:
    p=os.path.join(root,'source-tuple.json')
    if not os.path.exists(p): continue
    key=json.load(open(p))
    if key['task_id'] in ('old-task','peer-task'):
        assert key['attempt_id']=='attempt-0-1',key
        state=json.load(open(os.path.join(root,'pi','state.json')))['state']
        assert state['source']['task_id']==key['task_id'],state
        found[key['task_id']]=root
assert set(found)=={'old-task','peer-task'},found
assert found['old-task']!=found['peer-task'],found
for task in found:
    shown=json.loads(subprocess.check_output([bin,'--dir',wg,'show',task,'--json']))
    assert shown['pi_watchdog']['source']['task_id']==task,shown.get('pi_watchdog')
open(os.path.join(wg,'old-root'),'w').write(found['old-task'])
PY

# Materialize a read-only historical flat layout. Copying real old-task Pi
# state gives attempt-0-1 valid foreign evidence; 2..27 reproduce the incident
# shape. Capture a byte-level digest before the new task is prepared.
old_root="$(cat "$wg_dir/old-root")"
mkdir -p "$wg_dir/attempts/attempt-0-1/pi"
cp "$old_root/pi/state.json" "$wg_dir/attempts/attempt-0-1/pi/state.json"
for n in $(seq 2 27); do
  mkdir -p "$wg_dir/attempts/attempt-0-$n/pi"
  cp "$old_root/pi/state.json" "$wg_dir/attempts/attempt-0-$n/pi/state.json"
done
find "$wg_dir/attempts" -path '*/attempt-0-*/pi/state.json' -type f -print0 \
  | sort -z | xargs -0 sha256sum >"$scratch/before.sha"

# Simulate an upgrade where old-task exists only in the historical flat slot.
# The first mutating watchdog open lazily indexes a copy, then mutates only the
# canonical copy. Flat evidence must remain byte-identical.
rm -rf "$old_root"
"$WG_BIN" --dir "$wg_dir" pi-watchdog status old-task >/dev/null
find "$wg_dir/attempts" -path '*/attempt-0-*/pi/state.json' -type f -print0 \
  | sort -z | xargs -0 sha256sum >"$scratch/indexed.sha"
cmp "$scratch/before.sha" "$scratch/indexed.sha" \
  || loud_fail "lazy watchdog indexing mutated historical flat evidence"

"$WG_BIN" --dir "$wg_dir" add 'fresh task' --id fresh-task \
  --exec "printf launched > '$scratch/launched'" >/dev/null
"$WG_BIN" --dir "$wg_dir" publish fresh-task --only >/dev/null
spawn_out="$scratch/spawn.out"
"$WG_BIN" --dir "$wg_dir" spawn fresh-task --executor shell >"$spawn_out" 2>&1 \
  || loud_fail "fresh task spawn failed: $(cat "$spawn_out")"
for _ in $(seq 1 100); do [[ -s "$scratch/launched" ]] && break; sleep 0.05; done
[[ -s "$scratch/launched" ]] || loud_fail "fresh handler did not launch"

find "$wg_dir/attempts" -path '*/attempt-0-*/pi/state.json' -type f -print0 \
  | sort -z | xargs -0 sha256sum >"$scratch/after.sha"
cmp "$scratch/before.sha" "$scratch/after.sha" \
  || loud_fail "historical flat attempt evidence changed"

python3 - "$wg_dir" "$WG_BIN" <<'PY'
import glob,json,os,subprocess,sys
wg,bin=sys.argv[1:]
rows=[json.loads(line) for line in open(os.path.join(wg,'graph.jsonl')) if line.strip()]
t=next(row for row in rows if row.get('id')=='fresh-task')
lc=t['lifecycle']
assert lc['attempt_sequence']==1,lc
assert lc['current_attempt']['id']=='attempt-0-1',lc
reserved=[e for e in lc['audit'] if e.get('event_kind')=='attempt-reserved']
assert len(reserved)==1,reserved
shown=json.loads(subprocess.check_output([bin,'--dir',wg,'show','fresh-task','--json']))
assert shown.get('pi_watchdog') is None,shown.get('pi_watchdog')
assert not shown.get('worktree_observer') or shown['worktree_observer']['source']['task_id']=='fresh-task',shown.get('worktree_observer')
roots=[]
for p in glob.glob(os.path.join(wg,'attempts','by-source-tuple','*','source-tuple.json')):
    key=json.load(open(p))
    if key['task_id']=='fresh-task': roots.append((p,key))
assert len(roots)==1,roots
assert roots[0][1]['attempt_id']=='attempt-0-1',roots
PY

grep -q 'historical flat slot attempt-0-1 belongs to task' "$spawn_out" \
  || loud_fail "spawn did not emit one reconciliation diagnostic: $(cat "$spawn_out")"
[[ "$(grep -c 'historical flat slot' "$spawn_out")" -eq 1 ]] \
  || loud_fail "foreign-slot reconciliation repeated"

echo 'PASS: task-local attempt-0-1 namespaces isolate observer/Pi/finalizer readers, 1-27 historical bytes stay exact, and fresh preparation launches once'
