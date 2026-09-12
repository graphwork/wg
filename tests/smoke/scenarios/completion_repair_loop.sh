#!/usr/bin/env bash
# Credential-free real-entry-point regression for bounded completion repair.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"

# The checked-in validation contract invokes this file directly, while every
# smoke scenario must execute below the Rust subreaper/ownership harness. Bounce
# that direct entry through one ignored test which calls smoke::run_scenario;
# the harness child receives the exact candidate binary environment unchanged.
if [[ -z "${WG_SMOKE_HARNESS_RUN_ID:-}" \
    || "${WG_SMOKE_SUBREAPER_TOKEN:-}" != "$WG_SMOKE_HARNESS_RUN_ID" ]]; then
    export WG_SMOKE_DIRECT_SCENARIO="$HERE/completion_repair_loop.sh"
    cd "$ROOT"
    exec cargo test --locked --lib \
        smoke::tests::run_explicit_scenario_under_process_harness \
        -- --exact --ignored --nocapture
fi

. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the real TUI flow"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"
mkdir -p "$repo" "$home/.config"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_GRAPH_ID WG_PROJECT_ROOT WG_WORKTREE_PATH \
  WG_WORKTREE_ACTIVE WG_BRANCH WG_WORKER_ATTEMPT_ID WG_WORKER_ATTEMPT_FENCE \
  WG_WORKER_GENERATION WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_WORKER_CONTROL_MODE || true

cd "$repo"
git init -q -b main
git config user.email repair@test.invalid
git config user.name Repair
echo base > base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID WG_DIR="$repo/.wg" "$WG_BIN" "$@"; }
worker_done(){ env WG_TASK_ID="$1" WG_AGENT_ID="$2" "$WG_BIN" --dir "$repo/.wg" done "$1"; }

# An unchanged retry is evidence-backed, redacted, retains the exact owner and
# attempt, consumes no extra opportunity, and then blocks all further model or
# validation work until one explicit human decision is recorded.
calls="$scratch/unchanged.calls"
cat > validation-fail.sh <<SH
#!/usr/bin/env bash
printf x >> '$calls'
printf 'api_key=supersecret repair failure\\n' >&2
exit 9
SH
chmod +x validation-fail.sh
command="./validation-fail.sh"
wgrun add "Unchanged completion repair" --id repair-unchanged \
  --validation-command "$command" \
  -d $'Produce the requested report.\n\n## Validation\n- [ ] prose is not executable authority' >/dev/null
wgrun contract repair-unchanged report >/dev/null
wgrun publish repair-unchanged --only >/dev/null
wgrun claim repair-unchanged --actor repair-worker >/dev/null
wgrun show repair-unchanged --json >"$scratch/preflight.json"
python3 - "$scratch/preflight.json" "$command" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); command=sys.argv[2]
p=x['completion_preflight']
assert [c['command'] for c in p['checks']]==[command,'<WG regular-file and immutable-artifact integrity check>'],p
assert 'validation_commands[0]' in p['checks'][0]['provenance'],p
assert p['checks'][1]['purpose']=='artifact-integrity',p
assert p['prose_is_authority'] is False,p
assert p['repair_boundary']=='task-and-validation-fixtures',p
assert p['deterministic_repair_budget']==2,p
assert 'host-bound immutable' in p['evidence_capture'],p
PY

if worker_done repair-unchanged repair-worker >"$scratch/first.out" 2>"$scratch/first.err"; then
  loud_fail "first failing deterministic check completed the task"
fi
if grep -q 'supersecret' "$scratch/first.err"; then
  loud_fail "raw validation secret leaked to worker stderr"
fi
wgrun show repair-unchanged --json >"$scratch/first.json"
python3 - "$scratch/first.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); r=x['completion_repair']
assert x['status']=='in-progress' and x['assigned']=='repair-worker',x
assert r['disposition']=='repairing' and r['opportunities_used']==1 and r['opportunity_limit']==2,r
assert r['reason_code']=='deterministic-check-failed',r
assert r['evidence']['content_digest'].startswith('b3:') and r['candidate_identity'].startswith('b3:'),r
assert '[REDACTED]' in r['diagnostic_excerpt'] and 'supersecret' not in r['diagnostic_excerpt'],r
assert r['attempt_id'] and r['fence']>0 and r['requirements_digest'].startswith('b3:'),r
PY

