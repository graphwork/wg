#!/usr/bin/env bash
# Required deep FLIP gates immutable candidates before merge; bounded is absent.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then WG_BIN="$WG_SMOKE_CANDIDATE_BIN"; else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project/src" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<EOF
#!/usr/bin/env bash
set -euo pipefail
model=""; argv=("\$@")
while ((\$#)); do case "\$1" in --model) model="\$2"; shift 2;; *) shift;; esac; done
case "\$model" in
  deep-pass|deep-find) exec '$HERE/../../fixtures/fake-pi-deep/pi' "\${argv[@]}";;
  source-worker)
    cat >/dev/null || true
    case "\$WG_TASK_ID" in
      pass-source)
        printf 'pub const MODE: &str = "deep";\n' > src/api.rs
        printf 'pub const MODES: &[&str] = &["legacy", "deep"];\n' > src/registry.rs
        wg artifact "\$WG_TASK_ID" src/api.rs >/dev/null
        wg artifact "\$WG_TASK_ID" src/registry.rs >/dev/null;;
      reject-source) printf 'pub const MODE: &str = "rejected";\n' > src/api.rs; wg artifact "\$WG_TASK_ID" src/api.rs >/dev/null;;
      unavailable-source) printf 'unavailable candidate\n' > src/unavailable.rs; wg artifact "\$WG_TASK_ID" src/unavailable.rs >/dev/null;;
    esac
    wg log "\$WG_TASK_ID" 'Validated: deterministic source validation passed' >/dev/null
    wg done "\$WG_TASK_ID" >/dev/null
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"candidate complete"}],"provider":"test","model":"source-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}';;
  *) echo "selected Pi route unavailable: \$model" >&2; exit 88;;
esac
EOF
chmod +x "$fakebin/pi"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY AWS_SECRET_ACCESS_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email flip@test.invalid && git config user.name Flip && printf 'pub const MODE: &str = "legacy";\n' > src/api.rs && printf 'pub const MODES: &[&str] = &["legacy"];\n' > src/registry.rs && git add src && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:source-worker --reasoning high --auto-assign false --auto-evaluate false --eval-gate-all false --flip-enabled false --set-model flip_inference pi:test:deep-pass --set-model flip_comparison pi:test:deep-pass --set-reasoning flip_inference high --set-reasoning flip_comparison high --no-reload >/dev/null
wgrun evaluate rollout start >/dev/null
cat >"$scratch/fake.json" <<'JSON'
{"schema":1,"kind":"fake-pi-lifecycle","success":true,"route":"pi:test:fake","source_completions":1,"evaluation_verdicts":1,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"before_viz_cid":"b3:fb","after_viz_cid":"b3:fa"}
JSON
cat >"$scratch/deep.json" <<'JSON'
{"schema":1,"kind":"deep-readonly-flip","success":true,"route":"pi:test:deep-pass","source_completions":1,"evaluation_verdicts":1,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"observation_only":true,"latent_intent_findings":1,"counterfactual_findings":1,"cross_system_findings":1,"before_viz_cid":"b3:db","after_viz_cid":"b3:da"}
JSON
cat >"$scratch/gate.json" <<'JSON'
{"schema":1,"kind":"flip-required-gate","success":true,"route":"pi:test:deep-pass","source_completions":3,"evaluation_verdicts":3,"never_ran_evaluations":0,"stuck_pending_evaluations":0,"duplicate_records":0,"duplicate_verdicts":0,"worker_slots_used":0,"build_slots_used":0,"worktrees_created":0,"admission_deferrals_neutral":true,"native_codex_route_preserved":true,"observation_only":true,"latent_intent_findings":1,"counterfactual_findings":1,"cross_system_findings":1,"semantic_reject_preserved":true,"infrastructure_retry_converged":true,"restart_boundaries_proven":true,"main_unchanged_pending_reject_unavailable":true,"main_advanced_once_on_pass":true,"gate_left_disabled":true,"before_viz_cid":"b3:gb","after_viz_cid":"b3:ga"}
JSON
wgrun evaluate rollout advance --stage fake-pi-validated --evidence "$scratch/fake.json" >/dev/null
wgrun evaluate rollout advance --stage deep-readonly-canary-passed --evidence "$scratch/deep.json" >/dev/null
wgrun evaluate rollout advance --stage flip-required --evidence "$scratch/gate.json" >/dev/null
status=$(wgrun --json evaluate rollout status)
python3 -c 'import json,sys;x=json.load(sys.stdin);assert x["auto_evaluate"] is False and x["eval_gate_all"] is False and x["global_flip_enabled"] is True' <<<"$status"

