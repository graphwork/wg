#!/usr/bin/env bash
# Real tmux/PTY regression for bound-tui-chat.
#
# A valid public chat_page_size=100000 must not publish the valid 100,000
# record / 97,100,000-byte history to Chat. Wait for the actual newest-history
# snapshot, send the real Help key, and require visible acknowledgement <100ms.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON" "python3 is needed to build the exact history fixture"

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg add "Bound chat projection probe" --id bound-chat-probe >add.log 2>&1 \
    || loud_fail "fixture task failed: $(tail -10 add.log)"

cat >>.wg/config.toml <<'TOML'

[tui]
chat_history = true
chat_history_max = 100000
chat_page_size = 100000
TOML

history=.wg/chat-history-0.jsonl
python3 - "$history" <<'PY'
import json
import os
import sys

path = sys.argv[1]
record_bytes = 971
records = 100_000
with open(path, "wb", buffering=1024 * 1024) as out:
    for index in range(records):
        marker = f"CHAT-SNAPSHOT-{index:05d}"
        obj = {
            "role": "user",
            "text": marker,
            "timestamp": "2026-07-14T00:00:00+00:00",
        }
        encoded = json.dumps(obj, separators=(",", ":")).encode()
        padding = record_bytes - 1 - len(encoded)
        if padding < 0:
            raise SystemExit("record template exceeds exact fixture width")
        # Put the marker at the end so it is visible in the bottom viewport of
        # the newest wrapped message after the asynchronous snapshot installs.
        obj["text"] = ("x" * padding) + marker
        encoded = json.dumps(obj, separators=(",", ":")).encode()
        if len(encoded) != record_bytes - 1:
            raise SystemExit(f"bad record width: {len(encoded)}")
        out.write(encoded + b"\n")

actual = os.path.getsize(path)
expected = records * record_bytes
if actual != expected:
    raise SystemExit(f"bad fixture size: {actual} != {expected}")
PY

[[ $(wc -l <"$history") -eq 100000 ]] \
    || loud_fail "history fixture does not contain 100,000 records"
[[ $(wc -c <"$history") -eq 97100000 ]] \
    || loud_fail "history fixture is not exactly 97,100,000 bytes"

session="wgsmoke-bounded-chat-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

tmux new-session -d -s "$session" -x 140 -y 45 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg tui --no-mouse --show-keys"

capture() {
    tmux capture-pane -p -t "$session" 2>/dev/null || true
}

# Do not measure the neutral shell. The regression appeared only after the
# asynchronous Chat snapshot installed, so wait for the newest persisted marker.
snapshot=0
for _ in $(seq 1 400); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'CHAT-SNAPSHOT-99999'; then
        snapshot=1
        break
    fi
    sleep 0.025
done
(( snapshot == 1 )) \
    || loud_fail "bounded Chat snapshot did not become visible within 10s; screen:\n$(capture)"

start_ns=$(date +%s%N)
deadline_ns=$(( start_ns + 100000000 ))
tmux send-keys -t "$session" '?'
acked=0
while (( $(date +%s%N) < deadline_ns )); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'Navigation'; then
        acked=1
        break
    fi
    sleep 0.002
done
elapsed_ms=$(( ($(date +%s%N) - start_ns) / 1000000 ))
(( acked == 1 )) \
    || loud_fail "Help was not acknowledged below 100ms after Chat snapshot (elapsed=${elapsed_ms}ms)"
(( elapsed_ms < 100 )) \
    || loud_fail "Help acknowledgement ${elapsed_ms}ms exceeded the 100ms hard limit"

# Static reachability audit: both interactive consumers assert the same hard
# projection, and all public page-size call sites flow through the clamp.
repo_root="$(cd "$HERE/../../.." && pwd)"
state="$repo_root/src/tui/viz_viewer/state.rs"
render="$repo_root/src/tui/viz_viewer/render.rs"
[[ $(rg -c 'bounded_chat_page_size\(' "$state") -ge 4 ]] \
    || loud_fail "not every config/CLI/pagination source uses bounded_chat_page_size"
rg -q 'CHAT_HISTORY_PAGE_MAX_RECORDS: usize = 200' "$state" \
    || loud_fail "200-record projection constant missing"
rg -q 'CHAT_HISTORY_PAGE_MAX_BYTES: usize = 1024 \* 1024' "$state" \
    || loud_fail "1 MiB projection constant missing"
rg -q 'pub fn update_chat_search.*' "$state" \
    || loud_fail "chat search entry point missing"
rg -q 'self\.chat\.enforce_history_projection\(\);' "$state" \
    || loud_fail "chat search/publication projection assertion missing"
rg -q 'app\.chat\.enforce_history_projection\(\);' "$render" \
    || loud_fail "Chat draw projection assertion missing"

# Close Help and the real TUI. The shared helper trap removes the exact 97.1MB
# fixture and session on success, failure, INT, and TERM.
tmux send-keys -t "$session" Escape q
for _ in $(seq 1 50); do
    tmux has-session -t "$session" 2>/dev/null || break
    sleep 0.01
done

printf 'PASS: valid 100000-record/97100000-byte Chat snapshot Help acknowledged in %sms (<100ms)\n' "$elapsed_ms"
