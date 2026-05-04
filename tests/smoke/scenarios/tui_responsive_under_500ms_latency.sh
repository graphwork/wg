#!/usr/bin/env bash
# Scenario: tui_responsive_under_500ms_latency
#
# Pins fix-tui-must: under simulated 500ms-latency filesystem, the TUI's
# main-thread API on `AsyncFs` must remain non-blocking — every request_*
# / cached_* / drain_responses call returns in < 50ms p99 even while the
# worker is stuck on a slow disk read.
#
# Mechanism: WG_ASYNC_FS_TEST_LATENCY_MS=500 is read once by the worker
# (via OnceLock) at process start. Each disk op then sleeps 500ms before
# the actual syscall. We run two unit tests under the bin's test harness:
#
#   1. main_thread_api_never_blocks — sanity: 3500 main-thread calls
#      complete in < 1s with no single call > 50ms (no slow injection).
#   2. main_thread_api_unblocked_under_simulated_500ms_latency — the
#      headline test. Worker is artificially slow; main-thread API stays
#      microsecond-fast.
#
# Backed by tests in src/tui/viz_viewer/async_fs.rs. A regression here
# blocks `wg done` on tasks that own this scenario.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

# Run from the workgraph repo root.
repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "MISSING CARGO" "cargo not on PATH"
fi

if [[ ! -f "$repo_root/Cargo.toml" ]]; then
    loud_skip "NOT IN WORKGRAPH REPO" "Cargo.toml not at $repo_root"
fi

cd "$repo_root"

# Test 1: baseline non-blocking dispatch. No latency injection.
log1=$(mktemp -t tui_responsive_baseline.XXXXXX.log)
add_cleanup_hook "rm -f $log1"
if ! cargo test --bin wg \
        tui::viz_viewer::async_fs::tests::main_thread_api_never_blocks \
        -- --nocapture >"$log1" 2>&1; then
    loud_fail "main_thread_api_never_blocks failed:
$(tail -40 "$log1")"
fi
if ! grep -q "test result: ok\. 1 passed" "$log1"; then
    loud_fail "main_thread_api_never_blocks did not report '1 passed'. Output:
$(tail -40 "$log1")"
fi

# Test 2: with 500ms injected latency, main-thread calls stay fast.
# The injected latency is read once per process, so this MUST be a fresh
# `cargo test` invocation (or rather, a fresh test binary spawn). Cargo
# will reuse the same compiled binary, but each `cargo test` invocation
# spawns it fresh — that picks up the env var.
log2=$(mktemp -t tui_responsive_injected.XXXXXX.log)
add_cleanup_hook "rm -f $log2"
if ! WG_ASYNC_FS_TEST_LATENCY_MS=500 cargo test --bin wg \
        tui::viz_viewer::async_fs::tests::main_thread_api_unblocked_under_simulated_500ms_latency \
        -- --ignored --nocapture >"$log2" 2>&1; then
    loud_fail "main_thread_api_unblocked_under_simulated_500ms_latency failed:
$(tail -40 "$log2")"
fi
if ! grep -q "test result: ok\. 1 passed" "$log2"; then
    loud_fail "main_thread_api_unblocked_under_simulated_500ms_latency did not report '1 passed'. Output:
$(tail -40 "$log2")"
fi

echo "PASS: TUI main-thread API stays unblocked under simulated 500ms FS latency"
exit 0
