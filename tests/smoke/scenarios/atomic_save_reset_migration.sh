#!/usr/bin/env bash
# Candidate reset/retry retention plus fail-closed legacy migration evidence.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null || loud_skip "MISSING CARGO" "candidate build requires cargo"
ROOT=$(git -C "$HERE" rev-parse --show-toplevel) || loud_fail "cannot find repository root"
(cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate build failed"
export PATH="$ROOT/target/debug:$PATH"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR WG_BRANCH
# Exercise the real candidate operator flow: retry waits for/fences the old
# owner, preserves dirty leased WIP for ordinary retry, labels a new attempt,
# and only explicit --fresh discards the retained worktree.
WG_SMOKE_SCENARIO=atomic_save_reset_migration \
  bash "$HERE/retry_current_profile.sh" \
  || loud_fail "candidate retry/retained-work flow failed"
# Exact reducer ordering requires WorkSaved -> AbortedPreserved before a new
# generation tuple can exist, and rejects the stale old actor afterwards.
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults reset_retry_saves_before_generation -- --exact) \
  || loud_fail "reset/retry ordering fault test failed"
# Active legacy Done is preserved as immutable source evidence, quarantined to
# a non-satisfying projection, and a second migration classification is a no-op.
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults legacy_done_without_evidence_is_quarantined -- --exact) \
  || loud_fail "legacy migration fault test failed"
echo "PASS: candidate retry preserved dirty work under a new attempt, stale authority was fenced, and legacy Done migration retained evidence while quarantining idempotently"
