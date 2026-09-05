#!/usr/bin/env bash
# Credential-free end-to-end proof that completion blockers are durable,
# candidate-bound waiting states rather than source/provider failures.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# Keep daemon sockets below sockaddr_un limits even when a worker supplies a
# deeply nested owned TMPDIR.
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgs-completion-$$}"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config" "$fakebin" "$scratch/review-state"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
ln -s "$WG_BIN" "$fakebin/wg"

# One fake Pi binary serves both real isolated source workers and deterministic
# completion reviewers. Source invocations refuse replay. Reviewer outcomes are
# task/candidate ordered: Land candidate A rejects; candidate B passes FLIP and
# Eval. The budget case rejects A; B must park before another provider call.
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
model=""
while (($#)); do
  case "$1" in
    --model) model="$2"; shift 2 ;;
    *) shift ;;
  esac
done
prompt=$(cat || true)
state="${FAKE_REVIEW_STATE:?}"
task="${WG_TASK_ID:-unbound}"

case "$model" in
  fake-review|openrouter:fake-review)
    if grep -q 'worksgood-flip-blind-inference-v1' <<<"$prompt"; then
      if grep -q 'Produce resilient.txt\|exact candidate B is reviewed\|worker_summary\|requirements_digest' <<<"$(sed -n '/BEGIN BLIND CANDIDATE EVIDENCE/,/END BLIND CANDIDATE EVIDENCE/p' <<<"$prompt")"; then
        echo "phase-I canonical input leaked forbidden original-intent fields" >&2; exit 96
      fi
      printf '%s|inference|%s\n' "$task" "$model" >>"$state/review-calls"
      python3 - <<'PY'
import json
text=json.dumps({'goal':'reconstructed fixture goal','constraints':[],'invariants':[],'failure_modes':[]},separators=(',',':'))
event={'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
       'provider':'test','model':'fake-review','stopReason':'stop','rawStopReason':'completed',
       'usage':{'input':1,'output':1,'cacheRead':0,'cacheWrite':0,'totalTokens':2,
                'cost':{'total':0}}}}
print(json.dumps(event,separators=(',',':')))
PY
      exit 0
    fi
    count_file="$state/$task.review-count"
    n=$(($(cat "$count_file" 2>/dev/null || echo 0) + 1))
    printf '%s\n' "$n" >"$count_file"
    phase=eval
    grep -q 'worksgood-flip-comparison-v1' <<<"$prompt" && phase=comparison
    printf '%s|%s|%s|%s\n' "$task" "$phase" "$n" "$model" >>"$state/review-calls"
    verdict=pass; code=""; message=""
    if [[ "$task" == land-resilient && "$n" == 1 ]]; then
      verdict=reject; code=fixture.candidate_a; message="revise candidate A"
    elif [[ "$task" == budget-resilient ]]; then
      verdict=reject; code=fixture.budget_a; message="repair budget candidate A"
    fi
    python3 - "$verdict" "$code" "$message" <<'PY'
import json,sys
verdict,code,message=sys.argv[1:]
findings=[] if verdict == 'pass' else [{'code':code,'message':message}]
text=json.dumps({'verdict':verdict,'findings':findings},separators=(',',':'))
event={'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
       'provider':'test','model':'fake-review','stopReason':'stop','rawStopReason':'completed',
       'usage':{'input':1,'output':1,'cacheRead':0,'cacheWrite':0,'totalTokens':2,
                'cost':{'total':0}}}}
