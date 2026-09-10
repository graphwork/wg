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
: "${WG_FAKE_PROVIDER_STATE:?}"
: "${WG_FAKE_PROJECT:?}"
if [[ "${WG_HANDLER_QUIESCENT:-0}" == "1" ]]; then
  task_id="${WG_TASK_ID:-unknown}"
  printf '%s\n' "$task_id" >>"$WG_FAKE_PROVIDER_STATE/review-calls.txt"
  prompt=$(cat)
  if [[ "$task_id" == "reviewer-fail" ]]; then
    printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"reviewer transport unavailable"}}'
    exit 1
  elif [[ "$prompt" == "FLIP PHASE I —"* ]]; then
    verdict='{"goal":"verify recovered fixture output","constraints":["preserve exact recovery route"],"invariants":["candidate remains attributable"],"failure_modes":[]}'
  elif [[ "$task_id" == "semantic" ]]; then
    verdict='{"verdict":"reject","findings":[{"code":"fixture.semantic","message":"fixture semantic rejection"}]}'
  else
    verdict='{"verdict":"pass","findings":[]}'
  fi
  encoded=$(printf '%s' "$verdict" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
  printf '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":%s}],"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0.0}}}}\n' "$encoded"
  exit 0
fi
: "${WG_TASK_ID:?}"
key="$WG_FAKE_PROVIDER_STATE/$WG_TASK_ID.count"
lock="$key.lock"
while ! mkdir "$lock" 2>/dev/null; do sleep 0.01; done
n=0; [[ -f "$key" ]] && n=$(cat "$key")
n=$((n + 1)); printf '%s\n' "$n" >"$key"
rmdir "$lock"
printf 'attempt=%s task=%s executor=%s model=%s operation=%s route=%s plan=%s\n' \
  "$n" "$WG_TASK_ID" "${WG_EXECUTOR_TYPE:-missing}" "${WG_MODEL:-missing}" \
  "${WG_SOURCE_PROVIDER_OPERATION_ID:-missing}" "${WG_SOURCE_PROVIDER_ROUTE_ID:-missing}" \
  "${WG_SOURCE_PROVIDER_PLAN_ID:-missing}" >>"$WG_FAKE_PROVIDER_STATE/retained-work.txt"
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
  auth)
    printf '%s\n' '{"type":"error","status":401,"error":{"type":"authentication_error","message":"invalid api key"}}'
    exit 1
    ;;
  credit)
    printf '%s\n' '{"type":"error","status":402,"error":{"type":"payment_required","message":"insufficient credits"}}'
    exit 1
    ;;
  config-hard)
    printf '%s\n' '{"type":"error","status":400,"error":{"type":"invalid_request_error","message":"model or request configuration invalid"}}'
    exit 1
    ;;
  semantic|reviewer-fail)
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"stop","rawStopReason":"completed","content":[{"type":"text","text":"candidate awaiting independent review"}]}}'
    cp "$WG_FAKE_PROJECT/.wg/partial-$WG_TASK_ID.txt" "$WG_FAKE_PROJECT/result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" add "result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" commit -qm "candidate $WG_TASK_ID"
    WG_HANDLER_QUIESCENT=1 wg done "$WG_TASK_ID" \
      >"$WG_FAKE_PROVIDER_STATE/$WG_TASK_ID.done.log" 2>&1 || true
    exit 0
    ;;
  pause-race)
    if [[ "$n" -eq 1 ]]; then
      printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"temporary upstream outage"}}'
      exit 1
    elif [[ "$n" -eq 2 ]]; then
      : >"$WG_FAKE_PROVIDER_STATE/pause-race.started"
      for _ in $(seq 1 300); do
        [[ -f "$WG_FAKE_PROVIDER_STATE/pause-race.release" ]] && break
        sleep 0.02
      done
      printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"temporary upstream outage while paused"}}'
      exit 1
    fi
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","stopReason":"stop","rawStopReason":"completed","content":[{"type":"text","text":"resumed recovery succeeded"}]}}'
    cp "$WG_FAKE_PROJECT/.wg/partial-$WG_TASK_ID.txt" "$WG_FAKE_PROJECT/result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" add "result-$WG_TASK_ID.txt"
    git -C "$WG_FAKE_PROJECT" commit -qm "complete $WG_TASK_ID"
    WG_HANDLER_QUIESCENT=1 wg done "$WG_TASK_ID" \
      >"$WG_FAKE_PROVIDER_STATE/$WG_TASK_ID.done.log" 2>&1 || true
    exit 0
    ;;
  cancel)
    : >"$WG_FAKE_PROVIDER_STATE/cancel.started"
    for _ in $(seq 1 300); do
      [[ -f "$WG_FAKE_PROVIDER_STATE/cancel.release" ]] && break
      sleep 0.02
    done
    printf '%s\n' '{"type":"error","status":503,"error":{"type":"provider_unavailable","message":"late failure after operator cancellation"}}'
    exit 1
    ;;
  route-drift)
    printf '%s\n' '{"type":"error","status":429,"error":{"type":"rate_limit_error","message":"route must remain exact","metadata":{"retry_after":3}}}'
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
# Every daemon/wrapper/internal invocation of bare `wg` must resolve the exact
# candidate, even when the harness gives it a nonstandard filename.
ln -s "$WG_BIN" "$fakebin/wg"
[[ "$(readlink -f "$fakebin/wg")" == "$(readlink -f "$WG_BIN")" ]] \
    || loud_fail "candidate wg shim does not resolve to the requested binary"

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
grep -Eq 'retry 0/3; window [0-9]+s; next ' "$show_pty" \
    || loud_fail "PTY show omitted attempts/window/next retry: $(cat "$show_pty")"
