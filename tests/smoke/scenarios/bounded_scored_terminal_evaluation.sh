#!/usr/bin/env bash
# Credential-free real terminal flow for task-centric receipt-backed scoring.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$repo_root/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$repo_root" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
args="$*"; input=$(cat)
printf 'ARGS %s\n' "$args" >>"${FAKE_EVAL_LOG:?}"
if grep -q 'worksgood-terminal-scored-evaluation-v1' <<<"$input"; then
  printf 'SCORE_CALL\n' >>"${FAKE_EVAL_LOG:?}"
  if [[ "${FAKE_EVAL_FAIL:-}" == 1 ]]; then echo 'fixture provider unavailable' >&2; exit 42; fi
  [[ " $args " == *" --provider test "* && " $args " == *" --model fake-score "* && " $args " == *" --thinking xhigh "* ]] || {
    echo "wrong scoring argv: $args" >&2; exit 43;
  }
  printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"overall_score\":0.83,\"dimensions\":{\"correctness\":0.91,\"completeness\":0.84,\"efficiency\":0.76,\"style_adherence\":0.88,\"downstream_usability\":0.8,\"coordination_overhead\":0.72,\"blocking_impact\":0.9},\"notes\":\"receipt-bound fake Pi score\"}"}],"provider":"test","model":"fake-score","usage":{"input":31,"output":9,"cacheRead":7,"cacheWrite":2,"totalTokens":49,"cost":{"total":0.0042}}}}'
else
  [[ " $args " == *" --model fake-review "* || " $args " == *" --model fake-score "* ]] || { echo "wrong review argv: $args" >&2; exit 44; }
  printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"verdict\":\"pass\",\"findings\":[]}"}],"provider":"test","model":"fake-review","usage":{"input":2,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"total":0.0001}}}}'
fi
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_EVAL_LOG="$scratch/pi.log"
unset WG_TASK_ID WG_AGENT_ID WG_DIR WG_WORKER_CAPABILITY WG_WORKER_IPC TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY
: >"$FAKE_EVAL_LOG"

