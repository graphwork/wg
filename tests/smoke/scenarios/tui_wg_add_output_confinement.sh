#!/usr/bin/env bash
# Real tmux/PTY regression: a live chat runs several `wg add` commands while
# graph snapshots, resize/SIGWINCH and mosh-compatible input policy are active.
# Command output must remain in the embedded chat PTY; only ratatui may write it
# to the outer terminal, at cells inside the chat panel.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the physical TUI/PTY assertion"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required to inspect PTY bytes and screen cells"

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"

cat >chat-driver.sh <<'DRIVER'
#!/usr/bin/env bash
set -u
printf 'WG_ADD_OWNER_READY pid=%s fd1=%s fd2=%s\n' \
    "$$" "$(readlink "/proc/$$/fd/1")" "$(readlink "/proc/$$/fd/2")"
while IFS= read -r line; do
    [[ "$line" == "add-now" ]] || continue
    wg add 'Bind deterministic E97 shards' --no-place
    sleep 0.06
    wg add 'Implement leased peer lifecycle' --no-place
    sleep 0.06
    wg add 'Prove READY membership' --no-place
    sleep 0.06
    wg add 'Join exact weighted reducer' --no-place
    printf 'WG_ADD_OWNER_DONE\n'
done
DRIVER
chmod +x chat-driver.sh

env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg chat create --name add-owner --command "bash $scratch/chat-driver.sh" \
    >chat-create.log 2>&1 \
    || loud_fail "custom chat creation failed: $(tail -20 chat-create.log)"
printf '%s\n' \
    '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}' \
    >.wg/tui-state.json

outer="wgsmoke-add-confinement-$$"
raw="$scratch/outer.raw"
cleanup_sessions() {
    tmux kill-session -t "$outer" 2>/dev/null || true
    # Canonical path-unique sessions are scoped to this scratch; ask tmux by
    # WG_DIR rather than guessing the digest-bearing name.
    while IFS= read -r session; do
        [[ -n "$session" ]] && tmux kill-session -t "$session" 2>/dev/null || true
    done < <(tmux list-panes -a -F '#{session_name} #{pane_start_command}' 2>/dev/null \
        | awk -v p="$scratch" 'index($0,p){print $1}' | sort -u)
}
add_cleanup_hook cleanup_sessions
capture() { tmux capture-pane -p -t "$outer" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 400); do
        screen=$(capture)
        printf '%s\n' "$screen" | grep -Fq "$needle" && return 0
        sleep 0.025
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}

# MOSH_CONNECTION selects the conservative keyboard negotiation path. The chat
# still uses a real nested tmux PTY; the outer pane is the physical ratatui
# terminal whose bytes are recorded by pipe-pane.
tmux new-session -d -s "$outer" -x 200 -y 50 \
    "cd '$scratch' && env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER MOSH_CONNECTION=smoke wg tui"
# An agent/smoke can itself run inside a small attached tmux client whose
# `window-size latest` policy overrides new-session's requested geometry.
# Reassert the test pane size before waiting for the first chat render.
tmux resize-window -t "$outer" -x 200 -y 50
tmux pipe-pane -O -t "$outer" "cat >> '$raw'"
wait_screen 'WG_ADD_OWNER_READY' 'custom chat PTY did not become visible'

outer_pid=$(tmux display-message -p -t "$outer" '#{pane_pid}')
outer_tty=$(readlink "/proc/$outer_pid/fd/1")

# Make the best-effort tab-state write fail. Historically this emitted an
# inherited eprintln while ratatui owned the alternate screen, which moved the
# physical cursor and made the next differential graph/chat update paint into
# the wrong panel. The repaired path remains silent.
rm -f .wg/tui-state.json
mkdir .wg/tui-state.json

# Drive the owning chat exactly like a user prompt. Resize down and back while
# four graph mutations arrive in quick succession, exercising SIGWINCH plus the
# asynchronous snapshot lane without blocking input.
tmux send-keys -t "$outer" -l add-now
tmux send-keys -t "$outer" Enter
sleep 0.05
tmux resize-window -t "$outer" -x 142 -y 38
sleep 0.08
tmux resize-window -t "$outer" -x 200 -y 50
wait_screen 'WG_ADD_OWNER_DONE' 'chat did not finish its wg add burst'
coherent=0
for _ in $(seq 1 400); do
    capture >screen-with-chat.txt
    if python3 - screen-with-chat.txt 2>/dev/null <<'PY'
import sys
screen = open(sys.argv[1], encoding="utf-8").read()
ids = [
    "bind-deterministic-e97",
    "implement-leased-peer",
    "prove-ready-membership",
    "join-exact-weighted",
]
assert all(sum(line.startswith(task_id + " ") for line in screen.splitlines()) == 1 for task_id in ids)
assert screen.count("Added task:") == 4
assert screen.count("Use --after") == 4
PY
    then
        coherent=1
        break
    fi
    sleep 0.025
