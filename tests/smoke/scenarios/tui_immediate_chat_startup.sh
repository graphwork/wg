#!/usr/bin/env bash
# Real human flow for restore-immediate-chat.
#
# An already-running custom chat lives in tmux. The real `wg tui` starts while
# its unrelated full bootstrap is deliberately held for 5 seconds and 100MiB
# history/log artifacts exist. A keystroke sent immediately to the outer TUI
# must cross the prioritized metadata lane, reattach (not duplicate) the inner
# session, and reach that exact chat process before the full snapshot can land.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI/PTY flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required to inspect startup milestones"

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg chat new --name immediate --command cat >chat.log 2>&1 \
    || loud_fail "custom chat fixture failed: $(cat chat.log)"

# Persist the active tab explicitly: the fast lane must validate this pointer
# against authoritative graph metadata before attaching.
printf '%s\n' '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}' \
    > .wg/tui-state.json

# Large artifacts are intentionally unrelated to attach/routing. The 10k-task
# graph is appended only after the already-running chat has reached its prompt,
# matching a real long-lived conversation whose project grows large.
truncate -s 104857600 .wg/chat-history-0.jsonl
mkdir -p .wg/agents/huge
truncate -s 104857600 .wg/agents/huge/output.log

project_tag=$(basename "$scratch" | tr ':.' '--')
inner="wg-chat-${project_tag}-chat-0"
outer="wgsmoke-immediate-chat-$$"
trace="$scratch/startup.jsonl"
delivery="$scratch/chat-delivery.log"
cleanup_sessions() {
    [[ -n "${stall_writer:-}" ]] && kill "$stall_writer" 2>/dev/null || true
    rm -f "${storage_fifo:-}" 2>/dev/null || true
    tmux kill-session -t "$outer" 2>/dev/null || true
    tmux kill-session -t "$inner" 2>/dev/null || true
}
add_cleanup_hook cleanup_sessions

capture() {
    tmux capture-pane -p -t "$outer" 2>/dev/null || true
}

# This is the pre-existing chat process. `cat` is intentionally credential-free
# and byte-transparent, so the proof observes exactly what the outer TUI sends.
# Reattach must preserve its pane PID; creating a second handler/session is a
# failure even if echo appears.
tmux new-session -d -s "$inner" -x 120 -y 36 -- tee "$delivery"
inner_pid_before=$(tmux display-message -p -t "$inner" '#{pane_pid}')

# The active chat remains the first authoritative record, so the minimal lane
# stops there while full bootstrap later parses/layouts all 10k terminal tasks.
python3 - .wg/graph.jsonl <<'PY'
import json, sys
path = sys.argv[1]
with open(path, encoding="utf-8") as source:
    active = json.loads(source.readline())
with open(path, "a", encoding="utf-8") as out:
    for index in range(10_000):
        task = dict(active)
        task["id"] = f"unrelated-large-{index:05d}"
        task["title"] = f"Unrelated large project task {index}"
        task["status"] = "done"
        task["completed_at"] = "2026-07-16T00:00:00Z"
        task["tags"] = ["large-startup-fixture"]
        task.pop("executor_preset_name", None)
        task.pop("model", None)
        task.pop("command_argv", None)
        out.write(json.dumps(task, separators=(",", ":")) + "\n")
PY
[[ $(wc -l <.wg/graph.jsonl) -ge 10001 ]] \
    || loud_fail "large graph fixture was not created"

# A real project-local FIFO is the pathological storage data plane. The
# bootstrap worker blocks in File::read_to_end until we close this writer;
# meanwhile the independent authoritative chat lane must reattach and accept
# conversation bytes. After release, the same worker also exercises 5s latency.
storage_fifo="$scratch/.wg/bootstrap-storage-stall"
stall_ready="$scratch/storage-reader-is-blocked"
mkfifo "$storage_fifo"
(
    exec 9>"$storage_fifo"
    : >"$stall_ready"
    sleep 30
) &
stall_writer=$!

now_ms() { date +%s%3N; }
start_ms=$(now_ms)
tmux new-session -d -s "$outer" -x 140 -y 42 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER WG_TUI_STARTUP_TRACE='$trace' WG_TUI_TEST_STORAGE_STALL_PATH='$storage_fifo' WG_TUI_TEST_STORAGE_LATENCY_MS=5000 wg tui"