grep -q 'failure_reason_signal: rate-limit' "$show_pty" \
    || loud_fail "PTY show omitted direct rate-limit evidence: $(cat "$show_pty")"
grep -q 'exact route: pi:test:bounded-smoke' "$show_pty" \
    || loud_fail "PTY show omitted exact recovery route: $(cat "$show_pty")"
grep -q 'next action: wait for the bounded exact-route retry' "$show_pty" \
    || loud_fail "PTY show omitted safe waiting action"
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
grep -q 'attempt=1 task=recover executor=pi model=test:bounded-smoke' "$state/retained-work.txt" \
    && grep -q 'attempt=3 task=recover executor=pi model=test:bounded-smoke' "$state/retained-work.txt" \
    || loud_fail "retained source work or exact candidate route was not visible across retries"
python3 - "$state/retained-work.txt" <<'PY' || loud_fail "retry calls changed route/plan or reused an operation identity"
import sys
rows=[]
for line in open(sys.argv[1]):
 fields=dict(field.split('=',1) for field in line.split())
 if fields.get('task')=='recover': rows.append(fields)
assert len(rows)==3,rows
assert {r['executor'] for r in rows}=={'pi'},rows
assert {r['model'] for r in rows}=={'test:bounded-smoke'},rows
assert len({r['route'] for r in rows})==1,rows
assert len({r['plan'] for r in rows})==1,rows
assert all(r['route']!='missing' and r['plan']!='missing' for r in rows),rows
assert len({r['operation'] for r in rows})==3,rows
assert all(r['operation'].startswith('b3:') for r in rows),rows
PY
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
exhaust_show="$scratch/show-exhausted.txt"
script -qec "$WG_BIN --dir '$G' show exhaust" "$exhaust_show" >/dev/null 2>&1 \
    || loud_fail "PTY exhausted wg show failed"
grep -q 'reason automatic-retries-exhausted' "$exhaust_show" \
    && grep -q 'next action: inspect this task, then explicitly run `wg retry TASK --reason <WHY>`' "$exhaust_show" \
    || loud_fail "exhaustion did not present infrastructure attention guidance: $(cat "$exhaust_show")"
old_exhaust_episode=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
for row in map(json.loads,open(sys.argv[1])):
 if row.get('id')=='exhaust': print(row['source_provider_recovery']['episode_id'])
PY
)
retry_pty="$scratch/retry-exhausted.txt"
script -qec "$WG_BIN --dir '$G' retry exhaust --reason 'operator inspected retained work and confirmed replay safe'" "$retry_pty" >/dev/null 2>&1 \
    || loud_fail "displayed exhausted-task recovery action failed: $(cat "$retry_pty")"
wait_for backoff exhaust || loud_fail "operator retry did not form a new bounded episode"
python3 - "$G/graph.jsonl" "$old_exhaust_episode" <<'PY' || loud_fail "operator action silently reset the old automatic episode"
import json,sys
for row in map(json.loads,open(sys.argv[1])):
 if row.get('id')=='exhaust':
  rec=row['source_provider_recovery']
  assert rec['episode_id'] != sys.argv[2], rec
  assert rec['automatic_retries_used'] == 0, rec
  break
else: raise AssertionError('exhaust missing')
PY
wgrun pause exhaust >/dev/null || loud_fail "could not pause the new operator-authorized episode"

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

# Direct auth, credit, and hard request/configuration failures remain ordinary
# fail-stop source outcomes and never consume an automatic retry.
for id in auth credit config-hard; do add_ready "$id"; done
for id in auth credit config-hard; do
    for _ in $(seq 1 160); do
        [[ "$(count "$id")" -eq 1 ]] && [[ "$(task_field "$id" status)" == failed ]] && break
        sleep 0.1
    done
    [[ "$(count "$id")" -eq 1 && "$(record_state "$id")" == none ]] \
        || loud_fail "$id incorrectly entered source recovery"
done
sleep 3
for id in auth credit config-hard; do
    [[ "$(count "$id")" -eq 1 ]] || loud_fail "$id received a hidden retry"
done

