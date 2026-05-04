#!/usr/bin/env bash
# Scenario: tui_chat_ctrl_c_interrupts_hung_subprocess
#
# Drives the real TUI chat-tab PTY flow. A custom bash chat starts a hanging
# foreground subprocess, the user enters chat scroll mode with Ctrl+], then
# presses Ctrl+C. Ctrl+C must still interrupt the foreground subprocess and
# return control to the chat shell so the next command can run.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-chat-ctrl-c-$$"
project_tag="$(basename "$scratch" | tr '.:' '--')"
chat_session="wg-chat-${project_tag}-chat-0"

kill_tmux_sessions() {
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux kill-session -t "$chat_session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_sessions

cd "$scratch"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

if ! wg chat new --name shell --command "bash --noprofile --norc -i" >chat.log 2>&1; then
    loud_fail "create bash chat failed: $(cat chat.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

tmux new-session -d -s "$session" -x 180 -y 50 "wg tui"
for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "wg tui did not create tui.sock within 15s"
fi

for _ in $(seq 1 30); do
    if tmux has-session -t "$chat_session" 2>/dev/null; then
        break
    fi
    sleep 0.5
done
if ! tmux has-session -t "$chat_session" 2>/dev/null; then
    loud_fail "chat tmux session '$chat_session' did not appear"
fi

marker="$scratch/ctrl-c-started-$$"
tmux send-keys -t "$session" "sh -c 'echo started > \"$marker\"; sleep 999'" Enter

for _ in $(seq 1 20); do
    if [[ -f "$marker" ]]; then
        break
    fi
    sleep 0.25
done
if [[ ! -f "$marker" ]]; then
    loud_fail "hanging subprocess did not start in chat pane"
fi

# Enter the host TUI's chat scroll mode, then interrupt. This pins the
# user-visible failure mode where Ctrl+C was swallowed by the wrapper instead
# of reaching the focused chat PTY.
tmux send-keys -t "$session" "C-]"
sleep 0.2
tmux send-keys -t "$session" "C-c"
sleep 0.5
tmux send-keys -t "$session" "echo WG_INTERRUPT_OK" Enter

ok=""
for _ in $(seq 1 30); do
    pane="$(tmux capture-pane -t "$chat_session" -p 2>/dev/null || true)"
    if grep -q "WG_INTERRUPT_OK" <<<"$pane"; then
        ok=1
        break
    fi
    sleep 0.25
done

if [[ -z "$ok" ]]; then
    loud_fail "Ctrl+C did not return control to bash chat after hung subprocess.
Inner chat pane:
$(tmux capture-pane -t "$chat_session" -p 2>/dev/null | tail -40)
Outer TUI pane:
$(tmux capture-pane -t "$session" -p 2>/dev/null | tail -40)"
fi

echo "PASS: Ctrl+C from TUI chat scroll mode interrupts hung foreground subprocess and bash remains usable"
exit 0
