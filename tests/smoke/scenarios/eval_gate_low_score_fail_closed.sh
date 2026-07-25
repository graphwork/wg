#!/usr/bin/env bash
# Real CLI + durable writer/restart regression for fix-low-score-eval-gate.
# Two exact source attempts receive low FLIP/evaluator verdicts while 1.00
# evaluations of the system jobs also exist. Neither attempt may complete or
# unblock its dependent; bounded rescue terminates after attempt two.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
export WG_SMOKE_AGENT_OVERRIDE=1

scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
bindir="$scratch/bin"
mkdir -p "$project" "$home/.config" "$bindir"

cat >"$bindir/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
prompt=$(cat)
printf '%s\n' '{"type":"session","id":"low-gate-smoke","version":3}'
if grep -q 'inferred_prompt' <<<"$prompt"; then
  text='{"inferred_prompt":"Implement the exact persisted task and its validation requirements."}'
elif grep -q 'flip_score' <<<"$prompt"; then
  text='{"flip_score":0.18,"dimensions":{"fidelity":0.18},"notes":"required FLIP below threshold"}'
else
  exec 9>"${LOW_GATE_COUNTER:?}.lock"
  flock 9
  count=0
  [[ -f "$LOW_GATE_COUNTER" ]] && count=$(cat "$LOW_GATE_COUNTER")
  count=$((count + 1))
  printf '%s' "$count" >"$LOW_GATE_COUNTER"
  flock -u 9
  if [[ "$count" == 1 ]]; then score=0.20; else score=0.12; fi
  text="{\"score\":$score,\"dimensions\":{\"correctness\":$score,\"completeness\":$score},\"notes\":\"required evaluator below threshold\"}"
fi
python3 - "$text" <<'PY'
import json,sys
print(json.dumps({"type":"turn_end","message":{"role":"assistant","provider":"openai-codex","model":"gpt-5.6-sol","content":[{"type":"text","text":sys.argv[1]}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"total":0.001}}}}))
PY
FAKE_PI
cat >"$bindir/claude" <<'NO_CLAUDE'
#!/usr/bin/env bash
echo 'unexpected claude fallback' >&2
exit 97
NO_CLAUDE
chmod +x "$bindir/pi" "$bindir/claude"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export PATH="$bindir:$PATH"
export LOW_GATE_COUNTER="$scratch/evaluator-count"
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed.txt
git add seed.txt
git commit -q -m seed

wg init --route pi >/dev/null
# Keep the fixture focused on the exact source pipeline.
python3 - <<'PY'
from pathlib import Path
p=Path('.wg/config.toml')
s=p.read_text()
s='\n'.join(line for line in s.splitlines() if not line.startswith('executor = '))+'\n'
s=s.replace('openrouter:deepseek/deepseek-chat','pi:openrouter:deepseek/deepseek-r1')
p.write_text(s)
PY
wg config --auto-assign false --auto-evaluate true --flip-enabled true \
  --eval-gate-threshold 0.70 --eval-gate-all true \
  --flip-verification-threshold 0.70 --no-reload >/dev/null
for role in evaluator flip_inference flip_comparison; do
  wg config --local --set-model "$role" 'pi:openai-codex:gpt-5.6-sol' --no-reload >/dev/null
done
python3 - <<'PY'
from pathlib import Path
p=Path('.wg/config.toml'); s=p.read_text()
if 'max_verify_failures =' in s:
    import re
    s=re.sub(r'max_verify_failures\s*=\s*\d+', 'max_verify_failures = 1', s)
elif '[coordinator]' in s:
    s=s.replace('[coordinator]', '[coordinator]\nmax_verify_failures = 1', 1)
else:
    s+='\n[coordinator]\nmax_verify_failures = 1\n'
p.write_text(s)
PY

wg add 'hard-gated source' --id low-source \
  -d $'## Deliverables\n- artifact.txt\n\n## Validation\n- [ ] low verdicts never pass' >/dev/null
wg add 'must stay blocked' --id dependent --after low-source >/dev/null
wg publish low-source --only >/dev/null
wg publish dependent --only >/dev/null
printf 'attempt output\n' >artifact.txt
wg artifact low-source artifact.txt >/dev/null

