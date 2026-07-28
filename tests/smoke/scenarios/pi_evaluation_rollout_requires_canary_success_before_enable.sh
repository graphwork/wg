#!/usr/bin/env bash
# RED-first staged rollout regression for the Pi evaluation plane.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
project="$scratch/project"; home="$scratch/home"
mkdir -p "$project" "$home/.config"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL
(cd "$project" && git init -q -b main && git config user.email rollout@test.invalid && git config user.name Rollout && printf 'base\n' > base.txt && git add base.txt && git commit -qm base && "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "$WG_BIN" --dir "$G" "$@"); }

# Configure an exact fake Pi identity so the real daemon/reload path can run
# credential-free; no model call occurs in this rollout-control scenario.
wgrun config --local --model pi:test:fake --reasoning low --no-reload >/dev/null
# Explicit persisted start is safe even when legacy defaults/config drift.
wgrun evaluate rollout start >/dev/null
cleanup(){ wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
start_daemon(){ wgrun service start --no-coordinator-agent --no-supervise >/dev/null; }
restart_daemon(){ wgrun service stop >/dev/null 2>&1 || true; start_daemon; wgrun --json evaluate rollout status >/dev/null; }
start_daemon
status=$(wgrun --json evaluate rollout status)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["stage"]=="disabled"; assert x["auto_evaluate"] is False; assert x["eval_gate_all"] is False; assert x["global_flip_enabled"] is False' <<<"$status"

# Neither the controller nor a raw config edit may skip forward.
if wgrun evaluate rollout advance --stage advisory >/tmp/rollout-skip.out 2>&1; then
  loud_fail "rollout accepted a direct disabled -> advisory skip"
fi
grep -qi 'next required stage.*fake-pi-validated' /tmp/rollout-skip.out || loud_fail "stage-skip refusal was not actionable"
if wgrun config set agency.auto_evaluate true >/tmp/rollout-dotted.out 2>&1; then
  loud_fail "generic dotted config setter bypassed the managed rollout"
fi
grep -qi 'managed evaluation rollout owns' /tmp/rollout-dotted.out || loud_fail "dotted setter refusal was not actionable"
cp "$G/config.toml" "$scratch/config.safe"
python3 - "$G/config.toml" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]); s=p.read_text()
assert 'rollout_stage = "disabled"' in s
p.write_text(s.replace('rollout_stage = "disabled"', 'rollout_stage = "advisory"', 1))
PY
if wgrun service reload >/tmp/rollout-reload.out 2>&1; then
  loud_fail "service reload accepted a forged rollout stage"
fi
grep -Eqi 'rollout|canary|stage' /tmp/rollout-reload.out || loud_fail "reload refusal hid rollout reason"
cp "$scratch/config.safe" "$G/config.toml"

cat >"$scratch/fake.json" <<'JSON'
{"schema":1,"kind":"fake-pi-lifecycle","success":true,"route":"pi:test:fake","source_completions":1,"evaluation_verdicts":1,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"before_viz_cid":"b3:before","after_viz_cid":"b3:after","notes":["publish-execute-lazy-evaluate-terminal","cancel-skip-nonspawn-zero"]}
JSON
cat >"$scratch/bounded.json" <<'JSON'
{"schema":1,"kind":"bounded-live-canary","success":true,"route":"pi:luna:bounded-canary","source_completions":1,"evaluation_verdicts":1,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"before_viz_cid":"b3:before-bounded","after_viz_cid":"b3:after-bounded","notes":["optional secondary only"]}
JSON
cat >"$scratch/deep.json" <<'JSON'
{"schema":1,"kind":"deep-readonly-flip","success":true,"route":"pi:luna:deep-canary","source_completions":1,"evaluation_verdicts":1,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"observation_only":true,"latent_intent_findings":1,"counterfactual_findings":3,"cross_system_findings":1,"before_viz_cid":"b3:before-deep","after_viz_cid":"b3:after-deep","notes":["explicit deep read-only; not bounded FLIP"]}
JSON
cat >"$scratch/gate.json" <<'JSON'
{"schema":1,"kind":"flip-required-gate","success":true,"route":"pi:openai-codex:gpt-5.6-luna","source_completions":3,"evaluation_verdicts":3,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"observation_only":true,"latent_intent_findings":1,"counterfactual_findings":2,"cross_system_findings":1,"semantic_reject_preserved":true,"infrastructure_retry_converged":true,"restart_boundaries_proven":true,"main_unchanged_pending_reject_unavailable":true,"main_advanced_once_on_pass":true,"gate_left_disabled":true,"before_viz_cid":"b3:before-required","after_viz_cid":"b3:after-required","notes":["operator activates only after exact-main install"]}
JSON

