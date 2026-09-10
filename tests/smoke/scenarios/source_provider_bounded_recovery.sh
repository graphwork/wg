#!/usr/bin/env bash
# Real dispatcher + generated wrapper proof for bounded source-provider recovery.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v script >/dev/null 2>&1 || loud_skip "MISSING PTY DRIVER" "util-linux script is required"

# Keep daemon.sock under sockaddr_un.sun_path even when the outer harness uses
# a deeply nested per-worker build directory.
export WG_SMOKE_ROOT="${WG_SOURCE_PROVIDER_SMOKE_ROOT:-/tmp/wgspbr-$$}"
scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
    export CARGO_TARGET_DIR="$scratch/candidate-target"
    (cd "$repo_root" && CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo build --quiet --bin wg) \
        || loud_fail "candidate wg build failed"
    WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

project="$scratch/project"
home="$scratch/home"
fakebin="$scratch/fakebin"
state="$scratch/provider-state"
mkdir -p "$project" "$home/.config/workgraph" "$fakebin" "$state"
: >"$home/.config/workgraph/config.toml"

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -eu
if [[ "${WG_HANDLER_QUIESCENT:-0}" == "1" ]]; then
  prompt=$(cat)
  if [[ "$prompt" == "FLIP PHASE I —"* ]]; then
    verdict='{"goal":"verify recovered fixture output","constraints":["preserve exact recovery route"],"invariants":["candidate remains attributable"],"failure_modes":[]}'
  else
    verdict='{"verdict":"pass","findings":[]}'
  fi
  encoded=$(printf '%s' "$verdict" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
  printf '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":%s}],"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0.0}}}}\n' "$encoded"
  exit 0
fi
: "${WG_FAKE_PROVIDER_STATE:?}"
: "${WG_FAKE_PROJECT:?}"
: "${WG_TASK_ID:?}"
key="$WG_FAKE_PROVIDER_STATE/$WG_TASK_ID.count"
lock="$key.lock"
while ! mkdir "$lock" 2>/dev/null; do sleep 0.01; done
n=0; [[ -f "$key" ]] && n=$(cat "$key")
n=$((n + 1)); printf '%s\n' "$n" >"$key"
rmdir "$lock"
printf 'attempt=%s task=%s\n' "$n" "$WG_TASK_ID" >>"$WG_FAKE_PROVIDER_STATE/retained-work.txt"
printf 'attempt=%s\n' "$n" >>"$WG_FAKE_PROJECT/.wg/partial-$WG_TASK_ID.txt"
case "$WG_TASK_ID" in
  disabled|exhaust|manual)
    printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"temporary upstream outage"}}'
    exit 1
    ;;
  recover)
    if [[ "$n" -eq 1 ]]; then
      printf '%s\n' '{"type":"error","status":429,"error":{"type":"rate_limit_error","message":"rate limited","metadata":{"retry_after":2}}}'
      exit 1
    elif [[ "$n" -eq 2 ]]; then
      printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"temporary upstream outage"}}'
      exit 1
    fi
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"stop","rawStopReason":"completed","content":[{"type":"text","text":"recovered on exact source route"}]}}'
    cp "$WG_FAKE_PROJECT/.wg/partial-$WG_TASK_ID.txt" "$WG_FAKE_PROJECT/result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" add "result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" commit -qm "complete $WG_TASK_ID"
    task_id="$WG_TASK_ID"
    # Exercise the ordinary completion controller plus independent exact-route
    # semantic reviewers. The quiescent marker applies only to those no-tool
    # reviewer subprocesses; no operator completion bypass is used.
    WG_HANDLER_QUIESCENT=1 wg done "$task_id" \
      >"$WG_FAKE_PROVIDER_STATE/$task_id.done.log" 2>&1 || {
      cat "$WG_FAKE_PROVIDER_STATE/$task_id.done.log" >&2
      exit 96
    }
    exit 0
    ;;
  beyond)
    printf '%s\n' '{"type":"error","status":429,"error":{"type":"rate_limit_error","message":"rate limited","metadata":{"retry_after":30}}}'
    exit 1
    ;;
  ambiguous)
    # A structured provider envelope proves only this request failed. The
    # earlier tool effect makes replay of the whole source attempt ambiguous.
    printf '%s\n' '{"type":"tool_execution_end","toolCallId":"t1","result":"published"}'
    printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"temporary upstream outage"}}'
    exit 1
    ;;
  *) exit 97 ;;
