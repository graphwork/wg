#!/usr/bin/env bash
# Smoke: wg-html-v2 — TUI-parity HTML viewer.
#
# Pins the regressions that wg-html-v2 closed:
#   1. Default = all tasks (not public-only — wg-html-redesign default was
#      wrong for the read-only TUI-sibling use case).
#   2. Edge spans must carry data-edges="from>to" attribution per character so
#      JS can distinguish upstream vs downstream when a node is selected.
#   3. CSS must preserve terminal status parity: live/in-progress is yellow,
#      ordinary open is the neutral theme foreground, and edge highlights use
#      render.rs:1500 magenta/cyan/yellow.
#   4. CSS must declare both auto (prefers-color-scheme) and manual
#      (data-theme) overrides for dark + light themes.
#   5. The page is statically rsync-friendly: opens over file:// with no
#      backend (no XHR/fetch usages in panel.js).
set -euo pipefail

# Build a small graph in a temp .wg so we exercise edge attribution
# deterministically (the host's .wg may or may not have a clean
# parent→child it shows in the truncated viz output).
WORK=$(mktemp -d)
OUTDIR=$(mktemp -d)
trap 'rm -rf "$WORK" "$OUTDIR"' EXIT

cd "$WORK"
wg --dir .wg init --executor claude --model claude:opus 2>/dev/null \
    || wg --dir .wg init >/dev/null 2>&1 \
    || true

# Two tasks with a single --after edge gives us a guaranteed connector glyph.
wg --dir .wg add 'parent task' --id parent-v2t -d 'parent for smoke'    >/dev/null
wg --dir .wg add 'child task'  --id child-v2t  --after parent-v2t \
    -d 'child for smoke'                                                       >/dev/null
# Exercise both user-visible statuses in the generated fixture without
# disturbing the parent→child dependency used by the edge assertions.
wg --dir .wg add 'live task' --id live-v2t -d 'live for palette smoke' >/dev/null
wg --dir .wg claim live-v2t --actor html-v2-smoke >/dev/null

# Render with all defaults — the spec says no flags = all tasks.
wg --dir .wg html --out "$OUTDIR" 2>&1

INDEX="$OUTDIR/index.html"
[ -f "$INDEX" ]                || { echo "FAIL: index.html not created";    exit 1; }
[ -f "$OUTDIR/style.css" ]     || { echo "FAIL: style.css missing";         exit 1; }
[ -f "$OUTDIR/panel.js"  ]     || { echo "FAIL: panel.js missing";          exit 1; }

# (1) Default = all tasks: footer says "all tasks" not "public-only".
grep -q 'all tasks' "$INDEX" \
    || grep -q 'Showing.*of.*tasks' "$INDEX" \
    || { echo "FAIL: default mode footer should reflect all-tasks default"; exit 1; }

# (2) Edge attribution per character.
grep -q 'class="edge"' "$INDEX" \
    || { echo "FAIL: no .edge spans found — char_edge_map not wired";       exit 1; }
grep -q 'data-edges="parent-v2t>child-v2t"' "$INDEX" \
    || { echo "FAIL: parent>child edge attribution missing on connector"; \
         grep -o 'data-edges="[^"]*"' "$INDEX" | head -5; \
         exit 1; }

# Both task ids must be wrapped as task-link spans with distinct status data,
# and the compact list must retain the matching badge classes.
python3 - "$INDEX" <<'PYEOF'
import re, sys
html = open(sys.argv[1]).read()
checks = [
    (r'data-task-id="parent-v2t"[^>]*data-status="open"', 'open graph task-link'),
    (r'data-task-id="live-v2t"[^>]*data-status="in-progress"', 'live graph task-link'),
    (r'class="badge open"', 'open task-list badge'),
    (r'class="badge in-progress"', 'live task-list badge'),
]
for pattern, label in checks:
    if not re.search(pattern, html):
        print(f'FAIL: generated HTML missing {label}')
        sys.exit(1)
PYEOF

# (3) TUI palette in CSS: live remains yellow/ochre while ordinary open
# resolves through --fg in default dark, OS-light, and explicit-light scopes.
CSS="$OUTDIR/style.css"
python3 - "$CSS" <<'PYEOF'
import sys
css = open(sys.argv[1]).read()
if css.count('--status-open: var(--fg);') != 3:
    print('FAIL: open must use var(--fg) in default, OS-light, and explicit-light palettes')
    sys.exit(1)
if '--status-open: rgb(' in css:
    print('FAIL: open regained a yellow/brown RGB color')
    sys.exit(1)
if '--status-in-progress: rgb(229, 229, 16);' not in css:
    print('FAIL: dark-theme live yellow is missing')
    sys.exit(1)
if css.count('--status-in-progress: rgb(180, 140, 0);') != 2:
    print('FAIL: OS-light and explicit-light live ochre declarations are missing')
    sys.exit(1)

# Both rendering surfaces consume the corrected variable.
open_link = '.viz-pre .task-link[data-status="open"]                  { color: var(--status-open); }'
open_badge = '.badge.open                  { color: var(--status-open); }'
for rule in (open_link, open_badge):
    if rule not in css:
        print(f'FAIL: corrected open variable is not consumed by {rule}')
        sys.exit(1)

