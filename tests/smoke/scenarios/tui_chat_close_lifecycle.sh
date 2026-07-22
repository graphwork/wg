#!/usr/bin/env bash
# Installed-binary tmux flow for terminal-chat routing and identity-pinned Close… lifecycle.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI/PTY flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required for graph assertions"

# The smoke gate is invoked by `wg done` from a worker. The fixture itself is
# a user-level TUI flow and must not inherit the parent's graph or worker-only
# service-control prohibition.
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_BRANCH WG_DIR \
    WG_PROJECT_ROOT WG_REASONING WG_SPAWN_EPOCH WG_SPAWN_RUN_ID \
    WG_TASK_TIMEOUT_SECS WG_WORKTREE_ACTIVE WG_WORKTREE_PATH

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
for name in cancel stop archive; do
    env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
        wg chat new --name "$name" --command cat >>chat.log 2>&1 \
        || loud_fail "custom chat '$name' fixture failed: $(tail -20 chat.log)"
done

python3 - .wg/graph.jsonl <<'PY'
import json, sys
path = sys.argv[1]
rows = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
by_id = {row["id"]: row for row in rows}
assert all(f".chat-{n}" in by_id for n in range(3)), sorted(by_id)
abandoned = dict(by_id[".chat-0"])
abandoned.update({
    "id": ".chat-36",
    "title": "Past Chat 36",
    "status": "abandoned",
    "completed_at": "2026-07-16T01:23:45Z",
    "executor_preset_name": "claude",
    "model": "claude:opus",
    "command_argv": [],
    "agent": None,
    "assigned": None,
})
with open(path, "a", encoding="utf-8") as out:
    out.write(json.dumps(abandoned, separators=(",", ":")) + "\n")
PY

printf '%s\n' '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0",".chat-1",".chat-2"],"active":".chat-0"}' \
    >.wg/tui-state.json

project_basename=$(basename "$scratch" | tr ':.' '--')
project_hash=$(python3 - "$scratch/.wg" <<'PY'
import hashlib, os, sys
print(hashlib.sha256(os.path.realpath(sys.argv[1]).encode()).hexdigest()[:16])
PY
)
project_tag="${project_basename}-${project_hash}"
session_for() { printf 'wg-chat-%s-chat-%s' "$project_tag" "$1"; }
outer="wgsmoke-chat-close-$$"
cleanup_sessions() {
    tmux kill-session -t "$outer" 2>/dev/null || true
    for n in 0 1 2; do tmux kill-session -t "$(session_for "$n")" 2>/dev/null || true; done
}
add_cleanup_hook cleanup_sessions
capture() { tmux capture-pane -p -t "$outer" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 300); do
        screen=$(capture)
        printf '%s\n' "$screen" | grep -Fq "$needle" && return 0
        sleep 0.02
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
wait_not_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 300); do
        screen=$(capture)
        ! printf '%s\n' "$screen" | grep -Fq "$needle" && return 0
        sleep 0.02
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
assert_graph() {
    local task=$1 status=$2 archived=$3
    python3 - .wg/graph.jsonl "$task" "$status" "$archived" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
row = next(r for r in rows if r["id"] == sys.argv[2])
assert row["status"] == sys.argv[3], row
has_archived = "archived" in row.get("tags", [])
assert has_archived == (sys.argv[4] == "yes"), row
PY
}
assert_coherent_archive_frame() {
    local archived_id=$1; shift
    capture >.wg/archive-screen.txt
    if ! python3 - .wg/archive-screen.txt "$archived_id" "$@" <<'PY'
import sys
screen_path, archived, *sentinels = sys.argv[1:]
screen = open(screen_path, encoding="utf-8").read()
lines = screen.splitlines()
assert "can't find session" not in screen, screen
assert archived not in screen, (archived, screen)
assert screen.count("↯  ⌁  ⌂") == 1, screen
assert screen.count("⊞") == 1, screen
assert ".chat-0" in screen, screen
for stale in ("Graph | NAV", "Commands •", "Close…", "Prev", "Next", "Choose", "┌", "┐", "└", "┘"):
    assert stale not in screen, (stale, screen)
for sentinel in sentinels:
    count = sum(line.startswith(sentinel + " ") for line in lines)
    assert count == 1, (sentinel, count, screen)
PY
    then
        loud_fail "Archive frame was physically incoherent: $(capture | tr '\n' '|')"
    fi
}

tmux new-session -d -s "$outer" -x 120 -y 36 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg tui"
# Reassert geometry when the smoke itself runs under a stale/narrow tmux
# client whose `window-size latest` policy overrides new-session -x/-y.
tmux resize-window -t "$outer" -x 120 -y 36
wait_screen '.chat-0' 'live .chat-0 identity did not render'
wait_screen '⊞' 'live Chat context did not render compact New-chat tile'
initial_screen=$(capture)
printf '%s\n' "$initial_screen" | grep -Fq 'Close…' \
    && loud_fail "legacy always-visible Close… action remained in Chat context"

