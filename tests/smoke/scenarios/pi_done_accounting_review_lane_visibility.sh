#!/usr/bin/env bash
# Real credential-free CLI flow for completed Pi accounting + review visibility.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
fixture="$repo_root/tests/smoke/fixtures/pi_event_stream.jsonl"
[[ -f "$fixture" ]] || loud_fail "missing Pi stream fixture"
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
if [[ " $* " == *" --model fake-worker "* ]]; then
  cat "${PI_WORKER_FIXTURE:?}"
  while [[ ! -f "${PI_WORKER_RELEASE:?}" ]]; do sleep 0.05; done
  exit 0
fi
state="${FAKE_PI_STATE:?}"; n=0; [[ -f "$state" ]] && n=$(cat "$state"); n=$((n+1)); printf '%s' "$n" >"$state"
if [[ "$n" == 1 ]]; then sleep 3; exit 0; fi
if [[ "$n" == 2 ]]; then verdict=reject; else verdict=pass; fi
python3 - "$verdict" "$n" <<'PY'
import json,sys
verdict,n=sys.argv[1],int(sys.argv[2])
findings=[{'code':'fixture.second_reject','message':'repair and resubmit'}] if verdict=='reject' else []
text='not-json' if n == 1 else json.dumps({'verdict':verdict,'findings':findings},separators=(',',':'))
event={'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
       'provider':'test','model':'fake-review','stopReason':'stop','usage':{'input':n,'output':1,
       'cacheRead':0,'cacheWrite':0,'totalTokens':n+1,'cost':{'total':0.001}}}}
print(json.dumps(event,separators=(',',':')))
PY
FAKE_PI
chmod +x "$fakebin/pi"
ln -s "$WG_BIN" "$fakebin/wg"
export HOME="$home" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH" FAKE_PI_STATE="$scratch/pi-calls"
export PI_WORKER_FIXTURE="$fixture" PI_WORKER_RELEASE="$scratch/release-worker"
export WG_COMPLETION_REVIEW_TIMEOUT_SECS=1
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL WG_WORKER_IPC WG_GRAPH_ID WG_SPAWN_RUN_ID WG_SPAWN_EPOCH
cd "$project"
git init -q -b main; git config user.email pi-accounting@test.invalid; git config user.name PiAccounting
printf 'base\n' > base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"; wgrun(){ "$WG_BIN" --dir "$G" "$@"; }
wgrun config --local --model pi:test:fake-review --reasoning low --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:fake-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:fake-review --set-reasoning evaluator low --no-reload >/dev/null
wgrun add 'Pi completed fixture' --id pi-done --model pi:test:fake-worker -d $'Produce report.txt.\n\n## Validation\n- [ ] exact reviewed report' >/dev/null
wgrun contract pi-done report >/dev/null; wgrun publish pi-done --only >/dev/null
# Exercise daemon-authorized dispatch and the real spawn wrapper: fake Pi
# stdout is captured once into raw_stream.jsonl while the worker remains live
# through completion review.
wgrun service start --no-coordinator-agent --no-supervise >/dev/null
agent=""
for _ in $(seq 1 200); do
  if [[ -f "$G/service/registry.json" ]]; then
    agent=$(python3 - "$G/service/registry.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
print(next((a['id'] for a in x.get('agents',{}).values() if a.get('task_id')=='pi-done'),''))
PY
)
  fi
  [[ -n "$agent" && -f "$G/agents/$agent/raw_stream.jsonl" ]] && grep -q '"type":"turn_end"' "$G/agents/$agent/raw_stream.jsonl" && break
  sleep 0.05
