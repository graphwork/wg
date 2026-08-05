#!/usr/bin/env bash
# Candidate-binary daemon regression: a failed prerequisite with no live owner
# cannot leave its unfinished descendant silently open forever.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg missing: $WG_BIN"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME" "$scratch/project"
cd "$scratch/project"
"$WG_BIN" init --no-agency >/dev/null
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" config --local -m pi:openrouter:example/failed-prerequisite --no-reload >/dev/null
"$WG_BIN" --dir "$G" config set dispatcher.poll_interval 1 >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_assign false >/dev/null
"$WG_BIN" --dir "$G" add source --id source -d $'## Validation\n- source' >/dev/null
"$WG_BIN" --dir "$G" add descendant --id descendant --after source -d $'## Validation\n- descendant' >/dev/null
"$WG_BIN" --dir "$G" publish source --only >/dev/null
"$WG_BIN" --dir "$G" publish descendant --only >/dev/null

python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('kind')=='task' and row.get('id')=='source':
        row['status']='failed'; row['retry_count']=1
        row['failure_class']='agent-exit-nonzero'
        row['failure_reason']='source execution failed before model progress'
        row['session_id']='session-exact-zero-progress'
        row['assigned']=None
        row['lifecycle']={
          'generation':1,'revision':1,'fence':2,'attempt_sequence':1,
          'current_attempt':{'id':'attempt-1-1','generation':1,'fence':2,'actor_id':'agent-dead','disposition':'failed'},
          'audit':[]
        }
    rows.append(row)
with open(p,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
PY

cleanup() { "$WG_BIN" --dir "$G" service stop --force --kill-agents >/dev/null 2>&1 || true; }
trap cleanup EXIT
"$WG_BIN" --dir "$G" service start --max-agents 0 --no-chat-agent --force >/dev/null

converged=false
for _ in $(seq 1 100); do
  if python3 - "$G/graph.jsonl" <<'PY' >/dev/null 2>&1
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
s=next(x for x in rows if x.get('kind')=='task' and x.get('id')=='source')
d=next(x for x in rows if x.get('kind')=='task' and x.get('id')=='descendant')
assert s['status']=='open', s
assert s['lifecycle']['generation']==2, s['lifecycle']
assert s.get('session_id')=='session-exact-zero-progress', s
assert any(e.get('reason_code')=='nonsemantic_failed_prerequisite_retry' for e in s['lifecycle'].get('audit',[])), s['lifecycle']
assert d['status']=='open', d
PY
  then converged=true; break; fi
  sleep 0.1
done
$converged || loud_fail "daemon left descendant silently blocked after every owner exited: $(cat "$G/graph.jsonl")"

ready=$("$WG_BIN" --dir "$G" ready)
grep -q 'source' <<<"$ready" || loud_fail "bounded convergence created no runnable source action: $ready"
[[ -f "$G/service/convergence-state.json" ]] || loud_fail "durable convergence state missing"
echo "PASS: candidate daemon converted ownerless non-semantic failed prerequisite into one bounded, evidence-preserving retry; descendant was never silently stranded"