print(json.dumps(event,separators=(',',':')))
PY
    exit 0
    ;;
  fake-worker|openrouter:fake-worker)
    count_file="$state/$task.source-count"
    n=$(($(cat "$count_file" 2>/dev/null || echo 0) + 1))
    printf '%s\n' "$n" >"$count_file"
    printf '%s|%s|%s\n' "$task" "$n" "$PWD" >>"$state/source-calls"
    [[ "$n" == 1 ]] || { echo "unchanged source was rerun for $task" >&2; exit 91; }
    case "$task" in
      land-resilient)
        printf '%s\n' "$PWD" >"$HOME/land-worker-pwd"
        printf 'candidate A\n' > resilient.txt
        git add resilient.txt && git commit -qm 'land candidate A'
        if wg done land-resilient >"$HOME/land-a.out" 2>"$HOME/land-a.err"; then
          echo "candidate A unexpectedly passed strict review" >&2; exit 92
        fi
        : >"$HOME/land-a-rejected"
        while [[ ! -e "$HOME/release-land-b" ]]; do sleep .05; done
        printf 'candidate B\n' > resilient.txt
        git add resilient.txt && git commit -qm 'land candidate B'
        git rev-parse HEAD >"$HOME/land-b-oid"
        wg done land-resilient >"$HOME/land-b.out" 2>"$HOME/land-b.err"
        : >"$HOME/land-b-blocked"
        ;;
      budget-resilient)
        printf '%s\n' "$PWD" >"$HOME/budget-worker-pwd"
        printf 'budget candidate A\n' > budget.txt
        git add budget.txt && git commit -qm 'budget candidate A'
        if wg done budget-resilient >"$HOME/budget-a.out" 2>"$HOME/budget-a.err"; then
          echo "budget candidate A unexpectedly passed strict review" >&2; exit 93
        fi
        : >"$HOME/budget-a-rejected"
        while [[ ! -e "$HOME/release-budget-b" ]]; do sleep .05; done
        printf 'budget candidate B\n' > budget.txt
        git add budget.txt && git commit -qm 'budget candidate B'
        git rev-parse HEAD >"$HOME/budget-b-oid"
        wg done budget-resilient >"$HOME/budget-b.out" 2>"$HOME/budget-b.err"
        : >"$HOME/budget-needs-review"
        ;;
      *) echo "unexpected source task $task" >&2; exit 94 ;;
    esac
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"source attempt settled"}],"provider":"test","model":"fake-worker","stopReason":"stop","rawStopReason":"completed","usage":{"input":2,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"total":0}}}}'
    ;;
  *) echo "unexpected fake Pi model: $model" >&2; exit 95 ;;
esac
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_REVIEW_STATE="$scratch/review-state"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT \
  WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH \
  OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY || true

cd "$project"
git init -q -b main
git config user.email completion-resilience@test.invalid
git config user.name CompletionResilience
printf 'base bytes\n' > base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
# The retained worker checkout is WG-owned orchestration state, not user work.
# Keep it out of the attached integration checkout's untracked-byte test so the
# only deliberate dirt below is the unrelated tracked edit under test.
printf '/.wg-worktrees/\n/daemon.log\n' >>.git/info/exclude
G="$project/.wg"
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
  "$WG_BIN" --dir "$G" "$@"; }

wgrun setup --route pi --model pi:openrouter:fake-worker --yes >/dev/null
wgrun config set models.reviewer.model pi:openrouter:fake-review >/dev/null
wgrun config set models.reviewer.reasoning low >/dev/null
wgrun config set models.flip_inference.model pi:openrouter:fake-review >/dev/null
wgrun config set models.flip_inference.reasoning low >/dev/null
wgrun config set models.flip_comparison.model pi:openrouter:fake-review >/dev/null
wgrun config set models.flip_comparison.reasoning low >/dev/null
wgrun config set models.evaluator.model pi:openrouter:fake-review >/dev/null
wgrun config set models.evaluator.reasoning low >/dev/null
wgrun config set agency.auto_assign false >/dev/null
wgrun config set agency.auto_evaluate false >/dev/null
wgrun config set agency.completion_review_strict true >/dev/null
wgrun config set agency.gate_max_attempts 2 >/dev/null
wgrun config set dispatcher.max_agents 1 >/dev/null
wgrun config set dispatcher.poll_interval 1 >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation true >/dev/null
wgrun config set dispatcher.resource_management.disk_sentinel_enabled false >/dev/null
git add .gitignore AGENTS.md CLAUDE.md worksgood.toml && git commit -qm init-wg

