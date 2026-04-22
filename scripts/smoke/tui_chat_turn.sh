#!/usr/bin/env bash
# End-to-end UX smoke: `wg tui` Chat tab drives a real PTY-embedded
# `wg nex` against a fake oai-compat LLM server, types two turns of
# dialogue, and asserts the canned responses render in the Chat pane.
#
# Tests the full UX stack: auto-PTY toggle → wg nex REPL rendering →
# key forwarding → HTTP request → SSE response → TUI render pipeline.
#
# Runs in CI. Exits 0 on pass, 1 on fail with diagnostics, 77 if a
# prerequisite (tmux / python3) is missing (automake "skipped" code).

set -u

POLL_DEADLINE=${POLL_DEADLINE:-10}

# Prereq check — skip (not fail) when the env can't run the test.
need_tools=(tmux python3)
for t in "${need_tools[@]}"; do
    if ! command -v "$t" >/dev/null; then
        echo "SKIP: $t not available"
        exit 77
    fi
done

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FAKE_SERVER="$REPO_ROOT/scripts/testing/fake_llm_server.py"
if [[ ! -f "$FAKE_SERVER" ]]; then
    echo "FAIL: fake_llm_server.py missing at $FAKE_SERVER"
    exit 1
fi

# Random high port to reduce collision with local dev.
PORT=$(python3 -c 'import socket,sys; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')

TMPHOME=$(mktemp -d)
SESSION=wg-smoke-chat-turn-$$
READY="$TMPHOME/fake.ready"
FAKE_PID=
cleanup() {
    tmux kill-session -t "$SESSION" 2>/dev/null
    # Stop the per-smoke daemon if we started one.
    if [[ -S "$TMPHOME/.wg/service/daemon.sock" ]]; then
        (cd "$TMPHOME" && wg service stop --kill-agents >/dev/null 2>&1) || true
    fi
    [[ -n "$FAKE_PID" ]] && kill "$FAKE_PID" 2>/dev/null
    wait 2>/dev/null
    cd /
    rm -rf "$TMPHOME"
}
trap cleanup EXIT

# Canned two-turn script: the fake serves these in order.
cat > "$TMPHOME/responses.txt" <<'EOF'
Hello traveler! What brings you here?

Glad to help — what else would you like to know?
EOF

# Start fake server, wait for it to signal ready.
python3 "$FAKE_SERVER" \
    --port "$PORT" \
    --responses "$TMPHOME/responses.txt" \
    --ready-file "$READY" \
    >"$TMPHOME/fake.stdout" 2>"$TMPHOME/fake.stderr" &
FAKE_PID=$!

for i in $(seq 1 20); do
    [[ -f "$READY" ]] && break
    sleep 0.2
done
if [[ ! -f "$READY" ]]; then
    echo "FAIL: fake server did not become ready"
    cat "$TMPHOME/fake.stderr"
    exit 1
fi

# Init a fresh workgraph pointing at the fake endpoint.
cd "$TMPHOME"
wg init --no-agency -m local:fake-model -e "http://127.0.0.1:$PORT" >/dev/null 2>&1

# Register a coordinator-1 session alias + the graph task so auto-PTY
# has something to spawn into.
python3 - <<PY
import json, pathlib
wg = pathlib.Path.cwd() / ".wg"
sess = wg / "chat" / "sessions.json"
sess.parent.mkdir(parents=True, exist_ok=True)
uuid = "019db700-0000-7000-8000-000000000042"
sess.write_text(json.dumps({
    "version": 0,
    "sessions": {uuid: {
        "kind": "coordinator",
        "created": "2026-04-22T21:00:00Z",
        "aliases": ["coordinator-1", "1"],
        "label": "test",
    }}
}))
(wg / "chat" / uuid).mkdir(parents=True, exist_ok=True)
PY
wg add ".coordinator-1" --id .coordinator-1 --tag coordinator-loop >/dev/null 2>&1

# `wg chat` (which the TUI calls to submit a message) IPCs the service
# daemon; it writes inbox.jsonl via that relay. Without a daemon, submit
# fails silently. Start a no-coordinator-agent daemon so the inbox path
# works but the daemon doesn't spawn its own coordinator subprocess
# that would fight our PTY-spawned nex for the session lock.
wg service start --no-coordinator-agent >/dev/null 2>&1 || true
# Wait up to 3s for the daemon socket to be ready.
for i in 1 2 3 4 5 6; do
    [[ -S "$TMPHOME/.wg/service/daemon.sock" ]] && break
    sleep 0.5
done

# Launch wg tui in a detached tmux session.
tmux kill-session -t "$SESSION" 2>/dev/null
tmux new-session -d -s "$SESSION" -x 180 -y 40 \
    "cd '$TMPHOME' && wg tui 2>$TMPHOME/tui.err"

# ---- Assertion 1: wg nex banner appears in the Chat pane ----
wait_for() {
    local needle="$1"
    for i in $(seq 1 "$POLL_DEADLINE"); do
        sleep 1
        if tmux capture-pane -t "$SESSION" -p 2>/dev/null | grep -qF -- "$needle"; then
            return 0
        fi
    done
    return 1
}

if ! wait_for "wg nex — interactive session"; then
    echo "FAIL: wg nex did not appear in Chat pane"
    echo "-- screen --"
    tmux capture-pane -t "$SESSION" -p 2>/dev/null | head -30
    echo "-- tui.err --"
    head -40 "$TMPHOME/tui.err"
    exit 1
fi

# Key sequence per turn (right-panel focused after auto-PTY):
#   1. Enter → activates `InputMode::ChatInput` on the Chat tab
#      (see `KeyCode::Enter` branch in `handle_right_panel_key`).
#   2. Type the message into the composer editor.
#   3. Enter → submits → `send_chat_message` writes to inbox.jsonl
#      → `wg nex --chat` picks it up → hits fake server → SSE
#      response → written to outbox.jsonl → rendered inside PTY.
# Direct key-forwarding to the PTY is a no-op: wg nex --chat reads
# the session inbox, not stdin (see handle_right_panel_key comment).

# ---- Turn 1: send "hi there", expect canned response 1 ----
tmux send-keys -t "$SESSION" Enter    # activate chat input
sleep 0.3
tmux send-keys -t "$SESSION" "hi there"
sleep 0.3
tmux send-keys -t "$SESSION" Enter    # submit

if ! wait_for "Hello traveler"; then
    echo "FAIL: turn 1 response not rendered"
    echo "-- screen --"
    tmux capture-pane -t "$SESSION" -p 2>/dev/null | head -30
    echo "-- fake.stderr --"
    cat "$TMPHOME/fake.stderr"
    echo "-- fake.stdout --"
    cat "$TMPHOME/fake.stdout"
    exit 1
fi

# ---- Turn 2: follow-up, expect canned response 2 ----
tmux send-keys -t "$SESSION" Enter
sleep 0.3
tmux send-keys -t "$SESSION" "tell me more"
sleep 0.3
tmux send-keys -t "$SESSION" Enter

if ! wait_for "Glad to help"; then
    echo "FAIL: turn 2 response not rendered"
    echo "-- screen --"
    tmux capture-pane -t "$SESSION" -p 2>/dev/null | head -30
    exit 1
fi

# ---- Final sanity: live wg nex child still present ----
if ! pgrep -f "wg nex --chat coordinator-1" >/dev/null; then
    echo "FAIL: wg nex child missing after two turns"
    exit 1
fi

echo "PASS: two-turn dialogue round-tripped through TUI + PTY + fake LLM"
