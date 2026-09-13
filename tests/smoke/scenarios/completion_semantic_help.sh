#!/usr/bin/env bash
# Credential-free real-CLI regression for explicit help after semantic rejection.
# The fake reviewer is a deterministic fixture, not live semantic proof.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"

if [[ -z "${WG_SMOKE_HARNESS_RUN_ID:-}" \
    || "${WG_SMOKE_SUBREAPER_TOKEN:-}" != "$WG_SMOKE_HARNESS_RUN_ID" ]]; then
    export WG_SMOKE_DIRECT_SCENARIO="$HERE/completion_semantic_help.sh"
    cd "$ROOT"
    exec cargo test --locked --lib \
        smoke::tests::run_explicit_scenario_under_process_harness \
        -- --exact --ignored --nocapture
fi

. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$repo" "$home/.config" "$fakebin"
WG_BIN="${WG_BIN:-${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
# _helpers.sh intentionally invokes `wg`; bind that name to the exact candidate
# so daemon restart evidence cannot accidentally come from a global install.
wg(){ "$WG_BIN" "$@"; }

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
: "${FAKE_REVIEW_STATE:?}"
args="$*"
if [[ " $args " == *" --list-models "* ]]; then
  printf 'provider model context max-out thinking images\n'
  printf 'test controlled-review 128K 16K yes no\n'
  exit 0
fi
prompt="$args $(cat || true)"
n=$(($(cat "$FAKE_REVIEW_STATE.count" 2>/dev/null || echo 0)+1))
printf '%s\n' "$n" >"$FAKE_REVIEW_STATE.count"
mode=$(cat "$FAKE_REVIEW_STATE.mode" 2>/dev/null || echo reject)
if [[ "$prompt" == *"FLIP PHASE II"* ]]; then
  if [[ "$mode" == reject ]]; then
    response='{"verdict":"reject","findings":[{"code":"fixture.missing-authority","message":"candidate lacks the operator-approved exact validation evidence"}]}'
  else
    response='{"verdict":"pass","findings":[]}'
  fi
elif [[ "$prompt" == *"FLIP PHASE I"* ]]; then
  response='{"goal":"controlled reconstructed intent","constraints":[],"invariants":[],"failure_modes":[]}'
elif [[ "$mode" == reject ]]; then
  response='{"verdict":"reject","findings":[{"code":"fixture.missing-authority","message":"candidate lacks the operator-approved exact validation evidence"}]}'
else
  response='{"verdict":"pass","findings":[]}'
fi
python3 - "$response" <<'PY'
import json,sys
print(json.dumps({"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":sys.argv[1]}],"provider":"test","model":"controlled-review","stopReason":"stop","usage":{"input":2,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"total":0.0001}}}}))
PY
SH
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_REVIEW_STATE="$scratch/review"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_GRAPH_ID WG_PROJECT_ROOT WG_WORKTREE_PATH \
  WG_WORKTREE_ACTIVE WG_BRANCH WG_WORKER_ATTEMPT_ID WG_WORKER_ATTEMPT_FENCE \
  WG_WORKER_GENERATION WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_WORKER_CONTROL_MODE || true

cd "$repo"
git init -q -b main
git config user.email semantic-help@test.invalid
git config user.name "Semantic Help"
echo base > base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID WG_DIR="$repo/.wg" "$WG_BIN" "$@"; }
worker(){
  local task="$1" agent="$2" binding
  shift 2
  binding=$(wgrun show "$task" --json | python3 -c \
    'import json,sys; x=json.load(sys.stdin)["lifecycle"]; a=x["current_attempt"]; print(x["generation"],a["id"],x["fence"])')
  read -r generation attempt fence <<<"$binding"
  env WG_TASK_ID="$task" WG_AGENT_ID="$agent" WG_WORKTREE_PATH="$repo" \
    WG_WORKER_CONTROL_MODE=trusted WG_WORKER_GENERATION="$generation" \
    WG_WORKER_ATTEMPT_ID="$attempt" WG_WORKER_ATTEMPT_FENCE="$fence" \
    "$WG_BIN" --dir "$repo/.wg" "$@"
}

cat >"$repo/.wg/config.toml" <<'TOML'
[agent]
model = "pi:test:controlled-review"

[models.default]
model = "pi:test:controlled-review"
reasoning = "high"

[models.reviewer]
model = "pi:test:controlled-review"
reasoning = "low"

[models.flip_inference]
model = "pi:test:controlled-review"
reasoning = "low"

[models.flip_comparison]
model = "pi:test:controlled-review"
reasoning = "low"

[models.evaluator]
model = "pi:test:controlled-review"
reasoning = "low"

[agency]
auto_assign = false
auto_evaluate = false
flip_enabled = true
completion_review_strict = true
TOML

wgrun add "Semantic contract correction" --id semantic-correction \
  --validation-command "test -s result.txt" \
  -d $'Produce result.txt.\n\n## Validation\n- [ ] exact operator-approved evidence is required' >/dev/null
