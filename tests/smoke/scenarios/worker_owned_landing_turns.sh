#!/usr/bin/env bash
# Candidate-binary surface plus the persistent queue's crash/restart/FIFO proof.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$REPO_ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || loud_skip "MISSING CANDIDATE" "set WG_SMOKE_CANDIDATE_BIN"

landing_help="$($WG_BIN landing-turn --help)"
grep -q 'request' <<<"$landing_help"
grep -q 'status' <<<"$landing_help"
grep -q 'renew' <<<"$landing_help"
grep -q 'release' <<<"$landing_help"
grep -q 'reclaim' <<<"$landing_help"

# Real candidate-binary daemon exercise: three isolated source agents produce
# reviewed candidates, one completes while the daemon is down, then restart
# converges the remaining agents without retrying source work.
WG_SMOKE_CANDIDATE_BIN="$WG_BIN" "$HERE/simple_local_fanout_restart.sh"

# These state-machine tests use three source bindings against one ref, exercise
# deterministic park/head wake/reacquire/release ordering, prove every grow-only
# entry survives, and reload persisted state between mutations. The expiry test
# is the crash/restart leg: an expired owner is fenced and the next exact ticket
# proceeds without deleting the following candidate.
(
  cd "$REPO_ROOT"
  cargo test --quiet --lib landing_turn::tests::fifo_acquire_release_wakes_next
  cargo test --quiet --lib landing_turn::tests::starvation_freedom_every_ticket_lands
  cargo test --quiet --lib landing_turn::tests::restart_recovery_resumes_head_only_when_lease_free
  cargo test --quiet --lib landing_turn::tests::expiry_auto_fences_and_advances
  cargo test --quiet --lib landing_turn::tests::exact_binding_required_to_release
  cargo test --quiet --lib landing_turn::tests::forged_or_stale_binding_cannot_renew_release_cancel_or_publish
  cargo test --quiet --lib landing_turn::tests::retained_source_rebinds_new_candidate_and_fences_old_publication
  cargo test --quiet --test completion_review_valve flip_then_eval_pass_opens_the_exact_manifest_valve
)

printf 'PASS worker-owned-landing-turns\n'
