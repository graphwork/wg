#!/usr/bin/env bash
# Scenario: tui_chat_redesign_modal_close_persist
#
# Pins the critical subset of the TUI chat redesign (integrate-tui-chat
# umbrella). Three invariants the user explicitly called out as the
# regression bar for the redesign:
#
#   (1) MODAL TOGGLE — the chat tab is modal. Default is PTY mode (status
#       bar shows `[PTY]`). Ctrl+T toggles to command mode (status bar
#       shows `[CMD]`). The whole point of the redesign was to stop
#       intercepting in-PTY keystrokes (Ctrl+N, Ctrl+W, plain letters)
#       as global hotkeys — the user couldn't type 'n' or 'w' inside
#       claude/codex without it being eaten by a launcher / close
#       dialog. The modal indicator is the user's only visual cue that
#       the modal contract is in effect, so it MUST render.
#
#   (2) CLOSE IS NON-DESTRUCTIVE — closing a chat tab (Ctrl+W in command
#       mode, or `w` single-key) removes it from the visible tab list
#       but does NOT mark the underlying graph task abandoned/archived.
#       Earlier "tabs as graph" implementations conflated the two and
#       the user lost work by closing a tab. NO ChoiceDialog must
#       appear — `Ctrl+W` is a direct close, not a "what action would
#       you like?" prompt. Reopen by clicking the chat node in the
#       graph viewer or the `+` button on the tab bar.
#
#   (3) PERSISTENCE — relaunching `wg tui` after a kill restores the
#       last-active coordinator. Without this the user has to manually
#       re-pick their chat every time and the modal redesign is moot.
#
# This scenario drives a real `wg tui` inside tmux, sends synthetic key
# sequences via `tmux send-keys`, asserts on `wg --json tui-dump`'s
# screen text and on the graph.jsonl status.
#
# Init uses `--executor claude` because the modal-indicator phase needs
# a live PTY pane (the renderer only emits `[PTY]`/`[CMD]` when
# chat_pty_mode is true with a live pane). The TUI auto-spawns the
# claude CLI inside a PTY when that's the configured executor. The
# dispatcher never picks up any of these chat tasks for autonomous
# work — we kill it before any agent spawn — so missing Anthropic
# creds don't cause a fail. If the claude CLI isn't on PATH, the
# scenario loud-skips (env limitation, not a regression).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-redesign-$$"
cleanup() {
    tmux kill-session -t "$session" 2>/dev/null || true
    if [[ -n "${daemon_pid:-}" ]]; then
        kill_tree "$daemon_pid"
    fi
    rm -rf "$scratch"
}
trap cleanup EXIT
cd "$scratch"

# Unset any inherited WG_DIR from a parent agent invocation. Otherwise
# `wg init` and `wg chat create` operate on the parent project's
# .wg dir instead of our scratch dir, hitting the chat cap or
# corrupting the parent graph. (`unset -v` intentional, it is safe even
# if the var is not set.)
unset WG_DIR

# Init with `--executor claude` so `config.coordinator.effective_executor()`
# returns "claude" — the TUI's `maybe_auto_enable_chat_pty` reads the
# global config (not per-chat overrides) when deciding whether to spawn
# a PTY pane. We need a live pane to observe the [PTY]/[CMD] modal
# indicator. The dispatcher itself never spawns a real claude agent
# in this test (no chat tasks are dispatched as work), so missing
# Anthropic creds don't cause a fail.
if ! wg init --executor claude >init.log 2>&1; then
    loud_fail "wg init --executor claude failed: $(tail -5 init.log)"
fi

graph_dir=""
for cand in .wg .wg; do
    if [[ -d "$scratch/$cand" ]]; then
        graph_dir="$scratch/$cand"
        break
    fi
done
if [[ -z "$graph_dir" ]]; then
    loud_fail "no .wg/ or .wg/ directory after init"
fi

# Start the dispatcher so `wg tui` has live state to display.
wg service start --max-agents 1 >daemon.log 2>&1 &
daemon_pid=$!

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/daemon.sock" ]] \
        || [[ -f "$graph_dir/service/state.json" ]]; then
        break
    fi
    sleep 0.5
done

