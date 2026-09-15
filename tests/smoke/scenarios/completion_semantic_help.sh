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
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the live TUI flow"

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
    response='{"verdict":"reject","findings":[{"code":"completion.missing_authoritative_runtime_evidence","message":"candidate lacks the operator-approved exact validation evidence","evidence":"test -s proof.txt"}]}'
  else
    response='{"verdict":"pass","findings":[]}'
  fi
elif [[ "$prompt" == *"FLIP PHASE I"* ]]; then
  response='{"goal":"controlled reconstructed intent","constraints":[],"invariants":[],"failure_modes":[]}'
elif [[ "$mode" == reject ]]; then
  response='{"verdict":"reject","findings":[{"code":"completion.missing_authoritative_runtime_evidence","message":"candidate lacks the operator-approved exact validation evidence","evidence":"test -s proof.txt"}]}'
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
wgrun add "Blocked semantic child" --id semantic-child --after semantic-correction >/dev/null
wgrun add "Blocked semantic grandchild" --id semantic-grandchild --after semantic-child >/dev/null
wgrun publish semantic-correction --only >/dev/null
wgrun claim semantic-correction --actor semantic-worker >/dev/null
git switch -qc worker/semantic-correction
echo result > result.txt
git add result.txt && git commit -qm semantic-candidate

worker semantic-correction semantic-worker done semantic-correction >"$scratch/reject.out" 2>"$scratch/reject.err" \
  || loud_fail "evidence-only semantic rejection escaped bounded help: $(cat "$scratch/reject.err")"
grep -q 'retained as NeedsAttention' "$scratch/reject.out" \
  || loud_fail "semantic evidence gap did not visibly retain completion help: $(cat "$scratch/reject.out")"
grep -q 'wg contract semantic-correction --add-validation-command' "$scratch/reject.out" \
  || loud_fail "semantic evidence gap omitted the one operator correction: $(cat "$scratch/reject.out")"
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

# Automatic help already retained the source worktree. Lost-response replay and
# wrapper exit retain one event and do not call validation or a reviewer.
if worker semantic-correction semantic-worker done semantic-correction >"$scratch/repeat.out" 2>"$scratch/repeat.err"; then
  loud_fail "repeated semantic evidence-gap completion escaped bounded attention"
fi
grep -q 'NeedsAttention: bounded completion help is stopped' "$scratch/repeat.err" \
  || loud_fail "repeat did not expose bounded help: $(cat "$scratch/repeat.err")"
worker semantic-correction semantic-worker fail semantic-correction \
  --reason 'worker exited after automatic evidence-gap help' >"$scratch/help.out"
grep -q "Saved work: $repo" "$scratch/help.out" || loud_fail "help output omitted saved-work location"
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "help/replay reran unchanged reviewer"

# A worker cannot widen its own repair scope or mutate contract authority.
if worker semantic-correction semantic-worker contract semantic-correction \
  --repair-boundary repository >"$scratch/scope.out" 2>"$scratch/scope.err"; then
  loud_fail "worker widened semantic repair scope"
fi
grep -q 'operator decision' "$scratch/scope.err" \
  || loud_fail "out-of-scope repair refusal was not explicit: $(cat "$scratch/scope.err")"

