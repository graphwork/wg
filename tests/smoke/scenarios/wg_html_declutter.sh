#!/usr/bin/env bash
# Smoke: wg-html-declutter — clean header + clickable Legend in side panel.
#
# Pins the regressions that wg-html-declutter closed:
#   1. The page header is just <h1>workgraph</h1> + "<n> tasks shown" —
#      no "click a task id to inspect" subtitle, no "Dependency graph
#      (click ... — magenta = upstream deps · cyan = downstream consumers)"
#      parenthetical headline above the viz.
#   2. A "Legend" button is wired in the header controls and the panel.js
#      script has the openLegend handler that fills the side panel from
#      the <template id="wg-legend-template"> element.
#   3. The legend template covers all four required sections: edge colors,
#      status colors, interactions, CLI flags.
#   4. The inline `<section class="legend-section">` block is removed —
#      the legend is now panel-only.
#   5. Edge color swatches reference the CSS variables (--edge-upstream
#      etc.) so they track the active theme.
set -euo pipefail

WORK=$(mktemp -d)
OUTDIR=$(mktemp -d)
trap 'rm -rf "$WORK" "$OUTDIR"' EXIT

cd "$WORK"
wg --dir .wg init --executor claude --model claude:opus 2>/dev/null \
    || wg --dir .wg init >/dev/null 2>&1 \
    || true
wg --dir .wg add 'parent task' --id pdc-parent -d 'parent for declutter smoke' >/dev/null
wg --dir .wg add 'child task'  --id pdc-child  --after pdc-parent \
    -d 'child for declutter smoke' >/dev/null

wg --dir .wg html --out "$OUTDIR" >/dev/null 2>&1

INDEX="$OUTDIR/index.html"
JS="$OUTDIR/panel.js"
[ -f "$INDEX" ] || { echo "FAIL: index.html missing"; exit 1; }
[ -f "$JS"    ] || { echo "FAIL: panel.js missing";   exit 1; }

# (1) Clean header: subtitle is just "<n> tasks shown" with no redundant text.
grep -qE '<p class="subtitle">[0-9]+ tasks shown</p>' "$INDEX" \
    || { echo "FAIL: subtitle should be just '<n> tasks shown'"; \
         grep -n 'class="subtitle"' "$INDEX"; exit 1; }
# Old redundant text MUST NOT appear in the page chrome (header + section
# headings — task JSON below WG_TASKS may legitimately contain these as
# historical task descriptions, so we limit the search to the header).
HEADER=$(awk '/<body>/,/<main/' "$INDEX")
echo "$HEADER" | grep -q 'click a task id to inspect' \
    && { echo "FAIL: redundant subtitle 'click a task id to inspect' still in header"; exit 1; }
echo "$HEADER" | grep -q '<h2>Dependency graph' \
    && { echo "FAIL: redundant '<h2>Dependency graph' headline still in chrome"; exit 1; }
echo "$HEADER" | grep -q 'magenta = upstream deps' \
    && { echo "FAIL: redundant edge-color parenthetical still in chrome"; exit 1; }

# (2) Legend button + JS wiring.
grep -q 'id="legend-toggle"' "$INDEX" \
    || { echo "FAIL: legend toggle button missing from header"; exit 1; }
grep -q "getElementById('legend-toggle')" "$JS" \
    || { echo "FAIL: panel.js doesn't bind the legend toggle button"; exit 1; }
grep -q 'function openLegend' "$JS" \
    || { echo "FAIL: panel.js missing openLegend()"; exit 1; }
grep -q "getElementById('wg-legend-template')" "$JS" \
    || { echo "FAIL: panel.js doesn't read the legend template"; exit 1; }

# (3) Legend template content — all four spec'd sections.
grep -q 'id="wg-legend-template"' "$INDEX" \
    || { echo "FAIL: legend template element missing"; exit 1; }
TEMPLATE=$(awk '/wg-legend-template/{p=1} p{print; if(/<\/template>/){exit}}' "$INDEX")
for needle in \
    'Edge colors' \
    'magenta' \
    'cyan' \
    'yellow' \
    'Status colors' \
    'Interactions' \
    'Click any task id' \
    'CLI flags' \
    '--chat' \
    '--since' \
    '--all' \
; do
    echo "$TEMPLATE" | grep -qF -e "$needle" \
        || { echo "FAIL: legend template missing '$needle'"; exit 1; }
done

# (4) Inline legend section block must be gone (legend is panel-only now).
grep -q 'class="legend-section"' "$INDEX" \
    && { echo "FAIL: inline legend-section block should be removed"; exit 1; }
# The "<h2>Legend</h2>" headline (which used to be in the section) is gone too.
echo "$HEADER" | grep -q '<h2>Legend</h2>' \
    && { echo "FAIL: inline <h2>Legend</h2> headline still in chrome"; exit 1; }

# (5) Edge swatches reference CSS variables for theme-aware colors.
echo "$TEMPLATE" | grep -q 'background:var(--edge-upstream)' \
    || { echo "FAIL: legend doesn't use --edge-upstream CSS variable"; exit 1; }
echo "$TEMPLATE" | grep -q 'background:var(--edge-downstream)' \
    || { echo "FAIL: legend doesn't use --edge-downstream CSS variable"; exit 1; }
echo "$TEMPLATE" | grep -q 'background:var(--edge-cycle)' \
    || { echo "FAIL: legend doesn't use --edge-cycle CSS variable"; exit 1; }

echo "PASS: wg-html-declutter — clean header + clickable Legend (template/button/JS wired)"
