#!/usr/bin/env bash
# Scenario: dev_check_branch_and_binary_freshness
#
# Regression (fix-cargo-install): building/installing wg from an abandoned
# agent worktree branch should be easy to spot before the user assumes the
# installed binary includes main. `wg dev-check` reports the current branch,
# main HEAD, and current binary mtime, and warns on non-main/stale builds.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)

init_repo() {
    local repo="$1"
    mkdir -p "$repo"
    git -C "$repo" init -b main >/dev/null || loud_fail "git init failed"
    git -C "$repo" config user.email wg-test@example.com
    git -C "$repo" config user.name "wg test"
    printf "main\n" >"$repo/README.md"
    git -C "$repo" add README.md
}

# Clean path: main branch, main commit older than the installed binary.
green_repo="$scratch/green"
init_repo "$green_repo"
GIT_AUTHOR_DATE="2000-01-01T00:00:00Z" \
GIT_COMMITTER_DATE="2000-01-01T00:00:00Z" \
    git -C "$green_repo" commit -m old-main >/dev/null || loud_fail "green commit failed"

green_out=$(cd "$green_repo" && wg dev-check 2>&1) || \
    loud_fail "wg dev-check failed on main:\n$green_out"

grep -q "branch: main" <<<"$green_out" || \
    loud_fail "green output missing branch: main:\n$green_out"
grep -q "main HEAD:" <<<"$green_out" || \
    loud_fail "green output missing main HEAD:\n$green_out"
grep -q "wg binary:" <<<"$green_out" || \
    loud_fail "green output missing binary mtime:\n$green_out"
grep -q "status: OK" <<<"$green_out" || \
    loud_fail "green output did not report OK:\n$green_out"

# Warning path: non-main branch and a future main commit make any current
# binary stale without modifying the real installed executable.
warn_repo="$scratch/warn"
init_repo "$warn_repo"
GIT_AUTHOR_DATE="2099-01-01T00:00:00Z" \
GIT_COMMITTER_DATE="2099-01-01T00:00:00Z" \
    git -C "$warn_repo" commit -m future-main >/dev/null || loud_fail "warn commit failed"
git -C "$warn_repo" checkout -b wg/agent-1398/fix-tui-perf >/dev/null || \
    loud_fail "checkout non-main branch failed"

warn_out=$(cd "$warn_repo" && wg dev-check 2>&1) || \
    loud_fail "wg dev-check failed on warning repo:\n$warn_out"

grep -q "branch: wg/agent-1398/fix-tui-perf" <<<"$warn_out" || \
    loud_fail "warn output missing non-main branch:\n$warn_out"
grep -q "status: WARN" <<<"$warn_out" || \
    loud_fail "warn output did not report WARN:\n$warn_out"
grep -q "not 'main'" <<<"$warn_out" || \
    loud_fail "warn output missing branch warning:\n$warn_out"
grep -q "older than local main HEAD" <<<"$warn_out" || \
    loud_fail "warn output missing stale binary warning:\n$warn_out"

echo "PASS: wg dev-check reports branch, main HEAD, binary mtime, and warns on non-main/stale builds"
