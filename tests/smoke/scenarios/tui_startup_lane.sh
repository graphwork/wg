#!/usr/bin/env bash
# Scenario: tui_startup_lane
#
# Pins the operator request "let the TUI open graph-focused (no chat pane)":
#
#   `wg tui` historically always opened with the right panel on the Chat
#   lane, so the operator could not *start* on the graph+inspector view even
#   though clicking a node already opened its Detail. This adds
#   `[tui] default_lane` and the `wg tui --lane <chat|task|workspace>` flag
#   (plus the `--no-chat` alias for `--lane task`).
#
# The scenario drives a real `wg tui` inside tmux (the actual human flow) and
# reads the live inspector state through the `wg --json tui-dump` IPC:
#
#   (1) default (neither config nor flag) still opens on Chat;
#   (2) `[tui] default_lane = "task"` opens graph-focused on Detail;
#   (3) `--lane` wins over the configured default;
#   (4) `--no-chat` is the ergonomic alias for `--lane task`.
#
# No LLM is required — uses the shell executor and only inspects the startup
# lane, so it stays credential-free.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-lane-$$"
cleanup() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup
cd "$scratch"
unset WG_DIR

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

# Two tasks so the graph view has real nodes to inspect.
for title in "Startup lane alpha" "Startup lane bravo"; do
    if ! wg add "$title" >add.log 2>&1; then
        loud_fail "wg add '$title' failed: $(tail -5 add.log)"
    fi
done

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"
config_file="$graph_dir/config.toml"

# Start (or restart) `wg tui` in a detached tmux session and wait for the
# dump socket. Any previous session is torn down first so each launch is the
# real "operator starts `wg tui`" flow.
launch_tui() {
    tmux kill-session -t "$session" 2>/dev/null || true
    rm -f "$graph_dir/service/tui.sock"
    # Pin the TUI to the exact `wg` under test. A long-lived tmux server may
    # carry an older PATH, so pass an explicit PATH rather than relying on the
    # server's inherited environment.
    local wg_bin wg_dir_path
    wg_bin="$(command -v wg)"
    wg_dir_path="$(dirname "$wg_bin")"
    tmux new-session -d -s "$session" -x 200 -y 60 \
        "PATH=$wg_dir_path:\$PATH wg tui $*"
    local i
    for i in $(seq 1 60); do
        if [[ -S "$graph_dir/service/tui.sock" ]]; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

dump_field() {
    wg --json tui-dump 2>/dev/null \
        | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" \
        | head -1
}

# Poll the live inspector until it reports the expected tab (bootstrap is
# asynchronous; the startup lane is applied when it lands).
wait_tab() {
    local expected="$1" ticks="${2:-60}" i got
    for i in $(seq 1 "$ticks"); do
        got="$(dump_field active_tab)"
        if [[ "$got" == "$expected" ]]; then
            return 0
        fi
        sleep 0.25
    done
    echo "expected active_tab='$expected' but last observed '$got'" >&2
    return 1
}

assert_tab_and_focus() {
    local tab="$1" focus="$2" label="$3"
    wait_tab "$tab" || loud_fail "$label: right panel did not open on '$tab'"
    local observed
    observed="$(dump_field focused_panel)"
    if [[ "$observed" != "$focus" ]]; then
        loud_fail "$label: focused_panel expected '$focus' but observed '$observed'"
    fi
    echo "OK: $label → active_tab=$tab focused_panel=$focus"
}

# (1) Default: no config, no flag → unchanged Chat startup.
launch_tui || loud_fail "default: wg tui did not create its dump socket within 30s"
assert_tab_and_focus "Chat" "graph" "default startup"

# (2) Config `[tui] default_lane = "task"` → graph + Detail inspector.
tmux kill-session -t "$session" 2>/dev/null || true
if grep -q '^default_lane[[:space:]]*=' "$config_file"; then
    sed -i 's/^default_lane[[:space:]]*=.*/default_lane = "task"/' "$config_file"
else
    printf '\n[tui]\ndefault_lane = "task"\n' >>"$config_file"
fi
launch_tui || loud_fail "config lane: wg tui did not create its dump socket within 30s"
assert_tab_and_focus "Detail" "graph" "config default_lane=task"

# (3) CLI `--lane` wins over the configured default (task config → workspace).
launch_tui --lane workspace \
    || loud_fail "cli override: wg tui did not create its dump socket within 30s"
assert_tab_and_focus "Activity" "graph" "cli --lane workspace over config task"

# (4) `--no-chat` is the alias for `--lane task`.
launch_tui --no-chat || loud_fail "no-chat: wg tui did not create its dump socket within 30s"
assert_tab_and_focus "Detail" "graph" "cli --no-chat alias"

echo "PASS: startup lane selection (default Chat, config task, CLI override, --no-chat alias)"
exit 0
