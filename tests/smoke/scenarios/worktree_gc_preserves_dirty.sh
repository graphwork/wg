#!/usr/bin/env bash
# Scenario: worktree_gc_preserves_dirty
#
# Regression for fix-worktree-gc-preserve-uncommitted:
# `wg worktree gc --dead-only --execute` must remove clean dead worktrees but
# fail closed around dirty dead worktrees. Dirty work must survive unless the
# operator passes the explicit destructive `--discard-uncommitted` flag.

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
    git config user.email "smoke@test" >/dev/null
    git config user.name "Smoke" >/dev/null
    echo "initial" > README.md
    git add README.md >/dev/null
    git commit -m "initial" >/dev/null
) || loud_fail "git initial commit setup failed"

wg_dir="$project_root/.wg"
mkdir -p "$wg_dir/service"

add_agent_worktree() {
    local agent_id="$1"
    local task_id="$2"
    local branch="wg/${agent_id}/${task_id}"
    local worktree_dir="$project_root/.wg-worktrees/$agent_id"
    mkdir -p "$(dirname "$worktree_dir")"
    ( cd "$project_root" && git worktree add "$worktree_dir" -b "$branch" HEAD ) \
        >/dev/null 2>&1 \
        || loud_fail "git worktree add failed for $agent_id"
    printf '%s\n' "$worktree_dir"
}

clean_agent="agent-clean-gc"
dirty_agent="agent-dirty-gc"
clean_wt="$(add_agent_worktree "$clean_agent" clean-task)"
dirty_wt="$(add_agent_worktree "$dirty_agent" dirty-task)"

echo "uncommitted agent work" > "$dirty_wt/UNCOMMITTED_WORK.txt"

if ! ( cd "$dirty_wt" && git status --porcelain ) | grep -q "UNCOMMITTED_WORK.txt"; then
    loud_fail "test setup wrong: expected dirty worktree to contain uncommitted work"
fi

dry_log="$scratch/gc-dry-run.log"
set +e
wg --dir "$wg_dir" worktree gc --dead-only >"$dry_log" 2>&1
dry_exit=$?
set -e

if [[ $dry_exit -ne 0 ]]; then
    loud_fail "dry-run exited non-zero.
dry-run log:
$(cat "$dry_log")"
fi

for expected in "clean removable" "dirty blocked" "$clean_agent" "$dirty_agent" "wg worktree archive <agent-id> --remove"; do
    if ! grep -q "$expected" "$dry_log"; then
        loud_fail "dry-run output missing expected text '$expected'.
dry-run log:
$(cat "$dry_log")"
    fi
done

if [[ ! -d "$clean_wt" || ! -d "$dirty_wt" ]]; then
    loud_fail "dry-run removed a worktree unexpectedly"
fi

execute_log="$scratch/gc-execute.log"
set +e
wg --dir "$wg_dir" worktree gc --dead-only --execute >"$execute_log" 2>&1
execute_exit=$?
set -e

if [[ $execute_exit -eq 0 ]]; then
    loud_fail "execute exited 0 despite skipped dirty worktree.
execute log:
$(cat "$execute_log")"
fi

if [[ -d "$clean_wt" ]]; then
    loud_fail "clean dead worktree survived execute.
execute log:
$(cat "$execute_log")"
fi

if [[ ! -d "$dirty_wt" ]]; then
    loud_fail "dirty dead worktree was removed without --discard-uncommitted.
execute log:
$(cat "$execute_log")"
fi

for expected in "skipped dirty" "$dirty_agent" "--discard-uncommitted" "wg worktree archive"; do
    if ! grep -q -- "$expected" "$execute_log"; then
        loud_fail "execute output missing expected text '$expected'.
execute log:
$(cat "$execute_log")"
    fi
done

discard_log="$scratch/gc-discard.log"
set +e
wg --dir "$wg_dir" worktree gc --dead-only --execute --discard-uncommitted >"$discard_log" 2>&1
discard_exit=$?
set -e

if [[ $discard_exit -ne 0 ]]; then
    loud_fail "discard opt-in exited non-zero.
discard log:
$(cat "$discard_log")"
fi

if [[ -d "$dirty_wt" ]]; then
    loud_fail "dirty worktree survived explicit --discard-uncommitted.
discard log:
$(cat "$discard_log")"
fi

if ! grep -q "DANGEROUS: --discard-uncommitted active" "$discard_log"; then
    loud_fail "discard output did not include loud destructive warning.
discard log:
$(cat "$discard_log")"
fi

echo "PASS: worktree gc preserves dirty work by default and discards only with explicit opt-in"
exit 0