wgrun show semantic-correction --json >"$scratch/help.json"
wgrun status --json >"$scratch/status.json"
python3 - "$scratch/rejected.json" "$scratch/help.json" "$scratch/status.json" "$repo" <<'PY'
import json,sys
before=json.load(open(sys.argv[1])); x=json.load(open(sys.argv[2])); status=json.load(open(sys.argv[3])); repo=sys.argv[4]
r=x['completion_repair']; c=x['completion_candidate']; rows=x['completion_review_activity']
assert x['status']=='in-progress' and x['assigned']=='semantic-worker' and x.get('completion_receipt') is None,x
assert c==before['completion_candidate'],(before,c)
assert len(rows)==1 and rows[0]['candidate_state']=='current' and rows[0]['verdict']=='reject',rows
b=c['review_binding']; s=r['semantic_review']
assert r['disposition']=='needs-attention' and r['reason_code']=='contract-correction-required',r
assert r['blocker_reason_code']=='flip-semantic-rejection' and r['exit_category']=='semantic-rejection',r
assert r['saved_work']==repo,r
assert (r['task_id'],r['generation'],r['attempt_id'],r['fence']) == (b['task_id'],b['generation'],b['attempt_id'],b['attempt_fence']),r
assert r['candidate_identity']==c['manifest']['content_digest'],r
assert s['reviewer_kind']=='flip' and s['candidate_sequence']==b['candidate_sequence'],s
assert s['review_receipt']==rows[0]['activity_id']==r['evidence']['content_digest'],(s,rows,r)
assert sum('NeedsAttention event=' in row['message'] for row in x['log'])==1,x['log']
assert any('saved_work='+repo in row['message'] for row in x['log']),x['log']
chains=[row for row in status['stalled_chains'] if row['root_task_id']=='semantic-correction']
assert len(chains)==1 and chains[0]['root_blocker']=='flip-semantic-rejection: semantic-rejection',chains
assert chains[0]['saved_work']==repo,chains
assert chains[0]['affected_downstream']==['semantic-child','semantic-grandchild'],chains
next_action=chains[0]['safe_operator_action']
assert next_action.count('wg contract semantic-correction --add-validation-command')==1,next_action
assert all(x not in next_action for x in ['wg retry','request-help','deliberate-stop']),next_action
open(sys.argv[2]+'.event','w').write(r['attention_event_id'])
open(sys.argv[2]+'.requirements','w').write(r['requirements_digest'])
open(sys.argv[2]+'.receipt','w').write(s['review_receipt'])
PY

# Drive the real TUI through tmux and its keyboard dispatcher. The inspector
# must expose the semantic root, affected waits, and the same single operator
# correction before any contract mutation occurs.
tui_session="wg-semantic-help-tui-$$"
# Downstream visibility was asserted from the live graph above. Reduce this
# disposable TUI fixture to one row so keyboard focus cannot depend on graph
# presentation ordering.
python3 - "$repo/.wg/graph.jsonl" <<'PY'
import json,sys
path=sys.argv[1]
rows=[line for line in open(path) if json.loads(line).get('id') not in {'semantic-child','semantic-grandchild'}]
open(path,'w').writelines(rows)
PY
printf '%s\n' semantic-correction >"$repo/.wg/.new_task_focus"
cleanup_semhelp_tui(){ tmux kill-session -t "$tui_session" >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup_semhelp_tui
tmux new-session -d -s "$tui_session" -x 180 -y 44 \
  "cd '$repo' && HOME='$home' XDG_CONFIG_HOME='$home/.config' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$repo/.wg' tui; sleep 30"
capture_semhelp_tui(){ tmux capture-pane -p -t "$tui_session" 2>/dev/null || true; }
for _ in $(seq 1 300); do
  capture_semhelp_tui | grep -Fq semantic-correction && break
  sleep 0.025
done
tmux send-keys -t "$tui_session" Home
tmux send-keys -t "$tui_session" Enter
found_semantic_help=0
for _ in $(seq 1 12); do
  sleep 0.1
  frame=$(capture_semhelp_tui)
  if grep -Fq 'ROOT BLOCKER: semantic-correction' <<<"$frame" \
      && grep -Fq 'one safe action:' <<<"$frame"; then
    found_semantic_help=1
    printf '%s\n' "$frame" >"$scratch/tui-semantic-help.txt"
    break
  fi
  tmux send-keys -t "$tui_session" PageDown
done
[[ "$found_semantic_help" == 1 ]] \
  || loud_fail "real TUI omitted semantic completion help: $(capture_semhelp_tui | tr '\n' '|')"
cleanup_semhelp_tui

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
worker semantic-timeout timeout-worker done semantic-timeout >/dev/null 2>"$scratch/timeout-reject.err" \
  || loud_fail "timeout fixture evidence gap escaped automatic help: $(cat "$scratch/timeout-reject.err")"
worker semantic-timeout timeout-worker fail semantic-timeout --class agent-hard-timeout --reason 'later wrapper timeout' >/dev/null
wgrun status --json >"$scratch/timeout-status.json"
python3 - "$scratch/timeout-status.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=[r for r in x['stalled_chains'] if r['root_task_id']=='semantic-timeout']
assert len(rows)==1 and rows[0]['root_blocker']=='flip-semantic-rejection: semantic-rejection',rows
assert 'wg contract semantic-timeout --add-validation-command' in rows[0]['safe_operator_action'],rows
PY

echo "PASS: controlled evidence-gap rejection -> automatic bounded help -> approved contract correction -> retained revalidation/review + live TUI visibility"
