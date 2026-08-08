#!/usr/bin/env bash
# Executable before/after regression pinned to the restrictive pre-fix tree.
set -euo pipefail
: "${WG_BIN:?set WG_BIN to the current candidate binary}"
repo=$(git rev-parse --show-toplevel)
historical_commit=da286458ac640a6c4a49b269284c39e1d9ff3fdf
scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-historical-worker-control.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/source"
git -C "$repo" archive "$historical_commit" | tar -x -C "$scratch/source"
# A dedicated target preserves a reproducible old executable without replacing
# the candidate under test. Dependencies are shared after the first run.
CARGO_TARGET_DIR="$repo/target/historical-worker-control" \
  cargo build --quiet --manifest-path "$scratch/source/Cargo.toml" --bin wg
old_bin="$repo/target/historical-worker-control/debug/wg"
[[ -x $old_bin ]]
# The historical source's own real-daemon/Fake-Pi fixture executes the exact
# own-task adapter and reproduces cross-task/graph enumeration refusal.
WG_BIN="$old_bin" bash "$scratch/source/tests/smoke/scenarios/worker_control_capability_broker.sh"
# The current candidate executes the same class of real worker as trusted and
# completes a quality-pass flow with cross-task mutations and release.
WG_BIN="$WG_BIN" bash "$repo/tests/smoke/scenarios/trust_first_local_worker_coordination.sh"
echo "PASS: historical $historical_commit reproduced scoped cross-task refusal; current candidate restored default trust-first coordination"
