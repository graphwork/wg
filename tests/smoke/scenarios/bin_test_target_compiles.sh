#!/usr/bin/env bash
# Scenario: bin_test_target_compiles
#
# Regression for prod-verify (2026-06-28): the `peer::run_add` signature
# grew a 7th `trust: Option<&str>` parameter (the M18 / wire-review
# split-trust-dial work) but the seven `#[cfg(test)]` callers in
# src/commands/peer.rs were never updated — they still passed 6 arguments.
#
# This compiled and shipped GREEN because CI deliberately runs only
# `cargo test --lib` / `--doc` / `--test integration_*` and NEVER compiles
# the binary crate's `#[cfg(test)]` modules (the lib-vs-bin blind spot
# documented in the project memory). So a broken bin unit test is invisible
# to every CI gate and to `cargo test --lib`.
#
# This scenario closes that blind spot: it compiles the binary test target
# (without running it) and FAILS if it does not build. Any future signature
# change that breaks a `#[cfg(test)]` caller in the `wg` binary crate is
# caught here at the smoke gate instead of rotting undetected.
#
# Asserts:
#   (a) `cargo test --bin wg --no-run` compiles successfully (exit 0)
#
# Loud SKIP (exit 77) when cargo or the workspace manifest is unavailable
# (e.g. a packaged runner with no toolchain).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

# Repo root is three levels up from scenarios/: <root>/tests/smoke/scenarios.
repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    loud_skip "NO CARGO" "cargo not on PATH; cannot compile the bin test target"
fi
if [ ! -f "$repo_root/Cargo.toml" ]; then
    loud_skip "NO WORKSPACE" "Cargo.toml not found at $repo_root; not a source checkout"
fi

echo "compiling the wg binary test target (catches broken bin #[cfg(test)] callers)..."
build_log="$(mktemp)"
add_cleanup_hook "rm -f '$build_log'"

if ( cd "$repo_root" && cargo test --bin wg --no-run ) >"$build_log" 2>&1; then
    echo "ok: 'cargo test --bin wg --no-run' compiled cleanly"
    echo "bin_test_target_compiles: PASS — the wg binary unit-test target builds"
    exit 0
fi

echo "---- cargo output (tail) ----" 1>&2
tail -40 "$build_log" 1>&2
loud_fail "the wg binary test target FAILED to compile — a #[cfg(test)] caller in \
the bin crate is broken (CI does not compile this target; see prod-verify)"
