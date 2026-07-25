#!/usr/bin/env bash
# Scenario: tui_log_scroll_controls
#
# Simulated-human regression for the per-task Log tab's Events view:
# keyboard PageUp/Up/End, mouse wheel, and scrollbar click/drag must move
# through overflowing event content without breaking live-tail behavior.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
    require_wg
    WG_BIN="$(command -v wg)"
fi
[[ -x "$WG_BIN" ]] || loud_fail "wg binary is not executable: $WG_BIN"

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive interactive TUI"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to mutate graph.jsonl and write raw stream fixtures"
fi

scratch=$(make_scratch)
session="wgsmoke-tuilog-scroll-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

project="$scratch/project"
graph_dir="$project/.wg"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$project" "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"

init_log="$scratch/init.log"
if ! "$WG_BIN" --dir "$graph_dir" init --no-agency >"$init_log" 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 "$init_log")"
fi
if [[ ! -f "$graph_dir/graph.jsonl" ]]; then
    loud_fail "could not locate the explicitly initialized graph at $graph_dir"
fi

add_log="$scratch/add.log"
if ! "$WG_BIN" --dir "$graph_dir" add "Scrollable live agent task" --id smoke-scroll >"$add_log" 2>&1; then
    loud_fail "wg add failed during smoke setup: $(tail -5 "$add_log")"
fi

python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "smoke-scroll":
        obj["status"] = "in-progress"
        obj["assigned"] = "agent-scroll"
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

agent_dir="$graph_dir/agents/agent-scroll"
mkdir -p "$agent_dir"
python3 - "$agent_dir/raw_stream.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
with open(path, "w") as f:
    f.write(json.dumps({"type": "system", "subtype": "init", "session_id": "scroll-smoke"}) + "\n")
    for i in range(180):
        marker = f"WG_LOG_SCROLL_EVENT_{i:03d}"
        f.write(json.dumps({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": marker}]}
        }) + "\n")
PY
: >"$agent_dir/output.log"

tmux new-session -d -s "$session" -x 200 -y 60 \
    "cd '$project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_USER=unknown '$WG_BIN' --dir '$graph_dir' tui"
sleep 4

# The isolated graph has no chat PTY, so startup is already in command mode.
# Open Log directly; Ctrl+O here would switch focus away from the graph row.
tmux send-keys -t "$session" 4
sleep 2

dump_screen() {
    local out="$1"
    if ! "$WG_BIN" --dir "$graph_dir" tui-dump >"$out" 2>&1; then
        loud_fail "wg tui-dump failed:\n$(cat "$out")"
    fi
}

assert_contains() {
    local file="$1"
    local marker="$2"
    local label="$3"
    if ! grep -q "$marker" "$file"; then
        loud_fail "$label: expected '$marker' in TUI dump.\nDump:\n$(cat "$file")"
    fi
}

assert_not_contains() {
    local file="$1"
    local marker="$2"
    local label="$3"
    if grep -q "$marker" "$file"; then
        loud_fail "$label: did not expect '$marker' in TUI dump.\nDump:\n$(cat "$file")"
    fi
}

assert_event_range() {
    local file="$1"
    local low="$2"
    local high="$3"
    local label="$4"
    local min_event max_event

    min_event=$(grep -oE 'WG_LOG_SCROLL_EVENT_[0-9]{3}' "$file" \
        | sed 's/.*_//' \
        | sort -n \
        | head -1)
    max_event=$(grep -oE 'WG_LOG_SCROLL_EVENT_[0-9]{3}' "$file" \
        | sed 's/.*_//' \
        | sort -n \
        | tail -1)

    if [[ -z "$min_event" || -z "$max_event" ]]; then
        loud_fail "$label: no visible WG_LOG_SCROLL_EVENT markers in TUI dump.\nDump:\n$(cat "$file")"
    fi

    min_event=$((10#$min_event))
    max_event=$((10#$max_event))
    if (( min_event < low || max_event > high )); then
        loud_fail "$label: expected visible event range within ${low}..${high}, got ${min_event}..${max_event}.\nDump:\n$(cat "$file")"
    fi
}

first_event_row() {
    python3 - "$1" <<'PY'
import sys
for idx, line in enumerate(open(sys.argv[1]).read().splitlines(), start=1):
    if "WG_LOG_SCROLL_EVENT_" in line:
        print(idx)
        raise SystemExit(0)
raise SystemExit(1)
PY
}

last_event_row() {
    python3 - "$1" <<'PY'
import sys
last = None
for idx, line in enumerate(open(sys.argv[1]).read().splitlines(), start=1):
    if "WG_LOG_SCROLL_EVENT_" in line or "WG_LOG_SCROLL_NO_YANK" in line:
        last = idx
if last is None:
    raise SystemExit(1)
print(last)
PY
}

scrollbar_x() {
    python3 - "$1" <<'PY'
import sys
for line in open(sys.argv[1]).read().splitlines():
    if "WG_LOG_SCROLL_EVENT_" not in line:
        continue
    idx = max(line.rfind(ch) for ch in "▲║█▼")
    if idx >= 0:
        print(idx + 1)
        raise SystemExit(0)
raise SystemExit(1)
PY
}

dump1="$scratch/dump-initial.txt"
dump_screen "$dump1"
assert_contains "$dump1" "WG_LOG_SCROLL_EVENT_179" "initial live-tail render"

# Keyboard: PageUp must jump by the viewport, Up must continue moving, End
# must return to live tail.
tmux send-keys -t "$session" PageUp
sleep 1
tmux send-keys -t "$session" Up
sleep 1
dump2="$scratch/dump-pageup.txt"
dump_screen "$dump2"
assert_event_range "$dump2" 100 170 "PageUp/Up keyboard scroll"
assert_not_contains "$dump2" "WG_LOG_SCROLL_EVENT_179" "PageUp/Up keyboard scroll"

tmux send-keys -t "$session" End
sleep 1
dump3="$scratch/dump-end.txt"
dump_screen "$dump3"
assert_contains "$dump3" "WG_LOG_SCROLL_EVENT_179" "End returns to live tail"

# Live-tail follows while pinned to bottom.
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"WG_LOG_SCROLL_LIVE_FOLLOW"}]}}\n' \
    >>"$agent_dir/raw_stream.jsonl"