wait_file(){
  local path="$1" label="$2"
  for _ in $(seq 1 400); do [[ -e "$path" ]] && return 0; sleep .05; done
  local tasks service daemon
  tasks=$(wgrun list --all 2>&1)
  service=$(wgrun service status 2>&1)
  daemon=$(tail -30 "$project/daemon.log" 2>/dev/null || true)
  loud_fail "timed out waiting for $label: tasks=$tasks service=$service daemon=$daemon"
}
stop_daemon(){
  wgrun service stop >/dev/null
  for _ in $(seq 1 100); do
    [[ ! -e "$G/service/state.json" ]] && return 0
    sleep .05
  done
  loud_fail "daemon state did not clear before restart"
}
agent_for(){
  python3 - "$G/service/registry.json" "$1" <<'PY'
import json,sys
try: x=json.load(open(sys.argv[1]))
except Exception: print(''); raise SystemExit
print(next((a['id'] for a in x.get('agents',{}).values() if a.get('task_id')==sys.argv[2]),''))
PY
}
wait_settled_stream(){
  local task="$1" agent=""
  for _ in $(seq 1 400); do
    agent=$(agent_for "$task" 2>/dev/null || true)
    if [[ -n "$agent" && -f "$G/agents/$agent/raw_stream.jsonl" ]] \
      && grep -q '"rawStopReason":"completed"' "$G/agents/$agent/raw_stream.jsonl"; then
      printf '%s\n' "$agent"; return 0
    fi
    sleep .05
  done
  loud_fail "Pi source attempt for $task never produced an exact terminal receipt"
}
manifest_commit(){
  python3 - "$1" "$G/completion/v3/objects" <<'PY'
import json,pathlib,sys
x=json.load(open(sys.argv[1])); objects=pathlib.Path(sys.argv[2])
ref=x['completion_candidate']['manifest']['content_digest'].removeprefix('b3:')
m=json.loads((objects/ref).read_text())
commits=[o['commit_oid'] for o in m['outputs'] if o.get('kind')=='git']
assert len(commits)==1,(x,m)
assert m['source_revision']==commits[0],(x,m)
print(commits[0])
PY
}

# Case 1: real daemon dispatch creates a retained isolated worker. Candidate A
# rejects. Candidate B receives exact FLIP+Eval passes, but unrelated tracked
# user bytes in the attached main checkout force LandingPending.
wgrun add 'Resumable dirty-checkout landing' --id land-resilient --priority 100 \
  -d $'Produce resilient.txt.\n\n## Validation\n- [ ] exact candidate B is reviewed and landed once' >/dev/null
wgrun publish land-resilient --only >/dev/null
main_before=$(git rev-parse refs/heads/main)
base_before=$(sha256sum base.txt | cut -d' ' -f1)
start_wg_daemon "$project" --no-chat-agent --interval 1
wait_file "$home/land-a-rejected" "candidate A rejection"
wgrun show land-resilient --json >"$scratch/land-a.json"
python3 - "$scratch/land-a.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); a=x['completion_review_activity']
assert x['status']=='in-progress',x
assert len(a)==1 and a[0]['reviewer_kind']=='flip' and a[0]['verdict']=='reject',a
assert a[0]['findings'][0]['code']=='fixture.candidate_a',a
assert not x.get('failure_reason') and x.get('retry_count',0)==0,x
PY
candidate_a=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["completion_candidate"]["manifest"]["content_digest"])' "$scratch/land-a.json")
worker_pwd=$(cat "$home/land-worker-pwd")
[[ "$worker_pwd" != "$project" && "$worker_pwd" == *".wg-worktrees/"* ]] \
  || loud_fail "Land source did not run in an isolated worker worktree: $worker_pwd"

