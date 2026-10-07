#!/usr/bin/env bash
# Scenario: tui_dashboard_agent_usage_columns
#
# Pins tui-fleet-usage-columns: the TUI dashboard agent table gains a compact
# TOKENS/TURNS/TOOLS column derived from each agent's bounded raw_stream.jsonl
# tail by the SAME `stream_event::live_usage_for_agent` derivation the pi plugin
# uses (no second parser), and the Agency tab's agent list shows the full
# tokens/turns/tools label. A row with no usable stream degrades to a blank
# segment (never fabricated zeros) — pinned by the unit tests
# `dashboard_usage_cell_omits_unknown_metrics` and
# `dashboard_rows_carry_live_usage_from_raw_stream_fixture`.
#
# Live wg tui/tmux human flow:
#   1. Boot a synthetic .wg layout with one in-progress task assigned to a fake
#      `pi` agent whose raw_stream.jsonl carries a deterministic pi NDJSON tail.
#   2. Launch the CANDIDATE `wg tui` (this worktree's build) inside tmux.
#   3. Reach the cached Dashboard (agent table) the real way a human does:
#      click the ⌂ Workspace lane glyph twice to open Workspace actions, then
#      click "Dashboard".
#   4. Read the rendered cell grid back with `wg tui-dump` and assert the
#      TOKENS/TURNS/TOOLS header + the compact `1.2k 2t 31x` row segment.
#   5. Press '2' to open the Agency tab and assert the full usage label.
#
# Credential-free. Requires: tmux, python3, cargo (to build the candidate).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

repo_root="$(cd "$HERE/../../.." && pwd)"

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the interactive TUI"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to write the registry fixture"
fi

scratch=$(make_scratch)

# Resolve the CANDIDATE binary. The new columns exist only in current source, so
# a stale PATH `wg` (or a stale target/debug/wg) would false-negative; (re)build
# this worktree's binary.
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-}"
if [[ -z "$WG_BIN" ]]; then
    command -v cargo >/dev/null 2>&1 \
        || loud_skip "MISSING CARGO" "cargo not on PATH and no WG_SMOKE_CANDIDATE_BIN"
    target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
    WG_BIN="$target_dir/debug/wg"
    build_log="$scratch/cargo-build.log"
    if ! (cd "$repo_root" && cargo build --bin wg) >"$build_log" 2>&1; then
        loud_fail "cargo build --bin wg (candidate) failed:
$(tail -40 "$build_log")"
    fi
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg binary is not executable: $WG_BIN"

session="wgsmoke-tuiusage-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

# The scratch may sit below a live graph; pin every WG process to the fixture
# graph and a scratch-owned home.
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

add_log="$scratch/add.log"
if ! "$WG_BIN" --dir "$graph_dir" add "Live pi usage task" --id smoke-usage >"$add_log" 2>&1; then
    loud_fail "wg add failed during smoke setup: $(tail -5 "$add_log")"
fi

# Mark the task in-progress and assigned to agent-fake.
python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "smoke-usage":
        obj["status"] = "in-progress"
        obj["assigned"] = "agent-fake"
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

# Registry entry: a live `pi` agent (fresh heartbeat so the row classifies as
# active, not stuck). This is what the dashboard/Agency surfaces read to know
# which raw-stream dialect (`pi`) to parse.
mkdir -p "$graph_dir/service"
python3 - "$graph_dir/service/registry.json" <<'PY'
import datetime, json, sys
now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
reg = {
    "agents": {
        "agent-fake": {
            "id": "agent-fake",
            "pid": 12345,
            "task_id": "smoke-usage",
            "executor": "pi",
            "started_at": now,
            "last_heartbeat": now,
            "status": "working",
            "output_file": "output.log",
            "model": "pi:openrouter:test/fake",
        }
    },
    "next_agent_id": 2,
}
json.dump(reg, open(sys.argv[1], "w"))
PY

