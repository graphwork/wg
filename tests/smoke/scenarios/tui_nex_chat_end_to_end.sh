#!/usr/bin/env bash
# Scenario: tui_nex_chat_end_to_end (smoke-tui-nex-end-to-end)
#
# Permanent simulated-human end-to-end smoke for nex chat in the TUI.
# Drives a real `wg tui` inside tmux, opens the new-chat launcher via
# the documented [CMD]-mode 'n' shortcut, fills the AddNew form for
# executor=nex / model=qwen3-coder / endpoint=lambda01, submits, then
# sends a chat message to the nex chat and asserts a real coordinator
# reply lands in the chat pane.
#
# This is the *user-visible* counterpart to integrate_nex_chat_end_to_end.sh
# (which exercises the same composition via the `wg chat ...` CLI). The
# CLI scenario verifies the IPC + supervisor wiring; this one verifies
# that the TUI's launcher + chat-input flow actually round-trips
# end-to-end without typing-time / dialog-time corruption against a
# live nex endpoint.
#
# Owners include every fix that this end-to-end exercise depends on:
#   * fix-nex-cursor-corruption  — typing into the launcher fields must
#     not be eaten by terminal cursor-block bytes.
#   * fix-supervisor-restart-backoff — clean handoff between launcher
#     submit and supervisor spawn must not trip the 10-min restart pause.
#   * fix-tui-supervisor-coexistence — TUI sentinel must not hold off
#     the supervisor spawn forever.
#   * fix-chat-dir-race — register_coordinator_session must mkdir before
#     IPC writers ENOENT on the very first message.
#   * integrate-nex-chat-end-to-end — `.chat-N` task ids strip the dot
#     for chat_ref so subprocess and IPC writers agree on the dir.
#   * smoke-tui-nex-end-to-end — owner of this scenario.
#   * design-nex-chat — design doc that pinned this as a permanent gate.
#
# Live-skip pattern: if the live endpoint is unreachable, loud_skip
# (exit 77). Pre-fix, this scenario FAILs (or LOUD_SKIPs) before the
# upstream fixes land.
#
# ── Implementation notes (read before changing this scenario) ──
#
# (a) The TUI auto-spawns a PTY pane for the default coordinator on
#     launch (claude/codex/native — anything that has an entry in
#     `maybe_auto_enable_chat_pty`). The PTY captures every keystroke
#     except Ctrl+T (the documented modal-toggle escape hatch). To
#     drive the launcher dialog, the smoke MUST first send Ctrl+T to
#     flip to [CMD] mode (focused_panel→Graph) so plain 'n' and the
#     subsequent navigation keys are interpreted by the TUI command
#     system rather than being typed into the embedded vendor CLI.
#
# (b) The TUI's create-coordinator IPC call does not pass `--json`
#     today, so the post-create JSON parsing in
#     `CommandEffect::CreateCoordinator` fails and `switch_coordinator`
#     is NOT called. The new chat is added to the tab bar but the
#     active coordinator stays at whatever it was before. The smoke
#     therefore presses the corresponding hotkey ([N+1]) to move the
#     active selection onto the new chat tab manually. If the auto-
#     switch is later wired up properly, the manual press becomes a
#     no-op.
#
# (c) The TUI's per-executor PTY spawn for the `native` branch reads
#     the GLOBAL default endpoint from `config.llm_endpoints`, not the
#     per-chat endpoint stored in `CoordinatorState`. To make the live
#     reply path work end-to-end, the smoke configures the global
#     default endpoint to lambda01 BEFORE launching the TUI; the
#     launcher's per-chat endpoint becomes a no-op for the TUI's nex
#     subprocess in that case but is still asserted on disk to pin
#     the launcher → graph wiring.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

ENDPOINT="${WG_LIVE_NEX_ENDPOINT:-https://lambda01.tail334fe6.ts.net:30000}"
MODEL="${WG_LIVE_NEX_MODEL:-qwen3-coder}"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

