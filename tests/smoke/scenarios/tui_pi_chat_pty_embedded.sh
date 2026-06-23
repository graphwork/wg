#!/usr/bin/env bash
# Scenario: tui_pi_chat_pty_embedded
#
# Pins fix-tui-pi: creating a Pi chat from the TUI launcher MUST attach the
# pane to a live PTY running the interactive `pi` binary, NOT route through
# `wg pi-handler` / `pi --mode rpc` on pipes (which leaves the pane stuck on
# "Booting agent...").
#
# Human-flow simulation: a fake `pi` binary is placed on PATH that prints a
# unique marker line to its PTY stdout and then idles. We launch `wg tui` in
# tmux, create a Pi chat through the launcher, and assert:
#   1. The fake `pi` marker appears in the tmux pane (live PTY output), and
#   2. No `pi --mode rpc` process was spawned for the visible chat pane.
#
# The previous regression (`.chat-39`) spawned `wg pi-handler --chat chat-N
# -m openrouter:...` which forked `pi --mode rpc` on pipes; the pane never
# showed child output. This scenario catches that by requiring the fake
# marker — pipe-stdio RPC would never reach the TUI pane.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-pi-pty-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session
cd "$scratch"

# ── Fake `pi` binary ──────────────────────────────────────────────────
# Emits a unique marker to stdout (the PTY), then idles reading stdin so the
# pane stays alive. It must NOT take a `--mode rpc` branch — if it sees
# `--mode rpc` on its argv it writes a sentinel to a side file so the test
# can detect the forbidden RPC routing.
fakedir="$scratch/fakebin"
mkdir -p "$fakedir"
marker="FAKE_PI_PTY_OK_$$"
rpc_sentinel="$scratch/pi_mode_rpc_spawned"
cat >"$fakedir/pi" <<PIEOF
#!/usr/bin/env bash
# fake pi for tui_pi_chat_pty_embedded smoke
echo "$marker"
# If invoked with --mode rpc, record that the forbidden path was taken.
for a in "\$@"; do
    if [[ "\$a" == "rpc" ]]; then
        echo "rpc" >"$rpc_sentinel"
    fi
done
# Idle so the pane stays alive (interactive mode waits for stdin).
while IFS= read -r line; do :; done
PIEOF
chmod +x "$fakedir/pi"

# Put the fake pi FIRST on PATH so the TUI's spawned `pi` resolves to it.
export PATH="$fakedir:$PATH"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

# Create a Pi chat with an explicit OpenRouter model (the explicit-model
# contract — pi_model_arg must resolve a provider/model pair).
if ! wg chat new --name piroom --executor pi --model "openrouter:z-ai/glm-5.2" >chat0.log 2>&1; then
    loud_fail "create pi chat failed: $(cat chat0.log)"
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

# Give the TUI time to auto-enable the chat PTY pane (maybe_auto_enable_chat_pty
# runs on focus / chat-tab render). The Pi chat is the first/only chat so it is
# active by default.
saw_marker=""
for _ in $(seq 1 40); do
    capture=$(tmux capture-pane -t "$session" -p 2>/dev/null || true)
    if echo "$capture" | grep -qF "$marker"; then
        saw_marker="$capture"
        break
    fi
    # Nudge focus into the right panel in case the graph panel owns keys.
    tmux send-keys -t "$session" Right 2>/dev/null || true
    sleep 0.25
done

# ── Assertion 1: live PTY output (the fake-pi marker) reached the pane ──
if [[ -z "$saw_marker" ]]; then
    capture=$(tmux capture-pane -t "$session" -p 2>/dev/null || true)
    loud_fail "TUI Pi chat pane did not show fake-pi PTY marker '$marker' within 10s. \
This means the pane was NOT attached to a live PTY running interactive pi — \
the 'Booting agent...' regression. Pane content:
$capture"
fi

# ── Assertion 2: no `pi --mode rpc` was spawned for the visible pane ────
# The fake pi writes the rpc sentinel ONLY if `--mode rpc` was on its argv.
# A small grace window in case a stray worker (non-TUI) is also running.
sleep 1
if [[ -f "$rpc_sentinel" ]]; then
    loud_fail "TUI Pi chat pane spawned pi with --mode rpc (forbidden). \
The visible chat pane must use the embedded PTY path, not the detached RPC \
worker. rpc_sentinel=$rpc_sentinel present."
fi

echo "FAKE_PI_MARKER_SEEN: yes"
echo "PI_MODE_RPC_SPAWNED: no"
echo "PASS: TUI Pi chat pane attached to live PTY (fake-pi marker visible), no --mode rpc"
exit 0
