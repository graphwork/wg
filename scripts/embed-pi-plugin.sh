#!/usr/bin/env bash
#
# embed-pi-plugin.sh — regenerate the committed, version-locked plugin bundle
# that the `wg` binary embeds (`include_dir!` of pi-plugin/embedded/).
#
# Pipeline (single source of truth = the Rust compat const):
#   1. read WG_PI_PLUGIN_COMPAT_VERSION from src/pi_plugin/mod.rs
#   2. stamp it into pi-plugin/src/version.ts (the plugin's runtime assertion)
#      and pi-plugin/embedded/version.json (the wire-compat stamp)
#   3. `npm ci && npm run build` (deterministic given the pinned lockfile)
#   4. copy the curated RUNTIME subset into pi-plugin/embedded/:
#        dist/*.js  +  host/wg-pi-host.mjs  +  package.json  +  version.json
#      (.d.ts / *.map / *.tsbuildinfo are excluded — not needed at runtime)
#
# This keeps `cargo install --path .` node-free (the bytes are already in the
# tree). CI re-runs this and `git diff --exit-code`s pi-plugin/embedded so a
# source edit without a re-embed fails loudly (anti-drift guarantee).
#
# Usage:
#   scripts/embed-pi-plugin.sh            # full: npm ci + build + copy
#   scripts/embed-pi-plugin.sh --no-install   # reuse existing node_modules
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/pi-plugin"
EMBED_DIR="$PLUGIN_DIR/embedded"
CONST_FILE="$REPO_ROOT/src/pi_plugin/mod.rs"

# 1. Single source of truth: the Rust const.
COMPAT="$(sed -n 's/.*WG_PI_PLUGIN_COMPAT_VERSION: &str = "\([^"]*\)".*/\1/p' "$CONST_FILE" | head -n1)"
if [ -z "$COMPAT" ]; then
  echo "embed-pi-plugin: could not read WG_PI_PLUGIN_COMPAT_VERSION from $CONST_FILE" >&2
  exit 1
fi
echo "embed-pi-plugin: compat version = $COMPAT"

# 2. Stamp version.ts (generated; committed) BEFORE building so tsc picks it up.
cat > "$PLUGIN_DIR/src/version.ts" <<EOF
/**
 * version.ts — the wg↔pi WIRE-COMPAT stamp (GENERATED — do not edit by hand).
 *
 * Single source of truth is the Rust const \`WG_PI_PLUGIN_COMPAT_VERSION\` in
 * \`src/pi_plugin/mod.rs\`. The \`make embed-pi-plugin\` step rewrites BOTH this
 * file and \`pi-plugin/embedded/version.json\` from that const so the three can
 * never silently diverge (a Rust unit test asserts const == embedded JSON, and
 * CI re-runs the embed and \`git diff --exit-code\`s the result).
 *
 * This is a *wire-compat* number, deliberately decoupled from the npm
 * \`package.json\` \`version\` of \`@worksgood/wg-pi-plugin\` — exactly as agency's
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
mkdir -p "$EMBED_DIR/dist" "$EMBED_DIR/host"

# dist/*.js only (excludes *.d.ts, *.js.map, *.d.ts.map, *.tsbuildinfo).
shopt -s nullglob
for f in "$PLUGIN_DIR"/dist/*.js; do
  cp "$f" "$EMBED_DIR/dist/"
done
shopt -u nullglob

cp "$PLUGIN_DIR/host/wg-pi-host.mjs" "$EMBED_DIR/host/"
cp "$PLUGIN_DIR/package.json" "$EMBED_DIR/package.json"

# version.json — the wire-compat stamp (matches the Rust const).
printf '{\n  "compat": "%s"\n}\n' "$COMPAT" > "$EMBED_DIR/version.json"

echo "embed-pi-plugin: wrote $EMBED_DIR"
ls -1 "$EMBED_DIR/dist"
