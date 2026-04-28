#!/usr/bin/env bash
# Smoke: wg html --all emits ASCII viz with clickable task IDs + side panel.
set -euo pipefail

OUTDIR=$(mktemp -d)
trap 'rm -rf "$OUTDIR"' EXIT

# Generate HTML site
wg html --all --out "$OUTDIR" 2>&1

INDEX="$OUTDIR/index.html"

[ -f "$INDEX" ] || { echo "FAIL: index.html not created"; exit 1; }

# ASCII viz element must be present (not SVG)
grep -q 'class="viz-pre"' "$INDEX" || { echo "FAIL: viz-pre element missing"; exit 1; }

# At least one clickable task-link span
grep -q 'class="task-link"' "$INDEX" || { echo "FAIL: no clickable task-link spans"; exit 1; }

# Side panel element
grep -q 'id="side-panel"' "$INDEX" || { echo "FAIL: side-panel missing"; exit 1; }

# Inline task JSON
grep -q 'window\.WG_TASKS' "$INDEX" || { echo "FAIL: WG_TASKS JSON missing"; exit 1; }

# JS panel function
grep -q 'openPanel' "$INDEX" || { echo "FAIL: openPanel JS missing"; exit 1; }

# ASCII content should match wg viz output — check that at least one task id
# from wg viz appears as a task-link in the index
FIRST_TASK=$(wg viz --all --no-tui --columns 120 2>/dev/null | head -1 | awk '{print $1}')
if [ -n "$FIRST_TASK" ]; then
    grep -q "data-task-id=\"$FIRST_TASK\"" "$INDEX" \
        || { echo "FAIL: task id '$FIRST_TASK' from viz not clickable in HTML"; exit 1; }
fi

# No raw </script> that could break the script block
# (we escape these in the JSON)
python3 - "$INDEX" <<'PYEOF'
import sys, json
content = open(sys.argv[1]).read()
# Find start of JSON object after "window.WG_TASKS = "
marker = 'window.WG_TASKS = '
pos = content.find(marker)
if pos < 0:
    print('FAIL: WG_TASKS assignment not found')
    sys.exit(1)
pos += len(marker)
# Use a JSON decoder to read exactly the JSON object
decoder = json.JSONDecoder()
try:
    obj, _ = decoder.raw_decode(content, pos)
except json.JSONDecodeError as e:
    print(f'FAIL: WG_TASKS JSON invalid: {e}')
    sys.exit(1)
print(f'JSON valid, tasks: {len(obj)}')
PYEOF
[ $? -eq 0 ] || exit 1

echo "PASS: wg html --all ASCII viz smoke test"
