#!/usr/bin/env bash
# Live writer -> satellite completion -> restarted reader regression for the
# 2026-07-19 PendingEval stall. The Pi/Terra route is real; only the remote
# model response is stubbed so this remains credential-free and deterministic.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
export WG_SMOKE_AGENT_OVERRIDE=1

scratch=$(make_scratch)
fake_bin="$scratch/fake-bin"
mkdir -p "$fake_bin" "$scratch/home"
cat >"$fake_bin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'ARGS %s\n' "$*" >>"${LIVE_EVAL_PI_LOG:?}"
provider=""; model=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --provider) provider="${2:-}"; shift 2 ;;
    --model) model="${2:-}"; shift 2 ;;
    *) shift ;;
  esac
done
cat >/dev/null
[[ "$provider" == openai-codex && "$model" == gpt-5.6-terra ]] || {
  echo "wrong explicit route provider=$provider model=$model" >&2; exit 42;
}
printf '%s\n' '{"type":"session","id":"live-eval-smoke","version":3}'
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","provider":"openai-codex","model":"gpt-5.6-terra","content":[{"type":"text","text":"{\"score\":0.88,\"dimensions\":{\"correctness\":0.89,\"completeness\":0.85,\"efficiency\":0.81,\"style_adherence\":0.94,\"downstream_usability\":0.94,\"coordination_overhead\":0.88,\"blocking_impact\":0.93},\"notes\":\"live Pi Terra verdict\"}"}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"total":0.001}}}}'
SH
cat >"$fake_bin/claude" <<'SH'
#!/usr/bin/env bash
echo CLAUDE_FALLBACK_FORBIDDEN >>"${LIVE_EVAL_PI_LOG:?}"
exit 99
SH
chmod +x "$fake_bin/pi" "$fake_bin/claude"

export PATH="$fake_bin:$PATH"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export LIVE_EVAL_PI_LOG="$scratch/pi.log"
: >"$LIVE_EVAL_PI_LOG"
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY

project="$scratch/project"
mkdir -p "$project"
cd "$project"
wg init >/dev/null
wg config --auto-assign false --auto-evaluate true --flip-enabled false --no-reload >/dev/null
wg config --local --set-model evaluator 'pi:openai-codex:gpt-5.6-terra' --no-reload >/dev/null
wg add 'live Pi evaluation digest' --id live-eval-source --no-place \
  -d $'## Validation\n- [ ] explicit Pi Terra evaluation is consumed' >/dev/null
# Materialize the evaluator scaffold without dispatching any worker.
wg pause live-eval-source >/dev/null
wg service tick --max-agents 0 >/dev/null
wg resume live-eval-source >/dev/null
wg claim live-eval-source >/dev/null
wg done live-eval-source --ignore-unmerged-worktree --skip-smoke >/dev/null

read -r plan_hash route handler <<EOF_PLAN
$(python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
t=next(r for r in rows if r.get('kind')=='task' and r.get('id')=='.evaluate-live-eval-source')
p=t['agency_dispatch']; c=p['calls'][0]
print(p['plan_hash'],c['route'],c['system']['handler'])
PY
)
EOF_PLAN
[[ "$route" == 'pi:openai-codex:gpt-5.6-terra' && "$handler" == pi ]] || \
  loud_fail "scaffold route drifted: route=$route handler=$handler"

# This runs the real persisted-plan evaluate command and Pi one-shot handler.
WG_AGENCY_TASK_ID=.evaluate-live-eval-source WG_AGENCY_PLAN_HASH="$plan_hash" \
  wg evaluate run live-eval-source >evaluate.log 2>&1 || \
  loud_fail "live Pi/Terra evaluator failed: $(cat evaluate.log)"
grep -q -- '--provider openai-codex.*--model gpt-5.6-terra' "$LIVE_EVAL_PI_LOG" || \
  loud_fail "Pi did not receive explicit Terra route: $(cat "$LIVE_EVAL_PI_LOG")"
! grep -q CLAUDE_FALLBACK_FORBIDDEN "$LIVE_EVAL_PI_LOG" || \
  loud_fail 'implicit Claude fallback was invoked'

