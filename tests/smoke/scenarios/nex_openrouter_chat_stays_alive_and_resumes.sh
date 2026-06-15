#!/usr/bin/env bash
# Scenario: nex_openrouter_chat_stays_alive_and_resumes
#
# Regression lock for fix-nex-chat23-eof-resume.
#
# The user-reported failure: creating a Nex/OpenRouter chat (blank
# endpoint → OpenRouter default) from the TUI produced a chat that
# stopped almost immediately — the handler's trace showed
# `session_end reason=eof turns=0` ~21ms after `session_start`, and
# `wg chat resume <id>` then failed with the internal error
# "at least one of --executor or --model must be provided" even though
# the chat had saved executor/model metadata (and `wg chat resume`
# exposes no such flags).
#
# Root cause: a stale `.handler.release-requested` marker (left by a
# takeover handoff / written at a split-brain literal path) made the
# fresh handler's inbox reader return None on its first read, which the
# agent loop treats as EOF. Resume sent SetChatExecutor{None,None},
# tripping the hidden-flags guard.
#
# This scenario asserts, against a fully isolated daemon and WITHOUT any
# live LLM credentials (the inbox-driven handler blocks on the inbox and
# stays alive without ever calling the model):
#   1. After `wg chat create --exec nex -m openrouter:<model>` the handler
#      comes up LIVE and stays alive — no immediate session_end reason=eof.
#   2. `wg chat show` and `wg session status` agree there is a live handler.
#   3. No trace shows `"reason":"eof"` with `"turns":0` for the chat.
#   4. `wg chat resume <id>` succeeds (exit 0) and never prints the
#      hidden-flags error — it reconstructs executor/model from saved
#      metadata.
#   5. After resume the handler is live again.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x nex >init.log 2>&1; then
    loud_fail "wg init -x nex failed: $(tail -5 init.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
wgd="$WG_SMOKE_DAEMON_DIR"

# 1. Create a Nex/OpenRouter chat with a BLANK endpoint (OpenRouter default).
if ! wg --dir "$wgd" chat create --exec nex -m openrouter:minimax/minimax-m3 \
        --name smk --json >create.log 2>&1; then
    loud_fail "chat create failed: $(tail -5 create.log)"
fi
cid=$(grep -oE '"chat_id"[[:space:]]*:[[:space:]]*[0-9]+' create.log | grep -oE '[0-9]+$' | head -1)
if [[ -z "$cid" ]]; then
    loud_fail "could not parse chat_id from: $(cat create.log)"
fi

# 2. The handler must come up live within a few seconds and STAY alive —
#    this is the EOF regression. The inbox-driven handler blocks on the
#    inbox without calling the model, so no credentials are needed.
live=""
for _ in $(seq 1 20); do
    status_out=$(wg --dir "$wgd" session status "chat-$cid" 2>&1 || true)
    if grep -q "live pid=" <<<"$status_out"; then
        live="$status_out"
        break
    fi
    sleep 0.5
done
if [[ -z "$live" ]]; then
    loud_fail "handler never came up live for chat-$cid (EOF regression). \
session status: $(wg --dir "$wgd" session status "chat-$cid" 2>&1)
daemon.log tail:
$(tail -20 "$scratch/daemon.log" 2>/dev/null)"
fi

# It must remain alive (not exit at turns=0 a beat later).
sleep 2
status_out=$(wg --dir "$wgd" session status "chat-$cid" 2>&1 || true)
if ! grep -q "live pid=" <<<"$status_out"; then
    loud_fail "handler died shortly after launch (turns=0 EOF regression): $status_out"
fi

# 3. No trace may show an immediate EOF exit for this chat.
if find "$wgd/chat" -name 'trace.ndjson' 2>/dev/null | while read -r f; do
        grep -H '"reason":"eof"' "$f"
    done | grep -q .; then
    loud_fail "a chat trace shows session_end reason=eof — the handler EOF'd: \
$(find "$wgd/chat" -name 'trace.ndjson' -exec grep -l '"reason":"eof"' {} \;)"
fi

# 4. `wg chat show` must agree there's a live handler and NOT report stopped.
show_out=$(wg --dir "$wgd" chat show "$cid" 2>&1 || true)
if ! grep -qE 'handler[[:space:]]*:[[:space:]]*live pid=' <<<"$show_out"; then
    loud_fail "wg chat show does not report a live handler: $show_out"
fi
if grep -qE 'status[[:space:]]*:[[:space:]]*stopped' <<<"$show_out"; then
    loud_fail "wg chat show reports 'stopped' despite a live handler: $show_out"
fi

# 5. Resume must NOT error with the hidden-flags message and must exit 0.
#    First stop, then resume using saved metadata.
wg --dir "$wgd" chat stop "$cid" >stop.log 2>&1 || true
resume_out=$(wg --dir "$wgd" chat resume "$cid" 2>&1)
resume_rc=$?
if [[ "$resume_rc" -ne 0 ]]; then
    loud_fail "wg chat resume $cid exited $resume_rc: $resume_out"
fi
if grep -qiE 'at least one of --executor or --model' <<<"$resume_out"; then
    loud_fail "wg chat resume still hits the hidden-flags bug: $resume_out"
fi

# 6. After resume the handler must be live again.
live_after=""
for _ in $(seq 1 20); do
    status_out=$(wg --dir "$wgd" session status "chat-$cid" 2>&1 || true)
    if grep -q "live pid=" <<<"$status_out"; then
        live_after="$status_out"
        break
    fi
    sleep 0.5
done
if [[ -z "$live_after" ]]; then
    loud_fail "handler not live after resume: $(wg --dir "$wgd" session status "chat-$cid" 2>&1)"
fi

echo "PASS: nex/openrouter chat stayed alive (no turns=0 eof), show+status agree on a live handler, and resume worked from saved metadata"
exit 0