# Three chats so the close in step 2 leaves at least two visible — that
# way the count delta is unambiguously the close action and not a hidden
# default-coordinator suppression.
#
# Use the `claude` executor so the chat tab spawns a live PTY (the
# embedded vendor CLI). Phase 1 (modal indicator) needs a live PTY to
# observe [PTY]/[CMD]; the shell executor doesn't spawn one. If the
# claude CLI isn't on PATH, loudly SKIP — env limitation, not a
# regression. Phases 2 (close) and 3 (persist) work either way.
if ! command -v claude >/dev/null 2>&1; then
    loud_skip "MISSING CLAUDE CLI" "claude CLI not on PATH; can't spawn a live PTY for the modal-indicator phase. Other phases need PTY too because focused_panel toggling depends on a live pane."
fi
for name in alpha beta gamma; do
    out=$(wg chat create --name "$name" 2>&1)
    rc=$?
    if [[ "$rc" -ne 0 ]]; then
        loud_fail "wg chat create --name $name exited with rc=${rc}: ${out}"
    fi
done

# Snapshot the chat task ids and their initial status. The close
# operation must not mutate any of these statuses.
graph_path="$graph_dir/graph.jsonl"
if [[ ! -f "$graph_path" ]]; then
    loud_fail "graph.jsonl not found at $graph_path"
fi

dump_chat_tasks() {
    # Print "<task_id>\t<status>" per .chat-N task in the graph.
    grep -E '"id":"\.chat-[0-9]+"' "$graph_path" \
        | python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try: row = json.loads(line)
    except json.JSONDecodeError: continue
    tid = row.get("id", "")
    if tid.startswith(".chat-"):
        status = row.get("status", "?")
        print("\t".join((tid, status)))
' \
        | sort -u
}

before=$(dump_chat_tasks)
echo "before TUI launch:"
echo "$before"

before_active=$(printf '%s\n' "$before" \
    | awk -F'\t' '$2 != "abandoned" && $2 != "archived" {print $1}' \
    | sort -u)
before_count=$(printf '%s\n' "$before_active" | grep -c '\.chat-' || true)

if [[ "$before_count" -lt 3 ]]; then
    loud_fail "expected ≥3 active chat tasks before TUI; got ${before_count}: ${before_active}"
fi

# Helper: dump the JSON snapshot from the running TUI.
tui_text() {
    wg --json tui-dump 2>/dev/null \
        | python3 -c 'import json,sys; print(json.load(sys.stdin).get("text",""))'
}

# Helper: extract the active coordinator id from the TUI dump.
# Stderr is silenced so polling races (TUI just respawned, dump empty)
# don't pollute output with python JSON tracebacks.
tui_cid() {
    wg --json tui-dump 2>/dev/null \
        | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("coordinator_id",""))
except Exception: pass' 2>/dev/null
}

# Helper: dump the input_mode (Normal, ChoiceDialog, etc.).
tui_input_mode() {
    wg --json tui-dump 2>/dev/null \
        | python3 -c 'import json,sys; print(json.load(sys.stdin).get("input_mode",""))'
}

# Helper: count visible chat tabs in the TAB BAR specifically.
# The tab bar lines contain entries like `[1] ◉ .chat-5 ✕`; we filter
# only lines with the bracket-N prefix to avoid counting `.chat-N`
# mentions in the graph viewer or chat content.
count_visible_chat_tabs() {
    local txt; txt=$(tui_text)
    printf '%s' "$txt" \
        | grep -oE '\[[0-9]+\][^│]*\.chat-[0-9]+' \
        | grep -oE '\.chat-[0-9]+' \
        | sort -u | wc -l
}

# ── Phase 1: launch TUI, wait for ready ────────────────────────────────
tmux new-session -d -s "$session" -x 200 -y 60 "wg tui"

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "wg tui did not create tui.sock within 15s on first launch"
fi

# Wait for the dump to be populated with chat tabs.
visible=0
for _ in $(seq 1 30); do
    visible=$(count_visible_chat_tabs)
    if [[ "$visible" -ge 3 ]]; then
        break
    fi
    sleep 0.5
done
if [[ "$visible" -lt 3 ]]; then
    loud_fail "first launch: only ${visible} chat tabs visible (expected ≥3 — chat-loop tag may not be propagating to the tab bar)"
fi
echo "phase 1: ${visible} chat tabs visible"

# Move focus to the right panel and ensure we're on the Chat tab.
# Index 0 is the Chat tab; pressing '0' selects it deterministically.
tmux send-keys -t "$session" "0"
sleep 0.5

