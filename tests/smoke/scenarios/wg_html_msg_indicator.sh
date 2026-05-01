#!/usr/bin/env bash
# Smoke: wg-html-surface — message indicator parity with TUI.
#
# Pins the regressions that wg-html-surface closed:
#   1. Tasks with messages get a `.msg-indicator` span next to the task id in
#      the index task list. Tasks without messages do NOT.
#   2. The indicator carries `data-msg-action="messages"` so panel.js scrolls
#      the inspector to the Messages section on click.
#   3. The per-task page renders a `<section id="messages-section">` with the
#      messages, escaped against XSS.
#   4. The inline tasks JSON exposes a `msg` field for messaged tasks (empty
#      bundles are omitted), so the inspector can render its Messages section.
#   5. CSS provides theme-aware colors for the indicator (msg-unseen / msg-seen
#      / msg-replied) — same status palette as the TUI envelope glyph.
#   6. panel.js gained `renderMessagesPanelHtml` + `scrollPanelToMessages`
#      helpers AND a click handler for `data-msg-action="messages"` on the
#      body delegation listener.
#   7. No JS framework was added — panel.js is still served as a single
#      vanilla file with no fetch / no module imports.
set -euo pipefail

WORK=$(mktemp -d)
OUTDIR=$(mktemp -d)
trap 'rm -rf "$WORK" "$OUTDIR"' EXIT

cd "$WORK"
# `init --route claude-cli` is the offline-friendly path used by sibling html
# scenarios — it never reaches the network.
wg --dir .wg init --route claude-cli >/dev/null 2>&1 || true

wg --dir .wg add 'task with msg'   --id msg-task-a -d 'a' >/dev/null
wg --dir .wg add 'task without msg' --id msg-task-b -d 'b' >/dev/null
wg --dir .wg add 'task with reply' --id msg-task-c -d 'c' >/dev/null

# Auto-detect logic in `wg msg send` rewrites `--from user` to `$WG_TASK_ID`
# when present — strip it so the senders end up where we want.
unset WG_TASK_ID

wg --dir .wg msg send msg-task-a 'hello there'   >/dev/null
wg --dir .wg msg send msg-task-a 'second one'    >/dev/null
wg --dir .wg msg send msg-task-c 'inbound'         --from agent-x      >/dev/null
wg --dir .wg msg send msg-task-c 'outbound reply'  --from coordinator  >/dev/null

wg --dir .wg html --out "$OUTDIR" --all >/dev/null 2>&1

INDEX="$OUTDIR/index.html"
CSS="$OUTDIR/style.css"
JS="$OUTDIR/panel.js"
TASK_A="$OUTDIR/tasks/msg-task-a.html"
TASK_B="$OUTDIR/tasks/msg-task-b.html"
TASK_C="$OUTDIR/tasks/msg-task-c.html"

for f in "$INDEX" "$CSS" "$JS" "$TASK_A" "$TASK_B" "$TASK_C"; do
    [ -f "$f" ] || { echo "FAIL: $f not created"; exit 1; }
done

# (1) Indicator on rows with messages, NOT on rows without.
grep -q 'data-task-id="msg-task-a"[^>]*>.*msg-indicator' "$INDEX" \
    || grep -qE 'msg-indicator[^"]*"[^>]*data-task-id="msg-task-a"' "$INDEX" \
    || { echo "FAIL: msg-task-a is missing the .msg-indicator span"; exit 1; }
grep -q 'msg-indicator[^"]*" data-task-id="msg-task-c"' "$INDEX" \
    || { echo "FAIL: msg-task-c is missing the .msg-indicator span"; exit 1; }
if grep -q 'msg-indicator[^"]*" data-task-id="msg-task-b"' "$INDEX"; then
    echo "FAIL: msg-task-b (no messages) should NOT carry an indicator"
    exit 1
fi

# Status-class breakdown: A is coordinator-only (status=none), C has agent
# inbound + coordinator outbound (status=replied). Verify the glyph + class.
grep -q 'msg-indicator msg-none[^"]*"[^>]*data-task-id="msg-task-a"' "$INDEX" \
    || { echo "FAIL: msg-task-a should have msg-none status class"; exit 1; }
grep -q 'msg-indicator msg-replied[^"]*"[^>]*data-task-id="msg-task-c"' "$INDEX" \
    || { echo "FAIL: msg-task-c should have msg-replied status class"; exit 1; }

