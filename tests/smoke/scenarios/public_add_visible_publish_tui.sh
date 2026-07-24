#!/usr/bin/env bash
# Live human-flow regression for the visible add -> explicit publish lifecycle.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

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
[agency]
auto_place = false
auto_assign = false
auto_evaluate = false
flip_enabled = false
TOML

session="wg-visible-add-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 240); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.025
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}

# Start the real TUI before the external writer exists.
tmux new-session -d -s "$session" -x 120 -y 32 \
    "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
wait_screen "⌁" "TUI did not become ready"

# A separate CLI adds ordinary work while ambient worker/chat identity exists.
# Neither variable may hide or release the task.
env WG_TASK_ID=missing-parent WG_AGENT_ID=agent-smoke \
    "$WG_BIN" --dir "$G" add "live-visible-exact-target" --id live-visible-exact-target \
    >"$scratch/add.out" 2>"$scratch/add.err"
grep -Fq "Task is paused (draft mode)" "$scratch/add.out" || loud_fail "add was not a draft: $(cat "$scratch/add.out")"
grep -Fq "wg publish live-visible-exact-target --only" "$scratch/add.out" \
    || loud_fail "add omitted explicit publish guidance: $(cat "$scratch/add.out")"

python3 - "$G/graph.jsonl" <<'PY' || loud_fail "draft is hidden or released"
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
t=next(x for x in rows if x.get("id")=="live-visible-exact-target")
assert t.get("paused") is True, t
assert t.get("unplaced", False) is False, t
assert t.get("status", "open").lower() == "open", t
assert not [x for x in rows if x.get("id", "").startswith((".assign-", ".flip-", ".evaluate-"))], rows
PY

# File watching must place the new row in the already-running TUI promptly.
wait_screen "live-visible-exact-target" "running TUI did not auto-refresh the external draft"

# Exercise exact search through the real key dispatcher, not a CLI substitute.
tmux send-keys -t "$session" /
tmux send-keys -t "$session" -l "live-visible-exact-target"
tmux send-keys -t "$session" Enter
wait_screen "live-visible-exact-target" "exact TUI search did not commit the new task"

# Add itself did not make work ready. Publish is the only release edge.
if "$WG_BIN" --dir "$G" ready | grep -Fq "live-visible-exact-target"; then
    loud_fail "draft became ready before publish"
fi
"$WG_BIN" --dir "$G" publish live-visible-exact-target --only >"$scratch/publish.out"
"$WG_BIN" --dir "$G" ready | grep -Fq "live-visible-exact-target" \
    || loud_fail "published task did not become ready"

# Root, explicit-independent, and agent-created work all remain ordinary visible drafts.
"$WG_BIN" --dir "$G" add "visible-root-fixture" --id visible-root-fixture >/dev/null
env WG_TASK_ID=live-visible-exact-target WG_AGENT_ID=worker-2 \
    "$WG_BIN" --dir "$G" add "visible-worker-child" --id visible-worker-child >/dev/null
env WG_TASK_ID=live-visible-exact-target WG_AGENT_ID=chat-2 \
    "$WG_BIN" --dir "$G" add "visible-independent" --id visible-independent --independent >/dev/null
python3 - "$G/graph.jsonl" <<'PY' || loud_fail "ambient identity created hidden work"
import json, sys
rows=[json.loads(x) for x in open(sys.argv[1]) if x.strip()]
by={x.get("id"):x for x in rows}
for i in ("visible-root-fixture", "visible-worker-child", "visible-independent"):
    t=by[i]
    assert t.get("paused") is True, t
    assert t.get("unplaced", False) is False, t
assert by["visible-worker-child"].get("after") == ["live-visible-exact-target"], by["visible-worker-child"]
assert by["visible-independent"].get("after", []) == [], by["visible-independent"]
PY
for id in live-visible-exact-target visible-root-fixture visible-worker-child visible-independent; do
    "$WG_BIN" --dir "$G" viz --all --no-tui | grep -Fq "$id" || loud_fail "$id missing from visible graph"
done

# Visibility survives normal lifecycle transitions and retry.
"$WG_BIN" --dir "$G" claim live-visible-exact-target >/dev/null
"$WG_BIN" --dir "$G" viz --all --no-tui | grep -Fq live-visible-exact-target || loud_fail "in-progress task hidden"
"$WG_BIN" --dir "$G" fail live-visible-exact-target --reason smoke-retry >/dev/null
"$WG_BIN" --dir "$G" retry live-visible-exact-target >/dev/null
"$WG_BIN" --dir "$G" claim live-visible-exact-target >/dev/null
"$WG_BIN" --dir "$G" done live-visible-exact-target >/dev/null
"$WG_BIN" --dir "$G" viz --all --no-tui | grep -Fq live-visible-exact-target || loud_fail "done task hidden from --all"

# The removed option is absent from help and hard-refuses without mutation.
! "$WG_BIN" add --help | grep -Fq -- "--no-place" || loud_fail "removed option leaked into help"
if "$WG_BIN" --dir "$G" add "must-not-exist" --no-place >"$scratch/refuse.out" 2>"$scratch/refuse.err"; then
    loud_fail "removed option was accepted"
fi
grep -Fq "wg publish 'must-not-exist' --only" "$scratch/refuse.err" || loud_fail "refusal lacked visible workflow"
! grep -Fq '"id":"must-not-exist"' "$G/graph.jsonl" || loud_fail "refused add mutated graph"

echo "PASS: external add stayed visible/draft, live TUI refreshed + exact-searched, and publish alone released work"