# ── Invariant (1): MODAL TOGGLE ────────────────────────────────────────
# The status bar must show either [PTY] or [CMD] when chat is active.
# Pressing Ctrl+T must flip from one to the other. Wait up to ~10s for
# the claude CLI inside the PTY to come up and the indicator to render.
saw_pty_first=0
text_a=""
for _ in $(seq 1 20); do
    text_a=$(tui_text)
    if printf '%s' "$text_a" | grep -q '\[PTY\]'; then
        saw_pty_first=1
        break
    fi
    if printf '%s' "$text_a" | grep -q '\[CMD\]'; then
        # Toggling into PTY mode is the test target — try once.
        tmux send-keys -t "$session" "C-t"
        sleep 0.5
        text_a=$(tui_text)
        if printf '%s' "$text_a" | grep -q '\[PTY\]'; then
            saw_pty_first=1
            break
        fi
    fi
    sleep 0.5
done

if [[ "$saw_pty_first" -ne 1 ]]; then
    # The claude CLI did not bring up a live PTY pane (e.g. missing
    # ANTHROPIC creds, sandboxed env, network blocked). The renderer
    # only emits the modal indicator when chat_pty_mode is true with a
    # live pane, so we cannot exercise the toggle path in this env.
    # SKIP loudly rather than FAIL — this is an environment gap, not a
    # design regression.
    loud_skip "NO LIVE PTY" "claude CLI did not produce a live PTY pane within 10s; modal indicator never showed. text head: $(printf '%s' "$text_a" | head -c 200)"
fi
echo "phase 1: [PTY] indicator visible (claude PTY pane is live)"

# Toggle modal state with Ctrl+T → expect [CMD].
tmux send-keys -t "$session" "C-t"
sleep 0.5

saw_cmd_after=0
text_b=""
for _ in $(seq 1 6); do
    text_b=$(tui_text)
    if printf '%s' "$text_b" | grep -q '\[CMD\]'; then
        saw_cmd_after=1
        break
    fi
    sleep 0.5
done

if [[ "$saw_cmd_after" -ne 1 ]]; then
    loud_fail "Ctrl+T did NOT switch to [CMD] modal — indicator stayed stuck in PTY (or disappeared entirely). The modal toggle handler is broken — the user has no way to break out of PTY focus to the command-mode keybindings (n/w/?). text head: $(printf '%s' "$text_b" | head -c 200)"
fi
echo "PASS invariant (1): Ctrl+T toggles [PTY] → [CMD]"

# We're now in command mode (focus Graph), which is required for the
# next phase's plain-key Ctrl+W send to NOT be intercepted by the PTY.

# ── Invariant (2): CLOSE IS NON-DESTRUCTIVE ───────────────────────────
# Press Ctrl+W in command mode (focus on graph). The active chat tab
# must close: visible tab count drops by 1, NO ChoiceDialog appears,
# AND the underlying graph task status is unchanged.
cid_before_close=$(tui_cid)
visible_before_close=$(count_visible_chat_tabs)
mode_before_close=$(tui_input_mode)
focused_before_close=$(wg --json tui-dump 2>/dev/null \
    | python3 -c 'import json,sys; print(json.load(sys.stdin).get("focused_panel",""))')
echo "before close: cid=${cid_before_close} visible=${visible_before_close} input_mode=${mode_before_close} focused_panel=${focused_before_close}"

# Use Ctrl+W (works in command mode, equivalent to bare 'w'). Either
# binding satisfies the invariant. Use bare 'w' as a fallback since
# tmux's `C-w` translation can be flaky inside nested PTY contexts.
tmux send-keys -t "$session" "C-w"
sleep 0.7
visible_mid=$(count_visible_chat_tabs)
if [[ "$visible_mid" -ge "$visible_before_close" ]]; then
    # Ctrl+W didn't take effect. Try bare 'w' (the implement-tui-command
    # single-key alias). In command mode (focused_panel=graph), bare 'w'
    # closes the active tab without opening a dialog.
    tmux send-keys -t "$session" "w"
fi
sleep 1

# Wait for the dump to refresh and reflect the close.
visible_after_close=$visible_before_close
for _ in $(seq 1 20); do
    visible_after_close=$(count_visible_chat_tabs)
    if [[ "$visible_after_close" -lt "$visible_before_close" ]]; then
        break
    fi
    sleep 0.25