# Wait only for the neutral first frame, then type immediately. Do not wait for
# the active chat pane or the delayed graph snapshot.
first=0
for _ in $(seq 1 100); do
    screen=$(tmux capture-pane -p -t "$outer" 2>/dev/null || true)
    if printf '%s\n' "$screen" | grep -Eq '0 tasks|Connecting active chat'; then
        first=1
        break
    fi
    sleep 0.01
done
(( first == 1 )) || loud_fail "storage-independent first frame did not paint"
for _ in $(seq 1 200); do
    [[ -f "$stall_ready" ]] && break
    sleep 0.01
done
[[ -f "$stall_ready" ]] \
    || loud_fail "bootstrap worker did not enter the real filesystem stall"
# Wait only for the prioritized pane milestone (never the delayed graph lane),
# then type at once. This remains far below the injected 5s bootstrap.
for _ in $(seq 1 100); do
    [[ -f "$trace" ]] && grep -q 'pane_attached' "$trace" && break
    sleep 0.005
done
grep -q 'pane_attached' "$trace" 2>/dev/null \
    || loud_fail "prioritized pane did not attach"
# The visible identity must be coherent with the pane we are about to type
# into. In particular, a delayed full snapshot may not leave a generic Chat
# heading or paint another tab's route over this reattached terminal.
identity_ready=0
for _ in $(seq 1 100); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -Eq 'Active: .*[.]chat-0.*connected.*route command'; then
        identity_ready=1
        break
    fi
    sleep 0.01
done
(( identity_ready == 1 )) \
    || loud_fail "active-chat identity/header did not agree with attached .chat-0 command pane: $(capture | tail -12 | tr '\n' ' ')"
payload="Z"
tmux send-keys -l -t "$outer" "$payload"
tmux send-keys -t "$outer" Enter

reached=0
for _ in $(seq 1 200); do
    inner_screen=$(tmux capture-pane -p -t "$inner" 2>/dev/null || true)
    if grep -q "$payload" "$delivery" 2>/dev/null; then
        reached=1
        break
    fi
    sleep 0.01
done
reach_ms=$(( $(now_ms) - start_ms ))
(( reached == 1 )) \
    || loud_fail "immediate TUI keystroke never reached the existing chat process; outer=$(tmux capture-pane -p -t "$outer" 2>/dev/null | tail -12 | tr '\n' ' '); inner=$(tmux capture-pane -p -t "$inner" 2>/dev/null | tail -12 | tr '\n' ' ') trace=$(tr '\n' ' ' <"$trace" 2>/dev/null || true)"
(( reach_ms < 3000 )) \
    || loud_fail "chat became interactive in ${reach_ms}ms; it followed unrelated bootstrap"
(( reach_ms < 5000 )) \
    || loud_fail "chat keystroke arrived only after the injected full-bootstrap delay"

inner_pid_after=$(tmux display-message -p -t "$inner" '#{pane_pid}')
[[ "$inner_pid_after" == "$inner_pid_before" ]] \
    || loud_fail "reattach replaced the existing handler: before=$inner_pid_before after=$inner_pid_after"
count=$(tmux list-sessions -F '#{session_name}' | grep -Fx "$inner" | wc -l | tr -d ' ')
[[ "$count" == 1 ]] || loud_fail "duplicate chat tmux session detected: count=$count"
# The delivery above happened while the storage worker was blocked on a real
# FIFO. Release it now; the worker then observes the separate 5s latency shim.
kill "$stall_writer" 2>/dev/null || true
wait "$stall_writer" 2>/dev/null || true
stall_writer=""
rm -f "$storage_fifo"

# Exact state-dependent command flow on the real PTY: plain n is child text;
# Ctrl+O is the host escape; n then opens New chat. Close it and return to
# capture. Full-label pointer boundaries are pinned separately by event/render
# tests because synthetic tmux send-keys does not preserve mouse coordinates.
tmux send-keys -t "$outer" n Enter
for _ in $(seq 1 100); do
    inner_screen=$(tmux capture-pane -p -t "$inner" 2>/dev/null || true)
    grep -qx 'n' "$delivery" 2>/dev/null && break
    sleep 0.01
done
grep -qx 'n' "$delivery" 2>/dev/null \
    || loud_fail "plain n in chat capture did not reach the child"
tmux send-keys -t "$outer" C-o n
for _ in $(seq 1 100); do
    screen=$(tmux capture-pane -p -t "$outer" 2>/dev/null || true)
    printf '%s\n' "$screen" | grep -q 'Add new...' && break
    sleep 0.01