esac
SH
chmod +x "$fakebin/pi"

export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export WG_GLOBAL_DIR="$home/.config/workgraph"
export WG_FAKE_PROVIDER_STATE="$state"
export WG_FAKE_PROJECT="$project"
export PATH="$fakebin:$(dirname "$WG_BIN"):$PATH"
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER

cd "$project"
git init -q -b main
git config user.email source-recovery@test.invalid
git config user.name SourceRecovery
printf 'base\n' >base.txt
printf '.wg/\n' >.gitignore
git add base.txt .gitignore
git commit -qm base
"$WG_BIN" init --no-agency >/dev/null || loud_fail "wg init failed"
G="$project/.wg"
wgrun(){ "$WG_BIN" --dir "$G" "$@"; }
wgrun setup --route pi --yes --model pi:test:bounded-smoke >/dev/null
wgrun config --local --model pi:test:bounded-smoke --auto-assign false --auto-evaluate false --no-reload >/dev/null
wgrun config set dispatcher.worktree_isolation false --no-reload >/dev/null
# This credential-free fixture launches a shell-only fake provider and never
# builds. Keep the scenario independent of host disk headroom projections.
wgrun config set dispatcher.resource_management.disk_sentinel_enabled false --no-reload >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 --no-reload >/dev/null
git add worksgood.toml
git commit -qm 'configure smoke route'

add_ready() {
    local id="$1"
    wgrun add "$id source-provider fixture" --id "$id" >/dev/null || loud_fail "add $id failed"
    wgrun publish "$id" --only >/dev/null || loud_fail "publish $id failed"
}
count() { cat "$state/$1.count" 2>/dev/null || echo 0; }
task_field() {
    python3 - "$G/graph.jsonl" "$1" "$2" <<'PY'
import json,sys
p,task_id,field=sys.argv[1:]
for line in open(p):
    row=json.loads(line)
    if row.get("kind")=="task" and row.get("id")==task_id:
        value=row.get(field)
        print("none" if value is None else value)
        raise SystemExit
raise SystemExit(2)
PY
}
record_state() {
    python3 - "$G/graph.jsonl" "$1" <<'PY'
import json,sys
p,task_id=sys.argv[1:]
for line in open(p):
    row=json.loads(line)
    if row.get("kind")=="task" and row.get("id")==task_id:
        rec=row.get("source_provider_recovery")
        print("none" if rec is None else rec.get("state","missing"))
        raise SystemExit
raise SystemExit(2)
PY
}
wait_for() {
    local want="$1" id="$2" tries="${3:-160}"
    for _ in $(seq 1 "$tries"); do
        [[ "$(record_state "$id" 2>/dev/null || true)" == "$want" ]] && return 0
        sleep 0.1
    done
    return 1
}

# Keep the helper's wrapper log outside the Git fixture while launching the
# real daemon from the project cwd.
ln -s project/.wg "$scratch/.wg"
export WG_SMOKE_DAEMON_LAUNCH_CWD="$project"

# Disabled is exact fail-stop, and enabling later does not backfill history.
add_ready disabled
start_wg_daemon "$scratch" --max-agents 1 --interval 1 --no-chat-agent --no-supervise
for _ in $(seq 1 120); do [[ "$(count disabled)" -eq 1 ]] && [[ "$(task_field disabled status)" == failed ]] && [[ "$(record_state disabled)" == none ]] && break; sleep 0.1; done
[[ "$(count disabled)" -eq 1 && "$(task_field disabled status)" == failed && "$(record_state disabled)" == none ]] \
    || loud_fail "disabled policy enrolled or repeated a source failure: count=$(count disabled) state=$(record_state disabled 2>/dev/null || echo missing) daemon=$(tail -80 "$G/service/daemon.log" 2>/dev/null || true)"