wgrun publish semantic-correction --only >/dev/null
wgrun claim semantic-correction --actor semantic-worker >/dev/null
git switch -qc worker/semantic-correction
echo result > result.txt
git add result.txt && git commit -qm semantic-candidate

if worker semantic-correction semantic-worker done semantic-correction >"$scratch/reject.out" 2>"$scratch/reject.err"; then
  loud_fail "strict FLIP rejection accepted the candidate"
fi
grep -q 'FLIP semantically rejected' "$scratch/reject.err" \
  || loud_fail "semantic rejection was not visible: $(cat "$scratch/reject.err")"
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "controlled two-phase FLIP fixture call count mismatch"
wgrun show semantic-correction --json >"$scratch/rejected.json"

# Wrong-task worker authority is rejected before graph mutation.
if env WG_TASK_ID=some-other-task WG_AGENT_ID=semantic-worker WG_WORKTREE_PATH="$repo" \
  WG_WORKER_CONTROL_MODE=trusted WG_WORKER_GENERATION=0 \
  WG_WORKER_ATTEMPT_ID=attempt-0-1 WG_WORKER_ATTEMPT_FENCE=1 \
  "$WG_BIN" --dir "$repo/.wg" fail semantic-correction --intent request-contract-correction \
  --reason 'wrong-task proposal' >"$scratch/wrong.out" 2>"$scratch/wrong.err"; then
  loud_fail "wrong-task worker wrote semantic attention"
fi

worker semantic-correction semantic-worker fail semantic-correction \
  --intent request-contract-correction \
  --reason 'approve proof.txt; api_key=must-not-leak' >"$scratch/help.out"
grep -q "Saved work: $repo" "$scratch/help.out" || loud_fail "help output omitted saved-work location"
# Lost-response replay and wrapper exit retain one event and do not call a reviewer.
worker semantic-correction semantic-worker fail semantic-correction \
  --intent request-contract-correction \
  --reason 'approve proof.txt; api_key=must-not-leak' >/dev/null
worker semantic-correction semantic-worker fail semantic-correction \
  --reason 'worker exited after explicit request' >/dev/null
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "help request reran unchanged reviewer"

wgrun show semantic-correction --json >"$scratch/help.json"
wgrun status --json >"$scratch/status.json"
python3 - "$scratch/rejected.json" "$scratch/help.json" "$scratch/status.json" "$repo" <<'PY'
import json,sys
before=json.load(open(sys.argv[1])); x=json.load(open(sys.argv[2])); status=json.load(open(sys.argv[3])); repo=sys.argv[4]
r=x['completion_repair']; c=x['completion_candidate']; rows=x['completion_review_activity']
assert x['status']=='in-progress' and x.get('completion_receipt') is None,x
assert c==before['completion_candidate'],(before,c)
assert len(rows)==1 and rows[0]['candidate_state']=='current' and rows[0]['verdict']=='reject',rows
b=c['review_binding']; s=r['semantic_review']
assert r['disposition']=='needs-attention' and r['reason_code']=='contract-correction-required',r
assert r['blocker_reason_code']=='flip-semantic-rejection' and r['exit_category']=='semantic-rejection',r
assert (r['task_id'],r['generation'],r['attempt_id'],r['fence']) == (b['task_id'],b['generation'],b['attempt_id'],b['attempt_fence']),r
assert r['candidate_identity']==c['manifest']['content_digest'],r
assert s['reviewer_kind']=='flip' and s['candidate_sequence']==b['candidate_sequence'],s
assert s['review_receipt']==rows[0]['activity_id']==r['evidence']['content_digest'],(s,rows,r)
assert sum('NeedsAttention event=' in row['message'] for row in x['log'])==1,x['log']
assert any('saved_work='+repo in row['message'] for row in x['log']),x['log']
assert 'must-not-leak' not in json.dumps(x),x
chains=[row for row in status['stalled_chains'] if row['root_task_id']=='semantic-correction']
assert len(chains)==1 and chains[0]['root_blocker']=='flip-semantic-rejection: semantic-rejection',chains
assert 'wg contract semantic-correction --add-validation-command' in chains[0]['safe_operator_action'],chains
open(sys.argv[2]+'.event','w').write(r['attention_event_id'])
open(sys.argv[2]+'.requirements','w').write(r['requirements_digest'])
open(sys.argv[2]+'.receipt','w').write(s['review_receipt'])
PY

