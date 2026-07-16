#!/usr/bin/env bash
set -euo pipefail

# Build the exact Pi source WG has tested, apply the maintained output-guard
# patch in its canonical packages/coding-agent location, and install the
# resulting npm tarball. This intentionally does not edit an existing
# node_modules tree in place.

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PI_REPOSITORY=https://github.com/earendil-works/pi.git
PI_COMMIT=b084d2fb395f0f1aa924cb07b14e5d0edab115e2
PI_VERSION=0.80.6
PATCH="$ROOT/docs/pi-integration/upstream-patch/output-guard-epipe/OUTPUT_GUARD_EPIPE.patch"

command -v git >/dev/null 2>&1 || { echo "error: git is required" >&2; exit 1; }
command -v npm >/dev/null 2>&1 || { echo "error: npm is required" >&2; exit 1; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/wg-patched-pi.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

echo "Cloning Pi ${PI_VERSION} source at ${PI_COMMIT}..."
git clone --quiet "$PI_REPOSITORY" "$WORK/pi"
git -C "$WORK/pi" checkout --quiet "$PI_COMMIT"

echo "Applying WG's maintained output-guard patch..."
git -C "$WORK/pi" apply --check "$PATCH"
git -C "$WORK/pi" apply "$PATCH"

echo "Installing Pi monorepo dependencies and building..."
npm --prefix "$WORK/pi" install --ignore-scripts
npm --prefix "$WORK/pi" run build
npm --prefix "$WORK/pi" --workspace @earendil-works/pi-coding-agent test -- output-guard.test.ts

echo "Packing and globally installing the patched coding-agent package..."
npm --prefix "$WORK/pi" pack --workspace @earendil-works/pi-coding-agent --pack-destination "$WORK"
TARBALL=$(find "$WORK" -maxdepth 1 -type f -name '*pi-coding-agent-*.tgz' -print -quit)
[[ -n "$TARBALL" ]] || { echo "error: npm pack did not produce a Pi tarball" >&2; exit 1; }
npm install --global "$TARBALL"

echo "Installed runtime evidence:"
command -v pi
pi --version
wg doctor 2>&1 | sed -n '/Pi output guard/,+3p' || true