if ! endpoint_reachable "${ENDPOINT}/v1/models"; then
    loud_skip "NEX ENDPOINT UNREACHABLE" "${ENDPOINT}/v1/models did not respond — set WG_LIVE_NEX_ENDPOINT to a reachable host"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-nex-$$"
session_relaunch=""

kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    if [[ -n "${session_relaunch:-}" ]]; then
        tmux kill-session -t "$session_relaunch" 2>/dev/null || true
    fi
}
add_cleanup_hook kill_tmux_session
cd "$scratch"

# Project default: claude:opus. The chat we create via the launcher will
# specify executor=native + model=nex:<MODEL> + endpoint=ENDPOINT per-task.
if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init -m claude:opus failed: $(tail -5 init.log)"
fi

# Configure the GLOBAL default endpoint to lambda01. See implementation
# note (c) above: the TUI's `native` PTY spawn ignores the per-chat
# endpoint and falls back to the global default, so we must set it
# explicitly for the live reply path to work.
if ! wg config --endpoint "$ENDPOINT" >config.log 2>&1; then
    loud_fail "wg config --endpoint failed: $(tail -5 config.log)"
fi

# Boot the daemon so `wg tui` and the supervisor are live.
start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

# ── Helpers: read the JSON from the running TUI's tui-dump IPC ──────
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

tui_text()         { tui_field text; }
tui_input_mode()   { tui_field input_mode; }
tui_focused()      { tui_field focused_panel; }
tui_cid()          { tui_field coordinator_id; }
tui_active_tab()   { tui_field active_tab; }

count_visible_chat_tabs() {
    local txt; txt=$(tui_text)
    printf '%s' "$txt" \
        | grep -oE '\[[0-9]+\][^│]*\.chat-[0-9]+' \
        | grep -oE '\.chat-[0-9]+' \
        | sort -u | wc -l
}

