#!/usr/bin/env bash
# Regression: integration-target amplification and mutable per-worker targets.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
repo_root="$(cd "$HERE/../../.." && pwd)"

command -v cargo >/dev/null 2>&1 || loud_skip "NO CARGO" "cargo not on PATH"
[ -f "$repo_root/Cargo.toml" ] || loud_skip "NO WORKSPACE" "source checkout unavailable"

cd "$repo_root"
count=$(cargo metadata --no-deps --format-version 1 | python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(sum("test" in t["kind"] for p in d["packages"] for t in p["targets"]))') || \
  loud_fail "could not inspect Cargo targets"
[ "$count" -le 16 ] || loud_fail "integration harness amplification returned: $count test crates"

grep -q '^debug = "line-tables-only"$' Cargo.toml || loud_fail "bounded test debuginfo profile missing"
grep -q '^incremental = false$' Cargo.toml || loud_fail "test incremental storage is not disabled"

log=$(mktemp -t wg-bounded-storage.XXXXXX)
add_cleanup_hook "rm -f '$log'"
if ! cargo test --lib target_cache::tests -- --test-threads=1 >"$log" 2>&1; then
  tail -80 "$log" >&2
  loud_fail "copy-on-write target cache fault tests failed"
fi
if ! cargo test --lib disk_sentinel::tests::zero_headroom_never_reports_healthy_even_with_zero_thresholds \
  -- --exact >"$log" 2>&1; then
  tail -80 "$log" >&2
  loud_fail "zero-headroom admission fault test failed"
fi

echo "bounded_worktree_build_storage: PASS — $count harnesses; CoW, invalidation, crash/GC and admission checks pass"
