#!/usr/bin/env bash
# Scenario: purge_chats_skips_active_by_default
#
# Pins the regression behind `wg-service-purge`:
#
#   "lol you archived _this_ chat too lol lol" — the user ran
#   `wg service purge-chats` to clean up 22 test chats spawned by a
#   runaway worker agent. The command obediently archived ALL of
#   them — including .chat-5 which was the chat the user was
#   actually using.
#
# Fix: `wg service purge-chats` skips chats considered "active"
# (recent consumer cursor activity, pending inbox traffic, or
# matching the calling shell's WG_CHAT_REF) by default. The legacy
# full-nuke is opt-in via --include-active.
#
# This scenario:
#   1. Builds a graph with three chat-loop tasks (.chat-5, .chat-6, .chat-7).
#   2. Marks .chat-5 as active by writing a fresh consumer cursor file.
#   3. Runs `wg service purge-chats` (daemon-offline path).
#   4. Asserts only .chat-5 retains its `chat-loop` tag — the other
#      two are archived.
#   5. Re-runs with --include-active and asserts .chat-5 is also
#      archived.
#
# No LLM credentials needed — pure CLI + graph mutation.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

wg_dir=$(graph_dir_in "$scratch") || loud_fail "no .wg/.workgraph dir under $scratch"
graph_path="$wg_dir/graph.jsonl"

# Append three chat-loop tasks directly to graph.jsonl. We use python to
# emit valid JSON — sticking to /usr/bin/env python3 keeps the dep
# footprint small (no jq required).
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 not on PATH; needed to mint graph rows"
fi

python3 - "$graph_path" <<'PY'
import json
import datetime
import sys

path = sys.argv[1]
ts = datetime.datetime.now(datetime.timezone.utc).isoformat()
with open(path, 'a') as f:
    for cid in (5, 6, 7):
        row = {
            "kind": "task",
            "id": f".chat-{cid}",
            "title": f"Chat {cid}",
            "status": "in-progress",
            "tags": ["chat-loop"],
            "created_at": ts,
        }
        f.write(json.dumps(row) + "\n")
PY

# Mark .chat-5 as active: a fresh consumer-cursor file is the
# user-attached signal `chat_session_is_idle` reads.
mkdir -p "$wg_dir/chat/5"
touch "$wg_dir/chat/5/.cursor"

# ── Default invocation: must skip the active chat ───────────────────
out=$(wg service purge-chats 2>&1)
rc=$?
if [[ "$rc" -ne 0 ]]; then
    loud_fail "wg service purge-chats exited rc=${rc}: $out"
fi

# Output assertions — the user must see what was skipped + the override hint.
if ! echo "$out" | grep -q 'skipped 1 active'; then
    loud_fail "expected 'skipped 1 active' in output, got: $out"
fi
if ! echo "$out" | grep -q '\.chat-5'; then
    loud_fail "expected '.chat-5' in skipped-active list, got: $out"
fi
if ! echo "$out" | grep -q 'include-active'; then
    loud_fail "expected '--include-active' override hint in output, got: $out"
fi

# Graph assertions — only .chat-5 keeps the chat-loop tag.
chat5_loop=$(grep -E '"id":"\.chat-5"' "$graph_path" | tail -1 | grep -c 'chat-loop' || true)
if [[ "$chat5_loop" -ne 1 ]]; then
    loud_fail "default purge archived the active chat (.chat-5 lost its chat-loop tag). graph row: $(grep -E '\"id\":\"\.chat-5\"' "$graph_path" | tail -1)"
fi

chat5_archived=$(grep -E '"id":"\.chat-5"' "$graph_path" | tail -1 | grep -c '"archived"' || true)
if [[ "$chat5_archived" -ne 0 ]]; then
    loud_fail "default purge tagged the active chat as archived. row: $(grep -E '\"id\":\"\.chat-5\"' "$graph_path" | tail -1)"
fi

for cid in 6 7; do
    archived=$(grep -E "\"id\":\"\\.chat-${cid}\"" "$graph_path" | tail -1 | grep -c '"archived"' || true)
    if [[ "$archived" -ne 1 ]]; then
        loud_fail "idle .chat-${cid} should be archived after default purge. row: $(grep -E "\"id\":\"\\.chat-${cid}\"" "$graph_path" | tail -1)"
    fi
done

# ── --include-active: must archive everything, including .chat-5 ────
out2=$(wg service purge-chats --include-active 2>&1)
rc2=$?
if [[ "$rc2" -ne 0 ]]; then
    loud_fail "wg service purge-chats --include-active exited rc=${rc2}: $out2"
fi

chat5_after_force=$(grep -E '"id":"\.chat-5"' "$graph_path" | tail -1 | grep -c '"archived"' || true)
if [[ "$chat5_after_force" -ne 1 ]]; then
    loud_fail "--include-active should archive .chat-5. row: $(grep -E '\"id\":\"\.chat-5\"' "$graph_path" | tail -1)"
fi

echo "PASS: purge-chats default skips active chat, --include-active overrides"
exit 0