verdict_file=$(find .wg/agency/eval-lifecycle/verdicts -type f -name '*.json' | head -1)
evaluation_id=$(python3 - "$verdict_file" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['evaluation_id'])
PY
)
eval_file=".wg/agency/evaluations/$evaluation_id.json"
[[ -f "$eval_file" && -f "$verdict_file" ]] || loud_fail 'writer did not persist both evidence files'
python3 - "$eval_file" "$verdict_file" <<'PY'
import json,sys
e=json.load(open(sys.argv[1])); v=json.load(open(sys.argv[2]))
assert e['score']==0.88 and len(e['dimensions'])==7,e
assert e['evaluator']=='pi:openai-codex:gpt-5.6-terra',e
assert v['score']==0.88 and v['evaluation_digest_schema']==2,v
assert v['evaluation_id']==e['id'],(v,e)
PY
before_eval=$(sha256sum "$eval_file" | cut -d' ' -f1)
before_verdict=$(sha256sum "$verdict_file" | cut -d' ' -f1)

# Preserve the exact pre-reader graph for two fail-closed tamper branches.
mkdir -p "$scratch/tampered-evaluation" "$scratch/tampered-verdict"
cp -a .wg "$scratch/tampered-evaluation/.wg"
cp -a .wg "$scratch/tampered-verdict/.wg"

wg done .evaluate-live-eval-source --ignore-unmerged-worktree --skip-smoke >/dev/null
after_eval=$(sha256sum "$eval_file" | cut -d' ' -f1)
after_verdict=$(sha256sum "$verdict_file" | cut -d' ' -f1)
[[ "$before_eval" == "$after_eval" && "$before_verdict" == "$after_verdict" ]] || \
  loud_fail "satellite completion mutated evidence: eval $before_eval->$after_eval verdict $before_verdict->$after_verdict"

# A fresh service process is the reader/restart boundary.
wg service tick --max-agents 1 >restart-reader.log 2>&1
if ! python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
t=next(r for r in rows if r.get('kind')=='task' and r.get('id')=='live-eval-source')
assert t['status']=='done',t
life=t['evaluation_lifecycle']; assert life['consumed_verdict'],life
assert sum('Consumed durable verdict' in x.get('message','') for x in t.get('log',[]))==1,t
PY
then
  loud_fail "restart reader did not consume: $(cat restart-reader.log)"
fi
for _ in 1 2 3; do wg service tick --max-agents 1 >/dev/null; done
python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
t=next(r for r in rows if r.get('kind')=='task' and r.get('id')=='live-eval-source')
assert t['status']=='done',t
assert sum('Consumed durable verdict' in x.get('message','') for x in t.get('log',[]))==1,t
PY
[[ "$before_eval" == "$(sha256sum "$eval_file" | cut -d' ' -f1)" ]] || loud_fail 'reader rewrote evaluation'
[[ "$before_verdict" == "$(sha256sum "$verdict_file" | cut -d' ' -f1)" ]] || loud_fail 'reader rewrote verdict'

# Tampering either side of the evidence pair must fail loudly and leave the
# pre-reader parent PendingEval. No ambiguous evidence is guessed.
for branch in tampered-evaluation tampered-verdict; do
  branch_dir="$scratch/$branch"
  if [[ "$branch" == tampered-evaluation ]]; then
    target="$branch_dir/.wg/agency/evaluations/$evaluation_id.json"
    python3 - "$target" <<'PY'
import json,sys
p=sys.argv[1]; x=json.load(open(p)); x['notes']='tampered'; open(p,'w').write(json.dumps(x,indent=2))
PY
  else
    target=$(find "$branch_dir/.wg/agency/eval-lifecycle/verdicts" -type f -name '*.json' | head -1)
    python3 - "$target" <<'PY'
import json,sys
p=sys.argv[1]; x=json.load(open(p)); x['score']=0.01; open(p,'w').write(json.dumps(x,indent=2))
PY
  fi
  (cd "$branch_dir" && wg service tick --max-agents 1 >tick.log 2>&1 || true)
  grep -Eq 'WG-EVAL-VERDICT-(EVIDENCE|INTEGRITY)' "$branch_dir/tick.log" || \
    loud_fail "$branch did not fail loudly: $(cat "$branch_dir/tick.log")"
  status=$(cd "$branch_dir" && wg show live-eval-source --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
  [[ "$status" == pending-eval ]] || loud_fail "$branch was consumed despite tamper: $status"
done

echo "PASS: explicit Pi/Terra writer pinned eval=$before_eval verdict=$before_verdict; restart consumed once; tampering failed closed"
