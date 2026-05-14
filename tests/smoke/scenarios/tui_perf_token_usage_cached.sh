#!/usr/bin/env bash
# Scenario: tui_perf_token_usage_cached
#
# Pins fix-tui-perf-2 fix 2 (parse_token_usage_live caching). A re-parse of
# the same output.log must be effectively free (a metadata syscall + map
# lookup), not a full JSONL re-parse.
#
# Backed by the integration test `bench_token_usage_cache_avoids_reparse`
# in tests/integration_tui_perf_benchmarks.rs. Regression here would mean
# the diagnose hot path #1 (live_token_usage / agency_token_usage walking
# every output.log per fs-event) has been re-introduced.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "MISSING CARGO" "cargo not on PATH"
fi

if [[ ! -f "$repo_root/Cargo.toml" ]]; then
    loud_skip "NOT IN WG REPO" "Cargo.toml not at $repo_root"
fi

log=$(mktemp -t tui_perf_token.XXXXXX.log)
add_cleanup_hook "rm -f $log"
cd "$repo_root"
if ! cargo test --test integration_tui_perf_benchmarks \
        bench_token_usage_cache_avoids_reparse \
        -- --nocapture >"$log" 2>&1; then
    loud_fail "bench_token_usage_cache_avoids_reparse failed:
$(tail -40 "$log")"
fi

if ! grep -q "test result: ok\. 1 passed" "$log"; then
    loud_fail "bench_token_usage_cache_avoids_reparse did not report '1 passed'. Output:
$(tail -40 "$log")"
fi

echo "PASS: parse_token_usage_live cache avoids re-parse on unchanged mtime"
exit 0
