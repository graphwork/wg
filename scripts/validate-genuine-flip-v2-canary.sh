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
