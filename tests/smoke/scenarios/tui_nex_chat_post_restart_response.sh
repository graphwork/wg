#!/usr/bin/env bash
# Scenario: tui_nex_chat_post_restart_response (fix-nex-tui)
#
# Permanent regression lock for the user-reported "post-restart no
# response" bug. Verify-fix-nex caught this manually: a nex chat in
# the TUI accepted the first prompt and rendered the reply, accepted
# a second prompt and rendered the reply, but after the TUI was
# restarted the next prompt was visible in the reattached PTY pane
# and never received a model response in the rendered output —
# despite the inner tmux session having the response.
#
# Root cause (fixed in src/tui/pty_pane.rs reader thread): the
# reader thread advanced `bytes_processed` BEFORE feeding the chunk
# to the vt100 parser. The TUI event loop reads bytes_processed via
# `chat_pty_has_new_bytes()` and snapshots the watermark via
# `update_task_pane_byte_watermarks()` — when the watermark snapshot
# raced into the gap (counter advanced, parser not yet updated), the
# redraw rendered the pre-chunk screen and pinned the watermark to
# the new counter value. No further bytes arrive in the typical
# wg-nex single-token reply case, so no further redraws fire and the
# model's reply lives in the parser but never reaches the user's
# screen.
#
# This scenario drives the canonical 13-step user flow against the
# live lambda01/qwen3-coder endpoint and asserts:
#   * first prompt -> reply visible in rendered TUI pane
#   * second prompt (in same TUI session) -> reply visible
#   * TUI killed; chat tmux session persists
#   * TUI restarted; chat tab reattaches with prior conversation
#   * NEW prompt after restart -> reply visible in rendered TUI pane
#
# The pre-fix failure mode is the post-restart prompt with no
# rendered reply (the regression that blocked implement-generalize-
# chat).
#
# Live-skip pattern: if the lambda endpoint is unreachable, loud_skip
# (exit 77) so the gap is greppable.

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
session="wgsmoke-tui-nex-postrestart-$$"
session_relaunch="wgsmoke-tui-nex-postrestart-relaunch-$$"

kill_tmux_sessions() {
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux kill-session -t "$session_relaunch" 2>/dev/null || true
    # Also kill the chat-N tmux sessions so the next scenario starts
    # clean. The chat sessions outlive `wg tui` by design; only this
    # cleanup hook closes them at the end of the smoke.
    local project_tag
    project_tag=$(basename "$scratch" | tr '.:' '--')
    tmux ls 2>/dev/null \
        | awk -F: -v tag="wg-chat-${project_tag}-" '$1 ~ "^"tag {print $1}' \
        | while read -r s; do
            tmux kill-session -t "$s" 2>/dev/null || true
        done
}
add_cleanup_hook kill_tmux_sessions
cd "$scratch"

# Project default: claude:opus. The chat we create via the launcher will
# specify executor=native + model=qwen3-coder + endpoint=ENDPOINT per-task.
if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init -m claude:opus failed: $(tail -5 init.log)"
fi

# Configure the GLOBAL default endpoint to lambda01 (see implementation
# note (c) in tui_nex_chat_end_to_end.sh — TUI's native PTY spawn
# falls back to the global default endpoint, not the per-chat one).
if ! wg config --endpoint "$ENDPOINT" >config.log 2>&1; then
    loud_fail "wg config --endpoint failed: $(tail -5 config.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

# ── Helpers: read the JSON from the running TUI's tui-dump IPC ──────
tui_dump_text() {
    wg --json tui-dump 2>/dev/null \
        | python3 -c "
import json, sys
try: print(json.load(sys.stdin).get('text',''))
except Exception: pass
" 2>/dev/null
}

tui_dump_field() {
    local field="$1"
    wg --json tui-dump 2>/dev/null \
        | python3 -c "
import json, sys
try: print(json.load(sys.stdin).get('$field', ''))
except Exception: pass
" 2>/dev/null
}

tui_input_mode() { tui_dump_field input_mode; }
tui_focused()    { tui_dump_field focused_panel; }
tui_cid()        { tui_dump_field coordinator_id; }

count_visible_chat_tabs() {
    tui_dump_text \
        | grep -oE '\[[0-9]+\][^│]*\.chat-[0-9]+' \
        | grep -oE '\.chat-[0-9]+' \
        | sort -u | wc -l
}

