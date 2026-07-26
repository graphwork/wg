#!/usr/bin/env bash
# Credential-free terminal flow for `wg retry --current-profile`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER
scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home"
export WG_GLOBAL_DIR="$home/.wg"
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed.txt
git add seed.txt
git commit -q -m seed

run_wg() {
  env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_WORKTREE_ACTIVE \
    -u WG_BRANCH -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL \
    HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg "$@"
}

run_wg retry --help | grep -q -- '--current-profile' \
  || loud_fail 'retry help does not expose --current-profile'
run_wg init >/dev/null 2>&1
run_wg profile create profile-a -m 'pi:openai-codex:worker-a' >/dev/null
run_wg profile create profile-b -m 'pi:openrouter:worker-b' >/dev/null
run_wg profile select profile-a --no-reload >/dev/null
GEN_A=$(python3 -c 'import json; print(json.load(open(".wg/profile-selection.json"))["profile_fingerprint"])')

# An unpinned task gets the profile's exact route/reasoning at command time.
run_wg add 'Unpinned retry' --id retry-unpinned >/dev/null
run_wg fail retry-unpinned --reason 'fixture failure' >/dev/null
out=$(run_wg retry retry-unpinned --current-profile)
grep -q "profile-a generation=$GEN_A executor=pi model=pi:openai-codex:worker-a reasoning=high" <<<"$out" \
  || loud_fail "retry output omitted the exact profile generation/route: $out"
python3 - "$GEN_A" <<'PY'
import json,sys
expected_generation=sys.argv[1]
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
task=next(row for row in rows if row.get('kind')=='task' and row['id']=='retry-unpinned')
assert task['status']=='open',task
assert task['model']=='pi:openai-codex:worker-a',task
assert task['reasoning']=='high',task
entry=next(e for e in task['log'] if e.get('actor')=='retry-current-profile')
assert 'profile=profile-a' in entry['message'],entry
assert f'generation={expected_generation}' in entry['message'],entry
assert 'executor=pi' in entry['message'],entry
PY

# A stale task pin is atomically replaced while retry-in-place preserves WIP.
run_wg add 'Pinned retry' --id retry-pinned \
  --model 'pi:openrouter:old-worker' --reasoning low >/dev/null
run_wg fail retry-pinned --reason 'old route failed' >/dev/null
mkdir -p .wg-worktrees
git worktree add -q .wg-worktrees/agent-old -b wg/agent-old/retry-pinned HEAD
printf 'preserve me\n' >.wg-worktrees/agent-old/uncommitted-wip.txt
run_wg retry retry-pinned --current-profile >/dev/null
test -f .wg-worktrees/agent-old/uncommitted-wip.txt \
  || loud_fail 'default --current-profile retry discarded the existing worktree/WIP'

# Flip the project profile after retry but before spawn. The attempt remains
# byte-exactly pinned to profile A rather than resolving profile B later.
run_wg profile select profile-b --no-reload >/dev/null
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
task=next(row for row in rows if row.get('kind')=='task' and row['id']=='retry-pinned')
assert task['model']=='pi:openai-codex:worker-a',task
assert task['reasoning']=='high',task
assert task.get('profile') is None,task
assert task.get('provider') is None,task
assert task.get('endpoint') is None,task
assert task.get('session_id') is None,task
PY

# --fresh composes with the new flag: discard WIP and pin profile B.
run_wg add 'Fresh retry' --id retry-fresh \
  --model 'pi:openai-codex:stale-worker' --reasoning low >/dev/null
run_wg fail retry-fresh --reason 'start over' >/dev/null
git worktree add -q .wg-worktrees/agent-fresh -b wg/agent-fresh/retry-fresh HEAD
printf 'discard me\n' >.wg-worktrees/agent-fresh/uncommitted-wip.txt
run_wg retry retry-fresh --fresh --current-profile >/dev/null
test ! -e .wg-worktrees/agent-fresh \
  || loud_fail '--fresh --current-profile did not discard the old worktree'

# Plain retry stays backward-compatible and retains its prior explicit route.
run_wg add 'Plain retry' --id retry-plain \
  --model 'pi:openai-codex:plain-old' --reasoning medium >/dev/null
run_wg fail retry-plain --reason 'plain fixture' >/dev/null
run_wg retry retry-plain >/dev/null

python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
tasks={row['id']:row for row in rows if row.get('kind')=='task'}
assert tasks['retry-fresh']['model']=='pi:openrouter:worker-b',tasks['retry-fresh']
assert tasks['retry-fresh']['reasoning']=='high',tasks['retry-fresh']
assert tasks['retry-plain']['model']=='pi:openai-codex:plain-old',tasks['retry-plain']
assert tasks['retry-plain']['reasoning']=='medium',tasks['retry-plain']
ops=[json.loads(line) for line in open('.wg/log/operations.jsonl') if line.strip()]
op=next(row for row in reversed(ops) if row.get('op')=='retry' and row.get('task_id')=='retry-fresh')
selection=op['detail']['current_profile']
assert selection['name']=='profile-b',selection
assert selection['executor']=='pi',selection
assert selection['model']=='pi:openrouter:worker-b',selection
assert selection['reasoning']=='high',selection
assert selection['generation'].startswith('b3:'),selection
PY

# The flag means project profile literally: with the association cleared it
# must fail closed rather than borrow machine-global profile/config state.
run_wg profile select --clear --no-reload >/dev/null
run_wg add 'No implicit fallback' --id retry-no-profile \
  --model 'pi:openai-codex:stay-put' --reasoning low >/dev/null
run_wg fail retry-no-profile --reason 'no-profile fixture' >/dev/null
if run_wg retry retry-no-profile --current-profile >"$scratch/no-profile.out" 2>&1; then
  loud_fail '--current-profile succeeded without a selected project profile'
fi
grep -q 'No project profile is selected' "$scratch/no-profile.out" \
  || loud_fail "missing fail-closed project-profile diagnostic: $(cat "$scratch/no-profile.out")"
status=$(run_wg show retry-no-profile --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
[[ "$status" == 'failed' ]] || loud_fail "failed resolution mutated the task to $status"

echo 'PASS: retry --current-profile pins exact route/generation now, preserves by default, composes with --fresh, fails closed without a project selection, and plain retry is unchanged'