printf 'unrelated tracked user edit -- preserve exactly\n' > base.txt
user_dirty_digest=$(sha256sum base.txt | cut -d' ' -f1)
touch "$home/release-land-b"
wait_file "$home/land-b-blocked" "candidate B LandingPending"
land_agent=$(wait_settled_stream land-resilient)
for _ in $(seq 1 100); do
  [[ $(wgrun show land-resilient --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == waiting ]] && break
  sleep .05
done
wgrun show land-resilient --json >"$scratch/land-pending.json"
wgrun show land-resilient >"$scratch/land-pending.show"
wgrun status >"$scratch/land-pending.status"
grep -qi 'Landing pending\|LandingPending' "$scratch/land-pending.show" \
  || loud_fail "terminal show omitted LandingPending"
grep -qi 'waiting\|landing' "$scratch/land-pending.status" \
  || loud_fail "terminal status omitted waiting landing"
[[ $(sha256sum base.txt | cut -d' ' -f1) == "$user_dirty_digest" ]] \
  || loud_fail "dirty landing changed unrelated tracked user bytes"
[[ $(git rev-parse refs/heads/main) == "$main_before" ]] \
  || loud_fail "dirty landing moved main despite LandingPending"

candidate_b=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["completion_candidate"]["manifest"]["content_digest"])' "$scratch/land-pending.json")
[[ "$candidate_a" != "$candidate_b" ]] || loud_fail "candidate A and B have the same immutable digest"
land_b_oid=$(cat "$home/land-b-oid")
[[ $(manifest_commit "$scratch/land-pending.json") == "$land_b_oid" ]] \
  || loud_fail "selected candidate B manifest does not bind the worker commit"
python3 - "$scratch/land-pending.json" "$main_before" "$worker_pwd" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); expected_main=sys.argv[2]; worker=sys.argv[3]
a=x['completion_review_activity']; c=x['completion_candidate']; b=x['completion_blocker']
assert x['status']=='waiting' and b['kind']=='landing-pending',x
assert b['target_ref_oid']==expected_main and b['integration_ref']=='refs/heads/main',b
assert b['worker_worktree']==worker,b
assert not x.get('failure_reason') and x.get('retry_count',0)==0,x
assert [(r['reviewer_kind'],r['verdict']) for r in a]==[
  ('flip','reject'),('flip','pass'),('eval','pass')],a
assert [r['binding']['candidate_sequence'] for r in a]==[1,2,2],a
assert len({(r['manifest_digest'],r['reviewer_kind']) for r in a})==len(a),a
assert c['review_binding']==a[-1]['binding']==a[-2]['binding'],(c,a)
assert c['manifest']['content_digest']==a[-1]['manifest_digest'],(c,a)
PY
[[ $(cat "$scratch/review-state/land-resilient.review-count") == 3 ]] \
  || loud_fail "candidate A/B did not make exactly reject, FLIP pass, Eval pass review calls"
python3 - "$scratch/review-state/review-calls" <<'PY'
import sys
rows=[line.strip().split('|') for line in open(sys.argv[1]) if line.startswith('land-resilient|')]
assert [r[1] for r in rows]==['inference','comparison','inference','comparison','eval'],rows
assert rows[0][2]==rows[2][2]=='fake-review',rows
assert [r[2] for r in (rows[1],rows[3],rows[4])]==['1','2','3'],rows
assert [r[3] for r in (rows[1],rows[3],rows[4])]==['fake-review']*3,rows
PY
[[ $(cat "$scratch/review-state/land-resilient.source-count") == 1 ]] \
  || loud_fail "Land source was rerun"

# Restart while the checkout is still dirty. The exact candidate, review
# binding, target-ref CAS and user bytes must survive. Leave dispatch enabled:
# the durable wait itself, not a disabled dispatcher, must prevent source replay.
# Stop it before the explicit resume so exactly one operator resume owns publication.
stop_daemon
review_calls_before_restart=$(sha256sum "$scratch/review-state/review-calls" | cut -d' ' -f1)
start_wg_daemon "$project" --no-chat-agent --max-agents 1 --interval 1
sleep 1.2
wgrun show land-resilient --json >"$scratch/land-restarted.json"
python3 - "$scratch/land-pending.json" "$scratch/land-restarted.json" <<'PY'
import json,sys
before,after=map(lambda p:json.load(open(p)),sys.argv[1:])
for key in ('status','completion_candidate','completion_blocker','completion_review_activity'):
    assert before[key]==after[key],(key,before[key],after[key])
