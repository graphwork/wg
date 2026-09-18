#!/usr/bin/env bash
# Smoke scenario: npm_pack_install_dry_run
#
# Pins the npm distribution first slice (implement-the-npm,
# docs/research/npm-distribution.md): pack the @worksgood npm packages, install
# them into clean prefixes, and prove the zero-script shim resolves + execs the
# binaries (`worksgood <version>` from the npm-installed layout), honors
# WG_BINARY_PATH, falls back to the cargo-install hint, that --ignore-scripts
# installs work, that the metapackage pulls the platform package AND the pi
# CLI, and that every package survives `npm publish --dry-run`.
#
# Exit 77 = loud SKIP (no prebuilt binaries, wrong platform, or npm registry
# unreachable). Network is required for the pi dependency pull.
set -euo pipefail

cd "$(dirname "$0")/../.."

BIN_DIR=""
for cand in target/release target/debug "$HOME/.cargo/bin"; do
  if [[ -x "${cand}/wg" && -x "${cand}/worksgood" && -x "${cand}/nex" ]]; then
    BIN_DIR="${cand}"
    break
  fi
done
if [[ -z "${BIN_DIR}" ]]; then
  echo "SKIP: no prebuilt wg/nex/worksgood binaries (cargo build --release --bins first)"
  exit 77
fi

if ! curl -fsSI --max-time 10 https://registry.npmjs.org/ >/dev/null 2>&1; then
  echo "SKIP: npm registry unreachable"
  exit 77
fi

exec bash scripts/npm/verify-install.sh --bin-dir "${BIN_DIR}"