session="wg-flip-first-$$"
cleanup(){ tmux kill-session -t "$session" 2>/dev/null || true; wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
tmux new-session -d -x 180 -y 55 -s "$session" "cd '$project' && env -u WG_TASK_ID -u WG_AGENT_ID HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on
dump(){ local raw; raw=$(wgrun --json tui-dump 2>/dev/null || true); [[ -n "$raw" ]] && python3 -c 'import json,sys;print(json.load(sys.stdin).get("text",""))' <<<"$raw"; }
capture(){ tmux capture-pane -p -t "$session" 2>/dev/null || true; }
coord(){ capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
 x=row.find(needle)
 if x>=0: print(x+1,y); raise SystemExit
raise SystemExit(1)' "$1"; }
click_text(){ local xy x y; xy=$(coord "$1") || loud_fail "click target $1 missing"; read -r x y <<<"$xy"; tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"; }
start_service(){ wgrun service start --max-agents 1 --model pi:test:source-worker --no-coordinator-agent --no-supervise >/dev/null; }
wait_state(){ local id="$1" py="$2" out=''; for _ in $(seq 1 800); do out=$(wgrun show "$id" --json 2>/dev/null || true); if python3 -c "$py" <<<"$out" 2>/dev/null; then printf '%s' "$out"; return 0; fi; sleep .05; done; loud_fail "timeout waiting for $id: $out"; }

base=$(git -C "$project" rev-parse refs/heads/main)
wgrun add 'Pass required FLIP' --id pass-source -d $'Ordinary coding task: update API and registry.\n\n## Validation\n- [ ] API and registry agree' >/dev/null; wgrun publish pass-source --only >/dev/null
start_service
pending=$(wait_state pass-source 'import json,sys;x=json.load(sys.stdin);assert x["status"]=="pending-eval" and x["flip_gate"]["state"] in ("flip-queued","flip-running")')
[[ "$(git -C "$project" rev-parse refs/heads/main)" == "$base" ]] || loud_fail "main advanced while required FLIP pending"
passed=$(wait_state pass-source 'import json,sys;x=json.load(sys.stdin);assert x["status"]=="done" and x["flip_gate"]["state"]=="flip-passed-merged";assert [r["product"] for r in x["evaluation_records"]]==["deep-readonly-flip"]')
pass_main=$(git -C "$project" rev-parse refs/heads/main); [[ "$pass_main" != "$base" ]] || loud_fail "passing FLIP did not merge"
wgrun service stop >/dev/null; start_service; sleep .4
[[ "$(git -C "$project" rev-parse refs/heads/main)" == "$pass_main" ]] || loud_fail "restart merged accepted candidate twice"

# Drive the actual selected-task TUI detail for the accepted required gate.
tmux send-keys -t "$session" /; tmux send-keys -t "$session" -l pass-source; sleep .1; tmux send-keys -t "$session" Enter; sleep .1; tmux send-keys -t "$session" Enter
seen=''; for _ in $(seq 1 220); do frame=$(dump); seen+=$'\n'"$frame"; grep -Fq 'Required FLIP Acceptance' <<<"$seen" && grep -Fq 'flip passed merged' <<<"$seen" && grep -Fq 'FULL_SYSTEM_INTENT_SATISFIED' <<<"$seen" && break; tmux send-keys -t "$session" PageDown; sleep .02; done
for needle in 'Required FLIP Acceptance' 'flip passed merged' 'FULL_SYSTEM_INTENT_SATISFIED'; do grep -Fq "$needle" <<<"$seen" || { printf '%s\n' "$seen" >&2; loud_fail "TUI missing $needle"; }; done

wgrun service stop >/dev/null
wgrun config --local --set-model flip_inference pi:test:deep-find --set-model flip_comparison pi:test:deep-find --no-reload >/dev/null
wgrun add 'Reject required FLIP' --id reject-source -d $'Ordinary coding task with a planted cross-component omission.\n\n## Validation\n- [ ] API and registry agree' >/dev/null; wgrun publish reject-source --only >/dev/null
start_service
rejected=$(wait_state reject-source 'import json,sys;x=json.load(sys.stdin);assert x["status"]=="pending-eval" and x["flip_gate"]["state"]=="flip-rejected-repair-needed" and x.get("retry_count",0)==0;assert x["flip_gate"]["report_id"]')
python3 - "$G/finalization/transactions/reject-source.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
assert x['phase']=='repair-needed', x
assert x['retained_reason'].startswith('acceptance.rejected:deep-report-'), x
assert not x.get('merge_receipt'), x
PY
[[ "$(git -C "$project" rev-parse refs/heads/main)" == "$pass_main" ]] || loud_fail "semantic reject changed main"

wgrun service stop >/dev/null
wgrun config --local --set-model flip_inference pi:test:unavailable --set-model flip_comparison pi:test:unavailable --no-reload >/dev/null
wgrun add 'Unavailable required FLIP' --id unavailable-source -d $'Ordinary coding task whose selected Pi adapter is unavailable.\n\n## Validation\n- [ ] preserve candidate' >/dev/null; wgrun publish unavailable-source --only >/dev/null
start_service
unavailable=$(wait_state unavailable-source 'import json,sys;x=json.load(sys.stdin);assert x["status"]=="pending-eval" and x["flip_gate"]["state"]=="flip-infrastructure-unavailable" and x.get("retry_count",0)==0')
if [[ "$(git -C "$project" rev-parse refs/heads/main)" != "$pass_main" ]]; then
  git -C "$project" log --oneline --decorate -8 >&2 || true
  loud_fail "infrastructure failure changed main"
fi

# The real graph-wide Activity surface safely distinguishes retained outcomes
# with candidate/report IDs and its normal relative + system clocks.
click_text "⌂"
activity=''; for _ in $(seq 1 240); do frame=$(capture); activity+=$'\n'"$frame"; grep -Fq '⌂ Activity' <<<"$frame" && grep -Fq 'FLIP infrastructure unavailable' <<<"$frame" && break; sleep .04; done
for _ in $(seq 1 24); do tmux send-keys -t "$session" PageUp; sleep .03; frame=$(capture); activity+=$'\n'"$frame"; grep -Fq 'FLIP passed—merged' <<<"$activity" && grep -Fq 'FLIP rejected—repair needed' <<<"$activity" && break; done
for needle in '⌂ Activity' 'FLIP rejected—repair needed' 'FLIP infrastructure unavailable' 'FLIP passed—merged' 'c=' 'r='; do grep -Fq "$needle" <<<"$activity" || { printf '%s\n' "$activity" >&2; loud_fail "Activity missing $needle"; }; done
grep -Eq '[0-9]+[smhd] · [0-9]{2}:[0-9]{2}:[0-9]{2}' <<<"$activity" || loud_fail "Activity omitted relative/system times"

# Explicit waiver is operator-only, candidate+report bound and audited. It is
# never a silent promotion and merges only the named retained bytes.
reject_candidate=$(python3 -c 'import json,sys;x=json.load(sys.stdin);print(x["flip_gate"]["candidate_id"])' <<<"$rejected")
reject_report=$(python3 -c 'import json,sys;x=json.load(sys.stdin);print(x["flip_gate"]["report_id"])' <<<"$rejected")
wgrun candidate waive "$reject_candidate" --report "$reject_report" --reason 'low-risk operator canary waiver' >/dev/null
waived=$(wgrun show reject-source --json)
python3 -c 'import json,sys;x=json.load(sys.stdin);assert x["status"]=="done"; assert any("AUDITED FLIP WAIVER" in e["message"] for e in x["log"])' <<<"$waived"

echo "PASS: deep-only required FLIP held main pending, merged pass exactly once, retained reject/unavailable without source retry, and exposed safe JSON repair actions plus accepted-gate TUI detail"
