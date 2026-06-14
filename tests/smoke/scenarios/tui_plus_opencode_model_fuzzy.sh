#!/usr/bin/env bash
# Scenario: tui_plus_opencode_model_fuzzy (add-model-fuzzy)
#
# Confirms the Model field's fuzzy autocomplete works through the REAL
# `wg tui` `+` menu for the OpenCode/OpenRouter path: the user does NOT type
# the exact route, they fuzzy-search `minimax m3` (two terms, a space, NO
# slash) and ACCEPT the highlighted suggestion. The launcher must normalize
# the accepted model into OpenCode's `openrouter/<vendor>/<model>` route and
# persist it into `.chat-N` + CoordinatorState + the generated argv.
#
# This is the autocomplete sibling of tui_plus_opencode_minimax.sh (which
# pins the raw free-text route path). Here the model arrives via fuzzy
# search, not exact typing.
#
# Flow:
#   1. Scratch WG project with a custom-command `cat` chat (so a PTY is
#      focused — the state where the `+` launcher class lives).
#   2. Seed a DECOY recent launcher-history combo (a different model) so a
#      "recall the last model" regression would be caught.
#   3. `wg tui`, `+`, Add-new, select opencode.
#   4. Tab to the Model field, type the fuzzy query `minimax m3` (a SEARCH
#      fragment — space-separated, no `/`), confirm the dropdown surfaces
#      `minimax/minimax-m3`, then Enter to ACCEPT + launch.
#   5. Assert `.chat-1`:
#        - executor_preset_name == opencode
#        - model preserves the minimax route (normalized openrouter route)
#        - NO endpoint override
#        - command_argv == ["opencode","--model","openrouter/minimax/minimax-m3"]
#      and CoordinatorState mirrors it.
#
# Credential-free: only persisted metadata + argv are asserted; no real
# OpenRouter / opencode call is made.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-oc-fuzzy-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

# Decoy recent launcher history: a DIFFERENT model than the one we fuzzy
# select below. A regression that submitted a "recent" model instead of the
# fuzzy-accepted suggestion would be caught by the exact-match asserts.
history_path="$fake_home/launcher-history.jsonl"
DECOY_MODEL="openrouter/stale-recent-DECOY/model-z"
printf '{"timestamp":"2026-01-01T00:00:00+00:00","executor":"opencode","model":"%s","source":"tui"}\n' \
    "$DECOY_MODEL" >"$history_path"

project="$scratch/project"
mkdir -p "$project"
cd "$project"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" WG_GLOBAL_DIR="$fake_global" \
        WG_LAUNCHER_HISTORY_PATH="$history_path" \
        wg "$@"
}

if ! run_wg init --no-agency -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

if ! run_wg chat new --name base --command cat --json >base-chat.json 2>&1; then
    loud_fail "creating base cat chat failed: $(cat base-chat.json)"
fi

screen_text() {
    tmux capture-pane -t "$session" -p -S -80 2>/dev/null || true
}

wait_screen_contains() {
    local needle="$1"
    local label="$2"
    local text=""
    for _ in $(seq 1 80); do
        text="$(screen_text)"
        if grep -qF "$needle" <<<"$text"; then
            return 0
        fi
        sleep 0.25
    done
    loud_fail "TUI screen never showed ${label} ('$needle'). Last screen:\n$text"
}

tmux new-session -d -s "$session" -x 180 -y 50 \
    "cd '$project' && env HOME='$fake_home' XDG_CONFIG_HOME='$fake_home/.config' WG_GLOBAL_DIR='$fake_global' WG_LAUNCHER_HISTORY_PATH='$history_path' wg tui"

wait_screen_contains "[PTY]" "focused base chat PTY"
wait_screen_contains ".chat-0" "base chat tab"

# `+` opens the launcher even while the chat PTY owns focus.
tmux send-keys -t "$session" "+"
wait_screen_contains "+ Add new" "default launcher Add-new row"

tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Enter
wait_screen_contains "Executor:" "Add-new executor row"

# opencode is the 3rd executor choice (claude, codex, opencode, nex): Right x2.
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ opencode" "selected opencode executor"

# Tab to the Model field, then type the FUZZY query (space-separated, no
# slash — a search fragment, not the exact route). The dropdown must surface
# the curated minimax route.
tmux send-keys -t "$session" Tab
sleep 0.2
tmux send-keys -t "$session" -l "minimax m3"
sleep 0.3
wait_screen_contains "minimax/minimax-m3" "model fuzzy dropdown surfaces minimax route"

# Enter accepts the highlighted suggestion and launches in one key.
tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$DECOY_MODEL" <<'PY'
import json
import sys
from pathlib import Path

decoy_model = sys.argv[1]
EXPECTED_ARG = "openrouter/minimax/minimax-m3"

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "opencode":
    raise SystemExit(f"expected executor_preset_name=opencode, got {chat.get('executor_preset_name')!r}: {chat}")

# The fuzzy-accepted model must preserve the minimax route — never the
# recent decoy or a default fallback.
model = chat.get("model") or ""
if "minimax/minimax-m3" not in model:
    raise SystemExit(f"chat model must preserve the fuzzy-selected minimax route, got {model!r}: {chat}")
if "DECOY" in model or "stale" in model:
    raise SystemExit(f"chat model leaked the recent-history decoy {decoy_model!r}: {chat}")

if "endpoint" in chat or "endpoint_override" in chat:
    raise SystemExit(f"opencode chat must not persist an endpoint override in graph row: {chat}")

argv = chat.get("command_argv") or []
expected = ["opencode", "--model", EXPECTED_ARG]
if argv != expected:
    raise SystemExit(
        f"expected normalized OpenCode argv {expected!r}, got {argv!r}. A fuzzy-"
        f"selected minimax route must launch as the openrouter route, not a "
        f"default/recent model: {chat}"
    )
joined = " ".join(argv).lower()
for bad in ("nano", "banana", "decoy", "stale", "gpt", "claude"):
    if bad in joined:
        raise SystemExit(f"argv contains an unexpected fallback/decoy token {bad!r}: {argv}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "opencode":
    raise SystemExit(f"expected coordinator executor_override=opencode, got {state!r}")
sm = state.get("model_override") or ""
if "minimax/minimax-m3" not in sm:
    raise SystemExit(f"coordinator model_override must preserve the minimax route, got {sm!r}: {state}")
if state.get("endpoint_override") not in (None, ""):
    raise SystemExit(f"opencode coordinator state must not carry endpoint_override: {state!r}")

print(
    "OK: .chat-1 + CoordinatorState carry the fuzzy-selected minimax route; "
    "argv = " + " ".join(argv)
)
PY
then
    loud_fail "opencode model-fuzzy launcher assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu model fuzzy search ('minimax m3') selects minimax/minimax-m3 for opencode, normalizing to openrouter/minimax/minimax-m3 (not the recent decoy)"
exit 0
