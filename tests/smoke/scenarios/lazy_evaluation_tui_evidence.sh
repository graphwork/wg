#!/usr/bin/env bash
# Real tmux/TUI + credential-free Fake-Pi flow for lazy candidate evaluation.
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
ln -s "$HERE/../../fixtures/fake-pi-lazy/pi" "$fakebin/pi"
ln -s "$WG_BIN" "$fakebin/wg"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL \
  -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY \
  HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email lazy@test.invalid && git config user.name Lazy && printf 'base\n' > base.txt && git add base.txt && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:fake-worker --reasoning low --auto-assign false --auto-evaluate true --flip-enabled false --no-reload >/dev/null

# Publish a visible workspace whose ordinary work cannot run yet, plus one
# source that Fake-Pi will actually complete.
wgrun add workspace-gate --id workspace-gate >/dev/null
for n in 1 2 3 4 5; do
  wgrun add "Queued $n" --id "queued-$n" --after workspace-gate >/dev/null
  wgrun publish "queued-$n" --only >/dev/null
done
wgrun add lazy-source --id lazy-source -d $'Credential-free Fake-Pi source.\n\n## Validation\n- immutable candidate completion' >/dev/null
wgrun publish lazy-source --only >/dev/null
python3 - "$G/graph.jsonl" <<'PY' || loud_fail "publication created eager evaluation work"
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
assert not [r for r in rows if r.get('id','').startswith(('.evaluate-','.flip-'))], rows
assert all(not r.get('evaluation_records') for r in rows), rows
assert len([r for r in rows if not r.get('id','').startswith('.')]) == 7, rows
PY
plain_before=$(wgrun viz --all --no-tui)
! grep -Eq '\.evaluate-|\.flip-' <<<"$plain_before" || loud_fail "default Viz was cluttered before completion"

session="wg-lazy-eval-$$"
cleanup(){ tmux kill-session -t "$session" 2>/dev/null || true; wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
tmux new-session -d -x 150 -y 46 -s "$session" "cd '$project' && env -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
dump(){ local raw; raw=$(wgrun --json tui-dump 2>/dev/null || true); [[ -n "$raw" ]] && python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))' <<<"$raw"; }
wait_dump(){ local needle=$1; for _ in $(seq 1 240); do dump | grep -Fq "$needle" && return 0; sleep .05; done; loud_fail "TUI never showed '$needle': $(tmux capture-pane -p -t "$session" | tr '\n' '|')"; }
wait_dump_absent(){ local needle=$1; for _ in $(seq 1 240); do ! dump | grep -Fq "$needle" && return 0; sleep .05; done; loud_fail "TUI kept showing '$needle': $(tmux capture-pane -p -t "$session" | tr '\n' '|')"; }
wait_dump 'lazy-source'
! dump | grep -Fq 'Evaluation Evidence (hidden)' || loud_fail "TUI showed evidence before candidate completion"

