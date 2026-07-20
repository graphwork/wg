#!/usr/bin/env bash
# Accelerated production regression: a handler-first Codex evaluator stays
# silent longer than the configured heartbeat window, survives daemon restart,
# remains truthful in registry/TUI, and completes exactly once. A second hung
# call proves the independent inference deadline still fails closed.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
export WG_SMOKE_AGENT_OVERRIDE=1

command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux required for live TUI status assertion"

WG_REAL="$(command -v wg)"
WG_REAL="$(readlink -f "$WG_REAL" 2>/dev/null || printf '%s' "$WG_REAL")"
scratch=$(make_scratch)
project="$scratch/project"
fake_bin="$scratch/fake-bin"
mkdir -p "$project" "$fake_bin" "$scratch/home"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export INLINE_CODEX_LOG="$scratch/codex.log"
export INLINE_CODEX_MODE="$scratch/codex.mode"
: >"$INLINE_CODEX_LOG"
printf 'success\n' >"$INLINE_CODEX_MODE"

cat >"$fake_bin/codex" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'START pid=%s args=%s\n' "$$" "$*" >>"${INLINE_CODEX_LOG:?}"
model=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --model) model="${2:-}"; shift 2 ;;
    *) shift ;;
  esac
done
cat >/dev/null
[[ "$model" == "gpt-5.6-luna" ]] || { echo "wrong model=$model" >&2; exit 42; }
mode=$(cat "${INLINE_CODEX_MODE:?}")
if [[ "$mode" == hard-timeout ]]; then
  trap 'printf "TERM pid=%s\n" "$$" >>"${INLINE_CODEX_LOG:?}"; exit 143' TERM
  sleep 30
elif [[ "$mode" == crash ]]; then
  sleep 1
  exit 17
else
  # No stdout/model event for eight seconds: longer than the accelerated
  # three-second registry heartbeat window.
  sleep 8
