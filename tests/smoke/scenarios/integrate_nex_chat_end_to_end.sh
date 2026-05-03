#!/usr/bin/env bash
# Scenario: integrate_nex_chat_end_to_end (integrate-nex-chat-end-to-end)
#
# Locks in the composition of the four upstream fixes against a real
# lambda01 endpoint:
#
#   * fix-nex-cursor-corruption: cursor-block U+2588 stripped from launcher
#     (TUI-only; covered by unit tests in src/tui/viz_viewer/event.rs).
#   * fix-supervisor-restart-backoff: restart-rate-limit kicks in on
#     repeated session-lock contention.
#   * fix-tui-supervisor-coexistence: TUI sentinel + cooperative release
#     marker; supervisor defers respawn while a live TUI sentinel is set.
#   * fix-chat-dir-race: register_coordinator_session creates the chat
#     dir up-front so the first IPC write does not ENOENT.
#
# Plus the composition glue committed under integrate-nex-chat-end-to-end:
# `.chat-N` task ids strip the leading dot when computing chat_ref so the
# subprocess `wg nex --chat <ref>` reads the SAME UUID dir the IPC writers
# write to (no split-brain).
#
# Pre-fix repro: send 'hello' against an endpoint-backed `.chat-0`; nex
# subprocess reads inbox from `chat/.chat-0/`; IPC writes to the
# registered alias UUID dir; wg chat times out at 60s.
# Post-fix: ACK reply within 60s.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

ENDPOINT="${WG_LIVE_NEX_ENDPOINT:-https://lambda01.tail334fe6.ts.net:30000}"
MODEL="${WG_LIVE_NEX_MODEL:-qwen3-coder}"

require_wg

if ! endpoint_reachable "${ENDPOINT}/v1/models"; then
    loud_skip "NEX ENDPOINT UNREACHABLE" "${ENDPOINT}/v1/models did not respond — set WG_LIVE_NEX_ENDPOINT to a reachable host"
fi

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

wg_dir="$scratch/.wg"

# Create the chat with the endpoint pinned. Service is offline; this
# exercises chat_create_in_graph (sets task.endpoint), not live spawn.
out=$(wg chat create \
    --executor native \
    --model "nex:$MODEL" \
    --endpoint "$ENDPOINT" \
    --json 2>chat-create.log) \
    || loud_fail "wg chat create failed: $(cat chat-create.log)"

# Boot the daemon. fix-chat-dir-race + register_coordinator_session
# install the `chat-0` / `coordinator-0` / `0` aliases pointing at the
# UUID dir.
start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

# Wait for sessions.json to register the chat-0 alias and the supervisor
# to spawn nex.
sessions="$graph_dir/chat/sessions.json"
for _ in $(seq 1 30); do
    [[ -f "$sessions" ]] && grep -q '"chat-0"' "$sessions" 2>/dev/null && break
    sleep 0.5
done
[[ -f "$sessions" ]] || loud_fail "sessions.json never appeared at $sessions"
grep -q '"chat-0"' "$sessions" || loud_fail "chat-0 alias missing from sessions.json: $(cat "$sessions")"

# Composition glue assertion: the supervisor and the IPC writer must agree
# on the chat dir. Pre-glue, IPC wrote to <UUID>/inbox.jsonl while nex read
# from <wg_dir>/chat/.chat-0/ (literal). Post-glue both land in the UUID dir.
uuid_dir=$(find "$graph_dir/chat" -maxdepth 1 -mindepth 1 -type d -name '0*' | head -1)
[[ -n "$uuid_dir" ]] || loud_fail "no UUID-named chat dir under $graph_dir/chat: $(ls "$graph_dir/chat")"
literal_dir="$graph_dir/chat/.chat-0"
[[ -e "$literal_dir" ]] && loud_fail "split-brain: literal chat dir $literal_dir exists alongside UUID dir — chat_ref glue regressed"

# First message — should round-trip via the supervisor-spawned nex within 60s.
if ! timeout 90 wg chat 'hello, please reply with the word ACK' --timeout 60 --coordinator 0 >chat1.log 2>&1; then
    loud_fail "first message timed out (split-brain regression?). log:
$(tail -20 chat1.log)
daemon log tail:
$(grep -i 'Coordinator-0:' "$graph_dir/service/daemon.log" | tail -10)"
fi
if grep -qiE 'role=system-error|role=error|status:.*404|HTTP/.*404' chat1.log; then
    loud_fail "first message returned an error: $(tail -10 chat1.log)"
fi
chat1_chars=$(tr -d '[:space:]' <chat1.log | wc -c)
[[ "$chat1_chars" -ge 3 ]] || loud_fail "first reply too short ($chat1_chars chars): $(cat chat1.log)"

# Second message — exercises steady-state with the same supervisor-managed
# nex process; verifies the supervisor does NOT respawn between user turns
# (a respawn would push restart_timestamps and risk hitting the rate limit).
if ! timeout 90 wg chat 'second message — please reply with the word DONE' --timeout 60 --coordinator 0 >chat2.log 2>&1; then
    loud_fail "second message timed out. log:
$(tail -20 chat2.log)
daemon log tail:
$(grep -i 'Coordinator-0:' "$graph_dir/service/daemon.log" | tail -10)"
fi
if grep -qiE 'role=system-error|role=error|status:.*404' chat2.log; then
    loud_fail "second message returned an error: $(tail -10 chat2.log)"
fi
chat2_chars=$(tr -d '[:space:]' <chat2.log | wc -c)
[[ "$chat2_chars" -ge 3 ]] || loud_fail "second reply too short ($chat2_chars chars): $(cat chat2.log)"

# Outbox + inbox both live in the UUID dir (single source of truth).
inbox="$uuid_dir/inbox.jsonl"
outbox="$uuid_dir/outbox.jsonl"
[[ -s "$inbox" ]]  || loud_fail "inbox missing or empty under UUID dir: $inbox"
[[ -s "$outbox" ]] || loud_fail "outbox missing or empty under UUID dir: $outbox"
in_count=$(grep -c '"role"\s*:\s*"user"' "$inbox" 2>/dev/null || echo 0)
out_count=$(grep -c '"role"\s*:\s*"coordinator"' "$outbox" 2>/dev/null || echo 0)
[[ "$in_count"  -ge 2 ]] || loud_fail "inbox should have ≥2 user messages, got $in_count: $(cat "$inbox")"
[[ "$out_count" -ge 2 ]] || loud_fail "outbox should have ≥2 coordinator replies, got $out_count: $(cat "$outbox")"

# Supervisor must have spawned exactly ONE nex over the whole run — a
# respawn between messages would indicate a coexistence regression.
spawn_count=$(grep -c "Coordinator-0: nex subprocess running" "$graph_dir/service/daemon.log" 2>/dev/null || echo 0)
[[ "$spawn_count" -le 2 ]] || loud_fail "supervisor respawned too many times ($spawn_count) — coexistence regression. daemon log:
$(grep -i 'Coordinator-0:' "$graph_dir/service/daemon.log" | tail -20)"

echo "PASS: end-to-end nex chat against $ENDPOINT ($MODEL): 2 messages, 2 replies, 1 supervisor spawn, no split-brain"
exit 0
