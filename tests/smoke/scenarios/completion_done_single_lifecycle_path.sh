#!/usr/bin/env bash
# Real CLI flow: reviewed Report completion must terminalize the exact attempt
# through the lifecycle reducer, never by a direct graph/registry writer.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$REPO_ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)

project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"verdict\":\"pass\",\"findings\":[]}"}],"provider":"test","model":"fake-review","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH"
unset WG_TASK_ID WG_AGENT_ID WG_DIR WG_WORKER_CAPABILITY WG_WORKER_IPC TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY

(cd "$project" &&
  git init -q -b main &&
  git config user.email canary@test.invalid &&
  git config user.name Canary &&
  printf 'base\n' > base.txt &&
  git add base.txt && git commit -qm base &&
  "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "$WG_BIN" --dir "$G" "$@"); }

wgrun config --local --model pi:test:fake-review --reasoning low \
  --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:fake-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:fake-review --set-reasoning evaluator low \
  --no-reload >/dev/null
wgrun add "Single lifecycle completion" --id single-lifecycle -d $'Produce report.txt.\n\n## Validation\n- [ ] exact reviewed report' >/dev/null
wgrun contract single-lifecycle report >/dev/null
wgrun publish single-lifecycle --only >/dev/null
wgrun claim single-lifecycle >/dev/null

(
  cd "$project"
  printf 'implemented and validated\n' > summary.txt
  printf 'reviewed report\n' > report.txt
  printf 'validation passed\n' > validation.log
  wgrun completion-object report.txt --media-type text/plain > output-ref.json
  wgrun completion-object validation.log --media-type text/plain \
    --evidence-kind validation > evidence-ref.json
  wgrun completion-manifest single-lifecycle --summary summary.txt \
    --output-ref output-ref.json --evidence-ref evidence-ref.json > manifest.json
  wgrun submit single-lifecycle --manifest manifest.json --summary summary.txt >/dev/null
  wgrun done single-lifecycle >/dev/null
)

python3 - "$G" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
task = next(
    row for row in map(json.loads, root.joinpath('graph.jsonl').read_text().splitlines())
    if row.get('id') == 'single-lifecycle'
)
assert task['status'] == 'done', task['status']
attempt = task['lifecycle']['current_attempt']
assert attempt['disposition'] == 'succeeded', attempt
events = task['lifecycle']['audit']
success = [event for event in events if event['event_kind'] == 'attempt-succeeded']
assert len(success) == 1, success
assert success[0]['actor_kind'] == 'finalizer', success[0]
assert success[0]['reason_code'] == 'reviewed_publication_committed', success[0]
assert success[0]['evidence_refs'] == [task['completion_receipt']], success[0]
ledger = root.joinpath('lifecycle/events.jsonl').read_text().splitlines()
assert sum('"event_kind":"attempt-succeeded"' in line for line in ledger) == 1, ledger
PY

printf 'PASS completion-done-single-lifecycle-path\n'
