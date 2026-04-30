#!/usr/bin/env bash
# Smoke: wg-html-resizable — drag-to-resize inspector panel + persisted width.
#
# Pins the regressions that wg-html-resizable closed:
#   1. The inspector panel HTML carries a `panel-resize-handle` element with
#      role=separator so the user has a target to grab.
#   2. CSS provides BOTH col-resize (wide layout, left-edge handle) and
#      row-resize (narrow layout, top-edge handle) cursors so the handle
#      reorients with the panel.
#   3. JS uses pointer events (pointerdown/pointermove/pointerup) so the
#      drag is responsive — width updates during the drag, not only at
#      release.
#   4. JS persists the chosen size to localStorage under
#      `wg-html-inspector-width-px` so reload preserves the user's preference.
#   5. JS sets `--panel-width` inline so the user-chosen value overrides the
#      :root default; the panel CSS already consumes `var(--panel-width)`.
#   6. No JS framework was added — panel.js is still served as a single
#      vanilla file with no fetch / no module imports.
set -euo pipefail

WORK=$(mktemp -d)
OUTDIR=$(mktemp -d)
trap 'rm -rf "$WORK" "$OUTDIR"' EXIT

cd "$WORK"
wg --dir .workgraph init --route claude-cli >/dev/null 2>&1 || true

wg --dir .workgraph add 'task one' --id resize-task-a -d 'a' >/dev/null
wg --dir .workgraph add 'task two' --id resize-task-b --after resize-task-a -d 'b' >/dev/null

wg --dir .workgraph html --out "$OUTDIR" >/dev/null 2>&1

INDEX="$OUTDIR/index.html"
CSS="$OUTDIR/style.css"
JS="$OUTDIR/panel.js"

[ -f "$INDEX" ] || { echo "FAIL: index.html not created"; exit 1; }
[ -f "$CSS"   ] || { echo "FAIL: style.css missing";       exit 1; }
[ -f "$JS"    ] || { echo "FAIL: panel.js missing";        exit 1; }

# (1) Inspector panel exposes a drag handle with role=separator.
grep -q 'id="panel-resize-handle"' "$INDEX" \
    || { echo "FAIL: panel-resize-handle element missing from index"; exit 1; }
grep -q 'role="separator"' "$INDEX" \
    || { echo "FAIL: resize handle missing role=separator (a11y)";    exit 1; }

# (2) CSS for both orientations.
grep -q '\.panel-resize-handle' "$CSS" \
    || { echo "FAIL: .panel-resize-handle CSS rule missing"; exit 1; }
grep -q 'col-resize' "$CSS" \
    || { echo "FAIL: col-resize cursor missing (wide layout)"; exit 1; }
grep -q 'row-resize' "$CSS" \
    || { echo "FAIL: row-resize cursor missing (narrow layout)"; exit 1; }

# (3) Pointer event wiring for live drag feedback.
grep -q 'pointerdown' "$JS" \
    || { echo "FAIL: panel.js missing pointerdown listener"; exit 1; }
grep -q 'pointermove' "$JS" \
    || { echo "FAIL: panel.js missing pointermove listener"; exit 1; }
grep -q 'pointerup'   "$JS" \
    || { echo "FAIL: panel.js missing pointerup listener";   exit 1; }

# (4) localStorage persistence under the documented key.
grep -q "'wg-html-inspector-width-px'" "$JS" \
    || { echo "FAIL: panel.js missing localStorage key wg-html-inspector-width-px"; exit 1; }
grep -q 'localStorage\.setItem' "$JS" \
    || { echo "FAIL: panel.js never writes to localStorage";  exit 1; }
grep -q 'localStorage\.getItem' "$JS" \
    || { echo "FAIL: panel.js never reads from localStorage"; exit 1; }

# (5) The user-chosen size is applied via the CSS variable the panel rule
#     already consumes (style.css: width: var(--panel-width)).
grep -q "setProperty('--panel-width'" "$JS" \
    || { echo "FAIL: panel.js does not set --panel-width inline";   exit 1; }
grep -q 'width: var(--panel-width)' "$CSS" \
    || { echo "FAIL: .side-panel rule does not consume --panel-width"; exit 1; }

# (6) Still vanilla — no module imports, no fetch, no framework tags.
if grep -qE '\b(import|require)\(' "$JS"; then
    echo "FAIL: panel.js contains module imports — must remain vanilla"
    exit 1
fi
if grep -qE '\bfetch\(' "$JS"; then
    echo "FAIL: panel.js contains fetch() — must be runtime-dep-free"
    exit 1
fi

# Smoke node-syntax check so a typo doesn't ship.
if command -v node >/dev/null 2>&1; then
    node --check "$JS" >/dev/null 2>&1 \
        || { echo "FAIL: panel.js has a syntax error per node --check"; exit 1; }
fi

echo "PASS: wg_html_resizable_inspector"
