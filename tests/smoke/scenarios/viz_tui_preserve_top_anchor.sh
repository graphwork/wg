#!/usr/bin/env bash
# Scenario: viz_tui_preserve_top_anchor
#
# Drives the real `wg viz --tui` surface in tmux. Start with the graph pane at
# the top, then add rows above the currently selected task from another shell
# via `wg insert before`. The TUI must stay pinned to graph row 0 so the newly
# inserted top rows are visible.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg viz --tui"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 not on PATH; cannot parse tui-dump JSON"
fi

scratch=$(make_scratch)
session="wgsmoke-viz-top-anchor-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session
cd "$scratch"

wg_fixture() {
    env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg "$@"
}

if ! wg_fixture init --no-agency --executor shell >init.log 2>&1; then
    loud_fail "wg init --no-agency --executor shell failed: $(tail -5 init.log)"
fi

if ! wg_fixture add "Anchor Root" --id anchor-root >>setup.log 2>&1; then
    loud_fail "wg add anchor-root failed: $(cat setup.log)"
fi
for i in $(seq 1 40); do
    id=$(printf 'old-%02d' "$i")
    if ! wg_fixture add "Old dependent $i" --id "$id" --after anchor-root >>setup.log 2>&1; then
        loud_fail "wg add $id failed: $(tail -10 setup.log)"
    fi
done

# The setup uses `wg add`, which writes the focus marker for normal user adds.
# This scenario specifically pins background graph growth/reordering where no
# new-task focus marker is present; `wg insert` below also uses that path.
rm -f .wg/.new_task_focus

tmux new-session -d -s "$session" -x 140 -y 32 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER WG_USER=unknown wg viz --tui --no-mouse"

for _ in $(seq 1 40); do
    if [[ -S ".wg/service/tui.sock" ]]; then
        break
    fi
    sleep 0.25
done
if [[ ! -S ".wg/service/tui.sock" ]]; then
    loud_fail "wg viz --tui did not create tui.sock within 10s"
fi

dump_text() {
    wg_fixture --json tui-dump 2>/dev/null \
        | python3 -c 'import json,sys; print(json.load(sys.stdin).get("text",""))' 2>/dev/null
}

initial=""
for _ in $(seq 1 40); do
    initial="$(dump_text || true)"
    if printf '%s\n' "$initial" | grep -q "anchor-root"; then
        break
    fi
    sleep 0.25
done
if ! printf '%s\n' "$initial" | grep -q "anchor-root"; then
    loud_fail "initial wg viz --tui screen never showed anchor-root:\n$initial"
fi

# Make the starting intent explicit: user is watching the graph top.
# `g` is the real graph-pane "go top" key. Wait until the status bar shows
# L1/... (the TUI renders offsets as 1-based) so the regression check starts
# from a verified top anchor.
tmux send-keys -t "$session" g
top_dump=""
for _ in $(seq 1 40); do
    top_dump="$(dump_text || true)"
    if printf '%s\n' "$top_dump" | head -1 | grep -q 'L1/'; then
        break
    fi
    sleep 0.25
done
if ! printf '%s\n' "$top_dump" | head -1 | grep -q 'L1/'; then
    loud_fail "could not drive wg viz --tui to graph top before insertion:\n$top_dump"
fi

for i in $(seq 1 18); do
    id=$(printf 'top-%02d' "$i")
    if ! wg_fixture insert before anchor-root --title "Inserted top $i" --id "$id" >>insert.log 2>&1; then
        loud_fail "wg insert $id failed: $(tail -10 insert.log)"
    fi
done

after=""
for _ in $(seq 1 60); do
    after="$(dump_text || true)"
    if printf '%s\n' "$after" | grep -q "top-01"; then
        break
    fi
    sleep 0.25
done

if ! printf '%s\n' "$after" | grep -q "top-01"; then
    static="$(NO_COLOR=1 wg_fixture viz --no-tui 2>&1 || true)"
    loud_fail "top-of-graph inserted task top-01 is not visible after TUI refresh. Static graph has it; live screen drifted from the top.\n--- tui dump ---\n$after\n--- static viz ---\n$static"
fi

echo "PASS: wg viz --tui remains anchored at top and shows inserted top rows"
exit 0