if worker_done repair-unchanged repair-worker >"$scratch/repeat.out" 2>"$scratch/repeat.err"; then
  loud_fail "unchanged failed candidate completed the task"
fi
wgrun show repair-unchanged --json >"$scratch/repeat.json"
python3 - "$scratch/first.json" "$scratch/repeat.json" <<'PY'
import json,sys
a=json.load(open(sys.argv[1])); b=json.load(open(sys.argv[2])); r=b['completion_repair']
assert b['status']=='in-progress' and b['assigned']==a['assigned'],b
assert r['disposition']=='needs-attention' and r['reason_code']=='unchanged-candidate-repeated',r
assert r['opportunities_used']==1 and len(r['failed_candidates'])==1,r
assert r['attempt_id']==a['completion_repair']['attempt_id'] and r['fence']==a['completion_repair']['fence'],r
assert r['attention_event_id'].startswith('attention:'),r
PY
[[ "$(wc -c <"$calls")" == 2 ]] || loud_fail "expected exactly two validation executions"
if worker_done repair-unchanged repair-worker >"$scratch/blocked.out" 2>"$scratch/blocked.err"; then
  loud_fail "NeedsAttention task re-entered completion"
fi
[[ "$(wc -c <"$calls")" == 2 ]] || loud_fail "NeedsAttention reran deterministic validation"
grep -q 'NeedsAttention:' "$scratch/blocked.err" \
  || loud_fail "blocked repair did not explain human escalation: $(cat "$scratch/blocked.err")"

# The explicit request-help intent preserves saved work and source authority;
# it records one bounded operator decision rather than terminally failing.
wgrun fail repair-unchanged --intent request-help \
  --reason 'Approve fixture scope; api_key=anothersecret' >"$scratch/help.out"
wgrun show repair-unchanged --json >"$scratch/help.json"
python3 - "$scratch/help.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); r=x['completion_repair']
assert x['status']=='in-progress' and x['assigned']=='repair-worker',x
assert r['disposition']=='needs-attention' and r['reason_code']=='scope-approval-required',r
assert 'wg contract repair-unchanged --repair-boundary' in r['safe_next'],r
assert 'anothersecret' not in json.dumps(x),x
old_requirements=r['requirements_digest']
open(sys.argv[1]+'.requirements','w').write(old_requirements)
PY
wgrun contract repair-unchanged --repair-boundary repository >"$scratch/approved.out"
wgrun show repair-unchanged --json >"$scratch/approved.json"
python3 - "$scratch/approved.json" "$scratch/help.json.requirements" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); old=open(sys.argv[2]).read(); r=x['completion_repair']
assert r['disposition']=='repairing' and r['reason_code']=='operator-contract-update-approved',r
assert r['requirements_digest']==old,r # immutable old evidence binding is not rewritten
assert x['completion_preflight']['repair_boundary']=='repository',x['completion_preflight']
PY
if worker_done repair-unchanged repair-worker >"$scratch/approved-run.out" 2>"$scratch/approved-run.err"; then
  loud_fail "approved repair's still-failing gate completed the task"
fi
[[ "$(wc -c <"$calls")" == 3 ]] || loud_fail "operator approval did not reopen exactly one completion attempt"

# Distinct candidate bytes get exactly two opportunities. A third distinct
# failure exhausts the episode instead of silently extending it.
wgrun add "Finite completion repair" --id repair-budget \
  --validation-command "printf 'still failing\\n' >&2; exit 7" >/dev/null
wgrun contract repair-budget report >/dev/null
wgrun publish repair-budget --only >/dev/null
wgrun claim repair-budget --actor budget-worker >/dev/null
for revision in one two three; do
  printf '%s\n' "$revision" > candidate.txt
  if worker_done repair-budget budget-worker >"$scratch/budget-$revision.out" 2>"$scratch/budget-$revision.err"; then
    loud_fail "failing candidate $revision completed the task"
  fi