# Installed service + generated Pi wrapper performs the real source attempt,
# launch permit, terminal intent, quiescent checkpoint and completion.
(cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" service start --max-agents 1 --model pi:test:fake-worker --no-coordinator-agent --no-supervise >/dev/null)
state=open
details=''; count=0
for _ in $(seq 1 300); do
  details=$(wgrun show lazy-source --json 2>/dev/null || true)
  if [[ -n "$details" ]] && python3 -c 'import json,sys; json.load(sys.stdin)' <<<"$details" 2>/dev/null; then
    state=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' <<<"$details")
    count=$(python3 -c 'import json,sys; print(len(json.load(sys.stdin).get("evaluation_records", [])))' <<<"$details")
    [[ "$state" == done && "$count" == 1 ]] && break
  fi
  sleep .1
done
if [[ "$state" != done || "$count" != 1 ]]; then
  worker_tail=$(find "$G/agents" -name output.log -type f -exec tail -80 {} \; 2>/dev/null || true)
  loud_fail "Fake-Pi candidate did not mint exactly one record: $details WORKER=$worker_tail"
fi
python3 -c 'import json,sys; x=json.load(sys.stdin); r=x["evaluation_records"][0]; assert r["product"]=="bounded"; assert r["source"]["source_attempt_id"].startswith("attempt-"); assert r["source"]["candidate_digest"].startswith("wgcid:"); assert r["route"]["calls"][0]["exact_route"]=="pi:test:fake-worker"; assert r.get("consumed_verdict_id") is None' <<<"$details" || loud_fail "record binding/route mismatch"

# The workspace/Chat surface remains clean even after evidence exists. Digit 1
# is the real key-dispatch action for the selected task's Detail tab.
! dump | grep -Fq 'Evaluation Evidence (hidden)' || loud_fail "hidden evidence leaked into the default workspace"
# Use the real graph-search action to select the exact source (rather than
# relying on sort order), accept the jump, then Enter performs the actual
# selected-task Detail action. End scrolls the real inspector to the evidence.
# The TUI opens in Graph command focus; `/` owns the exact-task search.
tmux send-keys -t "$session" /
tmux send-keys -t "$session" -l 'lazy-source'
# Graph matching is a latest-wins background snapshot. Wait until the actual
# filtered frame owns the query before accepting it, or an immediate Enter can
# correctly find no current match while the derivation is still pending.
wait_dump_absent 'workspace-gate'
tmux send-keys -t "$session" Enter
wait_dump_absent '/lazy-source'
tmux send-keys -t "$session" Enter
sleep .2
# Evidence follows the potentially long immutable prompt/output sections and
# precedes later history. Walk the real Detail viewport until that section is
# painted instead of calling a render/library helper or assuming it is last.
visible=''; evidence_seen=''
for _ in $(seq 1 160); do
  visible=$(dump)
  if grep -Fq 'Evaluation Evidence (hidden)' <<<"$visible" || [[ -n "$evidence_seen" ]]; then
    evidence_seen+=$'\n'"$visible"
    if grep -Fq 'bounded-evaluation' <<<"$evidence_seen" \
      && grep -Fq 'pi:test:fake-worker' <<<"$evidence_seen" \
      && grep -Fq 'wgcid:v1:blake3:' <<<"$evidence_seen"; then
      break
    fi
  fi
  tmux send-keys -t "$session" PageDown
  sleep .03
done
grep -Fq 'Evaluation Evidence (hidden)' <<<"$evidence_seen" || loud_fail "Detail scrolling never revealed evaluation evidence: $(tmux capture-pane -p -t "$session" | tr '\n' '|')"
grep -Fq 'bounded-evaluation' <<<"$evidence_seen" || loud_fail "Detail action hid bounded product"
grep -Fq 'pi:test:fake-worker' <<<"$evidence_seen" || loud_fail "Detail action hid pinned route"
grep -Fq 'wgcid:v1:blake3:' <<<"$evidence_seen" || loud_fail "Detail action hid candidate CID"

# Replayed completion and daemon restart cannot duplicate the semantic key.
wgrun done lazy-source >/dev/null
wgrun service stop >/dev/null 2>&1 || true
(cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" service start --max-agents 1 --model pi:test:fake-worker --no-coordinator-agent --no-supervise >/dev/null)
sleep .4
wgrun service stop >/dev/null 2>&1 || true
count=$(wgrun show lazy-source --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin).get("evaluation_records", [])))')
[[ "$count" == 1 ]] || loud_fail "restart/replay duplicated evaluation record ($count)"
plain_after=$(wgrun viz --all --no-tui)
! grep -Eq '\.evaluate-|\.flip-' <<<"$plain_after" || loud_fail "default Viz gained evaluation satellites"

echo "PASS: publication stayed uncluttered; genuine Fake-Pi candidate minted one hidden bounded record; TUI detail action alone revealed exact attempt/candidate/route; replay remained exactly once"
