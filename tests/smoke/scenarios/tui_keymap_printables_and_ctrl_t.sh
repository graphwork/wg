#!/usr/bin/env bash
# Scenario: tui_keymap_printables_and_ctrl_t (bug-tui-keymap-fix)
#
# Drives a real `wg tui` in tmux with a custom-command chat whose child process
# records raw stdin bytes. This pins the user-visible keymap contract:
#   * bare printables typed while the chat PTY has focus reach the child
#     (`+` must not open the add-chat launcher),
#   * Ctrl+T reaches the child (reserved for executor thinking-toggle),
#   * Ctrl+O is the host command-mode toggle and is symmetric.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a live TUI"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the stdin recorder"
fi

scratch=$(make_scratch)
session="wgsmoke-keymap-$$"

kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    local project_tag
    project_tag=$(basename "$scratch" | tr '.:' '--')
    tmux ls 2>/dev/null \
        | awk -F: -v tag="wg-chat-${project_tag}-" '$1 ~ "^"tag {print $1}' \
        | while read -r s; do
            tmux kill-session -t "$s" 2>/dev/null || true
        done
}
add_cleanup_hook kill_tmux_session

cd "$scratch"

if ! wg init --no-agency >init.log 2>&1; then
    loud_fail "wg init --no-agency failed: $(tail -5 init.log)"
fi
graph_dir=$(graph_dir_in "$scratch") || loud_fail "no .wg dir after wg init"

key_log="$scratch/child-keys.bin"
recorder="$scratch/stdin_recorder.py"
cat >"$recorder" <<'PY'
import os
import sys
import tty

log_path = sys.argv[1]
fd = sys.stdin.fileno()
tty.setraw(fd)
print("WG_KEYMAP_RECORDER_READY", flush=True)
while True:
    try:
        data = os.read(fd, 1)
    except OSError:
        break
    if not data:
        break
    with open(log_path, "ab") as f:
        f.write(data)
        f.flush()
    try:
        os.write(sys.stdout.fileno(), b".")
    except OSError:
        break
PY

cmd="python3 -u '$recorder' '$key_log'"
out=$(wg chat new --name keymap --command "$cmd" --json 2>&1) \
    || loud_fail "wg chat new --command recorder failed: $out"

tui_dump_json() {
    wg --json tui-dump 2>/dev/null
}

tui_field() {
    local field="$1"
    tui_dump_json | python3 -c "
import json, sys
try: print(json.load(sys.stdin).get('$field', ''))
except Exception: pass
" 2>/dev/null
}

tui_text()       { tui_field text; }
tui_input_mode() { tui_field input_mode; }
tui_focused()    { tui_field focused_panel; }

focused_is_pty() {
    case "$(tui_focused)" in
        RightPanel|right_panel|panel) return 0 ;;
        *) return 1 ;;
    esac
}

focused_is_graph() {
    case "$(tui_focused)" in
        Graph|graph) return 0 ;;
        *) return 1 ;;
    esac
}

wait_for_field() {
    local field="$1" want="$2" iters="${3:-40}"
    for _ in $(seq 1 "$iters"); do
        if [[ "$(tui_field "$field")" == "$want" ]]; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_text() {
    local needle="$1" iters="${2:-60}"
    for _ in $(seq 1 "$iters"); do
        if printf '%s' "$(tui_text)" | grep -qF "$needle"; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_key_hex() {
    local expected="$1" iters="${2:-40}"
    for _ in $(seq 1 "$iters"); do
        local got=""
        if [[ -f "$key_log" ]]; then
            got=$(python3 - "$key_log" <<'PY'
import pathlib
import sys
print(pathlib.Path(sys.argv[1]).read_bytes().hex())
PY
)
        fi
        if [[ "$got" == "$expected" ]]; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

tmux new-session -d -s "$session" -x 160 -y 45 "wg tui"

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "wg tui did not create tui.sock within 15s"
fi
if ! wait_for_field input_mode Normal 40; then
    loud_fail "TUI never reached Normal mode (input_mode=$(tui_input_mode))"
fi
if ! wait_for_text "WG_KEYMAP_RECORDER_READY" 80; then
    loud_fail "custom command PTY did not render recorder readiness. text head: $(tui_text | head -c 500)"
fi

# The custom command chat should start in PTY focus. If a prior persisted state
# put it in command mode, Ctrl+O returns focus to the child.
if ! focused_is_pty; then
    tmux send-keys -t "$session" "C-o"
    sleep 0.5
fi
for _ in $(seq 1 20); do
    if focused_is_pty; then
        break
    fi
    sleep 0.25
done
if ! focused_is_pty; then
    loud_fail "chat PTY never gained focus (focused_panel=$(tui_focused), text head: $(tui_text | head -c 300))"
fi

# Regression target: bare '+' and another printable are text for the child, and
# Ctrl+T is a reserved passthrough chord. Pre-fix, '+' opened the launcher and
# Ctrl+T toggled command mode, so the raw child bytes were missing.
tmux send-keys -t "$session" -l "+"
sleep 0.2
tmux send-keys -t "$session" -l "a"
sleep 0.2
tmux send-keys -t "$session" "C-t"

if ! wait_for_key_hex "2b6114" 40; then
    got="<missing>"
    [[ -f "$key_log" ]] && got=$(python3 - "$key_log" <<'PY'
import pathlib
import sys
print(pathlib.Path(sys.argv[1]).read_bytes().hex())
PY
)
    loud_fail "expected child stdin bytes +, a, Ctrl+T (hex 2b6114), got ${got}; '+' or Ctrl+T was intercepted by the host or not forwarded. input_mode=$(tui_input_mode), focused_panel=$(tui_focused), screen head: $(tui_text | head -c 500)"
fi
if [[ "$(tui_input_mode)" != "Normal" ]]; then
    loud_fail "bare printable opened a host modal; input_mode=$(tui_input_mode)"
fi
if ! focused_is_pty; then
    loud_fail "Ctrl+T toggled host focus instead of passing through; focused_panel=$(tui_focused)"
fi
echo "phase 1: +, a, and Ctrl+T reached the focused child"

tmux send-keys -t "$session" "C-o"
for _ in $(seq 1 20); do
    if focused_is_graph; then
        break
    fi
    sleep 0.25
done
if ! focused_is_graph; then
    loud_fail "Ctrl+O did not enter command mode (focused_panel=$(tui_focused), text head: $(tui_text | head -c 300))"
fi
tmux send-keys -t "$session" "C-o"
for _ in $(seq 1 20); do
    if focused_is_pty; then
        break
    fi
    sleep 0.25
done
if ! focused_is_pty; then
    loud_fail "Ctrl+O did not return to chat PTY focus (focused_panel=$(tui_focused))"
fi

echo "phase 2: Ctrl+O toggles PTY focus / command mode"
echo "=== tui_keymap_printables_and_ctrl_t: PASS ==="
