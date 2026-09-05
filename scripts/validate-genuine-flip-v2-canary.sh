#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

# Proof binding and phase-I prompt visibility.
cargo test --test completion_review_valve \
  genuine_flip_proof_rejects_every_broken_execution_binding -- --exact
cargo test --lib \
  completion_review_model::tests::blind_flip_prompt_hides_original_intent_until_fresh_comparison \
  -- --exact
cargo test --test completion_review_valve \
  flip_rejection_returns_to_source_without_invoking_eval -- --exact

# Use this exact submitted tree's candidate binary for the live isolated smokes.
cargo build --quiet --bin wg
candidate_bin="${CARGO_TARGET_DIR:-$root/target}/debug/wg"
WG_SMOKE_CANDIDATE_BIN="$candidate_bin" \
  bash tests/smoke/scenarios/completion_resilience_e2e.sh
WG_SMOKE_CANDIDATE_BIN="$candidate_bin" \
  bash tests/smoke/scenarios/worker_owned_landing_turns.sh

# Repeat the completion/restart paths with the exact installed image and prove
# that the invoked CLI and live daemon are the coordinated post-88e79dc9 bytes.
installed_wg=$(command -v wg)
installed_sha=$(sha256sum "$installed_wg" | awk '{print $1}')
daemon_pid=$($installed_wg status | sed -n 's/Service: running (PID \([0-9]*\).*/\1/p')
test -n "$daemon_pid"
test "$(sha256sum "/proc/$daemon_pid/exe" | awk '{print $1}')" = "$installed_sha"
test "$(stat -Lc %i "/proc/$daemon_pid/exe")" = "$(stat -Lc %i "$installed_wg")"
git merge-base --is-ancestor 88e79dc94d8c89ab70d3c7407d36b47a013b8ea1 HEAD
WG_SMOKE_CANDIDATE_BIN="$installed_wg" \
  bash tests/smoke/scenarios/completion_resilience_e2e.sh
WG_SMOKE_CANDIDATE_BIN="$installed_wg" \
  bash tests/smoke/scenarios/worker_owned_landing_turns.sh

# Checked-in Rust and Pi-plugin policy.
cargo fmt --check
cargo clippy
npm --prefix worksgood-pi ci
npm --prefix worksgood-pi run build
npm --prefix worksgood-pi run selftest
node worksgood-pi/host/wg-pi-host.mjs --selftest --force-compat-mismatch
npm --prefix worksgood-pi test
scripts/embed-worksgood-pi.sh --no-install
git diff --exit-code worksgood-pi/embedded worksgood-pi/src/version.ts
