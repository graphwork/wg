#!/usr/bin/env bash
# Kill real candidate-spawned workers and require bounded non-running convergence.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null || loud_skip "MISSING CARGO" "candidate build requires cargo"
ROOT=$(git -C "$HERE" rev-parse --show-toplevel) || loud_fail "cannot find repository root"
(cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate build failed"
WG_BIN="$ROOT/target/debug/wg"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR WG_BRANCH
scratch=$(make_scratch); project="$scratch/project"; home="$scratch/home"; fake="$scratch/fake"; sync="$scratch/sync"
mkdir -p "$project" "$home" "$fake" "$sync"
cat >"$fake/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
if [[ ${WG_TASK_ID:?} == dead-dirty ]]; then printf 'valuable dirty WIP\n' >dead-worker-wip.txt; fi
printf '%s\n' "$$" >"${DEAD_SYNC:?}/${WG_TASK_ID}.pid"
printf '%s\n' "$PWD" >"${DEAD_SYNC:?}/${WG_TASK_ID}.worktree"
sleep 600
SH
chmod +x "$fake/pi"
export PATH="$fake:$(dirname "$WG_BIN"):$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config" DEAD_SYNC="$sync" OPENROUTER_API_KEY=fake
cd "$project"; git init -q -b main; git config user.email dead-worker@test.invalid; git config user.name DeadWorker
printf 'base\n' >README; git add README; git commit -qm base
"$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
wgrun(){ env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
wgrun config --local --model pi:openrouter:test/model --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
for id in dead-clean dead-dirty; do
  wgrun add "$id" --id "$id" --model pi:openrouter:test/model -d $'dead owner fixture\n\n## Validation\n- converge without false success' >/dev/null
  wgrun publish "$id" --only >/dev/null
done
start_wg_daemon "$project" --max-agents 2 --no-coordinator-agent --no-supervise
for _ in $(seq 1 240); do [[ -s "$sync/dead-clean.pid" && -s "$sync/dead-dirty.pid" ]] && break; sleep .1; done
[[ -s "$sync/dead-clean.pid" && -s "$sync/dead-dirty.pid" ]] || loud_fail "candidate workers did not launch"
dirty_wt=$(cat "$sync/dead-dirty.worktree")
[[ -f "$dirty_wt/dead-worker-wip.txt" ]] || loud_fail "dirty worker did not create WIP"
for id in dead-clean dead-dirty; do
  key=${id//-/_}
  eval "old_${key}=\$(cat \"$sync/$id.pid\")"
  eval "worktree_${key}=\$(cat \"$sync/$id.worktree\")"
  pid=$(cat "$sync/$id.pid"); kill -KILL "$pid"; for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep .05; done
  kill -0 "$pid" 2>/dev/null && loud_fail "$id child survived SIGKILL"
done
# Wrapper exit + daemon convergence must reach either an exact same-attempt
# replacement process or a preserved non-running terminal hold. An actively
# running replacement is not a stranded InProgress projection.
for _ in $(seq 1 400); do
  converged=yes
  for id in dead-clean dead-dirty; do
    status=$(wgrun show "$id" --json 2>/dev/null | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])' || true)
    [[ $status != done ]] || loud_fail "$id worker death created false Done"
    if [[ $status == in-progress ]]; then
      oldvar="old_${id//-/_}"; oldpid=${!oldvar}; newpid=$(cat "$sync/$id.pid" 2>/dev/null || true)
      if [[ -z $newpid || $newpid == "$oldpid" ]] || ! kill -0 "$newpid" 2>/dev/null; then converged=no; fi
    fi
  done
  [[ $converged == yes ]] && break
  sleep .1
done
[[ $converged == yes ]] || loud_fail "dead workers neither resumed exactly nor reached non-running holds"
for id in dead-clean dead-dirty; do
  status=$(wgrun show "$id" --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  if [[ $status == in-progress ]]; then
    key=${id//-/_}; expected="worktree_${key}"; current=$(cat "$sync/$id.worktree")
    [[ $current == "${!expected}" ]] || loud_fail "$id replacement did not retain the exact worktree"
    wgrun show "$id" --json | python3 -c 'import json,sys;j=json.load(sys.stdin); assert any(e.get("reason_code")=="proven_dead_owner_resume_same_session" for e in j["lifecycle"]["audit"])' \
      || loud_fail "$id replacement lacked exact-resume lifecycle authority"
  fi
done
[[ -f "$dirty_wt/dead-worker-wip.txt" ]] || find "$project/.wg" -type f -exec grep -l 'valuable dirty WIP' {} + | grep -q . \
  || loud_fail "dirty dead-worker WIP was neither retained nor saved"
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults dead_worker_without_intent_converges_nonrunning -- --exact) \
  || loud_fail "dead-owner convergence reducer fault test failed"
echo "PASS: real clean/dirty candidate workers were SIGKILLed, never became Done, reached exact-worktree resumed replacements or non-running holds, and dirty WIP remained recoverable"