# (2) Indicator click action attribute is present.
grep -q 'data-msg-action="messages"' "$INDEX" \
    || { echo "FAIL: indicator missing data-msg-action attribute"; exit 1; }

# (3) Per-task page renders the Messages section, escapes content.
grep -q 'id="messages-section"' "$TASK_A" \
    || { echo "FAIL: msg-task-a per-task page missing #messages-section"; exit 1; }
grep -q 'hello there' "$TASK_A" \
    || { echo "FAIL: msg-task-a body 'hello there' missing from page"; exit 1; }
if grep -q 'id="messages-section"' "$TASK_B"; then
    echo "FAIL: msg-task-b (no messages) should NOT have a Messages section"
    exit 1
fi
grep -q 'msg-incoming' "$TASK_C" \
    || { echo "FAIL: msg-task-c page missing .msg-incoming row"; exit 1; }
grep -q 'msg-outgoing' "$TASK_C" \
    || { echo "FAIL: msg-task-c page missing .msg-outgoing row"; exit 1; }

# (4) Inline tasks JSON exposes the msg bundle for messaged tasks only.
# Use python rather than escape-fragile grep — the inline JSON is a single
# long object literal and balanced-brace scanning in shell is fiddly.
python3 - "$INDEX" <<'PY' || { echo "FAIL: WG_TASKS JSON shape check"; exit 1; }
import json, re, sys
src = open(sys.argv[1]).read()
m = re.search(r'window\.WG_TASKS\s*=\s*(\{.*?\});</script>', src)
if not m:
    print("FAIL: WG_TASKS literal not found in index.html"); sys.exit(1)
tasks = json.loads(m.group(1))
if "msg" not in tasks.get("msg-task-a", {}):
    print("FAIL: msg-task-a missing msg field in WG_TASKS"); sys.exit(1)
if "msg" not in tasks.get("msg-task-c", {}):
    print("FAIL: msg-task-c missing msg field in WG_TASKS"); sys.exit(1)
if "msg" in tasks.get("msg-task-b", {}):
    print("FAIL: msg-task-b unexpectedly carries a msg field"); sys.exit(1)
m_a = tasks["msg-task-a"]["msg"]
m_c = tasks["msg-task-c"]["msg"]
if m_a.get("status") != "none":
    print("FAIL: msg-task-a status should be 'none', got", m_a.get("status")); sys.exit(1)
if m_c.get("status") != "replied":
    print("FAIL: msg-task-c status should be 'replied', got", m_c.get("status")); sys.exit(1)
if not m_a.get("messages") or len(m_a["messages"]) != 2:
    print("FAIL: msg-task-a should have 2 messages, got", len(m_a.get("messages") or [])); sys.exit(1)
print("OK: WG_TASKS msg bundle shape")
PY

# (5) Theme-aware indicator CSS — status classes + the unread-row highlight.
grep -q '\.msg-indicator' "$CSS" \
    || { echo "FAIL: .msg-indicator CSS rule missing"; exit 1; }
grep -q '\.msg-indicator\.msg-unseen' "$CSS" \
    || { echo "FAIL: .msg-unseen rule missing"; exit 1; }
grep -q '\.msg-indicator\.msg-seen' "$CSS" \
    || { echo "FAIL: .msg-seen rule missing"; exit 1; }
grep -q '\.msg-indicator\.msg-replied' "$CSS" \
    || { echo "FAIL: .msg-replied rule missing"; exit 1; }
grep -q 'has-unread-msg' "$CSS" \
    || { echo "FAIL: .has-unread-msg styling missing"; exit 1; }
grep -q '\.messages-section' "$CSS" \
    || { echo "FAIL: .messages-section CSS rule missing"; exit 1; }

# (6) panel.js wires the inspector Messages section + click handler.
grep -q 'renderMessagesPanelHtml' "$JS" \
    || { echo "FAIL: panel.js missing renderMessagesPanelHtml helper"; exit 1; }
grep -q 'scrollPanelToMessages' "$JS" \
    || { echo "FAIL: panel.js missing scrollPanelToMessages helper"; exit 1; }
grep -q "msgAction" "$JS" \
    || { echo "FAIL: panel.js missing the data-msg-action click handler"; exit 1; }

# (7) Still vanilla — no module imports, no fetch, no framework tags.
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

echo "PASS: wg_html_msg_indicator"