sleep 3
dump4="$scratch/dump-live-follow.txt"
dump_screen "$dump4"
assert_contains "$dump4" "WG_LOG_SCROLL_LIVE_FOLLOW" "live-tail follow"

# After the user scrolls up, new content must not yank the viewport back down.
tmux send-keys -t "$session" PageUp
sleep 1
dump5="$scratch/dump-before-no-yank.txt"
dump_screen "$dump5"
assert_event_range "$dump5" 100 170 "scrolled-up anchor before append"

printf '{"type":"assistant","message":{"content":[{"type":"text","text":"WG_LOG_SCROLL_NO_YANK"}]}}\n' \
    >>"$agent_dir/raw_stream.jsonl"
sleep 3
dump6="$scratch/dump-no-yank.txt"
dump_screen "$dump6"
assert_event_range "$dump6" 100 170 "scrolled-up anchor after append"
assert_not_contains "$dump6" "WG_LOG_SCROLL_NO_YANK" "scrolled-up append should not auto-tail"

tmux send-keys -t "$session" End
sleep 1
dump7="$scratch/dump-no-yank-end.txt"
dump_screen "$dump7"
assert_contains "$dump7" "WG_LOG_SCROLL_NO_YANK" "End shows appended content"

# Mouse wheel over the log pane: send SGR mouse wheel reports into the TUI.
# Coordinates are 1-based terminal coordinates. Derive rows from the live
# dump so this stays stable across tmux/terminal layout differences.
wheel_row=$(first_event_row "$dump7") || loud_fail "could not derive wheel row from dump:\n$(cat "$dump7")"
for _ in $(seq 1 25); do
    tmux send-keys -t "$session" -l "$(printf '\033[<64;20;%sM' "$wheel_row")"
done
sleep 1
dump8="$scratch/dump-wheel-up.txt"
dump_screen "$dump8"
assert_event_range "$dump8" 40 140 "mouse wheel scroll up"
assert_not_contains "$dump8" "WG_LOG_SCROLL_NO_YANK" "mouse wheel scroll up"

wheel_row=$(first_event_row "$dump8") || loud_fail "could not derive wheel-down row from dump:\n$(cat "$dump8")"
for _ in $(seq 1 30); do
    tmux send-keys -t "$session" -l "$(printf '\033[<65;20;%sM' "$wheel_row")"
done
sleep 1
dump9="$scratch/dump-wheel-down.txt"
dump_screen "$dump9"
assert_contains "$dump9" "WG_LOG_SCROLL_NO_YANK" "mouse wheel scroll down to tail"

# Scrollbar click/drag: press near the top of the right-panel scrollbar,
# then drag near the bottom. SGR mouse encodes press=0, drag=32, release=m.
bar_x=$(scrollbar_x "$dump9") || loud_fail "could not derive scrollbar x from dump:\n$(cat "$dump9")"
bar_top_y=$(first_event_row "$dump9") || loud_fail "could not derive scrollbar top row from dump:\n$(cat "$dump9")"
bar_bottom_y=$(last_event_row "$dump9") || loud_fail "could not derive scrollbar bottom row from dump:\n$(cat "$dump9")"

tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$bar_x" "$bar_top_y")"
sleep 0.2
dump10="$scratch/dump-scrollbar-top.txt"
dump_screen "$dump10"
assert_event_range "$dump10" 0 80 "scrollbar click near top"

tmux send-keys -t "$session" -l "$(printf '\033[<32;%s;%sM' "$bar_x" "$bar_bottom_y")"
sleep 0.2
tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$bar_x" "$bar_bottom_y")"
sleep 1
dump11="$scratch/dump-scrollbar-bottom.txt"
dump_screen "$dump11"
assert_contains "$dump11" "WG_LOG_SCROLL_NO_YANK" "scrollbar drag near bottom"

echo "PASS: TUI Log/Events scroll controls work via keyboard, wheel, scrollbar, and live-tail"
exit 0
