#!/usr/bin/env bash
#
# embed-worksgood-pi.sh — regenerate the committed, version-locked WorksGood Pi
# bundle that the `wg` binary embeds (`include_dir!` of worksgood-pi/embedded/).
#
# Pipeline (single source of truth = the Rust compat const):
#   1. read WG_PI_PLUGIN_COMPAT_VERSION from src/pi_plugin/mod.rs
#   2. stamp it into worksgood-pi/src/version.ts (the extension's runtime
#      assertion) and worksgood-pi/embedded/version.json (the wire stamp)
#   3. `npm ci && npm run build` (deterministic given the pinned lockfile)
#   4. copy the curated RUNTIME subset into worksgood-pi/embedded/:
#        pi-worksgood/*.js + host/wg-pi-host.mjs + package.json + version.json
#      (.d.ts / *.map / *.tsbuildinfo are excluded — not needed at runtime)
#
# This keeps `cargo install --path .` node-free (the bytes are already in the
# tree). CI re-runs this and `git diff --exit-code`s worksgood-pi/embedded so a
# source edit without a re-embed fails loudly (anti-drift guarantee).
#
# Usage:
#   scripts/embed-worksgood-pi.sh              # full: npm ci + build + copy
#   scripts/embed-worksgood-pi.sh --no-install # reuse existing node_modules
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/worksgood-pi"
EMBED_DIR="$PLUGIN_DIR/embedded"
CONST_FILE="$REPO_ROOT/src/pi_plugin/mod.rs"

# 1. Single source of truth: the Rust const.
COMPAT="$(sed -n 's/.*WG_PI_PLUGIN_COMPAT_VERSION: &str = "\([^"]*\)".*/\1/p' "$CONST_FILE" | head -n1)"
if [ -z "$COMPAT" ]; then
  echo "embed-worksgood-pi: could not read WG_PI_PLUGIN_COMPAT_VERSION from $CONST_FILE" >&2
  exit 1
fi
echo "embed-worksgood-pi: compat version = $COMPAT"

# 2. Stamp version.ts (generated; committed) BEFORE building so tsc picks it up.
cat > "$PLUGIN_DIR/src/version.ts" <<EOF
/**
 * version.ts — the wg↔pi WIRE-COMPAT stamp (GENERATED — do not edit by hand).
 *
 * Single source of truth is the Rust const \`WG_PI_PLUGIN_COMPAT_VERSION\` in
 * \`src/pi_plugin/mod.rs\`. The \`make embed-worksgood-pi\` step rewrites BOTH this
 * file and \`worksgood-pi/embedded/version.json\` from that const so the three can
 * never silently diverge (a Rust unit test asserts const == embedded JSON, and
 * CI re-runs the embed and \`git diff --exit-code\`s the result).
 *
 * This is a *wire-compat* number, deliberately decoupled from the npm
 * \`package.json\` \`version\` of \`@worksgood/pi\` — exactly as agency's
 * \`WG_AGENCY_COMPAT_VERSION\` is decoupled from any package version. Bump it
 * whenever the wg↔plugin flag/contract surface changes.
 *
 * The plugin factory (\`src/index.ts\`) asserts this value against the wg binary
 * at startup and fails LOUDLY on mismatch.
 */
export const WG_PI_PLUGIN_COMPAT_VERSION = "$COMPAT";
EOF

# 3. Build the plugin.
if [ "${1:-}" = "--no-install" ]; then
  npm --prefix "$PLUGIN_DIR" run build
else
  npm --prefix "$PLUGIN_DIR" ci
  npm --prefix "$PLUGIN_DIR" run build
fi

# 4. Copy the curated runtime subset into a clean embedded/ tree.
rm -rf "$EMBED_DIR"
mkdir -p "$EMBED_DIR/pi-worksgood" "$EMBED_DIR/host"

# pi-worksgood/*.js only (excludes declarations, maps, and tsbuildinfo).
shopt -s nullglob
for f in "$PLUGIN_DIR"/pi-worksgood/*.js; do
  cp "$f" "$EMBED_DIR/pi-worksgood/"
done
shopt -u nullglob

cp "$PLUGIN_DIR/host/wg-pi-host.mjs" "$EMBED_DIR/host/"
cp "$PLUGIN_DIR/package.json" "$EMBED_DIR/package.json"

# version.json — the wire-compat stamp (matches the Rust const).
printf '{\n  "compat": "%s"\n}\n' "$COMPAT" > "$EMBED_DIR/version.json"

echo "embed-worksgood-pi: wrote $EMBED_DIR"
ls -1 "$EMBED_DIR/pi-worksgood"