done
[[ "$coherent" == 1 ]] \
    || loud_fail "graph/chat never settled into one coherent frame: $(capture | tr '\n' '|')"
(cd "$scratch" && wg tui-dump >logical-screen.txt 2>&1) \
    || loud_fail "tui-dump failed: $(cat logical-screen.txt)"

python3 - "$outer_tty" screen-with-chat.txt logical-screen.txt "$raw" <<'PY'
import re, sys
outer_tty, physical_path, logical_path, raw_path = sys.argv[1:]
physical = open(physical_path, encoding="utf-8").read()
logical = open(logical_path, encoding="utf-8").read()
raw = open(raw_path, "rb").read()

m = re.search(r"WG_ADD_OWNER_READY pid=\d+ fd1=(\S+) fd2=(\S+)", physical)
assert m, physical
inner_out, inner_err = m.groups()
assert inner_out == inner_err, (inner_out, inner_err)
assert inner_out != outer_tty, (inner_out, outer_tty)

# Physical and logical surfaces agree on all mutation/output sentinels.
for needle in [
    "WG_ADD_OWNER_DONE",
    "Added task: Bind deterministic E97 shards",
    "Added task: Implement leased peer lifecycle",
    "Added task: Prove READY membership",
    "Added task: Join exact weighted reducer",
]:
    assert physical.count(needle) == 1, (needle, physical.count(needle), physical)
    assert logical.count(needle) == 1, (needle, logical.count(needle), logical)

# At 200 columns the graph and chat panels are side by side. Agent command
# output must begin after the owning right-pane divider, never in graph cells.
output_lines = [
    line for line in physical.splitlines()
    if "Added task:" in line or "Use --after" in line
]
dividers = []
for line in output_lines:
    divider = line.find("│")
    col = min(i for i in (line.find("Added task:"), line.find("Use --after")) if i >= 0)
    assert divider >= 0 and col > divider, (divider, col, line, physical)
    dividers.append(divider)
right_content_col_1based = min(dividers) + 2

ids = [
    "bind-deterministic-e97",
    "implement-leased-peer",
    "prove-ready-membership",
    "join-exact-weighted",
]
for task_id in ids:
    rows = [line for line in physical.splitlines() if line.startswith(task_id + " ")]
    assert len(rows) == 1, (task_id, rows, physical)

# Every command-output occurrence in the outer byte stream must be a ratatui
# cell write with an explicit cursor address in the right panel. A child that
# inherited the outer PTY would instead contribute an unpositioned bare line.
for needle in (b"Added task:", b"Use --after"):
    starts = [m.start() for m in re.finditer(re.escape(needle), raw)]
    for start in starts:
        prefix = raw[max(0, start - 160):start]
        cursors = list(re.finditer(rb"\x1b\[(\d+);(\d+)H", prefix))
        assert cursors, (needle, prefix)
        col = int(cursors[-1].group(2))
        assert col >= right_content_col_1based, (needle, col, right_content_col_1based, prefix)

for forbidden in (b"failed to persist TUI tab state", b"can't find session"):
    assert forbidden not in raw, (forbidden, raw[-1000:])
PY

# Prove input remains live after the burst and force the poisoned tab-state
# persistence path: Ctrl+O enters command mode, w hides the chat. The final
# physical graph must settle without stale chat output, duplicated rows or
# displaced borders.
tmux send-keys -t "$outer" C-o
sleep 0.08
tmux send-keys -t "$outer" w
wait_screen 'Close Chat' 'post-burst input did not open the Close dialog'
tmux send-keys -t "$outer" h
sleep 0.35
capture >screen-graph-only.txt
python3 - screen-graph-only.txt "$raw" <<'PY'
import sys
screen = open(sys.argv[1], encoding="utf-8").read()
raw = open(sys.argv[2], "rb").read()
for task_id in [
    "bind-deterministic-e97",
    "implement-leased-peer",
    "prove-ready-membership",
    "join-exact-weighted",
]:
    rows = [line for line in screen.splitlines() if line.startswith(task_id + " ")]
    assert len(rows) == 1, (task_id, rows, screen)
assert "Added task:" not in screen, screen
assert b"failed to persist TUI tab state" not in raw
assert b"can't find session" not in raw
tops = [line for line in screen.splitlines() if line.startswith("┌") and line.endswith("┐")]
bottoms = [line for line in screen.splitlines() if line.startswith("└") and line.endswith("┘")]
assert len(tops) == len(bottoms), (tops, bottoms, screen)
PY

echo "PASS: nested wg add fd is confined to chat PTY; positioned outer bytes, async graph rows, mosh mode, resize and poisoned persistence stay coherent"