wait_for_input_mode() {
    local target="$1" timeout_iters="${2:-20}"
    for _ in $(seq 1 "$timeout_iters"); do
        local cur; cur=$(tui_input_mode)
        if [[ "$cur" == "$target" ]]; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_chat_tab_count() {
    local want="$1" timeout_iters="${2:-60}"
    for _ in $(seq 1 "$timeout_iters"); do
        local n; n=$(count_visible_chat_tabs)
        if [[ "$n" -ge "$want" ]]; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

# ── Step 1: launch wg tui under tmux ─────────────────────────────────
tmux new-session -d -s "$session" -x 220 -y 60 "wg tui"

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "wg tui did not create tui.sock within 15s on first launch"
fi

# Wait for the dump to settle (Normal mode).
if ! wait_for_input_mode "Normal" 30; then
    loud_fail "TUI never reached InputMode::Normal after launch (got '$(tui_input_mode)')"
fi

# The TUI auto-creates a default chat on first run when the graph is
# empty (claude executor → claude PTY). Wait for the default chat tab
# to appear and the [PTY]/[CMD] indicator to render.
if ! wait_for_chat_tab_count 1 30; then
    loud_fail "default chat tab never appeared in the tab bar (auto-create regression?). text head: $(tui_text | head -c 400)"
fi
default_chat_count=$(count_visible_chat_tabs)
echo "phase 0: default chat auto-spawned (visible chat tabs: ${default_chat_count})"

# ── Step 2: escape PTY mode if needed, then open the launcher ──────────
# See implementation note (a): the PTY pane swallows every key except
# Ctrl+T. Send Ctrl+T to flip to [CMD] mode (focused_panel→Graph) so
# plain 'n' opens the launcher rather than being typed into the
# embedded claude CLI.
text_at_launch=$(tui_text)
if printf '%s' "$text_at_launch" | grep -q '\[PTY\]'; then
    tmux send-keys -t "$session" "C-t"
    sleep 0.5
    saw_cmd=0
    for _ in $(seq 1 12); do
        cur=$(tui_text)
        if printf '%s' "$cur" | grep -q '\[CMD\]'; then
            saw_cmd=1
            break
        fi
        sleep 0.25
    done
    if [[ "$saw_cmd" -ne 1 ]]; then
        # No [CMD] indicator means either the PTY pane was never live
        # (so we never were in [PTY] in the first place — fall through)
        # or Ctrl+T failed to toggle. Try once more, then fall through.
        tmux send-keys -t "$session" "C-t"
        sleep 0.5
    fi
fi

# 'n' from Graph focus opens the launcher (event.rs handle_graph_key
# Char('n') handler).
tmux send-keys -t "$session" "n"
if ! wait_for_input_mode "Launcher" 20; then
    loud_fail "pressing 'n' did NOT open the launcher (input_mode=$(tui_input_mode), focused=$(tui_focused)). modal: $(tui_text | head -1) — either the binding broke or PTY was still capturing keys."
fi
echo "phase 1: launcher dialog opened"

# ── Step 3: navigate to "+ Add new..." (Down x2 from default codex/claude
# presets), Enter to flip into AddNew mode (executor radio focused,
# idx=0=claude). Right x2 → idx=2=nex.
tmux send-keys -t "$session" "Down"
sleep 0.15
tmux send-keys -t "$session" "Down"
sleep 0.15
tmux send-keys -t "$session" "Enter"
sleep 0.4
tmux send-keys -t "$session" "Right"
sleep 0.1
tmux send-keys -t "$session" "Right"
sleep 0.2

# ── Step 4: Tab to model field, type the model name ────────────────────
tmux send-keys -t "$session" "Tab"
sleep 0.2
tmux send-keys -t "$session" -l "$MODEL"
sleep 0.3

# ── Step 5: Tab to endpoint field, type the endpoint URL ───────────────
tmux send-keys -t "$session" "Tab"
sleep 0.2
tmux send-keys -t "$session" -l "$ENDPOINT"
sleep 0.3

# Sanity: launcher dump should still be in Launcher mode and the model +
# endpoint should be present in the dumped text. This is the
# fix-nex-cursor-corruption regression bar — pre-fix, typed characters
# could be eaten by cursor-block bytes flowing in from the embedded
# vendor CLI's status line.
launcher_text=$(tui_text)
if [[ "$(tui_input_mode)" != "Launcher" ]]; then
    loud_fail "launcher dialog dismissed unexpectedly while filling form (input_mode=$(tui_input_mode))"
fi
if ! printf '%s' "$launcher_text" | grep -qF "$MODEL"; then
    loud_fail "model '$MODEL' did not appear in launcher text after typing — launcher key routing is broken (cursor-corruption regression?). text head: $(printf '%s' "$launcher_text" | head -c 500)"
fi
if ! printf '%s' "$launcher_text" | grep -qF "$ENDPOINT"; then
    loud_fail "endpoint '$ENDPOINT' did not appear in launcher text after typing. text head: $(printf '%s' "$launcher_text" | head -c 500)"
fi
echo "phase 2: AddNew form populated (executor=nex model=$MODEL endpoint=$ENDPOINT)"

# ── Step 6: Enter to submit ────────────────────────────────────────────
tmux send-keys -t "$session" "Enter"

# Wait for launcher to dismiss + a new chat tab to appear in the bar.
# Number of tabs should grow from default_chat_count to default+1.
target_tab_count=$((default_chat_count + 1))
if ! wait_for_chat_tab_count "$target_tab_count" 60; then
    loud_fail "after launcher submit, the new chat tab did not appear within 30s. tab count stayed at $(count_visible_chat_tabs); expected ≥ ${target_tab_count}. tab bar: $(tui_text | head -c 500)"
fi

# Resolve the new chat task id from the graph (the most-recently-created
# .chat-N task is the one the launcher just minted).
new_chat_id=$(grep -E '"id":"\.chat-[0-9]+"' "$graph_dir/graph.jsonl" \
    | python3 -c '
import json, sys
rows = []
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try: row = json.loads(line)
    except json.JSONDecodeError: continue
    if row.get("id","").startswith(".chat-") and row.get("model"):
        rows.append(row)
if rows:
    rows.sort(key=lambda r: r.get("created_at",""))
    print(rows[-1]["id"])
')
if [[ -z "$new_chat_id" ]]; then
    loud_fail "no .chat-N task with a model field found in graph.jsonl after launcher submit (the launcher's per-chat overrides did not land). graph tail: $(tail -2 "$graph_dir/graph.jsonl")"
fi
new_cid=${new_chat_id#.chat-}
echo "phase 3: launcher created ${new_chat_id} (cid=${new_cid})"

# Verify the created task carries the launcher's per-chat overrides.
new_chat_line=$(grep -F "\"id\":\"${new_chat_id}\"" "$graph_dir/graph.jsonl" | head -1)
if ! printf '%s' "$new_chat_line" | grep -qF "\"endpoint\":\"$ENDPOINT\""; then
    loud_fail "launcher-created chat ${new_chat_id} missing endpoint=$ENDPOINT (launcher → graph wiring broke). row: ${new_chat_line}"
fi
if ! printf '%s' "$new_chat_line" | grep -qE "\"model\":\"(nex:)?${MODEL}"; then
    loud_fail "launcher-created chat ${new_chat_id} missing model=${MODEL} (launcher → graph wiring broke). row: ${new_chat_line}"
fi
echo "phase 3: graph row carries endpoint + model overrides"

# ── Step 7: switch the TUI's active tab to the new chat ───────────────
# See implementation note (b): the TUI's create-coordinator IPC does
# not pass --json, so the post-create JSON parse fails and the auto-
# switch is skipped. Press the hotkey for the new tab ([N+1] in
# 1-indexed terms) to move the active coordinator manually. When the
# auto-switch is wired, this becomes a no-op.
hotkey_n=$((new_cid + 1))
if [[ "$hotkey_n" -ge 1 && "$hotkey_n" -le 9 ]]; then
    tmux send-keys -t "$session" "$hotkey_n"
fi
sleep 1.0
for _ in $(seq 1 30); do
    cur_cid=$(tui_cid)
    if [[ "$cur_cid" == "$new_cid" ]]; then
        break
    fi
    sleep 0.5
done
cur_cid=$(tui_cid)
if [[ "$cur_cid" != "$new_cid" ]]; then
    loud_fail "after pressing tab hotkey [${hotkey_n}], TUI active coordinator_id is '${cur_cid}', expected '${new_cid}' — chat-tab navigation is broken."
fi
echo "phase 4: TUI active coordinator switched to cid=${new_cid}"

# ── Step 8: send 'hello' as a chat message ─────────────────────────────
# The new chat is a `native` (nex) chat. The TUI auto-enables a PTY
# pane running `wg nex --resume chat-N -m <model> -e <endpoint>` —
# stdin keystrokes are forwarded to nex's rustyline. With the global
# default endpoint configured to lambda01 (see note c), nex talks to
# the live model and replies on stdout, which the TUI renders into the
# chat pane.
#
# Send Ctrl+T once to ensure we are in [PTY] mode (focused_panel=
# RightPanel) so keystrokes route to the nex subprocess.
text_pre_chat=$(tui_text)
if printf '%s' "$text_pre_chat" | grep -q '\[CMD\]'; then
    tmux send-keys -t "$session" "C-t"
    sleep 0.5
fi

# Allow the nex PTY pane to come up. The pane spawn is deferred to the
# next render path (see consume_pending_chat_pty_spawn) so we may need
# to wait a moment for the prompt to appear.
sleep 2

PROMPT="hello, please reply with the word ACK"
tmux send-keys -t "$session" -l "$PROMPT"
sleep 0.5
tmux send-keys -t "$session" "Enter"

# ── Step 9: wait for a coordinator response in the chat pane ──────────
# The pane text should grow with the model's reply within 60s. Capture
# the pane periodically and check for non-trivial content beyond the
# user prompt.
got_response=0
prompt_chars=$(printf '%s' "$PROMPT" | tr -d '[:space:]' | wc -c)
for _ in $(seq 1 120); do
    pane=$(tmux capture-pane -t "$session" -p 2>/dev/null || echo "")
    # Look for content past the prompt.
    post=$(printf '%s' "$pane" \
        | awk -v p="$PROMPT" 'index($0,p){found=1} found{print}' \
        | tr -d '[:space:]' | wc -c)
    if [[ -n "$pane" ]] && [[ "$post" -gt "$((prompt_chars + 5))" ]]; then
        # Reject "system-error" / "404" lines as fake responses.
        if ! printf '%s' "$pane" | grep -qiE 'role=system-error|status:.*404|HTTP/.*404'; then
            got_response=1
            break
        fi
    fi
    sleep 0.5
done

if [[ "$got_response" -ne 1 ]]; then
    loud_fail "no coordinator reply visible in the TUI chat pane within 60s. pane snapshot:
$(tmux capture-pane -t "$session" -p 2>/dev/null | tail -30)
daemon log tail:
$(grep -i 'Coordinator' "$graph_dir/service/daemon.log" 2>/dev/null | tail -10)"
fi
echo "phase 5: coordinator reply rendered in chat pane"

# ── Step 10: kill the TUI tmux session, verify persistence proof ──────
target_cid="$new_cid"
tmux kill-session -t "$session" 2>/dev/null
session=""
for _ in $(seq 1 30); do
    if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done

state_path="$graph_dir/tui-state.json"
if [[ ! -f "$state_path" ]]; then
    loud_fail "tui-state.json was not written at $state_path — persistence is broken"
fi
persisted_cid=$(python3 -c "import json,sys; print(json.load(open('$state_path')).get('active_coordinator_id',''))" 2>/dev/null)
if [[ "$persisted_cid" != "$target_cid" ]]; then
    loud_fail "tui-state.json active_coordinator_id (${persisted_cid}) != target cid (${target_cid}) — persistence is wrong"
fi
echo "phase 6: tui-state.json persisted active_coordinator_id=${persisted_cid}"

# ── Step 11: relaunch wg tui, assert resume worked ────────────────────
session_relaunch="wgsmoke-tui-nex-relaunch-$$"
tmux new-session -d -s "$session_relaunch" -x 220 -y 60 "wg tui"

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
    if [[ -n "$restored_cid" ]]; then
        break
    fi
    sleep 0.5
done
if [[ "$restored_cid" != "$target_cid" ]]; then
    loud_fail "after relaunch the active coordinator id is '${restored_cid}', expected '${target_cid}' — resume not honored"
fi

# Tab bar must still show the new chat tab.
relaunch_text=$(tui_text)
if ! printf '%s' "$relaunch_text" | grep -qF "${new_chat_id}"; then
    loud_fail "after relaunch, '${new_chat_id}' entry missing from tab bar. tab bar text: $(printf '%s' "$relaunch_text" | head -c 500)"
fi
echo "phase 7: relaunch restored active_coordinator_id=${restored_cid} and tab bar shows ${new_chat_id}"

echo ""
echo "ALL PHASES PASS:"
echo "  phase 1-2: launcher dialog → AddNew form → nex / $MODEL / $ENDPOINT typed"
echo "  phase 3:   submit → graph row '${new_chat_id}' carries per-chat overrides"
echo "  phase 4:   TUI active tab switched to ${new_chat_id} (cid=${new_cid})"
echo "  phase 5:   chat input → '${PROMPT}' → coordinator reply rendered in pane"
echo "  phase 6-7: kill TUI → relaunch → active_coordinator_id=${restored_cid}, tab bar restored"
exit 0