done
wgrun show repair-budget --json >"$scratch/budget.json"
python3 - "$scratch/budget.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); r=x['completion_repair']
assert x['status']=='in-progress' and x['assigned']=='budget-worker',x
assert r['disposition']=='needs-attention' and r['reason_code']=='deterministic-repair-budget-exhausted',r
assert r['opportunities_used']==2 and r['opportunity_limit']==2,r
assert len(set(r['failed_candidates']))==3,r
PY

# Root blocker projection is cycle-safe/deduplicated in JSON and visible in
# human CLI output, with the whole downstream impact attached to the root.
wgrun add "Blocked child" --id repair-child --after repair-budget >/dev/null
wgrun add "Blocked grandchild" --id repair-grandchild --after repair-child >/dev/null
wgrun status --json >"$scratch/status.json"
python3 - "$scratch/status.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=[r for r in x['stalled_chains'] if r['root_task_id']=='repair-budget']
assert len(rows)==1,rows
r=rows[0]
assert r['root_blocker']=='deterministic-repair-budget-exhausted: nonzero-exit',r
assert r['active_repair'] is False,r
assert r['affected_downstream']==['repair-child','repair-grandchild'],r
PY
wgrun status >"$scratch/status.txt"
grep -q 'NeedsAttention — stalled dependency chains' "$scratch/status.txt" || loud_fail "human status omitted stalled heading"
grep -q 'ROOT repair-budget' "$scratch/status.txt" || loud_fail "human status omitted root blocker"
wgrun show repair-budget >"$scratch/show.txt"
grep -q 'Completion repair/NeedsAttention' "$scratch/show.txt" || loud_fail "human show omitted repair state"
grep -q 'affected downstream: repair-child, repair-grandchild' "$scratch/show.txt" || loud_fail "human show omitted stalled impact"

# Drive the real TUI through a tmux PTY and the keyboard dispatcher. The graph
# list order is presentation-owned, so inspect each visible task rather than
# assuming a fixed row; one inspector must show the same root and safe action.
session="wg-completion-repair-tui-$$"
python3 - "$repo/.wg/graph.jsonl" <<'PY'
import json,sys
path=sys.argv[1]
rows=[line for line in open(path) if json.loads(line).get('id')!='repair-unchanged']
open(path,'w').writelines(rows)
PY
printf '%s\n' repair-budget >"$repo/.wg/.new_task_focus"
cleanup_tui() { tmux kill-session -t "$session" >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup_tui
tmux new-session -d -s "$session" -x 180 -y 42 \
  "cd '$repo' && HOME='$home' XDG_CONFIG_HOME='$home/.config' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$repo/.wg' tui; rc=\$?; echo TUI_EXIT=\$rc; sleep 30"
capture_tui() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
for _ in $(seq 1 400); do
  capture_tui | grep -Fq 'repair-budget' && break
  sleep 0.025
done
capture_tui | grep -Fq 'repair-budget' \
  || loud_fail "TUI did not render the stalled task list: $(capture_tui | tr '\n' '|')"
found_blocker=0
for row in $(seq 0 8); do
  tmux send-keys -t "$session" Home
  for _ in $(seq 1 "$row"); do tmux send-keys -t "$session" Down; done
  tmux send-keys -t "$session" Enter End
  sleep 0.1
  if capture_tui | grep -Fq 'ROOT BLOCKER: repair-budget' \
      && capture_tui | grep -Fq 'one safe action:'; then
    found_blocker=1
    capture_tui >"$scratch/tui-blocker.txt"
    break
  fi
  tmux send-keys -t "$session" Escape
  sleep 0.025
done
[[ "$found_blocker" == 1 ]] \
  || loud_fail "real TUI inspector omitted root blocker/next action: $(capture_tui | tr '\n' '|')"

[[ ! -e "$ROOT/.wg" ]] \
  || loud_fail "scenario created protected runtime state inside the source checkout: $ROOT/.wg"
echo "completion repair loop canary passed"