# Exact terminal selection: chooser starts on live id 0; .chat-36 is fourth.
project_sessions_before=$(tmux list-sessions -F '#{session_name}' | grep -E "^wg-chat-${project_tag}-" || true)
sessions_before=$(printf '%s\n' "$project_sessions_before" | grep -c . || true)
tmux send-keys -t "$outer" '~'
chooser=0
for _ in $(seq 1 100); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -Fq 'Choose Chat'; then chooser=1; break; fi
    sleep 0.02
done
if (( chooser == 0 )); then
    tmux send-keys -t "$outer" C-o '~'
    wait_screen 'Choose Chat' 'chat chooser did not open'
fi
for _ in 1 2 3; do tmux send-keys -t "$outer" Down; sleep 0.05; done
tmux send-keys -t "$outer" Enter
wait_screen '.chat-36' 'abandoned .chat-36 did not render in Detail'
wait_screen 'abandoned' 'abandoned state did not render in Detail'
screen=$(capture)
printf '%s\n' "$screen" | grep -Fq '↯  ⌁  ⌂  .chat-36' \
    || loud_fail "abandoned .chat-36 did not open the contextual Task/Detail inspector"
printf '%s\n' "$screen" | grep -Fq '[ New chat ]' \
    && loud_fail "terminal Task/Detail context leaked the Chat action"
printf '%s\n' "$screen" | grep -Fq "Press 'c' or ':' to start typing" \
    && loud_fail "obsolete disconnected-composer hint rendered for abandoned chat"
printf '%s\n' "$screen" | grep -Fq 'Active: Past Chat 36' \
    && loud_fail "abandoned chat falsely became the active route"
sleep 0.15
project_sessions_after=$(tmux list-sessions -F '#{session_name}' | grep -E "^wg-chat-${project_tag}-" || true)
sessions_after=$(printf '%s\n' "$project_sessions_after" | grep -c . || true)
[[ "$sessions_after" == "$sessions_before" ]] \
    || loud_fail "abandoned selection spawned/reattached a chat session: [$project_sessions_before] -> [$project_sessions_after]"

# Return to the still-live chat. 0 selects the Chat panel while the active live
# identity remains pinned to 0 (terminal Detail never changed it).
tmux send-keys -t "$outer" 0
wait_screen '.chat-0' 'could not return to live .chat-0'
wait_screen '⊞' 'live Chat compact New-chat tile missing after terminal Detail return'
chat0_session=$(session_for 0)
for _ in $(seq 1 200); do tmux has-session -t "$chat0_session" 2>/dev/null && break; sleep 0.02; done
tmux has-session -t "$chat0_session" 2>/dev/null \
    || loud_fail "live chat-0 tmux session was not running"
chat0_pid=$(tmux display-message -p -t "$chat0_session" '#{pane_pid}')

# Cancel: modal is identity-explicit and opening/canceling is non-mutating.
tmux send-keys -t "$outer" C-o w
wait_screen 'Close Chat' 'command-mode w did not open Close… modal'
for needle in 'Chat: cancel' '.chat-0' 'in-progress' 'command' 'Hide/detach' 'Stop chat agent' 'Archive chat' 'Cancel'; do
    wait_screen "$needle" "Close… modal omitted '$needle'"
done
tmux send-keys -t "$outer" c
wait_not_screen 'Close Chat' 'Cancel did not close the modal'
tmux has-session -t "$chat0_session" 2>/dev/null \
    || loud_fail "Cancel stopped the live chat"
[[ "$(tmux display-message -p -t "$chat0_session" '#{pane_pid}')" == "$chat0_pid" ]] \
    || loud_fail "Cancel replaced the live chat process"
assert_graph .chat-0 in-progress no

# Hide/detach: safe default Enter hides only; exact agent keeps running.
tmux send-keys -t "$outer" w
wait_screen 'Close Chat' 'Hide did not reopen Close… modal'
tmux send-keys -t "$outer" h
wait_not_screen 'Close Chat' 'Hide did not close the modal'
wait_not_screen '↯  ⌁  ⌂  .chat-0' 'Hide left the detached chat active'
tmux has-session -t "$chat0_session" 2>/dev/null \
    || loud_fail "Hide/detach killed the agent"
[[ "$(tmux display-message -p -t "$chat0_session" '#{pane_pid}')" == "$chat0_pid" ]] \
    || loud_fail "Hide/detach replaced the agent"
assert_graph .chat-0 in-progress no