# Deterministic pi NDJSON tail: 2 turns of (600 in + 17 out) = 1234 total
# tokens, and 31 tool executions. The compact cell must read `1.2k 2t 31x` and
# the full label `1234 tokens (1200 in / 34 out) · 2 turns · 31 tool uses`.
mkdir -p "$graph_dir/agents/agent-fake"
{
    for _ in 1 2; do
        printf '%s\n' '{"type":"turn_end","message":{"usage":{"input":600,"output":17,"cost":{"total":0.01}}}}'
    done
    for _ in $(seq 1 31); do
        printf '%s\n' '{"type":"tool_execution_start","toolName":"bash"}'
    done
} >"$graph_dir/agents/agent-fake/raw_stream.jsonl"
: >"$graph_dir/agents/agent-fake/output.log"

# Launch the candidate TUI in tmux. Wide/tall so both tabs have room.
tui_err="$scratch/tui.err"
tmux new-session -d -s "$session" -x 200 -y 70 \
    "cd '$project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_USER=unknown '$WG_BIN' --dir '$graph_dir' tui 2>'$tui_err'; printf '%s\\n' \$? >'$scratch/tui.exit'"
tmux set-option -t "$session" mouse on
sleep 5

capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }

dump_fail() {
    loud_fail "$1
--- dump ---
$(cat "$2" 2>/dev/null)
--- tui.exit ---
$(cat "$scratch/tui.exit" 2>/dev/null || echo running)
--- tui.err ---
$(cat "$tui_err" 2>/dev/null)
--- pane ---
$(capture)"
}

# Locate a visible label in the live pane and click it with an SGR press/release
# — the exact terminal event stream tmux delivers to wg tui.
label_xy() {
    local needle=$1
    capture | python3 -c '
import sys
needle = sys.argv[1]
rows = sys.stdin.read().splitlines()
for y in range(len(rows)):
    x = rows[y].find(needle)
    if x >= 0:
        print(x + 1, y + 1)  # SGR coordinates are 1-based
        raise SystemExit(0)
raise SystemExit(1)
' "$needle"
}
mouse_click_label() {
    local needle=$1 xy x y
    xy=$(label_xy "$needle") || dump_fail "could not locate clickable '$needle'" "$scratch/tui.err"
    x=${xy% *}; y=${xy#* }
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$x" "$y")"
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$x" "$y")"
}

# Reach the cached agent Dashboard exactly as the symbolic context bar does:
# first ⌂ activates the Workspace lane (Activity), second ⌂ opens the Workspace
# actions dialog, then the "Dashboard" option opens the agent table.
mouse_click_label '⌂'
sleep 2
mouse_click_label '⌂'
sleep 2
mouse_click_label 'Dashboard'

dump_dash="$scratch/dump-dashboard.txt"
for _ in $(seq 1 20); do
    sleep 1
    "$WG_BIN" --dir "$graph_dir" tui-dump >"$dump_dash" 2>&1 || true
    grep -q '1.2k 2t 31x' "$dump_dash" && break
done

grep -q 'TOKENS TURNS TOOLS' "$dump_dash" \
    || dump_fail "dashboard header omitted the TOKENS/TURNS/TOOLS column" "$dump_dash"
grep -q '1.2k 2t 31x' "$dump_dash" \
    || dump_fail "dashboard agent row omitted the derived compact usage segment '1.2k 2t 31x'" "$dump_dash"

# '2' = Agency tab (index 2); no chat exists so plain digits switch tabs. Its
# agent list shows the full usage label.
tmux send-keys -t "$session" '2'
dump_agency="$scratch/dump-agency.txt"
for _ in $(seq 1 15); do
    sleep 1
    "$WG_BIN" --dir "$graph_dir" tui-dump >"$dump_agency" 2>&1 || true
    grep -q '1234 tokens (1200 in / 34 out)' "$dump_agency" && break
done

grep -q '1234 tokens (1200 in / 34 out)' "$dump_agency" \
    || dump_fail "agency agent header omitted the full token label" "$dump_agency"
grep -q '2 turns' "$dump_agency" \
    || dump_fail "agency agent header omitted the turn count" "$dump_agency"
grep -q '31 tool uses' "$dump_agency" \
    || dump_fail "agency agent header omitted the tool-use count" "$dump_agency"

echo "PASS: dashboard renders derived TOKENS/TURNS/TOOLS; Agency header shows full usage"
exit 0
