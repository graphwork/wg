#!/usr/bin/env bash
# Installed-binary terminal flow for the append-only adaptive review/learning ledger.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then WG_BIN="$WG_SMOKE_CANDIDATE_BIN"; else
  export CARGO_TARGET_DIR="$scratch/target"
  (cd "$repo_root" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home" "$fakebin"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
state="${FAKE_PI_STATE:?}"; n=0; [[ -f "$state" ]] && n=$(cat "$state"); n=$((n+1)); printf '%s' "$n" >"$state"
if [[ "$n" == 1 ]]; then sleep 3; exit 0; fi
if [[ "$n" == 2 ]]; then verdict=reject; else verdict=pass; fi
python3 - "$verdict" "$n" <<'PY'
import json,sys
verdict,n=sys.argv[1],int(sys.argv[2])
findings=[{'code':'fixture.semantic_reject','message':'candidate A misses the terminal requirement'}] if verdict=='reject' else []
text=json.dumps({'verdict':verdict,'findings':findings},separators=(',',':'))
event={'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
       'provider':'test','model':'adaptive-review','stopReason':'stop','usage':{'input':n,'output':1,
       'cacheRead':0,'cacheWrite':0,'totalTokens':n+1,'cost':{'total':n/1000}}}}
print(json.dumps(event,separators=(',',':')))
PY
FAKE_PI
chmod +x "$fakebin/pi"
export HOME="$home" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH" FAKE_PI_STATE="$scratch/pi-calls"
export WG_COMPLETION_REVIEW_TIMEOUT_SECS=1
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
cd "$project"
git init -q -b main; git config user.email adaptive@test.invalid; git config user.name Adaptive
printf 'base\n' >base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"; wgrun(){ "$WG_BIN" --dir "$G" "$@"; }
wgrun config --local --model pi:test:adaptive-review --reasoning low --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:adaptive-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:adaptive-review --set-reasoning evaluator low --no-reload >/dev/null
wgrun add 'Adaptive ledger demo' --id adaptive-demo -d $'Produce a reviewed report.\n\n## Validation\n- [ ] terminal review is append-only' >/dev/null
wgrun contract adaptive-demo report >/dev/null
wgrun publish adaptive-demo --only >/dev/null
wgrun claim adaptive-demo --actor fixture-worker >/dev/null

printf 'candidate A\n' >summary.txt; printf 'incomplete A\n' >report.txt; printf 'validation A\n' >validation.log
wgrun completion-object report.txt --media-type text/plain >output-ref.json
wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
wgrun completion-manifest adaptive-demo --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
# Live FLIP attempt 1 times out (infrastructure only); exact retry 2 rejects semantically.
wgrun submit adaptive-demo --manifest manifest.json --summary summary.txt >"$scratch/timeout.log" 2>&1
grep -q 'ReviewUnavailable' "$scratch/timeout.log" || loud_fail "live timeout was not surfaced"
wgrun submit adaptive-demo --manifest manifest.json --summary summary.txt >"$scratch/reject.log" 2>&1
grep -q 'FlipRejected' "$scratch/reject.log" || loud_fail "semantic reject was not surfaced"
# Candidate B is distinct and supersedes A; FLIP and Eval both pass.
printf 'candidate B repaired\n' >summary.txt; printf 'complete B\n' >report.txt; printf 'validation B\n' >validation.log
wgrun completion-object report.txt --media-type text/plain >output-ref.json
wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
wgrun completion-manifest adaptive-demo --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
wgrun submit adaptive-demo --manifest manifest.json --summary summary.txt >/dev/null
wgrun done adaptive-demo >/dev/null

reviews=$(wgrun --json reviews list adaptive-demo --candidate all)
python3 -c 'import json,sys; a=json.load(sys.stdin); assert len(a)==4,a; assert [x["ordinal"] for x in a[:2]]==[1,2],a; assert a[0]["outcome"]["class"]=="infrastructure" and a[1]["outcome"]=={"class":"semantic","outcome":"reject"},a; assert [x["current_candidate"] for x in a]==[False,False,True,True],a; assert {x["reviewer_kind"] for x in a[2:]}=={"flip","eval"},a; assert all(x["route"]["exact_route"]=="pi:test:adaptive-review" and x["route"]["reasoning"]=="low" for x in a),a' <<<"$reviews"
alias=$(python3 -c 'import json,sys; print(json.load(sys.stdin)[1]["alias"])' <<<"$reviews")
show=$(wgrun reviews show "$alias")
grep -q 'VIRTUAL REVIEW — not a graph task' <<<"$show" || loud_fail "virtual non-task banner missing"
grep -q 'Candidate: superseded' <<<"$show" || loud_fail "superseded history missing"
grep -q 'fixture.semantic_reject: candidate A misses the terminal requirement' <<<"$show" || loud_fail "bounded immutable finding detail missing"

learning=$(wgrun --json learning show adaptive-demo)
python3 -c 'import json,sys; e=json.load(sys.stdin); assert e["task_id"]=="adaptive-demo"; assert e["semantic_trajectory"]=={"passes":2,"rejects":1,"inconclusive":0,"candidate_count":2},e; assert e["infrastructure_summary"]["attempts"]==1,e' <<<"$learning"
[[ $(find "$G/agency/adaptive/v1/terminal-episodes" -name '*.json' | wc -l) -eq 1 ]] || loud_fail "learning episode was not exactly once"
wgrun learning show adaptive-demo >/dev/null
[[ $(find "$G/agency/adaptive/v1/terminal-episodes" -name '*.json' | wc -l) -eq 1 ]] || loud_fail "learning replay duplicated episode"

spend=$(wgrun --json spend)
python3 -c 'import json,sys; a=json.load(sys.stdin)["adaptive_agency"]; assert a["completion_flip"]["attempt_count"]==3,a; assert a["completion_eval"]["attempt_count"]==1,a; assert a["completion_flip"]["unknown_cost_attempts"]==1,a; assert abs(a["all_agency_provider_cost"]-0.009)<1e-9,a' <<<"$spend"
status=$(wgrun --json status)
python3 -c 'import json,sys; a=json.load(sys.stdin)["adaptive"]; assert a["review_attempts"]==4 and a["terminal_episodes"]==1,a' <<<"$status"
list=$(wgrun list --all)
grep -q 'adaptive virtual reviews (non-schedulable; no graph edges or lifecycle authority)' <<<"$list" || loud_fail "list virtual projection banner missing"
# No historical/synthetic agency task or edge was created.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1])]
assert [x['id'] for x in rows]==['adaptive-demo'],rows
assert not rows[0].get('after'),rows[0]
PY
before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
if wgrun retry "$alias" >"$scratch/mutate.log" 2>&1; then loud_fail "virtual alias retry unexpectedly succeeded"; fi
grep -q 'WG-VIRTUAL-REVIEW-NON-AUTHORITATIVE' "$scratch/mutate.log" || loud_fail "typed mutation refusal missing"
after=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
[[ "$before" == "$after" ]] || loud_fail "virtual alias mutation changed source graph"
echo 'PASS: live FLIP/Eval attempts, cost, findings, supersession, virtual aliases, and one terminal learning episode'