fi
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"score\":0.93,\"dimensions\":{\"correctness\":0.95,\"completeness\":0.92,\"efficiency\":0.90,\"style_adherence\":0.94,\"downstream_usability\":0.93,\"coordination_overhead\":0.91,\"blocking_impact\":0.94},\"notes\":\"silent Luna evaluator completed\"}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":11,"cached_input_tokens":0,"output_tokens":7}}'
printf 'DONE pid=%s\n' "$$" >>"${INLINE_CODEX_LOG:?}"
SH
cat >"$fake_bin/wg" <<'SH'
#!/usr/bin/env bash
printf 'FOREIGN_WG %s\n' "$*" >>"${INLINE_CODEX_LOG:?}"
exit 97
SH
cat >"$fake_bin/claude" <<'SH'
#!/usr/bin/env bash
printf 'CLAUDE_FORBIDDEN %s\n' "$*" >>"${INLINE_CODEX_LOG:?}"
exit 98
SH
cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
printf 'PI_FORBIDDEN %s\n' "$*" >>"${INLINE_CODEX_LOG:?}"
exit 99
SH
chmod +x "$fake_bin"/*

cd "$project"
"$WG_REAL" init --route codex-cli >/dev/null
"$WG_REAL" config --auto-assign false --auto-evaluate true --flip-enabled false --no-reload >/dev/null
"$WG_REAL" config --local --set-model evaluator codex:gpt-5.6-luna --no-reload >/dev/null

# Insert precision liveness/deadline knobs into their existing TOML sections.
python3 - .wg/config.toml <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]); lines=p.read_text().splitlines()
def put(section,key,value):
    global lines
    header=f'[{section}]'; start=lines.index(header)
    end=next((i for i in range(start+1,len(lines)) if lines[i].startswith('[')),len(lines))
    for i in range(start+1,end):
        if lines[i].split('=',1)[0].strip()==key:
            lines[i]=f'{key} = {value}'; return
    lines.insert(start+1,f'{key} = {value}')
put('agent','heartbeat_timeout_seconds','3')
put('agent','reaper_grace_seconds','1')
put('agency','inference_timeout','20')
put('dispatcher','poll_interval','1')
p.write_text('\n'.join(lines)+'\n')
PY

graph="$project/.wg"
export PATH="$fake_bin:$PATH"

scaffold_eval() {
  local id="$1"
  "$WG_REAL" --dir "$graph" add "silent evaluator source $id" --id "$id" --no-place \
    -d $'## Validation\n- [ ] silent inference completes' >/dev/null
  "$WG_REAL" --dir "$graph" pause "$id" >/dev/null
  "$WG_REAL" --dir "$graph" service tick --max-agents 0 >/dev/null
  "$WG_REAL" --dir "$graph" resume "$id" >/dev/null
  "$WG_REAL" --dir "$graph" claim "$id" >/dev/null
  "$WG_REAL" --dir "$graph" done "$id" --ignore-unmerged-worktree --skip-smoke >/dev/null
}

start_absolute_daemon() {
  (cd "$project" && "$WG_REAL" --dir "$graph" service start --max-agents 2 >"$scratch/daemon-start.log" 2>&1)
  local pid
  pid=$(wait_for_daemon_pid "$graph" 20) || loud_fail "daemon did not start: $(cat "$scratch/daemon-start.log")"
  register_wg_daemon "$pid" "$graph"
  WG_SMOKE_DAEMON_PID="$pid"
}

scaffold_eval silent-source
start_absolute_daemon

registry="$graph/service/registry.json"
agent_id=""
for _ in $(seq 1 100); do
  agent_id=$(python3 - "$registry" <<'PY' 2>/dev/null || true
import json,sys
try:r=json.load(open(sys.argv[1]))
except Exception:raise SystemExit
for aid,a in r.get('agents',{}).items():
  if a.get('task_id')=='.evaluate-silent-source' and a.get('status')=='working': print(aid); break
PY
)
  [[ -n "$agent_id" ]] && break
  sleep .1
done
[[ -n "$agent_id" ]] || loud_fail "inline evaluator never registered: $(cat "$registry" 2>/dev/null)"

read_hb() { python3 - "$registry" "$1" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['agents'][sys.argv[2]]['last_heartbeat'])
PY
}
hb0=$(read_hb "$agent_id")
sleep 4
hb1=$(read_hb "$agent_id")
[[ "$hb1" != "$hb0" ]] || loud_fail "silent inference heartbeat did not advance: $hb0"
kill -0 "$(python3 -c "import json;print(json.load(open('$registry'))['agents']['$agent_id']['pid'])")" 2>/dev/null \
  || loud_fail "silent evaluator was reaped at the shortened heartbeat window"

# Registry and the real TUI must both show an active inline evaluation without
# requiring token/model output.
"$WG_REAL" --dir "$graph" agents --json >"$scratch/agents-active.json"
python3 - "$scratch/agents-active.json" <<'PY'
import json,sys
rows=json.load(open(sys.argv[1]))
a=next(x for x in rows if x['task_id']=='.evaluate-silent-source')
assert a['status']=='working' and a['executor']=='codex',a
PY
session="wgsmoke-inline-heartbeat-$$"
cleanup_tui() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_tui
tmux new-session -d -s "$session" -x 180 -y 50 \
  "cd '$project' && '$WG_REAL' --dir '$graph' tui --no-mouse --show-keys"
screen=""
for _ in $(seq 1 40); do
  screen=$(tmux capture-pane -p -t "$session" 2>/dev/null || true)
  if grep -Eq 'evaluating|E1|1 (agent|running)|agent-[0-9]+' <<<"$screen"; then break; fi
  sleep .1
done
grep -Eq 'evaluating|E1|1 (agent|running)|agent-[0-9]+' <<<"$screen" \
  || loud_fail "TUI did not report the silent inline evaluator as active:\n$screen"
tmux kill-session -t "$session" 2>/dev/null || true

# Crash only the daemon. The detached inline wrapper + guarded watcher must
# continue, and the restarted daemon must accept its fresh heartbeat.
old_daemon="$WG_SMOKE_DAEMON_PID"
kill -KILL "$old_daemon" 2>/dev/null || true
for _ in $(seq 1 30); do kill -0 "$old_daemon" 2>/dev/null || break; sleep .1; done
sleep 1
start_absolute_daemon

for _ in $(seq 1 200); do
  status=$("$WG_REAL" --dir "$graph" show .evaluate-silent-source --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  [[ "$status" == done ]] && break
  sleep .1
done
[[ "${status:-}" == done ]] || loud_fail "silent evaluator did not finish once after restart (status=${status:-unknown})"
[[ $(grep -c '^START ' "$INLINE_CODEX_LOG") -eq 1 && $(grep -c '^DONE ' "$INLINE_CODEX_LOG") -eq 1 ]] \
  || loud_fail "silent evaluator retried or failed to complete exactly once: $(cat "$INLINE_CODEX_LOG")"

python3 - "$graph/graph.jsonl" "$registry" "$agent_id" <<'PY'
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
t=next(x for x in rows if x.get('kind')=='task' and x.get('id')=='.evaluate-silent-source')
assert t['agency_dispatch']['calls'][0]['route']=='codex:gpt-5.6-luna',t
assert t['model']=='codex:gpt-5.6-luna',t
r=json.load(open(sys.argv[2]))['agents'][sys.argv[3]]
assert r['model']=='codex:gpt-5.6-luna' and r['executor']=='codex',r
PY
! grep -Eq 'FOREIGN_WG|CLAUDE_FORBIDDEN|PI_FORBIDDEN' "$INLINE_CODEX_LOG" \
  || loud_fail "inline wrapper crossed execution/project systems: $(cat "$INLINE_CODEX_LOG")"
run_sh="$graph/agents/$agent_id/run.sh"
grep -Fq "'$WG_REAL' --dir '$graph' heartbeat-watch '$agent_id'" "$run_sh" \
  || loud_fail "wrapper is not absolute+graph-pinned: $(head -30 "$run_sh")"

# Independent hard timeout: heartbeat remains fresh, but a genuinely hung Luna
# inference is terminated and leaves neither model nor heartbeat descendants.
printf 'hard-timeout\n' >"$INLINE_CODEX_MODE"
python3 - .wg/config.toml <<'PY'
from pathlib import Path
p=Path('.wg/config.toml'); s=p.read_text(); s=s.replace('inference_timeout = 20','inference_timeout = 3'); p.write_text(s)
PY
scaffold_eval hung-source
hung_agent=""
for _ in $(seq 1 100); do
  hung_agent=$(python3 - "$registry" <<'PY' 2>/dev/null || true
import json,sys
try:r=json.load(open(sys.argv[1]))
except Exception:raise SystemExit
for aid,a in r.get('agents',{}).items():
  if a.get('task_id')=='.evaluate-hung-source' and a.get('status')=='working': print(aid); break
PY
)
  [[ -n "$hung_agent" ]] && break
  sleep .1
done
[[ -n "$hung_agent" ]] || loud_fail "hung evaluator never registered"
hung_hb0=$(read_hb "$hung_agent")
sleep 2
hung_hb1=$(read_hb "$hung_agent")
[[ "$hung_hb1" != "$hung_hb0" ]] || loud_fail "hung evaluator did not heartbeat before hard timeout"
for _ in $(seq 1 120); do
  hung_status=$("$WG_REAL" --dir "$graph" show .evaluate-hung-source --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  [[ "$hung_status" != in-progress ]] && break
  sleep .1
done
[[ "${hung_status:-in-progress}" != in-progress ]] || loud_fail "hard timeout did not transition hung evaluator"
grep -q '^TERM pid=' "$INLINE_CODEX_LOG" || loud_fail "hard timeout never terminated the hung model: $(cat "$INLINE_CODEX_LOG")"
hung_wrapper=$(python3 -c "import json;print(json.load(open('$registry'))['agents']['$hung_agent']['pid'])")
for _ in $(seq 1 30); do [[ ! -e "/proc/$hung_wrapper" ]] && break; sleep .1; done
[[ ! -e "/proc/$hung_wrapper" ]] || loud_fail "hard-timeout wrapper survived: pid=$hung_wrapper"
if [[ -d /proc ]]; then
  if pgrep -f "$graph/agents/$hung_agent|heartbeat-watch $hung_agent([[:space:]]|$)" >/dev/null 2>&1; then
    loud_fail "hard timeout left an inline heartbeat/model descendant"
  fi
fi

set_inference_timeout() {
  python3 - "$1" <<'PY'
from pathlib import Path
import sys,re
p=Path('.wg/config.toml'); s=p.read_text()
s=re.sub(r'(?m)^inference_timeout = \d+$',f'inference_timeout = {sys.argv[1]}',s)
p.write_text(s)
PY
}
wait_inline_agent() {
  local task="$1" found=""
  for _ in $(seq 1 120); do
    found=$(python3 - "$registry" "$task" <<'PY' 2>/dev/null || true
import json,sys
try:r=json.load(open(sys.argv[1]))
except Exception:raise SystemExit
for aid,a in r.get('agents',{}).items():
  if a.get('task_id')==sys.argv[2] and a.get('status')=='working': print(aid); break
PY
)
    [[ -n "$found" ]] && { printf '%s' "$found"; return 0; }
    sleep .1
  done
  return 1
}
crash_daemon() {
  local pid="$WG_SMOKE_DAEMON_PID"
  kill -KILL "$pid" 2>/dev/null || true
  for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || return 0; sleep .1; done
}
assert_inline_tree_gone() {
  local aid="$1" pid="$2" label="$3" descendants=""
  for _ in $(seq 1 50); do
    descendants=$(pgrep -f "$graph/agents/$aid|heartbeat-watch $aid([[:space:]]|$)" 2>/dev/null || true)
    if [[ ! -e "/proc/$pid" && -z "$descendants" ]]; then return 0; fi
    sleep .1
  done
  [[ ! -e "/proc/$pid" ]] || loud_fail "$label wrapper survived: pid=$pid"
  loud_fail "$label left an inline process/helper orphan: pids=$descendants"
}

# Child crash: the wrapper exits and parks/retries the transport without a
# heartbeat helper surviving it.
set_inference_timeout 20
printf 'crash\n' >"$INLINE_CODEX_MODE"
scaffold_eval crash-source
crash_agent=$(wait_inline_agent .evaluate-crash-source) || loud_fail "crash evaluator never registered"
crash_pid=$(python3 -c "import json;print(json.load(open('$registry'))['agents']['$crash_agent']['pid'])")
for _ in $(seq 1 80); do
  crash_status=$("$WG_REAL" --dir "$graph" show .evaluate-crash-source --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  [[ "$crash_status" != in-progress ]] && break
  sleep .1
done
[[ "${crash_status:-in-progress}" != in-progress ]] || loud_fail "child crash left evaluator in progress"
assert_inline_tree_gone "$crash_agent" "$crash_pid" "child crash"

# Direct SIGTERM and SIGKILL of the detached session leader both close the
# anonymous guard in-kernel. Reconcile with dispatch disabled and assert no
# watcher/model/sleep descendant remains.
for signal in TERM KILL; do
  printf 'success\n' >"$INLINE_CODEX_MODE"
  id="signal-${signal,,}-source"
  scaffold_eval "$id"
  aid=$(wait_inline_agent ".evaluate-$id") || loud_fail "$signal evaluator never registered"
  pid=$(python3 -c "import json;print(json.load(open('$registry'))['agents']['$aid']['pid'])")
  crash_daemon
  kill -"$signal" -- "-$pid" 2>/dev/null || kill -"$signal" "$pid" 2>/dev/null || true
  sleep .5
  "$WG_REAL" --dir "$graph" service tick --max-agents 0 >/dev/null 2>&1 || true
  assert_inline_tree_gone "$aid" "$pid" "SIG$signal"
  task_status=$("$WG_REAL" --dir "$graph" show ".evaluate-$id" --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
  [[ "$task_status" != in-progress ]] || loud_fail "SIG$signal did not transition evaluator"
  start_absolute_daemon
done

# Helper crash: losing the watcher must NOT exempt the dot task. Once its last
# beat goes stale, a dispatch-disabled reconciliation reaps the live wrapper.
printf 'success\n' >"$INLINE_CODEX_MODE"
scaffold_eval helper-crash-source
helper_agent=$(wait_inline_agent .evaluate-helper-crash-source) || loud_fail "helper-crash evaluator never registered"
helper_wrapper=$(python3 -c "import json;print(json.load(open('$registry'))['agents']['$helper_agent']['pid'])")
helper_pid=""
for _ in $(seq 1 30); do
  helper_pid=$(pgrep -P "$helper_wrapper" -f "heartbeat-watch $helper_agent" | head -1 || true)
  [[ -n "$helper_pid" ]] && break
  sleep .1
done
[[ -n "$helper_pid" ]] || loud_fail "managed heartbeat helper not found"
kill -KILL "$helper_pid"
crash_daemon
sleep 4
"$WG_REAL" --dir "$graph" service tick --max-agents 0 >/dev/null 2>&1 || true
assert_inline_tree_gone "$helper_agent" "$helper_wrapper" "helper crash"
helper_status=$("$WG_REAL" --dir "$graph" show .evaluate-helper-crash-source --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
[[ "$helper_status" != in-progress ]] || loud_fail "stopped helper blanket-exempted the dot task"

echo "PASS: silent codex:gpt-5.6-luna inline inference survived >heartbeat window + daemon restart, stayed visible, completed once; timeout/crash/TERM/KILL/helper-loss paths left no orphan"