done
[[ -n "$agent" && -f "$G/agents/$agent/raw_stream.jsonl" ]] || loud_fail "spawn wrapper did not capture Pi raw stream"
printf 'implemented and validated\n' >summary.txt; printf 'reviewed report\n' >report.txt; printf 'validation passed\n' >validation.log
wgrun completion-object report.txt --media-type text/plain >output-ref.json
wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
wgrun completion-manifest pi-done --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
# First exact FLIP exceeds the one-second bounded fixture deadline: an
# infrastructure failure receipt is retained without usage or an invented
# semantic verdict, and eval does not run. Review is advisory by default, so
# deterministic publication remains available.
wgrun submit pi-done --manifest manifest.json --summary summary.txt >"$scratch/unavailable.log" 2>&1
grep -q 'advisory model review status=ReviewUnavailable' "$scratch/unavailable.log" || loud_fail "reviewer failure was not surfaced as advisory unavailable"
# Second exact FLIP semantically rejects. Its receipt also remains visible and
# eval is again correctly skipped without gaining lifecycle authority.
wgrun submit pi-done --manifest manifest.json --summary summary.txt >"$scratch/reject.log" 2>&1
grep -q 'advisory model review status=FlipRejected' "$scratch/reject.log" || loud_fail "semantic advisory FLIP rejection was not surfaced"
# Exercise the real bridge, then simulate archival before the accepted review
# and Done projection. The first candidate selection already captured immutable
# source accounting, so neither registry nor live agent directory is required.
archive="$scratch/archived-agent"; mv "$G/agents/$agent" "$archive"
wgrun pi-stream-bridge --agent-dir "$archive" --exit-code 0 >/dev/null
python3 -c 'import json,sys; e=[json.loads(x) for x in open(sys.argv[1])]; r=[x for x in e if x.get("type")=="result"][-1]; assert r["usage"]["input_tokens"]==205 and abs(r["usage"]["cost_usd"]-0.05)<1e-9' "$archive/stream.jsonl"
rm -f "$G/service/registry.json"
# Resubmit exact bytes: fake FLIP + eval now pass, with provider-reported usage.
wgrun submit pi-done --manifest manifest.json --summary summary.txt >/dev/null
wgrun done pi-done >/dev/null
touch "$PI_WORKER_RELEASE"
show=$(wgrun --json show pi-done)
python3 -c 'import json,sys; d=json.load(sys.stdin); u=d["token_usage"]; assert d["status"]=="done"; assert (u["input_tokens"],u["output_tokens"],u["cache_read_input_tokens"])==(205,17,310),u; assert abs(u["cost_usd"]-0.05)<1e-9,u; assert d["actual_executor"]=="pi"; assert d["actual_model"]=="test:fake-worker"; a=d["completion_review_activity"]; assert len(a)==4,a; assert [x["verdict"] for x in a]==["unavailable","reject","pass","pass"],a; assert [x["candidate_state"] for x in a]==["superseded","superseded","current","current"],a; assert [x["binding"]["candidate_sequence"] for x in a]==[1,2,3,3],a; assert a[0]["failure_class"]=="reviewer_unavailable" and a[1]["failure_class"]=="semantic_rejection",a; assert a[1]["findings"][0]["code"]=="fixture.second_reject",a; assert all(x["duration_ms"]>=0 for x in a),a' <<<"$show"
spend=$(wgrun --json spend)
python3 -c 'import json,sys; d=json.load(sys.stdin); r=d["completion_review_lane"]; assert d["task_count"]==1 and d["total_input_tokens"]==205 and d["total_output_tokens"]==17,d; assert abs(d["total_cost"]-0.05)<1e-9,d; assert r["attempt_count"]==3,r; assert r["total_input_tokens"]==9,r; assert r["total_output_tokens"]==3,r; assert abs(r["total_cost"]-0.003)<1e-9,r' <<<"$spend"
# A real service restart must not erase the terminal projection captured before
# the pre-Done archival above.
wgrun service stop >/dev/null
wgrun service start --no-coordinator-agent --no-supervise >/dev/null; wgrun service stop >/dev/null
show=$(wgrun --json show pi-done)
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["token_usage"]["input_tokens"]==205; assert d["actual_executor"]=="pi"; assert len(d["completion_review_activity"])==4' <<<"$show"
# Raw serialization (not only `show`) must retain the immutable history after
# candidate supersession, landing/Done, and service restart.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
row=next(x for x in map(json.loads,open(sys.argv[1])) if x.get('id')=='pi-done')
a=row['completion_review_activity']
assert len(a)==4,a
assert [x['binding']['candidate_sequence'] for x in a]==[1,2,3,3],a
assert row['completion_candidate']['review_binding']['candidate_sequence']==3,row
PY
list=$(wgrun list --all)
list_json=$(wgrun --json list --all)
python3 -c 'import json,sys; x=json.load(sys.stdin); r=[v for v in x if v.get("kind")=="completion-review-activity"]; assert len(r)==4,r; assert len({v["receipt"] for v in r})==4,r; assert [v["candidate_state"] for v in r]==["superseded","superseded","current","current"],r; assert r[1]["verdict_failure_class"]=="semantic_rejection",r; assert r[1]["findings"][0]["code"]=="fixture.second_reject",r; assert not [v for v in x if v.get("kind")=="evaluation-lane-activity"],x' <<<"$list_json"
grep -q 'internal completion-review lane (virtual audit rows; not graph tasks)' <<<"$list" || loud_fail "list does not explain review lane semantics"
grep -q 'Flip.*Unavailable.*candidate=Superseded.*failure=ReviewerUnavailable.*route=pi:test:fake-review.*executor=pi' <<<"$list" || loud_fail "FLIP timeout virtual row missing"
grep -q 'Flip.*Reject.*candidate=Superseded.*failure=SemanticRejection.*route=pi:test:fake-review.*usage=2in/1out' <<<"$list" || loud_fail "FLIP rejection virtual row/usage missing"
grep -q 'finding \[fixture.second_reject\]: repair and resubmit' <<<"$list" || loud_fail "bounded rejection finding missing"
grep -q 'Eval.*Pass.*candidate=Current.*route=pi:test:fake-review.*usage=4in/1out' <<<"$list" || loud_fail "eval acceptance virtual row/usage missing"
trace=$(wgrun trace show pi-done)
grep -q 'Completion review lane (immutable audit records; not graph tasks)' <<<"$trace" || loud_fail "trace review lane semantics missing"
grep -q 'Eval Pass candidate=Current route=pi:test:fake-review.*usage=4in/1out' <<<"$trace" || loud_fail "trace review route/usage missing"
show_h=$(wgrun show pi-done); grep -q 'Completion review lane (immutable activity; not graph tasks)' <<<"$show_h" || loud_fail "show review lane missing"
help=$(wgrun list --help)
grep -q 'virtual audit rows' <<<"$help" || loud_fail "list help does not define review row semantics"