assert not after.get('failure_reason') and after.get('retry_count',0)==0,after
PY
[[ $(sha256sum base.txt | cut -d' ' -f1) == "$user_dirty_digest" ]] \
  || loud_fail "restart/finalizer retry changed unrelated user bytes"
[[ $(sha256sum "$scratch/review-state/review-calls" | cut -d' ' -f1) == "$review_calls_before_restart" ]] \
  || loud_fail "restart repeated semantic review for the same candidate digest"
stop_daemon
git restore -- base.txt
[[ $(sha256sum base.txt | cut -d' ' -f1) == "$base_before" ]] || loud_fail "operator cleanup did not restore base bytes"
[[ -z $(git status --porcelain) ]] \
  || loud_fail "integration checkout was not clean before resume: $(git status --short)"

wgrun resume land-resilient --only >"$scratch/land-resume.out" 2>"$scratch/land-resume.err"
wgrun show land-resilient --json >"$scratch/land-done.json"
actual_main=$(git rev-parse refs/heads/main)
resume_stdout=$(tr '\n' ' ' <"$scratch/land-resume.out")
resume_stderr=$(tr '\n' ' ' <"$scratch/land-resume.err")
[[ "$actual_main" == "$land_b_oid" ]] \
  || loud_fail "resume did not publish exact candidate B (expected=$land_b_oid actual=$actual_main stdout=$resume_stdout stderr=$resume_stderr)"
[[ $(cat resilient.txt) == 'candidate B' ]] || loud_fail "landed worktree does not contain candidate B bytes"
python3 - "$scratch/land-done.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); events=x['lifecycle']['audit']; logs=x['log']
assert x['status']=='done' and x['completion_disposition']=='landed',x
assert x.get('completion_blocker') is None and x['completion_receipt'].startswith('b3:'),x
assert sum(e['event_kind']=='attempt-succeeded' for e in events)==1,events
assert sum(e.get('reason_code')=='reviewed_publication_committed' for e in events)==1,events
assert sum(row.get('actor')=='land' for row in logs)==1,logs
assert not x.get('failure_reason') and x.get('retry_count',0)==0,x
PY
[[ $(cat "$scratch/review-state/land-resilient.review-count") == 3 ]] \
  || loud_fail "resume repeated candidate B review"
[[ $(cat "$scratch/review-state/land-resilient.source-count") == 1 ]] \
  || loud_fail "resume reran unchanged Land source"
classify=$(wgrun classify-failure --terminal --json --executor pi \
  --raw-stream "$G/agents/$land_agent/raw_stream.jsonl" --exit-code 1)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["state"]=="completed" and "failure_reason" not in x,x' <<<"$classify"

# Case 2: one rejected immutable candidate exhausts a one-revision strict
# budget. A materially revised B is selected, but no reviewer is called. The
# real Pi attempt then settles normally and the wrapper leaves NeedsReview,
# never Failed/provider-timeout and never source-retried.
wgrun config set agency.gate_max_attempts 1 >/dev/null
wgrun add 'Budget exhaustion is resumable review work' --id budget-resilient --priority 100 \
  -d $'Produce budget.txt.\n\n## Validation\n- [ ] strict budget exhaustion parks NeedsReview' >/dev/null
