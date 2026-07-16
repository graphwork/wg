#!/usr/bin/env bash
# Human-flow regression for move-large-graph.  A real 120x40 TUI is driven
# while a 10k-task/50k-edge graph is repeatedly replaced.  Input must remain
# responsive and the newest generation must eventually win.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is needed to generate the large fixture"

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"

graph="$scratch/.wg/graph.jsonl"
python3 - "$graph" <<'PY'
import json, sys
from pathlib import Path

path = Path(sys.argv[1])
with path.open("w") as out:
    for i in range(10_000):
        # Dense 11-node components average five edges/task (~49,995 total)
        # without creating one pathological 10k-node routing component.
        group_start = (i // 11) * 11
        after = [f"task-{j:05d}" for j in range(group_start, i)]
        out.write(json.dumps({
            "kind": "task",
            "id": f"task-{i:05d}",
            "title": f"Synthetic large graph row {i}",
            "status": "open",
            "priority": 10,
            "after": after,
            "created_at": "2026-07-13T00:00:00Z",
            "last_interaction_at": "2026-07-13T00:00:00Z",
        }, separators=(",", ":")) + "\n")
PY

session="wgsmoke-large-graph-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

launch_start_ms=$(date +%s%3N)
tmux new-session -d -s "$session" -x 120 -y 40 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg tui --no-mouse --show-keys"

capture() {
    tmux capture-pane -p -t "$session" 2>/dev/null || true
}

now_ms() {
    date +%s%3N
}

loaded=0
for _ in $(seq 1 300); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'task-'; then
        loaded=1
        break
    fi
    sleep 0.05
done
(( loaded == 1 )) || loud_fail "10k graph did not publish within 15s"
load_ms=$(( $(date +%s%3N) - launch_start_ms ))
(( load_ms < 2000 )) \
    || loud_fail "10k/50k initial graph publication ${load_ms}ms exceeded the 2s design budget"
pane_pid=$(tmux display-message -p -t "$session" '#{pane_pid}')
initial_rss_kib=$(ps -o rss= -p "$pane_pid" 2>/dev/null | tr -d ' ')
rss_peak_file="$scratch/rss-peak"
printf '%s\n' "${initial_rss_kib:-0}" >"$rss_peak_file"
(
    while kill -0 "$pane_pid" 2>/dev/null; do
        sample=$(ps -o rss= -p "$pane_pid" 2>/dev/null | tr -d ' ')
        peak=$(cat "$rss_peak_file" 2>/dev/null || printf '0')
        if [[ "$sample" =~ ^[0-9]+$ ]] && (( sample > peak )); then
            printf '%s\n' "$sample" >"$rss_peak_file"
        fi
        sleep 0.05
    done
) &
rss_monitor=$!
cleanup_rss_monitor() {
    kill "$rss_monitor" 2>/dev/null || true
    wait "$rss_monitor" 2>/dev/null || true
}
add_cleanup_hook cleanup_rss_monitor

# Replace the graph twenty times. Each rename carries a distinct final task;
# only the last identity is used for convergence so stale generations cannot
# satisfy the assertion accidentally.
python3 - "$graph" <<'PY' &
import json, os, sys, time
from pathlib import Path

path = Path(sys.argv[1])
base = path.read_bytes()
for generation in range(1, 21):
    marker = {
        "kind": "task",
        "id": f"mutation-{generation:02d}",
        "title": f"Mutation generation {generation}",
        "status": "open",
        "priority": 10,
        "after": ["task-09999"],
        "created_at": "2026-07-13T00:00:00Z",
        "last_interaction_at": "2026-07-13T00:00:00Z",
    }
    tmp = path.with_suffix(".next")
    tmp.write_bytes(base + json.dumps(marker, separators=(",", ":")).encode() + b"\n")
    os.replace(tmp, path)
    time.sleep(0.05)
PY
mutator=$!

# Drive the actual keymap while layouts are being superseded. The help overlay
# is an unambiguous on-screen acknowledgement independent of graph contents.
key_start=$(now_ms)
tmux send-keys -t "$session" '?'
acked=0
for _ in $(seq 1 30); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'Navigation'; then
        acked=1
        break
    fi
    sleep 0.01
done
key_ms=$(( $(now_ms) - key_start ))
(( acked == 1 )) || loud_fail "help key was starved during graph derivation"
(( key_ms < 100 )) || loud_fail "help key latency ${key_ms}ms exceeded 100ms design ceiling"
tmux send-keys -t "$session" Escape

# Search input is UI-owned and must not be erased by a late graph publication.
tmux send-keys -t "$session" / 'task-05000'
sleep 0.1
screen=$(capture)
printf '%s\n' "$screen" | grep -q 'task-05000' \
    || loud_fail "search draft was not visible during mutation"

wait "$mutator" || loud_fail "graph mutator failed"

# Query the final generation. The latest marker must become visible and stay
# visible for several frames; a stale completion must never roll it back.
tmux send-keys -t "$session" C-u 'mutation-20'
converged=0
for _ in $(seq 1 300); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'mutation-20'; then
        converged=1
        break
    fi
    sleep 0.05
done
(( converged == 1 )) || loud_fail "TUI did not converge to mutation generation 20"
for _ in $(seq 1 5); do
    sleep 0.1
    screen=$(capture)
    printf '%s\n' "$screen" | grep -q 'mutation-20' \
        || loud_fail "stale snapshot rolled the display back after convergence"
done

# Allow the bounded retirement lane to destroy the replaced generation and
# return its large arenas before measuring steady-state memory.
sleep 2

rss_kib=$(ps -o rss= -p "$pane_pid" 2>/dev/null | tr -d ' ')
rss_peak_kib=$(cat "$rss_peak_file" 2>/dev/null || printf '0')
# The design permits one installed generation, one cooperatively-cancelling
# build, and exactly one latest-wins pending request. A single installed
# 10k/50k generation is ~290 MiB process RSS on CI, so a 1 GiB peak ceiling
# accommodates that deliberate three-owner bound while catching a fourth or
# an unbounded refresh backlog.
if [[ "$rss_peak_kib" =~ ^[0-9]+$ ]] && (( rss_peak_kib > 1048576 )); then
    loud_fail "large-graph TUI peak RSS ${rss_peak_kib}KiB exceeded the bounded three-generation 1GiB ceiling"
fi

tmux send-keys -t "$session" Escape q
echo "PASS: 10k/50k TUI loaded in ${load_ms}ms, input stayed responsive, and latest snapshot converged (key=${key_ms}ms, rss=${initial_rss_kib:-?}->${rss_kib:-?}KiB, peak=${rss_peak_kib:-?}KiB)"