done
printf '%s\n' "$screen" | grep -q 'Add new...' \
    || loud_fail "Ctrl+O then command-mode n did not open New chat"
tmux send-keys -t "$outer" Escape
for _ in $(seq 1 100); do
    screen=$(capture)
    ! printf '%s\n' "$screen" | grep -q 'Add new...' && break
    sleep 0.01
done
printf '%s\n' "$screen" | grep -q 'Add new...' \
    && loud_fail "New-chat launcher did not close with Escape"
tmux send-keys -t "$outer" C-o

# The first proof ran with the coordinator daemon OFF. Turn it ON for the
# second half, then keep mutating the unrelated graph while proving another key
# is acknowledged. The helper registers the real daemon PID for cleanup.
start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"
[[ -f "$graph_dir/service/state.json" ]] || loud_fail "daemon-on phase did not start"
(
    # Repeated graph publication/mtime changes exercise the TUI watcher and
    # snapshot scheduler without asking the daemon to change chat ownership.
    for _ in $(seq 1 20); do
        touch .wg/graph.jsonl
        sleep 0.02
    done
) &
mutator=$!
# Daemon startup may publish a fresh graph between command-mode and PTY focus
# events; reassert the visible capture state before testing conversation bytes.
screen=$(capture)
if ! printf '%s\n' "$screen" | grep -q '\[PTY\]'; then
    tmux send-keys -t "$outer" Escape
    sleep 0.05
    tmux send-keys -t "$outer" C-o
fi
for _ in $(seq 1 100); do
    screen=$(capture)
    printf '%s\n' "$screen" | grep -q '\[PTY\]' && break
    sleep 0.01
done
printf '%s\n' "$screen" | grep -q '\[PTY\]' \
    || loud_fail "daemon-on phase could not focus the existing chat PTY: $(capture | tail -12 | tr '\n' ' ')"
second="Y"
tmux send-keys -t "$outer" Y Enter
for _ in $(seq 1 100); do
    inner_screen=$(tmux capture-pane -p -t "$inner" 2>/dev/null || true)
    grep -q "$second" "$delivery" 2>/dev/null && break
    sleep 0.01
done
wait "$mutator" || true
grep -q "$second" "$delivery" 2>/dev/null \
    || loud_fail "daemon-on continuous graph mutation starved chat input; outer=$(capture | tail -12 | tr '\n' ' '); delivery=$(tail -8 "$delivery" 2>/dev/null | tr '\n' ' '); inner=$(tmux display-message -p -t "$inner" '#{pane_current_command}:#{pane_pid}' 2>/dev/null || true)"
inner_pid_daemon=$(tmux display-message -p -t "$inner" '#{pane_pid}')
[[ "$inner_pid_daemon" == "$inner_pid_before" ]] \
    || loud_fail "daemon-on phase raced/replaced the attached chat handler: before=$inner_pid_before after=$inner_pid_daemon pane=$(tmux display-message -p -t "$inner" '#{pane_current_command}' 2>/dev/null || true) outer_alive=$(tmux has-session -t "$outer" 2>/dev/null && echo yes || echo no) service=$(tail -8 "$graph_dir/service/service.log" 2>/dev/null | tr '\n' ' ')"

# The reporter itself is asynchronous. Wait briefly for flush, then require the
# complete ordered startup story, including first accepted/echoed keystroke.
for _ in $(seq 1 100); do
    [[ -f "$trace" ]] && grep -q 'first_keystroke_accepted' "$trace" && break
    sleep 0.02
done
python3 - "$trace" <<'PY' || loud_fail "startup milestone trace was missing or out of order"
import json, sys
from pathlib import Path
rows = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines() if line.strip()]
names = [row["milestone"] for row in rows]
required = ["first_frame", "active_chat_metadata_ready", "pane_attached", "first_keystroke_accepted"]
for name in required:
    assert name in names, (name, names)
assert names.index("first_frame") < names.index("active_chat_metadata_ready") < names.index("pane_attached"), names
# The byte-transparent delivery log above is the authoritative echo proof;
# parser-level first_keystroke_echoed remains instrumented for normal panes.
PY

echo "PASS: existing chat accepted immediate input in ${reach_ms}ms during a real filesystem stall while full bootstrap/history/log work remained delayed; no duplicate handler"