# Exercise the actual human TUI path, not only its formatting helper. The
# selected task detail must render immutable review chronology without mutating
# the graph while `tui-dump` observes the live PTY screen.
if command -v tmux >/dev/null 2>&1; then
  tui_session="wg-review-activity-$$"
  cleanup_tui(){ tmux kill-session -t "$tui_session" 2>/dev/null || true; }
  add_cleanup_hook cleanup_tui
  tmux new-session -d -x 190 -y 60 -s "$tui_session" "cd '$project' && env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
  sleep .3
  tmux send-keys -t "$tui_session" 1 Enter
  tui_dump(){ "$WG_BIN" --dir "$G" --json tui-dump 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))'; }
  rendered=""
  for _ in $(seq 1 120); do
    rendered=$(tui_dump || true)
    grep -q 'FLIP: Pass route=pi:test:fake-review' <<<"$rendered" \
      && grep -q 'Eval: Pass route=pi:test:fake-review' <<<"$rendered" \
      && grep -q 'findings=b3:' <<<"$rendered" \
      && break
    sleep .05
  done
  grep -q 'FLIP: Pass route=pi:test:fake-review' <<<"$rendered" || loud_fail "live TUI omitted FLIP verdict/route"
  grep -q 'Eval: Pass route=pi:test:fake-review' <<<"$rendered" || loud_fail "live TUI omitted Eval verdict/route"
  grep -q 'findings=b3:' <<<"$rendered" || loud_fail "live TUI omitted verified findings digest"
  before_tui=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
  for _ in $(seq 1 5); do tui_dump >/dev/null; done
  after_tui=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
  [[ "$before_tui" == "$after_tui" ]] || loud_fail "TUI review observation mutated graph authority"
  tmux send-keys -t "$tui_session" q
fi
echo 'PASS: Pi Done accounting survives cleanup/restart; exact review attempts are visible and separately charged'