wgrun service stop >/dev/null || loud_fail "disabled-policy daemon stop failed"
wgrun config set dispatcher.source_provider_retry.enabled true --no-reload >/dev/null
wgrun config set dispatcher.source_provider_retry.max_automatic_retries 3 --no-reload >/dev/null
wgrun config set dispatcher.source_provider_retry.recovery_window_seconds 15 --no-reload >/dev/null
wgrun config set dispatcher.source_provider_retry.base_seconds 1 --no-reload >/dev/null
wgrun config set dispatcher.source_provider_retry.delay_cap_seconds 2 --no-reload >/dev/null
[[ "$(count disabled)" -eq 1 && "$(record_state disabled)" == none ]] \
    || loud_fail "enabling policy backfilled the old failed task: count=$(count disabled) state=$(record_state disabled 2>/dev/null || echo missing)"
# The production wrapper materializes guides and repository excludes. Freeze
# that daemon-generated baseline so Land validation sees only worker output.
git add .gitignore worksgood.toml AGENTS.md CLAUDE.md
git commit -qm 'freeze generated worker baseline'

# Direct 429 enrolls, Retry-After prevents an early call, and daemon restart
# keeps the same episode/budget. The third source call succeeds.
add_ready recover
start_wg_daemon "$scratch" --max-agents 1 --interval 1 --no-chat-agent --no-supervise
wait_for backoff recover || loud_fail "direct 429 did not enter backoff: $(wgrun --json show recover 2>/dev/null || true)"
[[ "$(count recover)" -eq 1 ]] || loud_fail "unexpected initial recover count"
show_pty="$scratch/show-backoff.txt"
script -qec "$WG_BIN --dir '$G' show recover" "$show_pty" >/dev/null 2>&1 \
    || loud_fail "PTY wg show failed"
grep -q 'source_provider_recovery: Backoff' "$show_pty" \
    || loud_fail "PTY show omitted Backoff: $(cat "$show_pty")"
grep -q 'next action:' "$show_pty" || loud_fail "PTY show omitted safe next action"
sleep 0.5
[[ "$(count recover)" -eq 1 ]] || loud_fail "Retry-After lower bound was violated"
episode_before=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
for line in open(sys.argv[1]):
 r=json.loads(line)
 if r.get('id')=='recover': print(r['source_provider_recovery']['episode_id'])
PY
)
wgrun service stop >/dev/null || loud_fail "first daemon stop failed"
start_wg_daemon "$scratch" --max-agents 1 --interval 1 --no-chat-agent --no-supervise
for _ in $(seq 1 240); do
    [[ "$(record_state recover 2>/dev/null || true)" == recovered ]] \
        && [[ "$(task_field recover status 2>/dev/null || true)" == done ]] \
        && break
    sleep 0.1
done
[[ "$(record_state recover)" == recovered && "$(task_field recover status)" == done && "$(count recover)" -eq 3 ]] \
    || loud_fail "restart-safe exact recovery did not converge: state=$(record_state recover) count=$(count recover) done=$(cat "$state/recover.done.log" 2>/dev/null || true) show=$(wgrun --json show recover 2>/dev/null || true) daemon=$(tail -80 "$G/service/daemon.log" 2>/dev/null || true)"
python3 - "$G/graph.jsonl" "$episode_before" <<'PY' || loud_fail "restart reset or duplicated recovery episode/budget"
import json,sys
for line in open(sys.argv[1]):
 r=json.loads(line)
 if r.get('id')=='recover':
  q=r['source_provider_recovery']
  assert q['episode_id']==sys.argv[2],q
  assert q['automatic_retries_used']==2,q
  assert q['exact_route']=='pi:test:bounded-smoke',q
  assert q['executor']=='pi' and q['model']=='test:bounded-smoke',q
  assert q['operation_id'].startswith('b3:'),q
  assert q['evidence_kind']=='provider-envelope',q
  assert q['execution_outcome']=='definitive-failure',q
  assert q.get('http_status') in (429,503),q
  assert q['failure_evidence_digest'].startswith('b3:'),q
  assert q['goal_requirements_digest'].startswith('b3:'),q
  assert q['route_id']=='pi|test|self-authenticated',q
  assert q['plan_id'].startswith('id:'),q
  assert r['lifecycle']['generation']==2,r
  assert r['status']=='done',r
  raise SystemExit
