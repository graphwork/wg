#!/usr/bin/env bash
# Scenario: pi_tool_hang_watchdog_hardcap
#
# Regression for the 2026-10-23 21-hour worker hang (`fix-tool-hang`):
#
#   (1) pi's built-in `bash` tool had NO default timeout, so a model-generated
#       command that reads stdin (a half-open heredoc in a python patch script)
#       waited forever at 0% CPU;
#   (2) the WG pi-watchdog OBSERVED the proof-silence (69473s vs soft=300s /
#       hard=600s) and only LOGGED — it never acted;
#   (3) `pi-stream-observe --follow-pid` spun at ~6% CPU forever.
#
# Asserts, against the checked-in fixes:
#   (a) the `bash` tool_call mutator injects a bounded default timeout and
#       honors explicit values + config/env overrides (plugin vitest);
#   (b) a hard-cap-silent Pi attempt is KILLED, failed with
#       `watchdog-tool-silence-hardcap`, and `wg retry` resumes in place
#       (Rust integration test through the real bootstrap/observe/retry seams);
#   (c) soft silence is log-only, and the native follower exits non-zero on
#       prolonged no-progress instead of spinning forever (same test file).
#
# Loud SKIP (exit 77) when cargo / node toolchains are unavailable.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "NO CARGO" "cargo not on PATH; cannot run the watchdog hard-cap tests"
fi
if [ ! -f "$repo_root/Cargo.toml" ]; then
    loud_skip "NO WORKSPACE" "Cargo.toml not found at $repo_root; not a source checkout"
fi

rust_log="$(mktemp)"
add_cleanup_hook "rm -f '$rust_log'"

echo "running FIX B/C proof (watchdog hard cap kills + fails + retry in place; observer no-progress exit)..."
if ! ( cd "$repo_root" && cargo test --test integration_pi_watchdog_hardcap ) >"$rust_log" 2>&1; then
    echo "---- cargo output (tail) ----" 1>&2
    tail -60 "$rust_log" 1>&2
    loud_fail "the watchdog hard-cap / observer no-progress proof FAILED (see tail above)"
fi
grep -q "hard_cap_breach_kills_the_wedged_pi" "$rust_log" ||
    loud_fail "hard-cap kill test did not run"
grep -q "soft_silence_is_log_only" "$rust_log" ||
    loud_fail "soft-silence log-only test did not run"

# FIX A lives in the plugin (TypeScript). Skip loudly without a node toolchain.
if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
    loud_skip "NO NODE" "node/npm not on PATH; cannot run the plugin bash-timeout tests"
fi
if [ ! -d "$repo_root/worksgood-pi/node_modules" ]; then
    loud_skip "NO NODE_MODULES" "run 'npm ci' in worksgood-pi to exercise the plugin test"
fi

plugin_log="$(mktemp)"
add_cleanup_hook "rm -f '$plugin_log'"

echo "running FIX A proof (bash tool_call default-timeout injection)..."
if ! ( cd "$repo_root/worksgood-pi" && npx vitest run test/bash-timeout.test.ts ) >"$plugin_log" 2>&1; then
    echo "---- vitest output (tail) ----" 1>&2
    tail -60 "$plugin_log" 1>&2
    loud_fail "the plugin bash-timeout tests FAILED (see tail above)"
fi

echo "pi_tool_hang_watchdog_hardcap: PASS — default bash timeout, enforced hard cap, bounded observer"
exit 0
