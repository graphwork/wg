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
mode=${WG_WORKER_CONTROL_MODE:-}
[[ $mode == scoped || $mode == read-only ]] || exit 89
[[ ! -e .wg ]] || exit 83
if env -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL -u WG_WORKER_IPC wg list > env-strip.out 2>&1; then
  echo "stripping capability environment unexpectedly granted operator authority" >&2
  exit 93
fi
grep -q 'worker_control.capability_required_for_managed_process' env-strip.out
graph_guess="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.wg"
if env WG_WORKER_CONTROL_MODE=trusted WG_DIR="$graph_guess" wg list > mode-widen.out 2>&1; then
  echo "mutable mode environment unexpectedly widened worker authority" >&2
  exit 94
fi
grep -q 'worker_control.mode_override_refused' mode-widen.out
wg capabilities --json > capabilities.json
grep -q "\"mode\": \"$mode\"" capabilities.json
wg show "$WG_TASK_ID" --json > "$mode-show.json"
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
grep -Eq 'worker_control.(raw_graph_environment_refused|capability_unknown|graph_identity_mismatch)' guessed-path.out
if git add -f .wg >/dev/null 2>&1; then
  echo "absent .wg unexpectedly entered candidate" >&2
  exit 88
fi
if [[ $mode == scoped ]]; then
  wg log "$WG_TASK_ID" "brokered worker log"
  printf 'brokered\n' > brokered.txt
  git add capabilities.json env-strip.out mode-widen.out scoped-show.json brokered.txt
else
  wg msg list "$WG_TASK_ID" > read-only-messages.out
  grep -q 'operator message for read-only observation' read-only-messages.out
  if wg msg poll "$WG_TASK_ID" > read-only-poll.out 2>&1; then
    echo "read-only poll unexpectedly advanced message state" >&2
    exit 92
  fi
  grep -q 'worker_control.read_only_refused' read-only-poll.out
  if wg log "$WG_TASK_ID" "forbidden read-only log" > read-only-log.out 2>&1; then
    echo "read-only graph log unexpectedly allowed" >&2
    exit 90
  fi
  grep -q 'worker_control.read_only_refused' read-only-log.out
  if wg done "$WG_TASK_ID" > read-only-done.out 2>&1; then
    echo "read-only completion unexpectedly allowed" >&2
    exit 91
  fi
  grep -q 'worker_control.read_only_refused' read-only-done.out
  printf 'read-only\n' > read-only.txt
  git add capabilities.json env-strip.out mode-widen.out read-only-show.json read-only-messages.out read-only-poll.out read-only-log.out read-only-done.out read-only.txt
fi
git commit -qm "worker capability evidence: $mode"
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
wgrun config set worker_control.mode scoped >/dev/null
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

# The explicit read-only mode keeps observations but refuses own-task mutations
# and terminal completion through the same attempt-fenced capability channel.
wgrun service stop --force --kill-agents >/dev/null
wgrun config set worker_control.mode read-only >/dev/null
wgrun add "read-only broker probe" --id read-only-broker-probe >/dev/null
wgrun msg send read-only-broker-probe "operator message for read-only observation" >/dev/null
wgrun publish read-only-broker-probe --only >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null
read_only_worktree=""
for _ in $(seq 1 240); do
  read_only_worktree=$([[ -d "$project/.wg-worktrees" ]] && find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d -exec sh -c 'git -C "$1" show HEAD:read-only.txt >/dev/null 2>&1 && printf "%s\\n" "$1"' _ {} \; | head -1 || true)
  [[ -n $read_only_worktree ]] && break
  status=$(wgrun show read-only-broker-probe --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == failed || $status == abandoned ]] && loud_fail "read-only worker terminal status: $status"
  sleep 0.25
done
[[ -n $read_only_worktree ]] || loud_fail "read-only worker evidence absent"
git -C "$read_only_worktree" show HEAD:read-only.txt | grep -qx read-only || loud_fail "read-only worker commit absent"
git -C "$read_only_worktree" show HEAD:read-only-log.out | grep -q 'worker_control.read_only_refused' || loud_fail "read-only log refusal absent"
git -C "$read_only_worktree" show HEAD:read-only-done.out | grep -q 'worker_control.read_only_refused' || loud_fail "read-only completion refusal absent"

echo "PASS: live workers had no WG_DIR/.wg; OS process identity blocked WG_* stripping; scoped stayed own-task-only; read-only retained immutable observation but refused polling/writes; bearer tokens were not persisted"