status_of() {
  wg show "$1" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])'
}
plan_hash() {
  python3 - "$1" <<'PY'
import json,sys
sid=sys.argv[1]
for row in map(json.loads,open('.wg/graph.jsonl')):
    if row.get('kind')=='task' and row.get('id')==sid:
        print(row['agency_dispatch']['plan_hash']); break
else: raise SystemExit('missing '+sid)
PY
}
run_gate_attempt() {
  local attempt="$1" flip_hash eval_hash
  wg claim low-source >/dev/null
  wg done low-source --ignore-unmerged-worktree --skip-smoke >"done-$attempt.log"
  [[ "$(status_of low-source)" == pending-eval ]] || loud_fail "attempt $attempt did not enter required PendingEval"
  grep -q 'required gate' "done-$attempt.log" || loud_fail "PendingEval wording did not say required gate"

  flip_hash=$(plan_hash .flip-low-source)
  WG_AGENCY_TASK_ID=.flip-low-source WG_AGENCY_PLAN_HASH="$flip_hash" \
    wg evaluate run low-source --flip >"flip-$attempt.log" 2>&1 || \
    loud_fail "attempt $attempt FLIP writer failed: $(cat "flip-$attempt.log")"
  wg done .flip-low-source --ignore-unmerged-worktree --skip-smoke >/dev/null

  eval_hash=$(plan_hash .evaluate-low-source)
  WG_AGENCY_TASK_ID=.evaluate-low-source WG_AGENCY_PLAN_HASH="$eval_hash" \
    wg evaluate run low-source >"eval-$attempt.log" 2>&1 || \
    loud_fail "attempt $attempt evaluator writer failed: $(cat "eval-$attempt.log")"
  wg done .evaluate-low-source --ignore-unmerged-worktree --skip-smoke >/dev/null

  # Successful execution of evaluation jobs is not source-quality evidence.
  wg evaluate record --task .flip-low-source --score 1.0 --source system-job >/dev/null
  wg evaluate record --task .evaluate-low-source --score 1.0 --source system-job >/dev/null
  show_job=$(wg show .evaluate-low-source)
  grep -q 'not a source quality pass' <<<"$show_job" || loud_fail "system job Done masqueraded as quality pass"
}

run_gate_attempt 1
show_pending=$(wg show low-source)
grep -q 'Applicability: required' <<<"$show_pending" || loud_fail "show omitted required applicability"
grep -q 'Evaluator threshold: 0.70' <<<"$show_pending" || loud_fail "show omitted evaluator threshold"
grep -q 'FLIP policy: required-strict' <<<"$show_pending" || loud_fail "show omitted strict FLIP policy"
grep -q 'FLIP threshold: 0.70' <<<"$show_pending" || loud_fail "show omitted FLIP threshold"

# Reload a stricter ambient threshold. The visible attempt remains pinned to
# 0.70; status shows 0.95 while show preserves the persisted attempt contract.
wg config --eval-gate-threshold 0.95 --no-reload >/dev/null
status_out=$(wg status)
grep -q 'evaluator-threshold=0.95' <<<"$status_out" || loud_fail "status omitted reloaded effective threshold"
grep -q 'Evaluator threshold: 0.70' <<<"$(wg show low-source)" || loud_fail "reload drifted attempt-pinned threshold"
wg pause low-source >/dev/null
wg service tick --max-agents 1 >tick-1.log 2>&1
[[ "$(status_of low-source)" == open ]] || loud_fail "first low attempt was not bounded in-place rescue: $(wg show low-source); tick=$(cat tick-1.log)"
wg resume low-source --only >/dev/null
python3 - <<'PY'
import json
rows={r['id']:r for r in map(json.loads,open('.wg/graph.jsonl')) if r.get('kind')=='task'}
s=rows['low-source']; assert s['rescue_count']==1,s
assert rows['dependent']['status']=='open',rows['dependent']
assert s['evaluation_lifecycle']['source_attempt']==2,s
assert not any('done' == s['status'] for _ in [0]),s
PY
! wg ready | grep -q '^dependent\b' || loud_fail 'dependent unblocked after first low attempt'

run_gate_attempt 2
wg service tick --max-agents 1 >tick-2.log 2>&1
[[ "$(status_of low-source)" == failed ]] || loud_fail "second low attempt did not fail terminally"
! wg ready | grep -q '^dependent\b' || loud_fail 'dependent unblocked after second low attempt'
python3 - <<'PY'
import glob,json
rows={r['id']:r for r in map(json.loads,open('.wg/graph.jsonl')) if r.get('kind')=='task'}
s=rows['low-source']; assert s['status']=='failed',s
assert s['evaluation_lifecycle']['source_attempt']==2,s
assert s['evaluation_lifecycle']['outcome_provenance']['outcome']=='rejected',s
assert 'score=0.12' in s['failure_reason'] and 'score=0.18' in s['failure_reason'],s
verdicts=[json.load(open(p)) for p in glob.glob('.wg/agency/eval-lifecycle/verdicts/*.json')]
own=[v for v in verdicts if v['source_task']=='low-source']
assert len(own)==4,own
assert sorted(set(v['source_attempt'] for v in own))==[1,2],own
assert sum('Consumed durable verdict' in e.get('message','') for e in s.get('log',[]))==2,s
PY

# Advisory is structurally distinct: evaluator exists, source never enters
# PendingEval, and the terminal text refuses to call job execution a pass.
wg config --eval-gate-all false --no-reload >/dev/null
wg add 'advisory report' --id advisory-source -d $'## Validation\n- [ ] write a report\n' >/dev/null
wg publish advisory-source --only >/dev/null
wg claim advisory-source >/dev/null
wg done advisory-source --ignore-unmerged-worktree --skip-smoke >advisory.log
[[ "$(status_of advisory-source)" == done ]] || loud_fail 'advisory source masqueraded as PendingEval'
grep -q 'advisory only' advisory.log || loud_fail 'advisory terminal wording missing'
grep -q 'Applicability: advisory' <<<"$(wg show advisory-source)" || loud_fail 'show omitted advisory applicability'

echo 'PASS: low FLIP/evaluator verdicts failed closed across two exact attempts; system 1.00 scores did not mask; advisory stayed structurally distinct'
