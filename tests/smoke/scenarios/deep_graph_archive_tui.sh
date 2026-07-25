#!/usr/bin/env bash
# Deep graph + archived-boundary + live TUI regression.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v timeout >/dev/null 2>&1 || loud_skip "MISSING TIMEOUT" "timeout is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
    export CARGO_TARGET_DIR="$scratch/candidate-target"
    (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
    WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
cat >"$G/config.toml" <<'TOML'
[guardrails]
max_task_depth = 2
max_child_tasks_per_agent = 10

[agency]
auto_place = false
auto_assign = false
auto_evaluate = false
flip_enabled = false
TOML

# Build a 1,001-node visible chain directly so the smoke measures graph
# algorithms rather than N sequential whole-file CLI rewrites.
python3 - "$G/graph.jsonl" <<'PY'
import json, sys
path=sys.argv[1]
with open(path, "w") as f:
    for i in range(1001):
        row={
            "kind":"task", "id":f"deep-{i:04d}", "title":f"Deep task {i}",
            "status":"open", "paused":False, "visibility":"internal",
            "after":([] if i == 0 else [f"deep-{i-1:04d}"]),
            "before":([] if i == 1000 else [f"deep-{i+1:04d}"]),
            "estimate":{"hours":1.0}
        }
        f.write(json.dumps(row,separators=(",",":"))+"\n")
PY

# The obsolete key loads but never constrains mutation; help/settings hide it,
# while read-only lint gives a clear migration path.
! "$WG_BIN" config --help | grep -q -- '--max-task-depth' || loud_fail "obsolete depth setting leaked into config help"
lint=$("$WG_BIN" --dir "$G" config lint --local)
grep -q 'guardrails.max_task_depth' <<<"$lint" || loud_fail "legacy depth key lacked lint notice: $lint"
shown=$("$WG_BIN" --dir "$G" config --show --local)
! grep -q 'max_task_depth' <<<"$shown" || loud_fail "obsolete depth key leaked into config show"
timeout 15 "$WG_BIN" --dir "$G" add "Deep mutation" --id deep-1001 --after deep-1000 >"$scratch/add.out"
grep -q 'deep-1001' "$G/graph.jsonl" || loud_fail "depth>1000 add was rejected"

# Iterative/bounded graph commands: readiness, critical path, impact,
# why-blocked, reset closure, serialization/reload, ASCII/2D/HTML derivation.
timeout 15 "$WG_BIN" --dir "$G" ready >"$scratch/ready.out"
grep -q 'deep-0000' "$scratch/ready.out" || loud_fail "deep readiness lost the root"
timeout 15 "$WG_BIN" --dir "$G" critical-path --json >"$scratch/critical.json"
python3 - "$scratch/critical.json" <<'PY' || loud_fail "critical path did not include the deep chain"
import json,sys
v=json.load(open(sys.argv[1]))
assert v["task_count"] == 1002, v["task_count"]
PY
timeout 15 "$WG_BIN" --dir "$G" impact deep-0000 --json >"$scratch/impact.json"
python3 - "$scratch/impact.json" <<'PY' || loud_fail "impact did not enumerate all descendants"
import json,sys
v=json.load(open(sys.argv[1]))
assert v["impact_summary"]["total_tasks_affected"] == 1001, v["impact_summary"]
PY
timeout 15 "$WG_BIN" --dir "$G" why-blocked deep-1001 --json >"$scratch/why.json"
python3 - "$scratch/why.json" <<'PY' || loud_fail "deep why-blocked was not stack-safe/complete"
import json,sys
v=json.load(open(sys.argv[1]))
assert v["total_blockers"] == 1001, v["total_blockers"]
chain=v["blocking_chain"]
assert chain.get("format") == "flat-deep-chain", chain.keys()
assert len(chain["nodes"]) == 1002, len(chain["nodes"])
PY
timeout 15 "$WG_BIN" --dir "$G" trace show deep-0000 --recursive >"$scratch/trace.out"
grep -q 'deep-1001' "$scratch/trace.out" || loud_fail "recursive trace omitted the deep tail"
python3 - "$scratch/trace.out" <<'PY' || loud_fail "recursive trace indentation was not bounded"
import sys
assert max(len(line.rstrip("\n")) for line in open(sys.argv[1])) < 256
PY
timeout 15 "$WG_BIN" --dir "$G" reset deep-0000 --direction forward --dry-run >"$scratch/reset.out" 2>&1
grep -q 'closure=1002 task(s)' "$scratch/reset.out" || loud_fail "deep reset closure was incomplete"
timeout 15 "$WG_BIN" --dir "$G" viz --all --json >"$scratch/viz.json"
python3 - "$scratch/viz.json" <<'PY' || loud_fail "ASCII viz hid tasks or grew quadratic indentation"
import json,sys
v=json.load(open(sys.argv[1]))
assert len(v["task_order"]) == 1002, len(v["task_order"])
assert "deep-0000" in v["node_lines"] and "deep-1001" in v["node_lines"]
assert max(map(len,v["text"].splitlines())) < 256
PY
timeout 15 "$WG_BIN" --dir "$G" viz --all --graph >"$scratch/spatial.txt"
grep -q 'deep-1001' "$scratch/spatial.txt" || loud_fail "2D graph derivation omitted deep tail"
timeout 20 "$WG_BIN" --dir "$G" html --out "$scratch/html" >/dev/null
grep -q 'deep-1001' "$scratch/html/index.html" || loud_fail "HTML derivation omitted deep tail"

# Cycles remain independently detectable and every renderer returns safely.
"$WG_BIN" --dir "$G" edit deep-0000 --add-after deep-1001 --allow-cycle >/dev/null
timeout 15 "$WG_BIN" --dir "$G" cycles >"$scratch/cycles.out"
grep -q 'deep-0000' "$scratch/cycles.out" || loud_fail "cycle validation lost the deep cycle"
timeout 15 "$WG_BIN" --dir "$G" viz --all --graph >"$scratch/cycle-spatial.txt"
"$WG_BIN" --dir "$G" edit deep-0000 --remove-after deep-1001 >/dev/null

# Real running-TUI flow: external insertion is refreshed asynchronously and
# leaves a discoverable pulse/search jump even though the tail is offscreen.
session="wg-deep-graph-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 300); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.025
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
tmux new-session -d -s "$session" -x 120 -y 32 \
    "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
wait_screen 'deep-0000' 'deep TUI did not render promptly'
timeout 15 "$WG_BIN" --dir "$G" add "Live deep tail" --id live-deep-tail --after deep-1001 >/dev/null
focus_consumed=false
for _ in $(seq 1 600); do
    if [[ ! -e "$G/.new_task_focus" ]]; then
        focus_consumed=true
        break
    fi
    sleep 0.025
done
[[ "$focus_consumed" == true ]] || loud_fail "running TUI did not consume the deep-task pulse/jump marker within 15s"
# Chat startup may own initial keyboard focus; a real click in the visible
# graph pane deterministically returns focus before the exact search jump.
tmux send-keys -t "$session" -l "$(printf '\033[<0;10;3M\033[<0;10;3m')"
sleep 0.3
tmux send-keys -t "$session" /
sleep 0.1
tmux send-keys -t "$session" -l 'live-deep-tail'
wait_screen '/live-deep-tail' 'graph search did not accept keyboard input after the live refresh'
wait_screen 'live-deep-tail  (open)' 'deep graph search did not produce an exact match promptly'
tmux send-keys -t "$session" Enter
jump_committed=false
for _ in $(seq 1 300); do
    screen=$(capture)
    if grep -Fq 'live-deep-tail  (open)' <<<"$screen" \
        && ! grep -Fq '/live-deep-tail' <<<"$screen"; then
        jump_committed=true
        break
    fi
    sleep 0.025
done
[[ "$jump_committed" == true ]] || loud_fail "exact TUI jump did not commit and retain the deep inserted task: $(capture | tr '\n' '|')"

# Archive a 600-task completed prefix through an active successor. The active
# file keeps compact boundary nodes and exact cut edges; viz collapses the old
# prefix to the single adjacent marker, and undo restores the original history.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[json.loads(x) for x in open(p) if x.strip()]
for row in rows:
    if row.get("kind") == "task" and row.get("id","").startswith("deep-"):
        try: i=int(row["id"].split("-")[1])
        except ValueError: continue
        if i < 600:
            row["status"]="done"
            row["completed_at"]="2024-01-01T00:00:00Z"
with open(p,"w") as f:
    for row in rows: f.write(json.dumps(row,separators=(",",":"))+"\n")
PY
mapfile -t prefix_ids < <(printf 'deep-%04d\n' $(seq 0 599))
timeout 20 "$WG_BIN" --dir "$G" archive "${prefix_ids[@]}" >"$scratch/archive.out"
python3 - "$G/graph.jsonl" <<'PY' || loud_fail "archive boundary/history invariant failed"
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
by={x["id"]:x for x in rows}
assert by["deep-0599"]["kind"] == "archivedboundary", by["deep-0599"]
assert by["deep-0600"]["after"] == ["deep-0599"], by["deep-0600"]
assert sum(x.get("kind") == "archivedboundary" for x in rows) == 600
PY
timeout 15 "$WG_BIN" --dir "$G" viz --all --no-tui >"$scratch/archived-viz.txt"
grep -q 'deep-0599.*archived boundary' "$scratch/archived-viz.txt" || loud_fail "adjacent archived boundary not shown"
! grep -q 'deep-0000' "$scratch/archived-viz.txt" || loud_fail "archived prefix was not collapsed from active view"
"$WG_BIN" --dir "$G" ready | grep -q 'deep-0600' || loud_fail "archived completed boundary blocked active successor"

# The already-running TUI refreshes to the induced view and can jump to its boundary.
tmux send-keys -t "$session" /
tmux send-keys -t "$session" -l 'deep-0599'
wait_screen '/deep-0599' 'running TUI did not accept archived-boundary search'
wait_screen 'archived boundary' 'running TUI did not expose archived boundary'
tmux send-keys -t "$session" Enter
boundary_committed=false
for _ in $(seq 1 300); do
    screen=$(capture)
    if grep -Fq 'archived boundary' <<<"$screen" \
        && ! grep -Fq '/deep-0599' <<<"$screen"; then
        boundary_committed=true
        break
    fi
    sleep 0.025
done
[[ "$boundary_committed" == true ]] || loud_fail "archived boundary jump did not commit: $(capture | tr '\n' '|')"

timeout 20 "$WG_BIN" --dir "$G" archive --undo >"$scratch/undo.out"
python3 - "$G/graph.jsonl" <<'PY' || loud_fail "archive undo did not restore exact chain edges"
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
by={x["id"]:x for x in rows}
assert not [x for x in rows if x.get("kind") == "archivedboundary"]
for i in range(600):
    row=by[f"deep-{i:04d}"]
    assert row.get("after",[]) == ([] if i == 0 else [f"deep-{i-1:04d}"]), row
    assert row.get("before",[]) == [f"deep-{i+1:04d}"], row
PY

echo "PASS: >1000-depth graph mutation/analysis/viz/TUI stayed bounded; archive boundaries preserved and restored exact history"