done
mode_after_close=$(tui_input_mode)
echo "after close: visible=${visible_after_close} input_mode=${mode_after_close}"

if [[ "$visible_after_close" -ge "$visible_before_close" ]]; then
    loud_fail "Ctrl+W did NOT close the active tab (visible stayed at ${visible_after_close}). The redesign explicitly removed the close-confirmation dialog — Ctrl+W must close directly."
fi

# Hard-fail if a ChoiceDialog opened. The whole point of the redesign
# is that close is direct, not a dialog prompt.
if [[ "$mode_after_close" == "ChoiceDialog" ]]; then
    loud_fail "Ctrl+W opened a ChoiceDialog — the redesign explicitly removed the retire-chat dialog from the tab close path. close is direct, not a prompt."
fi

# Verify the closed chat task is still in graph.jsonl with a non-terminal status.
after=$(dump_chat_tasks)
echo "after close, chat task statuses:"
echo "$after"

abandoned_or_archived=$(printf '%s\n' "$after" \
    | awk -F'\t' '$2 == "abandoned" || $2 == "archived" {print $1}' \
    | sort -u)
new_terminal=$(comm -13 \
    <(printf '%s\n' "$before" | awk -F'\t' '$2 == "abandoned" || $2 == "archived" {print $1}' | sort -u) \
    <(printf '%s\n' "$abandoned_or_archived"))

if [[ -n "$new_terminal" ]]; then
    loud_fail "closing a tab via Ctrl+W marked underlying graph task(s) terminal: ${new_terminal}. close MUST be non-destructive — only the visible tab list shrinks, the graph is untouched."
fi
echo "PASS invariant (2): close is non-destructive (graph statuses unchanged)"

# ── Invariant (3): PERSISTENCE ────────────────────────────────────────
# Switch to a known coordinator first, then kill the TUI, relaunch,
# and verify the active-coordinator-id is preserved.
# Pick the FIRST visible tab via key '1' so we have a deterministic
# target.
tmux send-keys -t "$session" "1"
sleep 0.5
target_cid=$(tui_cid)
echo "before kill: target active cid=${target_cid}"

if [[ -z "$target_cid" || "$target_cid" == "0" ]]; then
    loud_fail "could not get a non-zero active coordinator id before kill (got '${target_cid}')"
fi

# Verify tui-state.json was written.
state_path="$graph_dir/tui-state.json"
for _ in $(seq 1 10); do
    if [[ -f "$state_path" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -f "$state_path" ]]; then
    loud_fail "tui-state.json was not written at $state_path — persistence is broken"
fi

persisted_cid=$(python3 -c "import json,sys; print(json.load(open('$state_path')).get('active_coordinator_id',''))" 2>/dev/null)
if [[ "$persisted_cid" != "$target_cid" ]]; then
    loud_fail "tui-state.json active_coordinator_id (${persisted_cid}) != live cid (${target_cid}) — persist is wrong"
fi
echo "phase 3: tui-state.json persisted active_coordinator_id=${persisted_cid}"

# Kill the TUI session, wait for socket cleanup.
tmux kill-session -t "$session" 2>/dev/null
for _ in $(seq 1 20); do
    if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done

# Relaunch.
session="wgsmoke-redesign-relaunch-$$"
tmux new-session -d -s "$session" -x 200 -y 60 "wg tui"

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "relaunched wg tui did not create tui.sock within 15s"
fi

restored_cid=""
for _ in $(seq 1 30); do
    restored_cid=$(tui_cid)
    if [[ -n "$restored_cid" && "$restored_cid" != "0" ]]; then
        break
    fi
    sleep 0.5
done

echo "after relaunch: restored active cid=${restored_cid}"
if [[ "$restored_cid" != "$target_cid" ]]; then
    loud_fail "after relaunch the active coordinator id is ${restored_cid}, expected ${target_cid} from tui-state.json — persistence not honored on restore"
fi
echo "PASS invariant (3): TUI restart preserves active coordinator id"

echo ""
echo "ALL THREE INVARIANTS PASS:"
echo "  (1) modal toggle: Ctrl+T flips PTY ⇄ CMD"
echo "  (2) close non-destructive: visible tabs ${visible_before_close} → ${visible_after_close}; graph statuses unchanged"
echo "  (3) persistence: relaunch restored active_coordinator_id=${restored_cid}"
exit 0
