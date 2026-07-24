#!/usr/bin/env bash
# Live retry/evaluation lifecycle regression for fix-eval-retry-pipeline-drift.
# A real daemon dispatches a fake Pi worker, the first source attempt is
# preempted, attempt 2 resumes in place, and the daemon then runs FLIP + eval.
# No credential or network access is required.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

# Service control is the human boundary under test, not an action by the
# worker running this smoke gate.
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
export WG_SMOKE_AGENT_OVERRIDE=1

scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
bindir="$scratch/bin"
sync="$scratch/sync"
mkdir -p "$project" "$home/.config" "$bindir" "$sync"

cat >"$bindir/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
prompt=$(cat)
task="${WG_TASK_ID:-agency-one-shot}"
printf 'task=%s args=%s\n' "$task" "$*" >>"${EVAL_RETRY_PI_LOG:?}"

emit() {
  local text="$1"
  printf '%s\n' '{"type":"session","id":"eval-retry-smoke","version":3}'
  python3 - "$text" <<'PY'
import json,sys
text=sys.argv[1]
print(json.dumps({"type":"turn_end","message":{"role":"assistant","provider":"openai-codex","model":"gpt-5.6-sol","content":[{"type":"text","text":text}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"total":0.001}}}}))
PY
}

case "$task" in
  retry-source)
    exec 9>"${EVAL_RETRY_SYNC:?}/counter.lock"
    flock 9
    count=0
    [[ -f "${EVAL_RETRY_SYNC}/source-count" ]] && count=$(cat "${EVAL_RETRY_SYNC}/source-count")
    count=$((count + 1))
    printf '%s' "$count" >"${EVAL_RETRY_SYNC}/source-count"
    flock -u 9
    if [[ "$count" == 1 ]]; then
      touch "${EVAL_RETRY_SYNC}/attempt-1-started"
      trap 'exit 143' TERM INT
      while :; do sleep 0.1; done
    fi
    touch "${EVAL_RETRY_SYNC}/attempt-2-started"
    # Hold the source so the smoke can inspect/restart around the attempt mint.
    while [[ ! -f "${EVAL_RETRY_SYNC}/release-attempt-2" ]]; do sleep 0.05; done
    wg pause .flip-retry-source >/dev/null
    wg log retry-source 'fake Pi attempt 2 completed implementation' >/dev/null
    wg done retry-source --ignore-unmerged-worktree --skip-smoke >/dev/null
    touch "${EVAL_RETRY_SYNC}/attempt-2-done"
    emit 'implementation attempt 2 complete'
    ;;
  downstream)
    wg done downstream --ignore-unmerged-worktree --skip-smoke >/dev/null
    touch "${EVAL_RETRY_SYNC}/downstream-done"
    emit 'downstream dispatched after current-attempt verdict'
    ;;
  *)
    if grep -q 'inferred_prompt' <<<"$prompt"; then
      emit '{"inferred_prompt":"Implement the retry-safe evaluation lifecycle and validate it."}'
    elif grep -q 'flip_score' <<<"$prompt"; then
      emit '{"flip_score":0.96,"dimensions":{"fidelity":0.96},"notes":"attempt two FLIP"}'
    else
      emit '{"score":0.95,"dimensions":{"correctness":0.95,"completeness":0.95},"notes":"attempt two evaluation"}'
    fi
    ;;
esac
FAKE_PI
chmod +x "$bindir/pi"
cat >"$bindir/claude" <<'NO_CLAUDE'
#!/usr/bin/env bash
echo 'unexpected claude fallback' >&2
exit 97
NO_CLAUDE
chmod +x "$bindir/claude"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bindir:$PATH"
export EVAL_RETRY_SYNC="$sync"
export EVAL_RETRY_PI_LOG="$scratch/pi.log"
: >"$EVAL_RETRY_PI_LOG"
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed.txt
git add seed.txt
git commit -q -m seed