# Modifier precedence is deliberate: chat then paused override base status;
# agency changes opacity only; selected relationships force their accent.
chat = '.viz-pre .task-link.chat-agent { color: var(--chat-task); }'
paused = '.viz-pre .task-link.paused {'
agency = '.viz-pre.viz-agency .task-link.is-agency {'
if not (css.index(open_link) < css.index(chat) < css.index(paused)):
    print('FAIL: chat/paused status override order changed')
    sys.exit(1)
agency_block = css[css.index(agency):css.index('}', css.index(agency))]
if 'color:' in agency_block or 'opacity: 0.6' not in agency_block:
    print('FAIL: agency modifier must dim without replacing status color')
    sys.exit(1)
for accent in ('--edge-upstream', '--edge-downstream'):
    if f'color: var({accent}) !important;' not in css:
        print(f'FAIL: {accent} no longer overrides status colors')
        sys.exit(1)
if 'body[data-selected] .viz-pre .task-link.is-selected' not in css:
    print('FAIL: selected-task overlay missing')
    sys.exit(1)
if '.badge.paused                { color: var(--status-paused); }' not in css:
    print('FAIL: paused badge override missing')
    sys.exit(1)
PYEOF

for needle in \
    'rgb(80, 220, 100)'   `# Done`           \
    'rgb(220, 60, 60)'    `# Failed`         \
    'rgb(60, 160, 220)'   `# Waiting`        \
    'rgb(140, 230, 80)'   `# PendingEval`    \
    'rgb(188, 63, 188)'   `# upstream edge (magenta)` \
    'rgb(17, 168, 205)'   `# downstream edge (cyan)`  \
; do
    grep -qF "$needle" "$CSS" \
        || { echo "FAIL: CSS missing TUI palette color '$needle'"; exit 1; }
done

# (4) Dark + light theme + override hooks.
grep -q '@media (prefers-color-scheme: light)' "$CSS" \
    || { echo "FAIL: prefers-color-scheme media query missing";   exit 1; }
grep -q '\[data-theme="light"\]' "$CSS" \
    || { echo "FAIL: manual light override missing";              exit 1; }
grep -q '\[data-theme="dark"\]'  "$CSS" \
    || { echo "FAIL: manual dark override missing";               exit 1; }

# Theme toggle wiring on the index page.
grep -q 'id="theme-toggle"' "$INDEX" \
    || { echo "FAIL: theme toggle button missing"; exit 1; }
grep -q "localStorage.getItem('wg-html-theme')" "$INDEX" \
    || { echo "FAIL: theme bootstrap missing — would flash on load"; exit 1; }

# Edge highlight CSS classes the JS layer applies to spans on selection.
for cls in 'is-upstream' 'is-downstream' 'is-cycle' 'is-selected'; do
    grep -q "$cls" "$CSS" \
        || { echo "FAIL: edge highlight class '.$cls' missing from CSS"; exit 1; }
done

# panel.js wires the click → highlight pipeline.
JS="$OUTDIR/panel.js"
grep -q 'applyHighlight' "$JS" \
    || { echo "FAIL: applyHighlight helper missing from panel.js"; exit 1; }
grep -q 'is-upstream'    "$JS" \
    || { echo "FAIL: panel.js doesn't apply is-upstream class";    exit 1; }
grep -q 'is-downstream'  "$JS" \
    || { echo "FAIL: panel.js doesn't apply is-downstream class";  exit 1; }
grep -q 'prefers-color-scheme' "$JS" \
    || { echo "FAIL: panel.js doesn't honor OS prefers-color-scheme"; exit 1; }

# (5) Static / rsync-friendly: panel.js MUST NOT XHR or fetch a backend.
if grep -qE '\b(fetch|XMLHttpRequest|axios)\b' "$JS"; then
    echo "FAIL: panel.js makes network calls — breaks the static / file:// contract"
    exit 1
fi

# Inline JSON blobs validate as JSON.
python3 - "$INDEX" <<'PYEOF'
import sys, json
content = open(sys.argv[1]).read()
markers = ['window.WG_TASKS = ', 'window.WG_EDGES = ', 'window.WG_CYCLES = ']
for marker in markers:
    pos = content.find(marker)
    if pos < 0:
        print(f'FAIL: {marker.strip()} not found')
        sys.exit(1)
    pos += len(marker)
    decoder = json.JSONDecoder()
    try:
        obj, _ = decoder.raw_decode(content, pos)
    except json.JSONDecodeError as e:
        print(f'FAIL: {marker.strip()} JSON invalid: {e}')
        sys.exit(1)

# Reachability JSON for the parent task should record child as a downstream
# consumer (the JS uses this to decide which edges to highlight cyan).
import re
m = re.search(r'window\.WG_EDGES = ({.*?});', content, re.DOTALL)
if not m:
    print('FAIL: WG_EDGES JSON not found via regex'); sys.exit(1)
edges = json.loads(m.group(1))
parent = edges.get('parent-v2t', {})
if 'child-v2t' not in (parent.get('down') or []):
    print(f"FAIL: WG_EDGES['parent-v2t'].down should include 'child-v2t', got: {parent}")
    sys.exit(1)
child = edges.get('child-v2t', {})
if 'parent-v2t' not in (child.get('up') or []):
    print(f"FAIL: WG_EDGES['child-v2t'].up should include 'parent-v2t', got: {child}")
    sys.exit(1)
print('JSON reachability checks passed')
PYEOF
[ $? -eq 0 ] || exit 1

echo "PASS: wg-html-v2 TUI-parity smoke (clickable viz + edge attribution + themed palette + static)"
