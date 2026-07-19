#!/usr/bin/env bash
# Scenario: wg_done_preserves_squash_attribution
#
# Drives the real `wg done` worktree merge path. A two-commit branch models
# the Luca U1/U3 landing shapes: the oldest commit has an external author and
# one co-author, while the integration commit has another author and repeats
# the external author in a blank-line-separated Co-authored-by line. The
# squash must keep the oldest author, retain the other identities exactly
# once, and land both files.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
project_root="$scratch/project"
mkdir -p "$project_root"

git init -b main "$project_root" >/dev/null 2>&1 \
    || loud_fail "git init failed in $project_root"
(
    cd "$project_root"
    git config user.email "merge@example.com"
    git config user.name "Merge User"
    git config commit.gpgsign false
    echo initial >README.md
    git add README.md
    git commit -m initial >/dev/null
) || loud_fail "git initial commit setup failed"

agent_id="agent-smoke-attribution"
task_id="squash-attribution"
branch="wg/${agent_id}/${task_id}"
worktree_dir="$project_root/.wg-worktrees/$agent_id"
mkdir -p "$(dirname "$worktree_dir")"
(
    cd "$project_root"
    git worktree add "$worktree_dir" -b "$branch" HEAD >/dev/null
) || loud_fail "git worktree add failed"

(
    cd "$worktree_dir"
    echo baseline >luca.txt
    git add luca.txt
    git commit --author "Luca Pinello <lucapinello@gmail.com>" \
        -m $'source baseline\n\nCo-authored-by: Claude Opus 4.8 <noreply@anthropic.com>' >/dev/null

    echo integration >integration.txt
    git add integration.txt
    git commit --author "Erik Integrator <erik@example.com>" \
        -m $'integration hardening\n\nCo-authored-by: Luca Pinello <lucapinello@gmail.com>' >/dev/null
) || loud_fail "source attribution fixture commits failed"

wg_dir="$project_root/.wg"
mkdir -p "$wg_dir"
cat >"$wg_dir/graph.jsonl" <<EOF
{"kind":"task","id":"${task_id}","title":"Squash attribution","status":"in-progress","created_at":"2026-07-19T00:00:00+00:00"}
EOF

# This is a harness invocation, not a worker attempting to bypass its gate.
# Avoid recursion into the parent smoke gate while retaining the worktree vars
# that select the production merge path.
unset WG_AGENT_ID
unset WG_SMOKE_AGENT_OVERRIDE
done_log="$scratch/wg-done.log"
set +e
WG_WORKTREE_PATH="$worktree_dir" \
WG_BRANCH="$branch" \
WG_PROJECT_ROOT="$project_root" \
    wg --dir "$wg_dir" done "$task_id" --skip-smoke >"$done_log" 2>&1
done_exit=$?
set -e
if [[ $done_exit -ne 0 ]]; then
    loud_fail "wg done failed attribution merge (exit $done_exit):
$(cat "$done_log")"
fi

commit=$(git -C "$project_root" show -s --format='%an <%ae>%n%B' main) \
    || loud_fail "cannot inspect squash commit"
first_line=${commit%%$'\n'*}
[[ "$first_line" == "Luca Pinello <lucapinello@gmail.com>" ]] \
    || loud_fail "squash author was not preserved; commit:
$commit"

count_erik=$(grep -Fxc 'Co-authored-by: Erik Integrator <erik@example.com>' <<<"$commit" || true)
count_claude=$(grep -Fxc 'Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>' <<<"$commit" || true)
count_luca=$(grep -Fc 'Co-authored-by: Luca Pinello' <<<"$commit" || true)
[[ "$count_erik" -eq 1 ]] \
    || loud_fail "additional source author was not retained exactly once; commit:
$commit"
[[ "$count_claude" -eq 1 ]] \
    || loud_fail "source co-author trailer was not retained exactly once; commit:
$commit"
[[ "$count_luca" -eq 0 ]] \
    || loud_fail "primary author was redundantly emitted as co-author; commit:
$commit"

# The lines must form one valid Git trailer block, not merely appear in the
# body (the malformed blank-line-separated U3 source is the regression shape).
parsed_trailers=$(git -C "$project_root" show -s \
    --format='%(trailers:key=Co-authored-by,valueonly)' main)
grep -Fqx 'Erik Integrator <erik@example.com>' <<<"$parsed_trailers" \
    || loud_fail "Erik identity is not in Git's parsed trailer block; commit:
$commit"
grep -Fqx 'Claude Opus 4.8 <noreply@anthropic.com>' <<<"$parsed_trailers" \
    || loud_fail "Claude identity is not in Git's parsed trailer block; commit:
$commit"

for path in luca.txt integration.txt; do
    git -C "$project_root" cat-file -e "main:$path" 2>/dev/null \
        || loud_fail "$path did not land in the squash"
done

echo "PASS: wg done preserved source author and co-author attribution across squash landing"
exit 0
