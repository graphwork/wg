#!/usr/bin/env bash
# Scenario: tui_plus_opencode_minimax (fix-tui-opencode)
#
# Pins the user-reported regression: launching an OpenCode chat for
# `minimax/minimax-m3` from the real `wg tui` `+` menu opened a DIFFERENT model
# (the user saw "nano banana", OpenCode's internal default). Root cause: the
# bare OpenRouter `vendor/model` route a user naturally types
# (`minimax/minimax-m3`) was passed to OpenCode WITHOUT the `openrouter/`
# prefix, so OpenCode could not resolve provider `minimax` and silently fell
# back to its own default model.
#
# This drives the ACTUAL human flow (tmux keystrokes into `wg tui`), not a CLI
# substitute, and inspects the persisted `.chat-N` graph row + CoordinatorState
# + normalized argv — not just screen text.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat (so a PTY
#      is focused — the state where the `+` regression class lives).
#   2. Launch `wg tui`, press `+`, choose Add-new, select `opencode`.
#   3. Type the bare route `minimax/minimax-m3` into the free-text model field.
#   4. Launch, then assert the new `.chat-1`:
#        - executor_preset_name == opencode
#        - model preserves the user-requested minimax route
#        - command_argv == ["opencode","--model","openrouter/minimax/minimax-m3"]
#          (normalized; NOT a default/recent/nano-banana model)
#        - no endpoint override anywhere
#      and CoordinatorState carries the same executor/model and no endpoint.
#
# Accepted UI spellings (all normalize to `openrouter/minimax/minimax-m3`):
#   - bare           minimax/minimax-m3            (typed here — the repro)
#   - provider-qual. openrouter:minimax/minimax-m3
#   - executor-qual. opencode:openrouter/minimax/minimax-m3

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-minimax-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

project="$scratch/project"
mkdir -p "$project"
cd "$project"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" WG_GLOBAL_DIR="$fake_global" \
        wg "$@"
}

if ! run_wg init --no-agency -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

# Existing live PTY chat: this is the state where the `+`/launcher class of
# bugs lives. `cat` keeps the smoke credential-free.
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
    "cd '$project' && env HOME='$fake_home' XDG_CONFIG_HOME='$fake_home/.config' WG_GLOBAL_DIR='$fake_global' wg tui"

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
wait_screen_contains "opencode" "opencode executor choice"

tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ opencode" "selected opencode executor"

# The realistic user input: a bare OpenRouter vendor/model route.
typed="minimax/minimax-m3"
tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" "$typed"
wait_screen_contains "$typed" "typed bare minimax route"
tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$typed" <<'PY'
import json
import sys
from pathlib import Path

typed = sys.argv[1]
EXPECTED_ARG = "openrouter/minimax/minimax-m3"

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "opencode":
    raise SystemExit(f"expected executor_preset_name=opencode, got {chat.get('executor_preset_name')!r}: {chat}")

# The chat must preserve the user-requested minimax route — never a
# preset/recent/default OpenRouter model.
model = chat.get("model") or ""
if "minimax/minimax-m3" not in model:
    raise SystemExit(f"chat model must preserve the requested minimax route, got {model!r}: {chat}")

if "endpoint" in chat or "endpoint_override" in chat:
    raise SystemExit(f"opencode chat must not persist an endpoint override in graph row: {chat}")

argv = chat.get("command_argv") or []
expected = ["opencode", "--model", EXPECTED_ARG]
if argv != expected:
    raise SystemExit(
        f"expected normalized OpenCode argv {expected!r}, got {argv!r}. "
        f"A bare minimax route must be passed as the openrouter route, not a "
        f"default/recent model: {chat}"
    )
# Defensively assert no fallback model leaked into the argv.
joined = " ".join(argv).lower()
for bad in ("nano", "banana", "step-3.7", "gpt", "claude", "opus"):
    if bad in joined:
        raise SystemExit(f"argv contains an unexpected fallback model token {bad!r}: {argv}")

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
PY
then
    loud_fail "minimax launcher assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu launches opencode for a bare minimax route, normalizing to openrouter/minimax/minimax-m3 (no default/nano-banana fallback)"
exit 0