run_wg() {
  env -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" PATH="$PATH" \
    EVAL_RETRY_SYNC="$EVAL_RETRY_SYNC" EVAL_RETRY_PI_LOG="$EVAL_RETRY_PI_LOG" \
    wg "$@"
}
wait_for() {
  local what="$1"; shift
  for _ in $(seq 1 300); do
    "$@" && return 0
    sleep 0.1
  done
  loud_fail "timed out waiting for $what; daemon=$(tail -120 "$project/.wg/service/daemon.log" 2>/dev/null || true)"
}
status_is() {
  local id="$1" expected="$2" actual
  actual=$(run_wg show "$id" --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') || return 1
  [[ "$actual" == "$expected" ]]
}

run_wg init --route pi >/dev/null
# The shipped Pi starter intentionally contains one-release legacy keys. Keep
# this regression's diagnostics focused on lifecycle state rather than repeat
# their deprecation warning on every subprocess.
python3 - <<'PY'
from pathlib import Path
path=Path('.wg/config.toml')
text=path.read_text()
text='\n'.join(line for line in text.splitlines() if not line.startswith('executor = '))+'\n'
text=text.replace('openrouter:deepseek/deepseek-chat', 'pi:openrouter:deepseek/deepseek-r1')
path.write_text(text)
PY
run_wg config --auto-assign false --auto-evaluate true --flip-enabled true --no-reload >/dev/null
for role in evaluator flip_inference flip_comparison; do
  run_wg config --local --set-model "$role" 'pi:openai-codex:gpt-5.6-sol' --no-reload >/dev/null
done
run_wg config --local --model 'pi:openai-codex:gpt-5.6-sol' --reasoning high --no-reload >/dev/null
run_wg add 'retry-safe source' --id retry-source \
  -d $'## Validation\n- [ ] attempt-two evaluation only' >/dev/null
run_wg add 'downstream after evaluation' --id downstream --after retry-source >/dev/null

# Materialize attempt-1 plans without letting a worker dispatch yet.
run_wg publish retry-source --only >/dev/null
run_wg publish downstream --only >/dev/null
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t={row['id']:row for row in rows if row.get('kind')=='task'}
for sid in ('.flip-retry-source','.evaluate-retry-source'):
    p=t[sid]['agency_dispatch']
    assert p['source_attempt']==1,(sid,p)
    assert all(c['route']=='pi:openai-codex:gpt-5.6-sol' for c in p['calls']),p
    assert all(c.get('reasoning')=='high' for c in p['calls']),p
PY

# Real daemon: attempt 1 starts and is preempted before completion.
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --interval 1 \
  --model 'pi:openai-codex:gpt-5.6-sol'
wait_for 'attempt 1 start' test -f "$sync/attempt-1-started"
run_wg retry retry-source --reason 'smoke preemption before completion' >/dev/null

# The retry transaction must already contain parent + both attempt-2 plans,
# before the daemon can claim either evaluation satellite.
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t={row['id']:row for row in rows if row.get('kind')=='task'}
life=t['retry-source']['evaluation_lifecycle']
assert life['source_attempt']==2,life
assert life.get('consumed_verdict') is None,life
for sid in ('.flip-retry-source','.evaluate-retry-source'):
    task=t[sid]; p=task['agency_dispatch']
    assert task['status']=='open',(sid,task)
    assert p['source_attempt']==2,(sid,p)
    assert p['pipeline_id']==life['pipeline_id'],(sid,p,life)
    assert all(c['route']=='pi:openai-codex:gpt-5.6-sol' for c in p['calls']),p
    assert all(c.get('reasoning')=='high' for c in p['calls']),p
PY

# Restart between retry mint and attempt-2 completion.
run_wg service stop >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --interval 1 \
  --model 'pi:openai-codex:gpt-5.6-sol'
wait_for 'attempt 2 start' test -f "$sync/attempt-2-started"
touch "$sync/release-attempt-2"
wait_for 'attempt 2 PendingEval' status_is retry-source pending-eval

# PendingEval is not worker progress, and all diagnosis surfaces name the
# active current-attempt evaluation instead of silently saying "progress".
status_out=$(run_wg status)
grep -q 'Evaluation: 1 active' <<<"$status_out" || loud_fail "status lacks eval health: $status_out"
show_out=$(run_wg show retry-source)
grep -q 'evaluation_health: active-evaluation' <<<"$show_out" || loud_fail "show lacks eval health: $show_out"
why_out=$(run_wg why-blocked retry-source)
grep -q 'Evaluation health: active-evaluation' <<<"$why_out" || loud_fail "why-blocked lacks eval health: $why_out"

# Restart again between source completion and evaluation dispatch, then release
# FLIP. The real daemon executes both Pi one-shot stages and evaluator.
run_wg service stop >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --interval 1 \
  --model 'pi:openai-codex:gpt-5.6-sol'
run_wg resume .flip-retry-source --only >/dev/null
wait_for 'attempt 2 verdict consumption' status_is retry-source done
wait_for 'downstream dispatch' test -f "$sync/downstream-done"

python3 - <<'PY'
import glob,json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t={row['id']:row for row in rows if row.get('kind')=='task'}
source=t['retry-source']; life=source['evaluation_lifecycle']
assert source['status']=='done',source
assert life['source_attempt']==2,life
assert life.get('consumed_verdict'),life
assert sum('Consumed durable verdict' in e.get('message','') for e in source.get('log',[]))==1,source
assert t['downstream']['status']=='pending-eval',t['downstream']
assert t['downstream'].get('started_at'),t['downstream']
verdicts=[json.load(open(p)) for p in glob.glob('.wg/agency/eval-lifecycle/verdicts/*.json')]
ours=[v for v in verdicts if v.get('source_task')=='retry-source']
assert ours,verdicts
assert all(v['source_attempt']==2 for v in ours),ours
assert all(v['pipeline_id']==life['pipeline_id'] for v in ours),ours
PY

# Repeated daemon ticks cannot consume twice or create another evaluation.
verdict_count=$(find .wg/agency/eval-lifecycle/verdicts -type f -name '*.json' | wc -l)
for _ in 1 2 3; do run_wg service tick --max-agents 0 >/dev/null; done
verdict_count_after=$(find .wg/agency/eval-lifecycle/verdicts -type f -name '*.json' | wc -l)
[[ "$verdict_count" == "$verdict_count_after" ]] || loud_fail "duplicate verdict: $verdict_count -> $verdict_count_after"
python3 - <<'PY'
import json
source=next(row for row in map(json.loads,open('.wg/graph.jsonl')) if row.get('kind')=='task' and row.get('id')=='retry-source')
assert sum('Consumed durable verdict' in e.get('message','') for e in source.get('log',[]))==1,source
PY

echo 'PASS: real service preempted attempt 1, atomically minted/rearmed attempt 2, survived restarts, consumed once, and dispatched downstream'
