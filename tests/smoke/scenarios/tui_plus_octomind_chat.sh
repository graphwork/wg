#!/usr/bin/env bash
# Scenario: tui_plus_octomind_chat (prototype-octomind-dexto-chat)
#
# Live human-flow proof that the WG TUI `+` new-chat menu can launch an
# Octomind live chat with an OpenRouter model preserved end-to-end.
#
# The user picks `octomind` in the Add-new executor radio and types the bare
# OpenRouter route `minimax/minimax-m3` (NO `octomind:` / `openrouter:`
# prefix). WG must:
#   - persist executor_preset_name == octomind on `.chat-1`,
#   - preserve the minimax route (never fall back to a default / role model),
#   - build the argv Octomind's CLI consumes:
#       ["octomind","run","-m","openrouter:minimax/minimax-m3","--sandbox"]
#     (its `-m` takes WG's `openrouter:<vendor>/<model>` spelling verbatim),
#   - carry NO endpoint override (Octomind is OpenRouter-first, like opencode).
# CoordinatorState must mirror the executor + model.
#
# Drives the REAL `wg tui` tmux flow (not `wg chat create`): the `+` menu, the
# executor radio, the Model field, and Enter-to-launch. Credential-free — only
# persisted metadata + argv are asserted, no OpenRouter / octomind call is made.
# SKIPs without tmux.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-octomind-$$"
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

# Radio order: claude, codex, opencode, nex, octomind, dexto, Custom Command.
# octomind is index 4 → Right x4 from the default (claude, idx 0).
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ octomind" "selected octomind executor"

# Tab to the Model field, type the bare OpenRouter route (explicit text — the
# fuzzy dropdown steps aside for an explicit `vendor/model` spec).
route="minimax/minimax-m3"
tmux send-keys -t "$session" Tab
sleep 0.2
tmux send-keys -t "$session" -l "$route"
wait_screen_contains "$route" "typed bare OpenRouter route"
tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - <<'PY'
import json
from pathlib import Path

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "octomind":
    raise SystemExit(f"expected executor_preset_name=octomind, got {chat.get('executor_preset_name')!r}: {chat}")

model = chat.get("model") or ""
if "minimax/minimax-m3" not in model:
    raise SystemExit(f"chat model must preserve the typed minimax route, got {model!r}: {chat}")

if "endpoint" in chat or "endpoint_override" in chat:
    raise SystemExit(f"octomind chat must not persist an endpoint override in graph row: {chat}")

argv = chat.get("command_argv") or []
expected = ["octomind", "run", "-m", "openrouter:minimax/minimax-m3", "--sandbox"]
if argv != expected:
    raise SystemExit(
        f"expected octomind argv {expected!r}, got {argv!r}. The typed minimax "
        f"route must launch as `octomind run -m openrouter:minimax/minimax-m3`, "
        f"not a default/role model: {chat}"
    )
joined = " ".join(argv).lower()
for bad in ("nano", "banana", "claude", "gpt", "default"):
    if bad in joined:
        raise SystemExit(f"argv contains an unexpected fallback token {bad!r}: {argv}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "octomind":
    raise SystemExit(f"expected coordinator executor_override=octomind, got {state!r}")
sm = state.get("model_override") or ""
if "minimax/minimax-m3" not in sm:
    raise SystemExit(f"coordinator model_override must preserve the minimax route, got {sm!r}: {state}")
if state.get("endpoint_override") not in (None, ""):
    raise SystemExit(f"octomind coordinator state must not carry endpoint_override: {state!r}")

print("OK: .chat-1 + CoordinatorState carry the octomind minimax route; argv = " + " ".join(argv))
PY
then
    loud_fail "octomind launcher assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu launches octomind with minimax/minimax-m3 → argv 'octomind run -m openrouter:minimax/minimax-m3 --sandbox' (model preserved, no endpoint)"
exit 0