# Manual pause wins while Backoff, and also when it races a retry already past
# its durable launch permit. The latter failure must park (not strand Running),
# then the same unextended episode may continue only after explicit resume.
add_ready pause-race
wait_for backoff pause-race || loud_fail "pause fixture did not enter backoff"
for _ in $(seq 1 240); do [[ -f "$state/pause-race.started" ]] && break; sleep 0.05; done
[[ -f "$state/pause-race.started" ]] || loud_fail "pause fixture retry never reached provider"
wgrun pause pause-race >/dev/null || loud_fail "manual pause failed"
: >"$state/pause-race.release"
wait_for paused pause-race || loud_fail "running retry failure stranded instead of parking"
sleep 2
[[ "$(count pause-race)" -eq 2 ]] || loud_fail "paused task received provider I/O"
wgrun resume pause-race --only >/dev/null || loud_fail "manual resume failed"
for _ in $(seq 1 240); do [[ "$(count pause-race)" -eq 3 ]] && break; sleep 0.1; done
[[ "$(count pause-race)" -eq 3 ]] || loud_fail "resumed episode did not receive its next bounded attempt"

# Cancellation before the source failure is terminal manual authority. A late
# wrapper failure remains fenced and cannot enroll or reopen the abandoned task.
add_ready cancel
for _ in $(seq 1 160); do [[ -f "$state/cancel.started" ]] && break; sleep 0.05; done
[[ -f "$state/cancel.started" ]] || loud_fail "cancel fixture never started"
wgrun abandon cancel --reason "operator cancelled before provider outcome" >/dev/null \
    || loud_fail "operator cancellation failed"
: >"$state/cancel.release"
sleep 2
[[ "$(count cancel)" -eq 1 && "$(task_field cancel status)" == abandoned && "$(record_state cancel)" == none ]] \
    || loud_fail "late cancelled outcome entered source recovery"

# Source success followed by semantic rejection or reviewer infrastructure
# failure stays in the pre-existing completion policy. Neither path reruns the
# source, and the direct source failures above never called a semantic reviewer.
add_ready semantic
add_ready reviewer-fail
for id in semantic reviewer-fail; do
    for _ in $(seq 1 240); do
        [[ "$(count "$id")" -eq 1 ]] && grep -q "^$id$" "$state/review-calls.txt" 2>/dev/null && break
        sleep 0.1
    done
    sleep 1
    [[ "$(count "$id")" -eq 1 && "$(record_state "$id")" == none ]] \
        || loud_fail "$id reviewer outcome incorrectly reran source"
done
python3 - "$state/review-calls.txt" <<'PY' || loud_fail "source failures wasted semantic reviewer calls"
import sys
calls=open(sys.argv[1]).read().splitlines()
allowed={'recover','pause-race','semantic','reviewer-fail'}
assert not (set(calls)-allowed), calls
assert 'semantic' in calls and 'reviewer-fail' in calls, calls
PY

# Route drift while Backoff fails closed; the dispatcher never falls back or
# contacts the fake provider on the changed route.
add_ready route-drift
wait_for backoff route-drift || loud_fail "route drift fixture did not enter backoff"
wgrun service stop >/dev/null || loud_fail "route-drift daemon stop failed"
wgrun config --local --model pi:test:changed-route --no-reload >/dev/null
start_wg_daemon "$scratch" --max-agents 1 --interval 1 --no-chat-agent --no-supervise
wait_for needs-attention route-drift || loud_fail "changed route did not request attention"
sleep 2
[[ "$(count route-drift)" -eq 1 ]] || loud_fail "route drift caused provider fallback/I/O"
wgrun service stop >/dev/null || loud_fail "route-drift final daemon stop failed"
wgrun config --local --model pi:test:bounded-smoke --no-reload >/dev/null
start_wg_daemon "$scratch" --max-agents 1 --interval 1 --no-chat-agent --no-supervise

# The wrapper intentionally records the same exact failed operation before and
# after graph failure. Immutable telemetry deduplication must retain one record
# per physical operation, never one request per replayed observation.
python3 - "$G/service/provider-telemetry.jsonl" <<'PY' || loud_fail "duplicate telemetry evidence did not converge"
import json,sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
for task in ('auth','credit','config-hard','ambiguous','route-drift'):
    bound=[r for r in rows if r.get('task')==task]
    assert len(bound)==1,(task,bound)
PY

# The successful source publishes at most once even though multiple attempts
# and wrapper observations exist.
python3 - "$G/graph.jsonl" <<'PY' || loud_fail "successful recovery published more than once"
import json,sys
for row in map(json.loads,open(sys.argv[1])):
    if row.get('id')=='recover':
        audit=row['lifecycle']['audit']
        assert sum(e.get('event_kind')=='attempt-succeeded' for e in audit)==1,audit
        assert row.get('completion_receipt'),row
        break
else: raise AssertionError('recover missing')
PY

# An explicit operator retry is a new decision and atomically clears the old
# automatic episode before owner-reap/generation completion.
add_ready manual
wait_for backoff manual || loud_fail "manual-retry fixture did not enter backoff"
wgrun service stop --kill-agents >/dev/null || loud_fail "manual-retry owned-process teardown failed"
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

echo "PASS: dispatcher/wrapper recovery is bounded, exact-route, duplicate-safe, manually fenced, and semantically isolated with truthful PTY UX"
