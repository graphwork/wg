#!/usr/bin/env bash
# Real human-flow regression for bound-async-tui-log-enrichment. A valid
# 101,955,000-byte active output.log must not delay the neutral shell, input,
# or publication of the independently-derived base graph.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is needed to generate the valid 100MiB log"

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"

python3 - "$scratch" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
wg = root / ".wg"
agent_dir = wg / "agents" / "agent-huge"
agent_dir.mkdir(parents=True, exist_ok=True)
(wg / "service").mkdir(parents=True, exist_ok=True)

task = {
    "kind": "task",
    "id": "huge-log-probe",
    "title": "Huge active log base graph marker",
    "status": "in-progress",
    "priority": 10,
    "assigned": "agent-huge",
    "created_at": "2026-07-14T00:00:00Z",
    "started_at": "2026-07-14T00:00:00Z",
    "last_interaction_at": "2026-07-14T00:00:00Z",
}
(wg / "graph.jsonl").write_text(json.dumps(task, separators=(",", ":")) + "\n")

output = agent_dir / "output.log"
registry = {
    "agents": {
        "agent-huge": {
            "id": "agent-huge",
            "pid": 1,
            "task_id": "huge-log-probe",
            "executor": "claude",
            "started_at": "2026-07-14T00:00:00Z",
            "last_heartbeat": "2026-07-14T00:00:00Z",
            "status": "working",
            "output_file": str(output),
            "model": "claude:opus",
        }
    },
    "next_agent_id": 2,
}
(wg / "service" / "registry.json").write_text(json.dumps(registry))

# Exactly the valid fixture that exposed the regression: 105,000 complete
# assistant NDJSON records at 971 bytes each = 101,955,000 bytes.
prefix = '{"type":"assistant","message":{"usage":{"input_tokens":3,"output_tokens":2},"content":[{"type":"text","text":"'
suffix = '"}]}}\n'
padding = "x" * (971 - len(prefix.encode()) - len(suffix.encode()))
record = (prefix + padding + suffix).encode()
assert len(record) == 971
with output.open("wb") as handle:
    for _ in range(105_000):
        handle.write(record)
assert output.stat().st_size == 101_955_000
PY

session="wgsmoke-large-active-log-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

capture() {
    tmux capture-pane -p -t "$session" 2>/dev/null || true
}

now_ms() {
    date +%s%3N
}

start_ms=$(now_ms)
tmux new-session -d -s "$session" -x 120 -y 40 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg tui --no-mouse --show-keys"

screen=""
for _ in $(seq 1 20); do
    screen=$(capture)
    printf '%s\n' "$screen" | grep -q "0 tasks" && break
    sleep 0.005
done
first_ms=$(( $(now_ms) - start_ms ))
printf '%s\n' "$screen" | grep -q "0 tasks" \
    || loud_fail "neutral first frame did not paint within 100ms with a valid 101,955,000-byte log"
(( first_ms < 100 )) \
    || loud_fail "neutral first frame ${first_ms}ms exceeded the 100ms ceiling"

key_start=$(now_ms)
tmux send-keys -t "$session" '?'
for _ in $(seq 1 20); do
    screen=$(capture)
    printf '%s\n' "$screen" | grep -q "Navigation" && break
    sleep 0.005
done
key_ms=$(( $(now_ms) - key_start ))
printf '%s\n' "$screen" | grep -q "Navigation" \
    || loud_fail "help input was starved by active-log enrichment"
(( key_ms < 100 )) \
    || loud_fail "help acknowledgement ${key_ms}ms exceeded the 100ms ceiling"
tmux send-keys -t "$session" Escape

published=0
for _ in $(seq 1 200); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q "huge-log-probe"; then
        published=1
        break
    fi
    sleep 0.01
done
publish_ms=$(( $(now_ms) - start_ms ))
(( published == 1 )) \
    || loud_fail "base graph did not publish within 2s while active-log enrichment was pending"
(( publish_ms < 2000 )) \
    || loud_fail "base graph publication ${publish_ms}ms exceeded the 2s local budget"

tmux send-keys -t "$session" q
echo "PASS: valid 101,955,000-byte active log stayed behind bounded enrichment pages (first=${first_ms}ms, key=${key_ms}ms, base=${publish_ms}ms)"