wgrun publish budget-resilient --only >/dev/null
start_wg_daemon "$project" --no-chat-agent --max-agents 1 --interval 1
wait_file "$home/budget-a-rejected" "budget candidate A rejection"
wgrun show budget-resilient --json >"$scratch/budget-a.json"
budget_a=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["completion_candidate"]["manifest"]["content_digest"])' "$scratch/budget-a.json")
touch "$home/release-budget-b"
wait_file "$home/budget-needs-review" "NeedsReview parking"
budget_agent=$(wait_settled_stream budget-resilient)
for _ in $(seq 1 100); do
  [[ $(wgrun show budget-resilient --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])') == waiting ]] && break
  sleep .05
done
wgrun show budget-resilient --json >"$scratch/budget-waiting.json"
wgrun show budget-resilient >"$scratch/budget-waiting.show"
wgrun status >"$scratch/budget-waiting.status"
grep -qi 'Needs review\|NeedsReview' "$scratch/budget-waiting.show" \
  || loud_fail "terminal show omitted NeedsReview"
grep -qi 'waiting\|review' "$scratch/budget-waiting.status" \
  || loud_fail "terminal status omitted NeedsReview wait"
budget_b=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["completion_candidate"]["manifest"]["content_digest"])' "$scratch/budget-waiting.json")
[[ "$budget_a" != "$budget_b" ]] || loud_fail "budget repair did not select a distinct candidate B"
[[ $(manifest_commit "$scratch/budget-waiting.json") == "$(cat "$home/budget-b-oid")" ]] \
  || loud_fail "NeedsReview did not retain exact revised candidate B"
python3 - "$scratch/budget-waiting.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); a=x['completion_review_activity']; b=x['completion_blocker']; c=x['completion_candidate']
assert x['status']=='waiting' and x.get('assigned') is None,x
assert b['kind']=='needs-review' and 'semantic review ceiling 1/1' in b['reason'],b
assert len(a)==1 and a[0]['reviewer_kind']=='flip' and a[0]['verdict']=='reject',a
assert a[0]['findings'][0]['code']=='fixture.budget_a',a
assert c['manifest']['content_digest'] != a[0]['manifest_digest'],(c,a)
assert c['review_binding']['candidate_sequence']==2,c
assert c.get('flip_receipt') is None and c.get('eval_receipt') is None,c
assert not x.get('failure_reason') and not x.get('failure_class'),x
assert x.get('retry_count',0)==0,x
assert any('Completion waiting/NeedsReview' in row['message'] for row in x['log']),x['log']
PY
[[ $(cat "$scratch/review-state/budget-resilient.review-count") == 1 ]] \
  || loud_fail "budget exhaustion called the semantic reviewer again for candidate B"
[[ $(cat "$scratch/review-state/budget-resilient.source-count") == 1 ]] \
  || loud_fail "NeedsReview reran the settled Pi source"
classify=$(wgrun classify-failure --terminal --json --executor pi \
  --raw-stream "$G/agents/$budget_agent/raw_stream.jsonl" --exit-code 1)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["state"]=="completed" and "failure_reason" not in x,x' <<<"$classify"
# Neither task may have acquired provider-timeout/source-failure reporting.
if grep -Eqi 'provider[- ]timeout|failure_class[^\n]*(timeout|provider)' \
  "$scratch/land-done.json" "$scratch/budget-waiting.json"; then
  loud_fail "ordinary completion blockers were reported as provider timeout/failure"
fi

# Restart the already-settled budget case once more: exact B and NeedsReview
# remain byte-stable, with no source/review invocation.
stop_daemon
calls_before=$(sha256sum "$scratch/review-state/review-calls" "$scratch/review-state/source-calls")
start_wg_daemon "$project" --no-chat-agent --max-agents 1 --interval 1
sleep 1
wgrun show budget-resilient --json >"$scratch/budget-restarted.json"
python3 - "$scratch/budget-waiting.json" "$scratch/budget-restarted.json" <<'PY'
import json,sys
before,after=map(lambda p:json.load(open(p)),sys.argv[1:])
for key in ('status','completion_candidate','completion_blocker','completion_review_activity'):
    assert before[key]==after[key],(key,before[key],after[key])
assert not after.get('failure_reason') and after.get('retry_count',0)==0,after
PY
[[ "$calls_before" == "$(sha256sum "$scratch/review-state/review-calls" "$scratch/review-state/source-calls")" ]] \
  || loud_fail "restart reran source or repeated semantic review"

echo 'PASS: exact completion candidates survive strict rejection, dirty landing, restart/resume, and review-budget exhaustion without source/provider failure'