wait_for_input_mode() {
    local target="$1" timeout_iters="${2:-30}"
    for _ in $(seq 1 "$timeout_iters"); do
        if [[ "$(tui_input_mode)" == "$target" ]]; then
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

# Poll the TUI's rendered text for a pattern within a deadline. The
# regression bar of fix-nex-tui is "the response shows up in the
# RENDERED pane," not "the response shows up somewhere in the pipeline."
# So this checks the actual rendered text via tui-dump.
wait_for_rendered_substring() {
    local pattern="$1" deadline_secs="${2:-60}"
    local end=$(( $(date +%s) + deadline_secs ))
    while [[ $(date +%s) -lt $end ]]; do
        local txt; txt=$(tui_dump_text)
        if printf '%s' "$txt" | grep -qF "$pattern"; then
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

if ! wait_for_input_mode "Normal" 30; then
    loud_fail "TUI never reached InputMode::Normal after launch (got '$(tui_input_mode)')"
fi

if ! wait_for_chat_tab_count 1 30; then
    loud_fail "default chat tab never appeared in tab bar"
fi
default_chat_count=$(count_visible_chat_tabs)
echo "phase 0: default chat auto-spawned (visible: ${default_chat_count})"

# ── Step 2: open the launcher (Ctrl+T to escape PTY mode, then 'n') ─
text_at_launch=$(tui_dump_text)
if printf '%s' "$text_at_launch" | grep -q '\[PTY\]'; then
    tmux send-keys -t "$session" "C-t"
    sleep 0.5
fi

tmux send-keys -t "$session" "n"
if ! wait_for_input_mode "Launcher" 20; then
    loud_fail "pressing 'n' did not open the launcher (input_mode=$(tui_input_mode), focused=$(tui_focused))"
fi
echo "phase 1: launcher dialog opened"

# ── Step 3: navigate to AddNew, pick nex executor ────────────────────
tmux send-keys -t "$session" "Down" "Down" "Enter"
sleep 0.4
tmux send-keys -t "$session" "Right" "Right"
sleep 0.3

# ── Step 4: model + endpoint ─────────────────────────────────────────
tmux send-keys -t "$session" "Tab"
sleep 0.2
tmux send-keys -t "$session" -l "$MODEL"
sleep 0.3
tmux send-keys -t "$session" "Tab"
sleep 0.2
tmux send-keys -t "$session" -l "$ENDPOINT"
sleep 0.3

# ── Step 5: Enter to submit ──────────────────────────────────────────
tmux send-keys -t "$session" "Enter"

target_tab_count=$((default_chat_count + 1))
if ! wait_for_chat_tab_count "$target_tab_count" 60; then
    loud_fail "after launcher submit, new chat tab did not appear within 30s"
fi

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
    loud_fail "no .chat-N task with model field found in graph.jsonl"
fi
new_cid=${new_chat_id#.chat-}
echo "phase 2-3: launcher created ${new_chat_id} (cid=${new_cid})"

# Compute the chat tmux session name eagerly — it follows
# `wg-chat-<sanitized-project>-chat-N` (see chat_id::chat_tmux_session_name
# in the binary, which replaces `.` and `:` with `-`). We need this for
# diagnostic captures from the FIRST prompt onward.
project_tag=$(basename "$scratch" | tr '.:' '--')
chat_tmux_session="wg-chat-${project_tag}-chat-${new_cid}"

# ── Step 6: switch the TUI's active tab to the new chat ──────────────
hotkey_n=$((new_cid + 1))
if [[ "$hotkey_n" -ge 1 && "$hotkey_n" -le 9 ]]; then
    tmux send-keys -t "$session" "$hotkey_n"
fi
sleep 1.0
for _ in $(seq 1 30); do
    if [[ "$(tui_cid)" == "$new_cid" ]]; then break; fi
    sleep 0.5
done
if [[ "$(tui_cid)" != "$new_cid" ]]; then
    loud_fail "after pressing tab hotkey [${hotkey_n}], TUI cid='$(tui_cid)', expected '${new_cid}'"
fi
echo "phase 4: TUI active coordinator switched to cid=${new_cid}"

# Toggle back into PTY mode so keystrokes route to the nex subprocess.
text_pre_chat=$(tui_dump_text)
if printf '%s' "$text_pre_chat" | grep -q '\[CMD\]'; then
    tmux send-keys -t "$session" "C-t"
    sleep 0.5
fi

# Allow the nex PTY pane to come up.
sleep 2

# Helper: assert that the rendered TUI pane has new content past
# `marker` within `deadline_secs`. Returns 0 on success, 1 on
# timeout. This is the regression bar — pre-fix the bytes flow into
# the inner tmux session but never reach the rendered TUI pane.
assert_rendered_reply_after() {
    local marker="$1" deadline_secs="${2:-60}" min_chars="${3:-3}"
    local end=$(( $(date +%s) + deadline_secs ))
    while [[ $(date +%s) -lt $end ]]; do
        local txt; txt=$(tui_dump_text)
        # Skip until we find the marker, then count non-trivial chars
        # in the lines that follow (ignoring rendered chrome).
        local after
        after=$(printf '%s' "$txt" \
            | awk -v p="$marker" '
                index($0, p) { found=1; next }
                found {
                    gsub(/^[ \t│]+|[ \t│]+$/, "")
                    if ($0 != "" \
                        && $0 !~ /^\[PTY\]/ \
                        && $0 !~ /^>[[:space:]]*$/ \
                        && $0 !~ /^─/ \
                        && $0 !~ /^└/ \
                        && $0 !~ /^Ctrl\+/) {
                        print
                    }
                }
            ' \
            | tr -d '[:space:]│' | wc -c)
        if [[ "$after" -ge "$min_chars" ]]; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

# ── Step 7: send the FIRST prompt; assert reply renders ──────────────
# The first prompt elicits a SHORT response. Pre-fix, qwen3-coder's
# few-token reply ("OK") arrived in 1-2 chunks via the reader thread,
# and the byte-counter race could pin the watermark past those chunks
# before the parser ingested them — the model reply lived in the
# parser but never reached the rendered screen. We pick prompts that
# elicit short replies because those expose the race; long streamed
# replies are robust because each new token re-fires the redraw.
PROMPT1="Reply with only the word OK and nothing else."
tmux send-keys -t "$session" -l "$PROMPT1"
sleep 0.3
tmux send-keys -t "$session" "Enter"

if ! assert_rendered_reply_after "and nothing else" 60 2; then
    loud_fail "first prompt: rendered TUI pane never showed a reply within 60s. \
Pane:
$(tmux capture-pane -t "$session" -p 2>/dev/null | tail -25)
Inner chat session:
$(tmux capture-pane -t "$chat_tmux_session" -p 2>/dev/null | tail -10)"
fi
echo "phase 5: first prompt -> rendered reply visible"

# ── Step 8: send the SECOND prompt; assert reply renders ─────────────
# This is the regression bar that pre-fix shows in the inner tmux
# session but NOT in the rendered TUI pane. We pick a single-character
# response because that's the worst case for the byte-counter race —
# the entire reply arrives in one chunk and triggers no follow-up
# bytes that would otherwise mask the missed redraw.
PROMPT2="What is the third character of NEX2090? Reply with only that single character and nothing else."
tmux send-keys -t "$session" -l "$PROMPT2"
sleep 0.3
tmux send-keys -t "$session" "Enter"

if ! assert_rendered_reply_after "single character and nothing else" 60 1; then
    loud_fail "second prompt: rendered TUI pane never showed a reply within 60s — \
this is the pty_pane reader-thread byte-counter race (fix-nex-tui).
Rendered pane:
$(tmux capture-pane -t "$session" -p 2>/dev/null | tail -25)
Inner chat session:
$(tmux capture-pane -t "$chat_tmux_session" -p 2>/dev/null | tail -10)"
fi
echo "phase 6: second prompt -> rendered short reply visible (pre-restart byte-race fix verified)"

# ── Step 9-10: kill the TUI and confirm the chat tmux session persists ──
target_cid="$new_cid"

tmux kill-session -t "$session" 2>/dev/null
session=""
for _ in $(seq 1 30); do
    if [[ ! -S "$graph_dir/service/tui.sock" ]]; then break; fi
    sleep 0.5
done

if ! tmux has-session -t "$chat_tmux_session" 2>/dev/null; then
    loud_fail "after TUI exit, chat tmux session '$chat_tmux_session' was killed — \
persistence design broken (kill_underlying_session must NOT fire on TUI quit)"
fi
echo "phase 7: TUI killed; chat tmux session '$chat_tmux_session' persisted"

# ── Step 11: relaunch wg tui, assert reattach + history visible ──────
tmux new-session -d -s "$session_relaunch" -x 220 -y 60 "wg tui"

for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then break; fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "relaunched wg tui did not create tui.sock within 15s"
fi

restored_cid=""
for _ in $(seq 1 30); do
    restored_cid=$(tui_cid)
    if [[ "$restored_cid" == "$target_cid" ]]; then break; fi
    sleep 0.5
done
if [[ "$restored_cid" != "$target_cid" ]]; then
    loud_fail "after relaunch, active cid='$restored_cid', expected '$target_cid' — resume not honored"
fi

# Wait for prior conversation to be visible in rendered output.
if ! wait_for_rendered_substring "NEX2090" 30; then
    loud_fail "after relaunch, prior conversation marker (NEX2090 token) not visible in rendered pane within 30s. \
Pane: $(tmux capture-pane -t \"$session_relaunch\" -p 2>/dev/null | tail -25)"
fi
echo "phase 8: TUI relaunched, cid=${restored_cid}, prior conversation visible"

# ── Step 12-13: send a NEW prompt after restart; assert reply renders ──
# This is the canonical fix-nex-tui regression bar from verify-fix-nex.
# Pre-fix: the rendered TUI pane never showed the model reply for the
# post-restart prompt (even after 90s). The bytes flowed through tmux
# into the inner pane, but the TUI's pty reader-thread byte-counter
# race meant the watermark advanced past the response without firing
# a redraw.
PROMPT3="One last question: spell back to me the token I asked you to remember."
tmux send-keys -t "$session_relaunch" -l "$PROMPT3"
sleep 0.3
tmux send-keys -t "$session_relaunch" "Enter"

# Look for "NEXTUI23" anywhere in rendered text after the third prompt.
post_restart_marker="spell back to me"
deadline=$(( $(date +%s) + 90 ))
got_post_restart_reply=0
while [[ $(date +%s) -lt $deadline ]]; do
    txt=$(tui_dump_text)
    # Either the token itself, or substantial content past the prompt.
    if printf '%s' "$txt" | awk -v p="$post_restart_marker" '
            index($0, p) { found=1; next }
            found && /NEXTUI23/ { hit=1 }
            END { exit (hit ? 0 : 1) }'; then
        got_post_restart_reply=1
        break
    fi
    # Fallback: any 12+ non-trivial characters after the marker counts.
    after=$(printf '%s' "$txt" \
        | awk -v p="$post_restart_marker" '
            index($0, p) { found=1; next }
            found {
                gsub(/^[ \t│]+|[ \t│]+$/, "")
                if ($0 != "" && $0 !~ /^\[PTY\]/ && $0 !~ /^>[[:space:]]*$/ && $0 !~ /^─/ && $0 !~ /^└/) {
                    print
                }
            }
        ' \
        | tr -d '[:space:]│' | wc -c)
    if [[ "$after" -ge 12 ]]; then
        got_post_restart_reply=1
        break
    fi
    sleep 0.5
done

if [[ "$got_post_restart_reply" -ne 1 ]]; then
    loud_fail "POST-RESTART prompt: rendered TUI pane never showed a reply beyond '$post_restart_marker' within 90s — \
this is the exact fix-nex-tui regression. Rendered pane:
$(tmux capture-pane -t "$session_relaunch" -p 2>/dev/null | tail -25)
Inner chat session:
$(tmux capture-pane -t "$chat_tmux_session" -p 2>/dev/null | tail -10)"
fi
echo "phase 9: post-restart prompt -> rendered reply visible (fix-nex-tui regression closed)"

echo ""
echo "ALL PHASES PASS:"
echo "  phase 0-4: launcher dialog → AddNew form → submit → switch tab"
echo "  phase 5:   first prompt → rendered reply"
echo "  phase 6:   second prompt → rendered reply (pre-restart byte-race)"
echo "  phase 7:   TUI killed; chat tmux session '$chat_tmux_session' persisted"
echo "  phase 8:   TUI relaunched, prior conversation visible"
echo "  phase 9:   post-restart prompt → rendered reply (fix-nex-tui)"
exit 0