raise AssertionError('missing recover')
PY
grep -q 'attempt=1 task=recover' "$state/retained-work.txt" \
    && grep -q 'attempt=3 task=recover' "$state/retained-work.txt" \
    || loud_fail "retained source work was not visible across retries"
grep -q '^attempt=1$' "$project/result-recover.txt" \
    && grep -q '^attempt=3$' "$project/result-recover.txt" \
    || loud_fail "completed result did not preserve and publish retained partial work"
grep -q "Landed 'recover'" "$state/recover.done.log" \
    && ! grep -q 'UNAVAILABLE' "$state/recover.done.log" \
    || loud_fail "ordinary semantic-review completion controller did not accept the fixture result: $(cat "$state/recover.done.log" 2>/dev/null || true)"

# The fourth failed source call is the third and final automatic retry. No
# fifth call is authorized, and status presents NeedsAttention in a PTY.
add_ready exhaust
wait_for needs-attention exhaust 300 || loud_fail "retry exhaustion did not require attention"
sleep 2
[[ "$(count exhaust)" -eq 4 ]] || loud_fail "budget fence expected initial+3 calls, got $(count exhaust)"
status_pty="$scratch/status-attention.txt"
script -qec "$WG_BIN --dir '$G' status --all" "$status_pty" >/dev/null 2>&1 \
    || loud_fail "PTY wg status failed"
grep -q 'exhaust.*NeedsAttention.*retries 3/3' "$status_pty" \
    || loud_fail "PTY status omitted truthful exhaustion: $(cat "$status_pty")"

# Retry-After outside the window and a structured provider failure after an
# earlier tool effect both fail closed.
add_ready beyond
wait_for needs-attention beyond || loud_fail "out-of-window Retry-After was not rejected"
sleep 1
[[ "$(count beyond)" -eq 1 ]] || loud_fail "out-of-window Retry-After caused a retry"
add_ready ambiguous
for _ in $(seq 1 120); do [[ "$(count ambiguous)" -eq 1 ]] && break; sleep 0.1; done
sleep 2
[[ "$(count ambiguous)" -eq 1 && "$(record_state ambiguous)" == none ]] \
    || loud_fail "structured post-effect provider failure entered automatic recovery"

# An explicit operator retry is a new decision and atomically clears the old
# automatic episode before owner-reap/generation completion.
add_ready manual
wait_for backoff manual || loud_fail "manual-retry fixture did not enter backoff"
wgrun service stop >/dev/null || loud_fail "manual-retry daemon stop failed"
wgrun retry manual --reason "operator reconciled retained work" >/dev/null \
    || loud_fail "explicit wg retry was rejected"
[[ "$(record_state manual)" == none ]] \
    || loud_fail "explicit wg retry retained the automatic recovery episode"

# There is no planner/controller/evaluator task or scheduler state sidecar.
python3 - "$G" <<'PY' || loud_fail "recovery created a forbidden scheduler/task plane"
import json,os,sys
G=sys.argv[1]
rows=[json.loads(x) for x in open(os.path.join(G,'graph.jsonl')) if x.strip()]
ids=[r.get('id','') for r in rows if r.get('kind')=='task']
assert not [x for x in ids if x.startswith(('.retry-','.controller-','.evaluate-','.review-'))],ids
for name in ('planner.json','convergence.json','source-provider-retry.json'):
 assert not os.path.exists(os.path.join(G,name)),name
PY

echo "PASS: current dispatcher bounded direct source-provider recovery is retry-after/restart/budget safe with truthful PTY UX"
