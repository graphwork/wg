#!/usr/bin/env bash
# Candidate-binary credential-free worker control broker regression.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-worker-cap.XXXXXX")
trap 'env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$scratch/project/.wg" "$WG_BIN" service stop --force --kill-agents >/dev/null 2>&1 || true; [[ ${WG_SMOKE_KEEP_TMP:-0} == 1 ]] || rm -rf "$scratch"' EXIT
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home" "$scratch/bin"
ln -s "$WG_BIN" "$scratch/bin/wg"
cat >"$scratch/bin/pi" <<'SH'
#!/usr/bin/env bash
exec bash worker.sh
SH
chmod +x "$scratch/bin/pi"
export PATH="$scratch/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH

git -C "$project" init -q -b main
git -C "$project" config user.email worker-cap@test.invalid
git -C "$project" config user.name WorkerCap
cat >"$project/worker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ! -v WG_DIR ]] || { echo "WG_DIR leaked" >&2; exit 81; }
[[ -n ${WG_WORKER_CAPABILITY:-} && -S ${WG_WORKER_IPC:-/missing} ]] || exit 82
[[ ! -e .wg ]] || exit 83
wg show "$WG_TASK_ID" --json > scoped-show.json
if wg list > forbidden.out 2>&1; then
  echo "graph enumeration unexpectedly allowed" >&2
  exit 84
fi
grep -q 'worker_control.operation_refused' forbidden.out
if wg show another-task > cross-task.out 2>&1; then
  echo "cross-task read unexpectedly allowed" >&2
  exit 85
fi
grep -q 'worker_control.cross_task_refused' cross-task.out
if wg service status > service-control.out 2>&1; then
  echo "service control unexpectedly allowed" >&2
  exit 86
fi
grep -q 'worker_control.operation_refused' service-control.out
if WG_DIR=/tmp/guessed-control wg show "$WG_TASK_ID" > guessed-path.out 2>&1; then
  echo "raw graph environment unexpectedly accepted" >&2
  exit 87
fi
grep -q 'worker_control.raw_graph_environment_refused' guessed-path.out
if git add -f .wg >/dev/null 2>&1; then
  echo "absent .wg unexpectedly entered candidate" >&2
  exit 88
fi
wg log "$WG_TASK_ID" "brokered worker log"
printf 'brokered\n' > brokered.txt
git add scoped-show.json brokered.txt
git commit -qm 'worker capability evidence'
# Keep the owner alive so the scenario can inspect the capability boundary
# without asking this smoke to exercise the separate finish protocol.
sleep 120
SH
chmod +x "$project/worker.sh"
git -C "$project" add worker.sh
git -C "$project" commit -qm base
(
  cd "$project"
  env -u WG_DIR "$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
)
wgrun() { (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"); }
wgrun add "worker broker probe" --id worker-broker-probe >/dev/null
wgrun publish worker-broker-probe --only >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null

worktree=""
for _ in $(seq 1 240); do
  worktree=$([[ -d "$project/.wg-worktrees" ]] && find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d | head -1 || true)
  if [[ -n $worktree ]] && git -C "$worktree" show HEAD:brokered.txt 2>/dev/null | grep -qx brokered; then
    break
  fi
  status=$(wgrun show worker-broker-probe --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == failed || $status == abandoned ]] && loud_fail "broker worker terminal status: $status"
  sleep 0.25
done
[[ -n $worktree ]] || loud_fail "worker worktree was not created"
git -C "$worktree" show HEAD:brokered.txt | grep -qx brokered || loud_fail "brokered worker commit absent"
[[ ! -e "$worktree/.wg" ]] || loud_fail "worker candidate contained .wg"
registry="$project/.wg/service/worker-capabilities.json"
[[ -s $registry ]] || loud_fail "capability registry missing"
if grep -q 'wgcap_v1_' "$registry"; then loud_fail "bearer token persisted"; fi
grep -q 'brokered worker log' "$project/.wg/graph.jsonl" || loud_fail "brokered log absent"
grep -q '"outcome":"allowed"' "$project/.wg/service/worker-capability-audit.jsonl" || loud_fail "allow audit absent"
status_json=$(wgrun service status --json)
printf '%s' "$status_json" | grep -Eq '"capability_broker"[[:space:]]*:[[:space:]]*"enforced"' || loud_fail "broker status absent: $status_json"
printf '%s' "$status_json" | grep -Eq '"enforced"[[:space:]]*:[[:space:]]*false' || loud_fail "degraded filesystem status overclaimed: $status_json"

echo "PASS: live worker had no WG_DIR/.wg, scoped commands traversed authenticated IPC, graph enumeration was refused, capability bearer was not persisted, and degraded filesystem isolation was reported honestly"
