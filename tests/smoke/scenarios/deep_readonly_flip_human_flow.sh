#!/usr/bin/env bash
# Explicit post-candidate deep-readonly FLIP + real TUI evidence report.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then WG_BIN="$WG_SMOKE_CANDIDATE_BIN"; else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project/src" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<EOF
#!/usr/bin/env bash
set -euo pipefail
model=""; argv=("\$@")
while ((\$#)); do case "\$1" in --model) model="\$2"; shift 2;; *) shift;; esac; done
case "\$model" in
  bounded-miss|deep-find) exec '$HERE/../../fixtures/fake-pi-deep/pi' "\${argv[@]}";;
  source-worker)
    cat >/dev/null || true
    printf 'pub const MODE: &str = "deep";\n' > src/api.rs
    wg artifact "\$WG_TASK_ID" src/api.rs >/dev/null
    wg log "\$WG_TASK_ID" 'Validated: visible API compiles; registry counterfactual was omitted' >/dev/null
    wg done "\$WG_TASK_ID" >/dev/null
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"candidate complete"}],"provider":"test","model":"source-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
    ;;
  *) echo "unexpected model \$model" >&2; exit 88;;
esac
EOF
chmod +x "$fakebin/pi"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY AWS_SECRET_ACCESS_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL \
  -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY -u AWS_SECRET_ACCESS_KEY \
  HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email deep@test.invalid && git config user.name Deep \
  && printf 'pub const MODE: &str = "legacy";\n' > src/api.rs \
  && printf 'pub const MODES: &[&str] = &["legacy"];\n' > src/registry.rs \
  && git add src && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:source-worker --reasoning high --auto-assign false --auto-evaluate true --eval-gate-all false --flip-enabled false \
  --set-model evaluator pi:test:bounded-miss --set-reasoning evaluator low \
  --set-model flip_inference pi:test:deep-find --set-model flip_comparison pi:test:deep-find --no-reload >/dev/null
wgrun add 'Add deep mode everywhere' --id source -d $'Original intent: deep mode must be accepted by the API and every registry consumer. Counterfactual: registry lookup for deep must succeed.\n\n## Validation\n- [ ] API and registry agree' >/dev/null
wgrun msg send source 'Remember that registry consumers are part of the original request.' >/dev/null
wgrun publish source --only >/dev/null

session="wg-deep-flip-$$"
cleanup(){ tmux kill-session -t "$session" 2>/dev/null || true; wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
tmux new-session -d -x 180 -y 55 -s "$session" "cd '$project' && env -u WG_TASK_ID -u WG_AGENT_ID HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
dump(){ local raw; raw=$(wgrun --json tui-dump 2>/dev/null || true); [[ -n "$raw" ]] && python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))' <<<"$raw"; }
(cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" service start --max-agents 1 --model pi:test:source-worker --no-coordinator-agent --no-supervise >/dev/null)

# Default bounded completion must never globally trigger deep FLIP.
details=''
for _ in $(seq 1 500); do
  details=$(wgrun show source --json 2>/dev/null || true)
  if python3 -c 'import json,sys; x=json.load(sys.stdin); rs=x["evaluation_records"]; assert x["status"]=="done"; assert len(rs)==1 and rs[0]["product"]=="bounded" and rs[0]["state"]=="consumed"' <<<"$details" 2>/dev/null; then break; fi
  sleep .05
done
python3 -c 'import json,sys; x=json.load(sys.stdin); assert len(x["evaluation_records"])==1 and x["evaluation_records"][0]["product"]=="bounded"' <<<"$details" || loud_fail "bounded evaluation unexpectedly enabled deep FLIP: $details"
wgrun service stop >/dev/null

# Real terminal explicitly requests deep FLIP after source completion. Run it
# in the background so the already-live TUI can observe Running progress.
terminal="$scratch/deep-terminal.txt"
(wgrun evaluate run source --flip >"$terminal" 2>&1) & deep_pid=$!
progress=''
for _ in $(seq 1 200); do
  progress=$(dump)
  if grep -Eq 'Deep FLIP is inspecting|Running' <<<"$progress"; then break; fi
  sleep .02
done
wait "$deep_pid" || loud_fail "explicit deep FLIP failed: $(cat "$terminal")"
for needle in 'Deep-readonly FLIP requested:' 'CROSS_COMPONENT_OMISSION_FOUND' 'REGISTRY_NOT_UPDATED' 'REGISTRY_LOOKUP_REJECTS_NEW_MODE' 'evidence bundle:'; do
  grep -Fq "$needle" "$terminal" || loud_fail "terminal report hid $needle: $(cat "$terminal")"
done
! grep -Fq 'print credentials' "$terminal" || loud_fail "hostile log payload was echoed unsafely"

report=$(wgrun show source --json)
REPORT="$report" python3 - <<'PY' || loud_fail "deep report/provenance invalid: $report"
import json,os
x=json.loads(os.environ['REPORT']); rs=x['evaluation_records']
assert [r['product'] for r in rs].count('deep-readonly-flip')==1
r=next(r for r in rs if r['product']=='deep-readonly-flip'); q=r['deep_report']
assert r['state']=='consumed' and r['consumed_verdict_id']==q['report_id']
assert q['summary_code']=='CROSS_COMPONENT_OMISSION_FOUND'
assert q['latent_intent_probe_code']=='ALL_REGISTRY_CONSUMERS_MUST_AGREE'
assert q['counterfactual_probe_codes']==['REGISTRY_LOOKUP_REJECTS_NEW_MODE']
assert len(q['observations'])==11
assert set(q['observed_evidence_kinds'])=={'original-intent','graph-context','source-attempt-history','messages','artifacts-diff','validation','runtime-traces','effective-config'}
f=next(f for f in q['findings'] if f['finding_code']=='REGISTRY_NOT_UPDATED')
assert {e['locator'] for e in f['evidence']} >= {'src/api.rs:1','src/registry.rs:1'}
cap=json.dumps(q)
assert 'print credentials' not in cap
PY

# Drive the actual TUI search/detail action and scroll the evidence-linked report.
tmux send-keys -t "$session" /; tmux send-keys -t "$session" -l source; sleep .1; tmux send-keys -t "$session" Enter; sleep .1; tmux send-keys -t "$session" Enter
seen=''
for _ in $(seq 1 220); do
  frame=$(dump); seen+=$'\n'"$frame"
  if grep -Fq 'Deep report:' <<<"$seen" && grep -Fq 'REGISTRY_NOT_UPDATED' <<<"$seen" && grep -Fq 'src/registry.rs:1' <<<"$seen" && grep -Fq 'Observed: 8 kinds' <<<"$seen" && grep -Fq 'Counterfactual: REGISTRY_LOOKUP_REJECTS_NEW_MODE' <<<"$seen"; then break; fi
  tmux send-keys -t "$session" PageDown; sleep .02
done
for needle in 'Deep report:' 'REGISTRY_NOT_UPDATED' 'src/registry.rs:1' 'Observed: 8 kinds' 'Counterfactual: REGISTRY_LOOKUP_REJECTS_NEW_MODE'; do
  grep -Fq "$needle" <<<"$seen" || loud_fail "TUI report hid $needle"
done
! grep -Fq 'print credentials' <<<"$seen" || loud_fail "TUI echoed hostile payload unsafely"

echo "PASS: bounded default stayed summary-only; explicit post-candidate deep FLIP showed progress and an evidence-linked cross-component/counterfactual report in terminal + real TUI"