# Reopen a different live chat explicitly from the chooser; hidden chat-0 must
# stay detached and running in the background. Hide may focus the next live
# chat's PTY immediately, so escape to command mode before the chooser key.
tmux send-keys -t "$outer" C-o '~'
wait_screen 'Choose Chat' 'chooser did not open after Hide'
# Hide already advanced the active identity to chat-1, and the chooser opens
# on that identity. Enter selects it; Down would race the stale header and
# silently select chat-2 instead.
tmux send-keys -t "$outer" Enter
wait_screen '↯  ⌁  ⌂  .chat-1' 'live chat-1 did not open from chooser'

# Start the real daemon for canonical `wg chat stop`; TUI sentinel ownership
# makes the selected PTY the process that the successful effect must terminate.
start_wg_daemon "$scratch" --max-agents 1
chat1_session=$(session_for 1)
for _ in $(seq 1 200); do tmux has-session -t "$chat1_session" 2>/dev/null && break; sleep 0.02; done
tmux has-session -t "$chat1_session" 2>/dev/null \
    || loud_fail "live chat-1 tmux session was not running; sessions=$(tmux list-sessions -F '#{session_name}' | grep -E \"^wg-chat-${project_tag}-\" | tr '\n' ' '); screen=$(capture | tr '\n' '|')"

# Destructive-stage Enter is Cancel, not yes.
tmux send-keys -t "$outer" C-o w
wait_screen 'Close Chat' 'Stop did not open Close… modal'
tmux send-keys -t "$outer" s
wait_screen 'Confirm Stop chat agent' 'Stop did not enter destructive confirmation'
tmux send-keys -t "$outer" Enter
sleep 0.1
tmux has-session -t "$chat1_session" 2>/dev/null \
    || loud_fail "Enter default destructively stopped chat-1"
assert_graph .chat-1 in-progress no

# Explicit y runs the canonical stop. It resets the task to Open/resumable,
# kills the exact TUI-owned PTY, and detaches its tab instead of showing a dead composer.
tmux send-keys -t "$outer" w
wait_screen 'Close Chat' 'confirmed Stop did not open Close… modal'
tmux send-keys -t "$outer" s
wait_screen 'Confirm Stop chat agent' 'confirmed Stop did not enter confirmation'
tmux send-keys -t "$outer" y
for _ in $(seq 1 300); do
    if ! tmux has-session -t "$chat1_session" 2>/dev/null \
        && python3 - .wg/graph.jsonl <<'PY' 2>/dev/null
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
raise SystemExit(0 if next(r for r in rows if r['id']=='.chat-1')['status']=='open' else 1)
PY
    then break; fi
    sleep 0.02
done
! tmux has-session -t "$chat1_session" 2>/dev/null \
    || loud_fail "Stop left the chat-1 agent session running"
assert_graph .chat-1 open no
# Closing the stopped identity advances directly to the next live chat rather
# than leaving a disconnected composer. Pin that coherent handoff to chat-2.
wait_screen '↯  ⌁  ⌂  .chat-2' 'Stop did not advance to live chat-2'

# Archive has explicit Done+archived semantics and tears down its process.
chat2_session=$(session_for 2)
for _ in $(seq 1 200); do tmux has-session -t "$chat2_session" 2>/dev/null && break; sleep 0.02; done
tmux has-session -t "$chat2_session" 2>/dev/null \
    || loud_fail "live chat-2 tmux session was not running"
tmux send-keys -t "$outer" C-o w
wait_screen 'Close Chat' 'Archive did not open Close… modal'
tmux send-keys -t "$outer" a
wait_screen 'Confirm Archive chat' 'Archive did not enter destructive confirmation'
tmux send-keys -t "$outer" y
for _ in $(seq 1 300); do
    python3 - .wg/graph.jsonl <<'PY' 2>/dev/null && break
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
r=next(r for r in rows if r['id']=='.chat-2')
raise SystemExit(0 if r['status']=='done' and 'archived' in r.get('tags',[]) else 1)
PY
    sleep 0.02
done
assert_graph .chat-2 done yes
! tmux has-session -t "$chat2_session" 2>/dev/null \
    || loud_fail "Archive left the chat-2 agent session running"
wait_not_screen '.chat-2  (' 'Archive graph row did not leave the physical frame'
wait_screen '↯  ⌁  ⌂  .chat-0' 'Archive did not settle on one live identity'
# Lifecycle toasts intentionally overlay graph cells; wait for them to expire
# before asserting the settled physical/differential frame.
wait_not_screen 'Archived coordinator 2' 'Archive toast did not expire'
wait_not_screen 'Stopped chat agent 1' 'Stop toast did not expire'
assert_coherent_archive_frame .chat-2 .chat-0 .chat-1 .chat-36

echo 'PASS: exact Detail + command-mode Close Cancel/Hide/Stop + Archive settle on one borderless contextual frame'
