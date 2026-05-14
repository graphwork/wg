#!/usr/bin/env bash
# Scenario: tui_perf_message_stats_pair_folds
#
# Pins fix-tui-perf-2 fix 1 (single-pass + cached message_stats):
# `message_stats_pair_cached` over a 1000-task fixture must outperform
# the standalone-functions baseline by at least 1.5×, and re-scans of
# unmodified files must be cache hits (~free).
#
# This is the "bench E" entry from diagnose-tui-scales' specified scenarios.
# Backed by the integration test `bench_e_message_stats_pair_folds_to_one_read`
# in tests/integration_tui_perf_benchmarks.rs — running it here means a
# regression in the message-stats hot path blocks `wg done` on any task
# that owns this scenario.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

# Run from the WG repo root (parent of tests/smoke/).
repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "MISSING CARGO" "cargo not on PATH"
fi

if [[ ! -f "$repo_root/Cargo.toml" ]]; then
    loud_skip "NOT IN WG REPO" "Cargo.toml not at $repo_root"
fi

# Run only the bench-E test from the integration_tui_perf_benchmarks suite.
# We keep this in debug mode (no --release) because the test thresholds are
# tuned to be tolerant of unoptimized builds.
log=$(mktemp -t tui_perf_e.XXXXXX.log)
add_cleanup_hook "rm -f $log"
cd "$repo_root"
if ! cargo test --test integration_tui_perf_benchmarks \
        bench_e_message_stats_pair_folds_to_one_read \
        -- --nocapture >"$log" 2>&1; then
    loud_fail "bench_e_message_stats_pair_folds_to_one_read failed:
$(tail -40 "$log")"
fi

if ! grep -q "test result: ok\. 1 passed" "$log"; then
    loud_fail "bench_e did not report '1 passed'. Output:
$(tail -40 "$log")"
fi

echo "PASS: message_stats_pair_cached folds two reads into one with cache speedup"
exit 0
