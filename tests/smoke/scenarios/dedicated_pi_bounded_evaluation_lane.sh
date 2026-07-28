#!/usr/bin/env bash
# Real daemon + terminal/TUI flow for the hidden, no-worker Pi evaluation lane.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
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
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<EOF
#!/usr/bin/env bash
set -euo pipefail
model=""; argv=("\$@")
while ((\$#)); do case "\$1" in --model) model="\$2"; shift 2;; *) shift;; esac; done
if [[ "\$model" == fake-valid ]]; then
  exec '$HERE/../../fixtures/fake-pi-bounded/pi' "\${argv[@]}"
fi
[[ "\$model" == fake-worker ]] || { echo "unexpected model \$model" >&2; exit 88; }
for name in OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY; do
  [[ -z "\${!name:-}" ]] || { echo "credential unexpectedly present: \$name" >&2; exit 91; }
done
cat >/dev/null || true
printf 'dedicated lane candidate\n' > dedicated-lane-artifact.txt
wg artifact "\$WG_TASK_ID" dedicated-lane-artifact.txt >/dev/null
wg done "\$WG_TASK_ID" >/dev/null
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"candidate complete"}],"provider":"test","model":"fake-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
EOF
chmod +x "$fakebin/pi"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY AWS_SECRET_ACCESS_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL \
  -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY -u AWS_SECRET_ACCESS_KEY \
  HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email lane@test.invalid && git config user.name Lane && printf 'base\n' > base.txt && git add base.txt && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:fake-worker --reasoning low --auto-assign false --auto-evaluate true --eval-gate-all true --flip-enabled false --set-model evaluator pi:test:fake-valid --set-reasoning evaluator low --no-reload >/dev/null
wgrun add 'Dedicated Pi source' --id dedicated-source -d $'Implement one artifact.\n\n## Validation\n- [ ] artifact exists' >/dev/null
wgrun publish dedicated-source --only >/dev/null

session="wg-dedicated-pi-eval-$$"
cleanup(){ tmux kill-session -t "$session" 2>/dev/null || true; wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
tmux new-session -d -x 160 -y 50 -s "$session" "cd '$project' && env -u WG_TASK_ID -u WG_AGENT_ID -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
dump(){ local raw; raw=$(wgrun --json tui-dump 2>/dev/null || true); [[ -n "$raw" ]] && python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))' <<<"$raw"; }
for _ in $(seq 1 200); do dump | grep -Fq dedicated-source && break; sleep .05; done
dump | grep -Fq dedicated-source || loud_fail "TUI did not show source"

(cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" service start --max-agents 1 --model pi:test:fake-worker --no-coordinator-agent --no-supervise >/dev/null)
details=''
for _ in $(seq 1 400); do
  details=$(wgrun show dedicated-source --json 2>/dev/null || true)
  if python3 -c 'import json,sys; x=json.load(sys.stdin); r=x["evaluation_records"][0]; assert x["status"]=="done"; assert r["state"]=="consumed"' <<<"$details" 2>/dev/null; then break; fi
  sleep .1
done
DETAILS="$details" python3 - "$G" <<'PY' || loud_fail "dedicated record/usage/provenance invalid: $details"
import json,sys,os
x=json.loads(os.environ['DETAILS']); r=x['evaluation_records'][0]; a=r['attempts'][0]; v=r['verdict']
assert r['product']=='bounded' and r['state']=='consumed'
assert r['route']['calls'][0]['exact_route']=='pi:test:fake-valid'
assert a['executor']=='pi' and a['exact_route']=='pi:test:fake-valid' and a['reasoning']=='low'
assert a['usage']['input_tokens']==17 and a['usage']['output_tokens']==9 and abs(a['usage']['cost_usd']-.0033)<1e-9
assert r['consumed_verdict_id']==v['verdict_id'] and v['score']==.92
assert r['evidence_manifest_id'].startswith('wgcid:v1:blake3:')
assert len(r['attempts'])==1
manifest_path=os.path.join(sys.argv[1], 'evaluation', 'evidence', r['evidence_manifest_id'].replace(':','_'))
m=json.load(open(manifest_path))
for key in ['original_intent','task_contract','source_attempt_route','artifact_diff_summary','declared_validation','runtime_events','dependency_context','budgets','spotlight_contract']:
    assert key in m, key
assert m['source_attempt_route']['exact_route'].endswith('test:fake-worker')
assert m['artifact_diff_summary']['delta_manifest_digest'].startswith('wgcid:')
assert m['budgets']['total_bytes']==65536
assert len(json.dumps(m).encode()) <= m['budgets']['total_bytes']
# The only registered execution process is the source worker; no evaluator task
# or agent/worktree/build slot exists.
rows=[json.loads(line) for line in open(os.path.join(sys.argv[1],'graph.jsonl')) if line.strip()]
assert not [row for row in rows if row.get('id','').startswith('.evaluate-')]
PY
agent_dirs=$(find "$G/agents" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
[[ "$agent_dirs" == 1 ]] || loud_fail "evaluation allocated an agent/worker slot (agent dirs=$agent_dirs)"
worktree_dirs=$(find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
[[ "$worktree_dirs" -le 1 ]] || loud_fail "evaluation allocated a second worktree (count=$worktree_dirs)"
! find "$G/evaluation" -name .git -print -quit | grep -q . || loud_fail "evaluation runtime is a worktree"
[[ ! -e "$G/service/disk/build-targets" ]] || loud_fail "evaluation entered build admission/cache allocation"

# Operator terminal status names the separate lane and provenance.
status=$(wgrun service status)
grep -Fq 'Evaluation lane:' <<<"$status" || loud_fail "service status hid lane accounting: $status"
grep -Fq 'completed=1' <<<"$status" || loud_fail "service status hid completion: $status"
show=$(wgrun show dedicated-source)
for needle in 'pi:test:fake-valid' 'Pi-reported usage:' 'verdict:' 'consumed=true' 'evidence manifest:'; do
  grep -Fq "$needle" <<<"$show" || loud_fail "show hid $needle: $show"
done

# Drive the actual TUI search/detail action and scroll until route, verdict and
# Pi usage provenance are painted together.
tmux send-keys -t "$session" /
tmux send-keys -t "$session" -l dedicated-source
sleep .2
tmux send-keys -t "$session" Enter
sleep .1
tmux send-keys -t "$session" Enter
seen=''
for _ in $(seq 1 180); do
  frame=$(dump); seen+=$'\n'"$frame"
  if grep -Fq 'Evaluation Evidence (hidden)' <<<"$seen" \
    && grep -Fq 'pi:test:fake-valid' <<<"$seen" \
    && grep -Fq 'Pi usage:' <<<"$seen" \
    && grep -Fq 'Verdict: Pass' <<<"$seen"; then break; fi
  tmux send-keys -t "$session" PageDown
  sleep .03
done
for needle in 'Evaluation Evidence (hidden)' 'pi:test:fake-valid' 'Pi usage:' 'Verdict: Pass'; do
  grep -Fq "$needle" <<<"$seen" || loud_fail "TUI detail hid $needle"
done

# Repeated daemon ticks deliver nothing twice.
sleep .4
again=$(wgrun show dedicated-source --json)
python3 -c 'import json,sys; r=json.load(sys.stdin)["evaluation_records"][0]; assert len(r["attempts"])==1; assert r["consumed_verdict_id"]==r["verdict"]["verdict_id"]' <<<"$again" || loud_fail "duplicate delivery consumed"

echo "PASS: daemon completed source through dedicated no-worker Fake-Pi lane; terminal/TUI showed pinned route, state, verdict, failure/usage provenance; verdict stayed exactly once"