(cd "$project" && git init -q -b main && git config user.email eval@test.invalid && git config user.name Eval && printf 'base\n' >base.txt && git add base.txt && git commit -qm base && "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:fake-review --reasoning low --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:fake-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:fake-score --set-reasoning evaluator xhigh --no-reload >/dev/null

# Ineligible status failures must not create an evaluation row or mutate source state.
for fixture in failed waiting; do
  wgrun add "$fixture scoring refusal" --id "$fixture" -d $'## Validation\n- [ ] never score non-Done state' >/dev/null
  wgrun publish "$fixture" --only >/dev/null
  wgrun claim "$fixture" >/dev/null
  if [[ "$fixture" == failed ]]; then
    wgrun fail "$fixture" --reason 'fixture failure' >/dev/null
  else
    wgrun wait "$fixture" --until timer:1h >/dev/null
  fi
  before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
  if wgrun evaluate run "$fixture" --dry-run >"$scratch/$fixture.out" 2>&1; then
    loud_fail "$fixture was incorrectly eligible for scoring"
  fi
  after=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
  [[ "$before" == "$after" ]] || loud_fail "$fixture refusal mutated graph"
done
[[ ! -d "$G/agency/evaluations" || -z "$(find "$G/agency/evaluations" -type f -name '*.json' -print -quit)" ]] || loud_fail 'ineligible task created evaluation row'

# Build one real immutable reviewed Report completion.
wgrun add 'Bounded scored terminal evaluation' --id scored -d $'Produce report.txt.\n\n## Validation\n- [ ] exact receipt-backed scoring' >/dev/null
wgrun contract scored report >/dev/null
wgrun publish scored --only >/dev/null
wgrun claim scored >/dev/null
# Seed terminal-time execution attribution so replay also proves that the
# immutable observation, not a later cleanup-stripped mutable snapshot, owns it.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('id')=='scored':
        row['actual_executor']='pi'
        row['actual_model']='test:completed-work'
    rows.append(row)
open(p,'w').write(''.join(json.dumps(row,separators=(',',':'))+'\n' for row in rows))
PY
(
  cd "$project"
  printf 'implemented and validated\n' >summary.txt
  printf 'reviewed report bytes\n' >report.txt
  printf 'validation passed\n' >validation.log
  wgrun completion-object report.txt --media-type text/plain >output-ref.json
  wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
  wgrun completion-manifest scored --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
  wgrun submit scored --manifest manifest.json --summary summary.txt >/dev/null
  wgrun done scored >/dev/null
)

# Missing terminal observation is unverifiable and must remain neutral.
observation=$(find "$G/agency/terminal-observations" -type f -name '*.json' | head -1)
cp "$observation" "$scratch/observation.json"
rm "$observation"
graph_before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
if wgrun evaluate run scored --dry-run >"$scratch/missing-observation.out" 2>&1; then
  loud_fail 'missing terminal observation was incorrectly eligible'
fi
[[ "$graph_before" == "$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail 'unverifiable refusal mutated graph'
cp "$scratch/observation.json" "$observation"

# Dry-run re-verifies everything and exposes exact route/reasoning/evidence without writes.
eval_count(){ if [[ -d "$G/agency/evaluations" ]]; then find "$G/agency/evaluations" -type f -name '*.json' | wc -l | tr -d ' '; else printf '0\n'; fi; }
count_before=$(eval_count)
graph_before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
dry=$(wgrun --json evaluate run scored --dry-run)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["eligible"] and x["mutated"] is False and x["already_recorded"] is False,x; e=x["evaluator"]; assert e["route"]=="pi:test:fake-score" and e["reasoning"]=="xhigh",e; assert x["source_terminal_observation"]["key"]["completion_receipt"].startswith("b3:"),x; assert x["evidence_digest"].startswith("b3:") and 0<x["prompt_bytes"]<=131072,x' <<<"$dry"
[[ "$count_before" == "$(eval_count)" ]] || loud_fail 'dry-run wrote evaluation'
[[ "$graph_before" == "$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail 'dry-run mutated graph'
[[ "$(grep -c SCORE_CALL "$FAKE_EVAL_LOG" || true)" == 0 ]] || loud_fail 'dry-run invoked scoring provider'

# Provider/setup failure is loud but neutral: no score row and no task mutation.
if FAKE_EVAL_FAIL=1 wgrun --json evaluate run scored >"$scratch/provider-failure.out" 2>&1; then
  loud_fail 'provider failure unexpectedly produced a score'
fi
grep -q 'WG-EVALUATION-PROVIDER-UNAVAILABLE' "$scratch/provider-failure.out" || loud_fail 'provider failure was not visible/typed'
[[ "$(eval_count)" == 0 ]] || loud_fail 'provider failure created evaluation row'
[[ "$graph_before" == "$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail 'provider failure mutated task graph'

# One successful fake-Pi call creates exactly one rich canonical Agency evaluation.
live=$(wgrun --json evaluate run scored)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["created"] is True and x["idempotent_replay"] is False,x; e=x["evaluation"]; assert e["score"]==0.83 and len(e["dimensions"])==7,e; assert e["evaluator_route"]=="pi:test:fake-score" and e["evaluator_reasoning"]=="xhigh",e; u=e["evaluator_usage"]; assert (u["input_tokens"],u["output_tokens"],u["cache_read_input_tokens"],u["cache_creation_input_tokens"])==(31,9,7,2),u; assert abs(u["cost_usd"]-0.0042)<1e-12,u; assert e["source_terminal_observation"]["completion_receipt"].startswith("b3:"),e' <<<"$live"
[[ "$(eval_count)" == 1 && "$(grep -c SCORE_CALL "$FAKE_EVAL_LOG")" == 2 ]] || loud_fail 'provider failure + live scoring did not produce one neutral failure and one successful row'
eval_file=$(find "$G/agency/evaluations" -type f -name '*.json')
eval_hash=$(sha256sum "$eval_file" | cut -d' ' -f1)
[[ "$graph_before" == "$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail 'live scoring mutated task graph'

# Reproduce the live restart corruption exactly: a long-lived TUI built before
# review_binding/activity existed observed restart graph churn, decoded only its
# older Task shape, then issued a full replacement. Immutable candidate, FLIP,
# completion, terminal-observation, and scored-evaluation receipts survived.
python3 - "$G/graph.jsonl" "$scratch/lifecycle.json" <<'PY'
import json,sys
path=sys.argv[1]; rows=[]; found=False
for line in open(path):
    row=json.loads(line)
    if row.get('id')=='scored':
        found=True
        candidate=row['completion_candidate']
        assert candidate.get('review_binding'),candidate
        assert len(row.get('completion_review_activity',[]))==2,row
        open(sys.argv[2],'w').write(json.dumps(row['lifecycle'],sort_keys=True,separators=(',',':')))
        candidate.pop('review_binding')
        row.pop('completion_review_activity')
    rows.append(row)
assert found
open(path,'w').write(''.join(json.dumps(row,separators=(',',':'))+'\n' for row in rows))
PY
corrupt_hash=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
agency_hash=$(sha256sum "$eval_file" "$observation" | sha256sum | cut -d' ' -f1)

# Dry-run verifies the exact current receipt chain but changes nothing. Apply
# repairs one row, reports invalid/skipped counts, and a second apply is a no-op.
repair_dry=$(wgrun --json migrate review-identity --limit 1 --dry-run)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert (x["affected_candidates"],x["examined"],x["repaired"],x["invalid"],x["remaining"])==(1,1,1,0,0),x' <<<"$repair_dry"
[[ "$corrupt_hash" == "$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail 'review repair dry-run mutated raw graph'
repair=$(wgrun --json migrate review-identity --limit 1)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["repaired"]==1 and x["rows"][0]["binding_restored"] and len(x["rows"][0]["activity_ids_restored"])==2,x' <<<"$repair"
repair_again=$(wgrun --json migrate review-identity --limit 1)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["affected_candidates"]==0 and x["repaired"]==0,x' <<<"$repair_again"
repaired_show=$(wgrun show scored --json)
python3 -c 'import json,sys; x=json.load(sys.stdin); c=x["completion_candidate"]; a=x["completion_review_activity"]; assert c["review_binding"]["attempt_id"],c; assert [(r["reviewer_kind"],r["verdict"],r["candidate_state"]) for r in a]==[("flip","pass","current"),("eval","pass","current")],a; assert [r["model_route"] for r in a]==["pi:test:fake-review","pi:test:fake-score"] and all(r["usage"]["cost_usd"]==0.0001 for r in a),a' <<<"$repaired_show"
wgrun log scored 'receipt-backed projection save canary' >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
x=next(json.loads(line) for line in open(sys.argv[1]) if json.loads(line).get('id')=='scored')
assert x['completion_candidate'].get('review_binding'),x
assert len(x.get('completion_review_activity',[]))==2,x
PY

# Strip it once more, then use the real service start/reload/stop path. The
# first coordinator save repairs from receipts without a model call or a new
# lifecycle event; this is the restart path that failed in production.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('id')=='scored':
        row['completion_candidate'].pop('review_binding')
        row.pop('completion_review_activity')
    rows.append(row)
open(p,'w').write(''.join(json.dumps(row,separators=(',',':'))+'\n' for row in rows))
PY
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null
for _ in $(seq 1 40); do
  if python3 - "$G/graph.jsonl" <<'PY'
import json,sys
x=next(json.loads(line) for line in open(sys.argv[1]) if json.loads(line).get('id')=='scored')
raise SystemExit(0 if x['completion_candidate'].get('review_binding') and len(x.get('completion_review_activity',[]))==2 else 1)
PY
  then break; fi
  sleep .1
done
wgrun service reload >/dev/null
wgrun service stop >/dev/null

# Strip it a third time to prove the scored-evaluation entry point itself
# reconciles before eligibility/replay, not only the running service.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get('id')=='scored':
        row['completion_candidate'].pop('review_binding')
        row.pop('completion_review_activity')
        row.pop('actual_executor')
        row.pop('actual_model')
    rows.append(row)
open(p,'w').write(''.join(json.dumps(row,separators=(',',':'))+'\n' for row in rows))
PY

# Explicit replay after repair and changed config must return the old row
# before provider invocation. Immutable evaluation/observation bytes and task
# lifecycle are unchanged.
wgrun config --local --set-model evaluator pi:test:replacement \
  --set-reasoning evaluator low --no-reload >/dev/null
replay=$(FAKE_EVAL_FAIL=1 wgrun --json evaluate run scored)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["created"] is False and x["idempotent_replay"] is True,x; e=x["evaluation"]; assert e["evaluator_route"]=="pi:test:fake-score" and e["evaluator_reasoning"]=="xhigh",e' <<<"$replay"
[[ "$(eval_count)" == 1 && "$(grep -c SCORE_CALL "$FAKE_EVAL_LOG")" == 2 ]] || loud_fail 'repair/replay/restart/reload duplicated or re-called evaluation'
[[ "$eval_hash" == "$(sha256sum "$eval_file" | cut -d' ' -f1)" ]] || loud_fail 'immutable evaluation changed across repair/replay/restart'
[[ "$agency_hash" == "$(sha256sum "$eval_file" "$observation" | sha256sum | cut -d' ' -f1)" ]] || loud_fail 'immutable evaluation/observation bytes changed'
python3 - "$G/graph.jsonl" "$scratch/lifecycle.json" <<'PY'
import json,sys
x=next(json.loads(line) for line in open(sys.argv[1]) if json.loads(line).get('id')=='scored')
assert json.dumps(x['lifecycle'],sort_keys=True,separators=(',',':'))==open(sys.argv[2]).read(),x['lifecycle']
assert x['completion_candidate'].get('review_binding'),x
assert len(x.get('completion_review_activity',[]))==2,x
PY

# All requested observation surfaces retain route, reasoning, usage/cost, dimensions, receipt, and source.
show=$(wgrun --json evaluate show scored)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert len(x["evaluations"])==1,x; e=x["evaluations"][0]; assert e["score"]==0.83 and len(e["dimensions"])==7,e; assert e["evaluator_route"]=="pi:test:fake-score" and e["evaluator_reasoning"]=="xhigh",e; assert e["evaluator_usage"]["cost_usd"]==0.0042,e; assert e["source_terminal_observation"]["observation_id"].startswith("terminal-observation-v1:"),e' <<<"$show"
stats=$(wgrun --json agency stats)
python3 -c 'import json,sys; x=json.load(sys.stdin); o=x["overview"]; assert o["total_evaluations"]==1 and o["scored_terminal_observations"]==1 and o["unscored_terminal_observations"]==0,o; e=x["scored_evaluations"][0]; assert e["score"]==0.83 and e["source_terminal_observation"]["completion_receipt"].startswith("b3:"),e' <<<"$stats"

# No evaluator graph node or retired PendingEval lifecycle was resurrected.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
assert not [row for row in rows if row.get('id','').startswith('.evaluate-')], rows
source=next(row for row in rows if row.get('id')=='scored')
assert source['status']=='done',source
PY

echo 'PASS: bounded scored terminal evaluation is exact-route, receipt-bound, rich, neutral, and idempotent'