wgrun evaluate rollout advance --stage fake-pi-validated --evidence "$scratch/fake.json" >/dev/null
restart_daemon
# Bounded grading is optional/secondary and cannot precede or gate FLIP.
if wgrun evaluate rollout advance --stage bounded-canary-passed --evidence "$scratch/bounded.json" >/tmp/bounded-first.out 2>&1; then
  loud_fail "managed rollout still required/accepted bounded-first policy"
fi
wgrun evaluate rollout advance --stage deep-readonly-canary-passed --evidence "$scratch/deep.json" >/dev/null
restart_daemon
wgrun evaluate rollout advance --stage flip-required --evidence "$scratch/gate.json" >/dev/null
restart_daemon

status=$(wgrun --json evaluate rollout status)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["stage"]=="flip-required"; assert x["auto_evaluate"] is False; assert x["mode"]=="flip-required"; assert x["eval_gate_all"] is False; assert x["global_flip_enabled"] is True; assert len(x["evidence"])==3' <<<"$status"
if wgrun config --local --eval-gate-all true >/tmp/hard-gate.out 2>&1; then
  loud_fail "managed rollout accepted eval_gate_all even though FLIP selection is independent"
fi

# Record multiple observed completions plus explicit rollback thresholds through
# the actual terminal surface before exercising rollback.
cat >"$scratch/observation.json" <<'JSON'
{"schema":1,"kind":"source-observation","success":true,"route":"pi:luna:bounded-advisory","source_completions":3,"evaluation_verdicts":3,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"before_viz_cid":"b3:before-observation","after_viz_cid":"b3:after-observation","notes":["rollback: any duplicate or stuck pending","rollback: any worker/build/worktree use","rollback: any Codex rewrite or global gate"]}
JSON
wgrun evaluate rollout record-observation --evidence "$scratch/observation.json" >/dev/null
restart_daemon
# Exercise the actual operator rollback path, not an in-memory helper.
wgrun evaluate rollout rollback --reason 'operator canary threshold exercised' >/dev/null
restart_daemon
status=$(wgrun --json evaluate rollout status)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["stage"]=="disabled"; assert x["auto_evaluate"] is False; assert x["rollback_count"]==1; assert x["eval_gate_all"] is False; assert x["global_flip_enabled"] is False' <<<"$status"

# Audit is machine-readable and includes before/after Viz evidence.
evidence="$G/agency/evaluation-plane/canary-evidence.json"
python3 - "$evidence" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
assert x['schema']==1
assert [e['kind'] for e in x['evidence']]==['fake-pi-lifecycle','deep-readonly-flip','flip-required-gate','source-observation']
assert x['evidence'][-1]['source_completions'] >= 3
assert all(e['before_viz_cid'] and e['after_viz_cid'] for e in x['evidence'])
assert x['rollbacks'] and x['rollbacks'][-1]['reason']=='operator canary threshold exercised'
PY

echo "PASS: managed rollout reaches required deep FLIP without bounded-first, keeps auto_evaluate/eval_gate_all false, proves pass/reject/infrastructure/restart semantics, and rolls back atomically"