# A real daemon stop/start must not duplicate or lose the primary attention.
# Keep the Unix socket below sun_path's small fixed limit even when the smoke
# harness itself lives under a long target-directory path.
short_socket="/tmp/wg-semhelp-${WG_SMOKE_RUN_ID:0:8}-$$.sock"
cleanup_semhelp_socket(){ rm -f "$short_socket"; }
stop_semhelp_daemon(){
  kill -TERM "$WG_SMOKE_DAEMON_PID" 2>/dev/null || true
  for _ in $(seq 1 100); do
    kill -0 "$WG_SMOKE_DAEMON_PID" 2>/dev/null || return 0
    sleep 0.05
  done
  loud_fail "semantic-help daemon did not stop"
}
add_cleanup_hook cleanup_semhelp_socket
start_wg_daemon "$repo" --socket "$short_socket" --no-supervise --no-chat-agent --max-agents 0 --interval 60
rm -f "$repo/daemon.log"
stop_semhelp_daemon
start_wg_daemon "$repo" --socket "$short_socket" --no-supervise --no-chat-agent --max-agents 0 --interval 60
rm -f "$repo/daemon.log"
wgrun show semantic-correction --json >"$scratch/restarted.json"
python3 - "$scratch/restarted.json" "$scratch/help.json.event" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); event=open(sys.argv[2]).read(); r=x['completion_repair']
assert r['attention_event_id']==event and r['reason_code']=='contract-correction-required',r
assert sum('NeedsAttention event=' in row['message'] for row in x['log'])==1,x['log']
PY
stop_semhelp_daemon

# Operator approval changes requirements, clears the stale candidate, but does
# not rewrite the old rejection/evidence. A request against that stale binding
# is refused until ordinary new validation and review occur.
wgrun contract semantic-correction --add-validation-command "test -s proof.txt" >"$scratch/approved.out"
wgrun show semantic-correction --json >"$scratch/approved.json"
python3 - "$scratch/approved.json" "$scratch/help.json.requirements" "$scratch/help.json.receipt" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); old_req=open(sys.argv[2]).read(); receipt=open(sys.argv[3]).read(); r=x['completion_repair']
assert x.get('completion_candidate') is None,x
assert r['disposition']=='repairing' and r['reason_code']=='operator-contract-update-approved',r
assert r['requirements_digest']==old_req and r['semantic_review']['review_receipt']==receipt,r
assert x['completion_preflight']['checks'][1]['command']=='test -s proof.txt',x['completion_preflight']
PY
if worker semantic-correction semantic-worker fail semantic-correction --intent request-help \
  --reason 'stale request' >"$scratch/stale.out" 2>"$scratch/stale.err"; then
  loud_fail "operator contract update did not invalidate stale semantic help binding"
fi
grep -q 'stale for the current source attempt/fence/requirements/candidate' "$scratch/stale.err" \
  || loud_fail "stale binding refusal was imprecise: $(cat "$scratch/stale.err")"

printf 'operator approved proof\n' > proof.txt
git add proof.txt && git commit -qm approved-proof
printf 'pass\n' >"$scratch/review.mode"
worker semantic-correction semantic-worker done semantic-correction >"$scratch/done.out" 2>"$scratch/done.err" \
  || loud_fail "ordinary revalidation/review did not complete: $(cat "$scratch/done.err")"
wgrun show semantic-correction --json >"$scratch/done.json"
python3 - "$scratch/done.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=x['completion_review_activity']
assert x['status']=='done' and x['completion_disposition']=='landed',x
assert [(r['candidate_state'],r['reviewer_kind'],r['verdict']) for r in rows]==[
 ('superseded','flip','reject'),('current','flip','pass'),('current','eval','pass')],rows
assert x.get('completion_repair') is None,x
PY
[[ "$(cat "$scratch/review.count")" == 5 ]] || loud_fail "ordinary new candidate did not run exactly two-phase FLIP then Eval"

# Separate fixture: a timeout may terminalize source execution, but cannot
# conceal or erase the already-pending semantic operator decision from status.
printf 'reject\n' >"$scratch/review.mode"
wgrun add "Semantic timeout visibility" --id semantic-timeout --validation-command "test -s timeout.txt" >/dev/null
wgrun publish semantic-timeout --only >/dev/null
wgrun claim semantic-timeout --actor timeout-worker >/dev/null
git switch -qc worker/semantic-timeout refs/heads/main
echo timeout > timeout.txt
git add timeout.txt && git commit -qm timeout-candidate
worker semantic-timeout timeout-worker done semantic-timeout >/dev/null 2>"$scratch/timeout-reject.err" && loud_fail "timeout fixture rejection accepted"
worker semantic-timeout timeout-worker fail semantic-timeout --intent request-help --reason 'operator decision pending' >/dev/null
worker semantic-timeout timeout-worker fail semantic-timeout --class agent-hard-timeout --reason 'later wrapper timeout' >/dev/null
wgrun status --json >"$scratch/timeout-status.json"
python3 - "$scratch/timeout-status.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=[r for r in x['stalled_chains'] if r['root_task_id']=='semantic-timeout']
assert len(rows)==1 and rows[0]['root_blocker']=='flip-semantic-rejection: semantic-rejection',rows
assert 'repair-boundary' in rows[0]['safe_operator_action'],rows
PY

echo "PASS: controlled semantic rejection -> explicit attention -> approved contract correction -> ordinary revalidation/review (fixture, not live semantic proof)"
