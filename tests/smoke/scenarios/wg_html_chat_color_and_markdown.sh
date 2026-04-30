#!/usr/bin/env bash
# Smoke: polish-wg-html — chat task blue color + markdown description rendering.
#
# Pins two features added in polish-wg-html:
#   1. Chat task nodes (.chat-N / .coordinator-N) in the viz HTML must carry
#      the 'chat-agent' CSS class so the --chat-task CSS variable (TUI Blue,
#      rgb(36,114,200)) overrides the yellow open-status color.
#   2. Task description JSON must include a description_html field containing
#      parsed markdown (not raw text).
#   3. panel.js must wire the desc-toggle button for raw/pretty switching.
#   4. CSS must declare the --chat-task custom property.
set -euo pipefail

WORK=$(mktemp -d)
OUTDIR=$(mktemp -d)
trap 'rm -rf "$WORK" "$OUTDIR"' EXIT

cd "$WORK"
wg --dir .workgraph init --route claude-cli 2>/dev/null \
    || wg --dir .workgraph init -m claude:opus 2>/dev/null \
    || true

# Regular task with markdown description.
wg --dir .workgraph add 'regular task' --id reg-smoke-html \
    -d '## Header

- item one
- item two

```rust
fn main() {}
```' >/dev/null

# Chat task — .chat-N prefix marks it as a chat agent.
# Inject a .chat-1 task directly into graph.jsonl (wg add doesn't create .chat-N ids).
cat >> .workgraph/graph.jsonl <<'JSON'
{"kind":"task","id":".chat-1","title":"Chat agent","status":"open","after":[],"tags":["chat-loop"],"visibility":"internal","created_at":"2026-01-01T00:00:00Z"}
JSON

# Render.
wg --dir .workgraph html --out "$OUTDIR" 2>&1

INDEX="$OUTDIR/index.html"
CSS="$OUTDIR/style.css"
JS="$OUTDIR/panel.js"

[ -f "$INDEX" ] || { echo "FAIL: index.html not created"; exit 1; }
[ -f "$CSS"   ] || { echo "FAIL: style.css missing"; exit 1; }
[ -f "$JS"    ] || { echo "FAIL: panel.js missing"; exit 1; }

# (1) Chat task node must carry chat-agent class in the viz HTML.
grep -q 'class="task-link chat-agent"' "$INDEX" \
    || { echo "FAIL: .chat-1 node must have class=\"task-link chat-agent\" in viz HTML"; \
         grep -o 'class="task-link[^"]*"' "$INDEX" | grep chat | head -3; \
         exit 1; }

# (2) CSS must declare the --chat-task variable with the TUI Blue value.
grep -qF 'rgb(36, 114, 200)' "$CSS" \
    || { echo "FAIL: --chat-task rgb(36,114,200) missing from CSS"; exit 1; }
grep -q 'chat-task' "$CSS" \
    || { echo "FAIL: --chat-task variable not declared in CSS"; exit 1; }
grep -q 'chat-agent' "$CSS" \
    || { echo "FAIL: .chat-agent CSS rule missing"; exit 1; }

# (3) description_html must appear in the WG_TASKS JSON blob.
python3 - "$INDEX" <<'PYEOF'
import sys, json, re

content = open(sys.argv[1]).read()
m = re.search(r'window\.WG_TASKS = ({.*?});', content, re.DOTALL)
if not m:
    print('FAIL: WG_TASKS JSON not found'); sys.exit(1)
tasks = json.loads(m.group(1))

# Find the task with a description.
target = tasks.get('reg-smoke-html')
if target is None:
    print(f'FAIL: reg-smoke-html not in WG_TASKS; keys={list(tasks.keys())}'); sys.exit(1)

desc_html = target.get('description_html')
if desc_html is None:
    print('FAIL: description_html field missing from task JSON'); sys.exit(1)

if '<h2>' not in desc_html:
    print(f'FAIL: description_html should contain <h2> (rendered heading); got: {desc_html[:200]}'); sys.exit(1)

if '<li>' not in desc_html:
    print(f'FAIL: description_html should contain <li> (rendered list); got: {desc_html[:200]}'); sys.exit(1)

if '## Header' in desc_html:
    print(f'FAIL: raw markdown leaked into description_html (## Header should be rendered); got: {desc_html[:200]}'); sys.exit(1)

print('description_html checks passed')
PYEOF
[ $? -eq 0 ] || exit 1

# (4) panel.js must wire the desc-toggle button.
grep -q 'desc-toggle\|DESC_VIEW_KEY\|wg-html-desc-view' "$JS" \
    || { echo "FAIL: panel.js missing desc-toggle wiring"; exit 1; }

echo "PASS: wg-html-v2 polish — chat blue + markdown description rendering"
