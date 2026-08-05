#!/usr/bin/env bash
# Hermetic ten-worker real-daemon proof of worker-owned Land, Report, and
# Explore completion through the capability broker and deterministic Pi boundary.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  WG_BIN="$REPO_ROOT/target/debug/wg"
  [[ -x "$WG_BIN" ]] || {
    (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  }
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
model=""; argv=("$@")
while (($#)); do case "$1" in --model) model="$2"; shift 2;; *) shift;; esac; done
cat >/dev/null || true
if [[ "$model" == "fake-review" ]]; then
  printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"verdict\":\"pass\",\"findings\":[]}"}],"provider":"test","model":"fake-review","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
  exit 0
fi
[[ "$model" == "fake-worker" ]] || { echo "unexpected model: $model" >&2; exit 88; }
if [[ "$WG_TASK_ID" == completion-canary-0 || "$WG_TASK_ID" == completion-canary-1 ]]; then
  printf 'reviewed land %s\n' "$WG_TASK_ID" > "$WG_TASK_ID.txt"
  git add "$WG_TASK_ID.txt"
  git commit -m "$WG_TASK_ID candidate" >/dev/null
  mkdir -p "$HOME/land-barrier"
  : > "$HOME/land-barrier/$WG_TASK_ID"
  for _ in $(seq 1 300); do
    [[ $(find "$HOME/land-barrier" -type f | wc -l) -ge 2 ]] && break
    sleep .05
  done
  [[ $(find "$HOME/land-barrier" -type f | wc -l) -ge 2 ]]
  landed=0
  for _ in $(seq 1 8); do
    git merge refs/heads/main --no-edit >/dev/null
    printf 'implemented and validated output\n' > summary.txt
    printf 'validation passed at %s\n' "$(git rev-parse HEAD)" > validation.log
    wg completion-object validation.log --media-type text/plain --evidence-kind validation > evidence-ref.json
    wg completion-manifest "$WG_TASK_ID" --summary summary.txt --git --evidence-ref evidence-ref.json > manifest.json
    wg submit "$WG_TASK_ID" --manifest manifest.json --summary summary.txt >/dev/null
    rm -f summary.txt validation.log evidence-ref.json manifest.json
    if wg land "$WG_TASK_ID" >/dev/null; then landed=1; break; fi
  done
  (( landed == 1 ))
else
  printf 'implemented and validated output\n' > summary.txt
  printf 'reviewed report\n' > report.txt
  printf 'validation passed\n' > validation.log
  wg completion-object report.txt --media-type text/plain > output-ref.json
  wg completion-object validation.log --media-type text/plain --evidence-kind validation > evidence-ref.json
  wg completion-manifest "$WG_TASK_ID" --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json > manifest.json
  wg submit "$WG_TASK_ID" --manifest manifest.json --summary summary.txt >/dev/null
fi
wg done "$WG_TASK_ID" >/dev/null
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"report completed through immutable review"}],"provider":"test","model":"fake-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_DIR TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY AWS_SECRET_ACCESS_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_DIR \
  -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY -u AWS_SECRET_ACCESS_KEY \
  HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email canary@test.invalid && git config user.name Canary && printf 'base\n' > base.txt && git add base.txt && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:fake-worker --reasoning low --auto-assign false --auto-evaluate false --set-model reviewer pi:test:fake-review --set-reasoning reviewer low --set-model evaluator pi:test:fake-review --set-reasoning evaluator low --no-reload >/dev/null
if ! grep -q '^\[resource_management\]' "$G/config.toml"; then
  printf '\n[resource_management]\nmax_build_agents = 2\n' >> "$G/config.toml"
else
  loud_fail "fresh canary config unexpectedly contains resource_management"
fi
for i in $(seq 0 9); do
  id="completion-canary-$i"
  wgrun add "Worker-owned completion canary $i" --id "$id" -d $'Produce report.txt.\n\n## Validation\n- [ ] exact output reviewed' >/dev/null
  if (( i < 2 )); then contract=land; elif (( i % 3 == 0 )); then contract=explore; else contract=report; fi
  wgrun contract "$id" "$contract" >/dev/null
  if (( i < 2 )); then wgrun publish "$id" --only >/dev/null; fi
 done

cleanup(){ wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
wgrun service start --max-agents 4 --model pi:test:fake-worker --no-coordinator-agent --no-supervise >/dev/null

# Hold both Land workers at a barrier so they begin from the same main and
# necessarily race the compare/fast-forward publication.
for _ in $(seq 1 300); do
  [[ -d "$HOME/land-barrier" && $(find "$HOME/land-barrier" -type f | wc -l) -ge 2 ]] && break
  sleep .05
done
[[ -d "$HOME/land-barrier" && $(find "$HOME/land-barrier" -type f | wc -l) -ge 2 ]] || loud_fail "two concurrent Land workers did not reach the barrier"
for i in $(seq 2 9); do wgrun publish "completion-canary-$i" --only >/dev/null; done

all_done=0
for _ in $(seq 1 600); do
  all_done=1
  for i in $(seq 0 9); do
    state=$(wgrun show "completion-canary-$i" --json 2>/dev/null || true)
    STATUS="$state" python3 -c 'import json,os; assert json.loads(os.environ["STATUS"])["status"]=="done"' 2>/dev/null || { all_done=0; break; }
  done
  (( all_done == 1 )) && break
  sleep .1
done
if ! python3 - "$G" <<'PY'
import json,os,sys
G=sys.argv[1]
rows=[json.loads(line) for line in open(os.path.join(G,'graph.jsonl')) if line.strip()]
tasks={row['id']:row for row in rows if 'title' in row and row['id'].startswith('completion-canary-')}
for i in range(10):
    task=tasks[f'completion-canary-{i}']
    assert task['status']=='done', (task['id'], task['status'])
    assert task.get('completion_contract','land') in ('land','report','explore'), task['id']
    assert task['completion_receipt'], task['id']
    assert task['completion_candidate']['flip_receipt'], task['id']
    assert task['completion_candidate']['eval_receipt'], task['id']
# Two build slots force both Land workers to review the same initial main;
# exactly one wins the first compare/fast-forward and the losing same worker
# must integrate, revalidate, rebuild, and rereview (at least 3 selections total).
land_selections=sum(
    sum(1 for log in tasks[f'completion-canary-{i}'].get('log',[])
        if log.get('actor')=='completion-submit')
    for i in (0,1)
)
assert land_selections >= 3, ('moved-main same-worker repair was not exercised',land_selections)
assert not os.path.exists(os.path.join(G,'finalization')), 'legacy finalization directory exists'
assert not os.path.exists(os.path.join(G,'worker-control','transactions')), 'legacy SaveTransaction directory exists'
assert not [row for row in rows if row.get('id','').startswith(('.flip-','.evaluate-'))], 'review child task exists'
PY
then
  find "$G/agents" -maxdepth 2 -type f \( -name 'output.log' -o -name 'raw_stream.jsonl' -o -name 'wrapper.log' \) -print -exec tail -80 {} \; >&2 || true
  cat "$G/service/worker-capabilities.json" >&2 || true
  loud_fail "ten-worker completion canary did not converge"
fi
sleep .3
agents=$(find "$G/agents" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
[[ "$agents" == 10 ]] || loud_fail "review spawned/replaced source workers (agents=$agents, expected=10)"
wgrun service stop >/dev/null
printf 'PASS worker-owned-completion-canary workers=10\n'
