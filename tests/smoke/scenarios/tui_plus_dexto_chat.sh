#!/usr/bin/env bash
# Scenario: tui_plus_dexto_chat (prototype-octomind-dexto-chat)
#
# Live human-flow proof that the WG TUI `+` new-chat menu can launch a Dexto
# live chat with an OpenRouter model preserved end-to-end.
#
# Dexto's `--model` flag REJECTS provider/model OpenRouter routes
# ("looks like an OpenRouter-format ID … set provider/model explicitly in
# agent config"), and its chat action has no `--provider` flag. So WG pins the
# typed model through a generated per-chat agent YAML
# (`<chat_dir>/dexto-agent.yml`: `llm.provider: openrouter`, `model: <route>`,
# `apiKey: $OPENROUTER_API_KEY`) and launches `dexto --agent <path>
# --auto-approve`.
#
# The user picks `dexto` and types the bare route `minimax/minimax-m3`. WG must:
#   - persist executor_preset_name == dexto on `.chat-1`,
#   - build argv ["dexto","--agent","<abs>/dexto-agent.yml","--auto-approve"],
#   - write that YAML with `provider: openrouter` and `model: minimax/minimax-m3`
#     (the typed route preserved exactly, no fallback),
#   - carry NO endpoint override.
#
# Drives the REAL `wg tui` tmux flow. Credential-free — only persisted
# metadata + the generated config are asserted; no dexto / OpenRouter call is
# made. SKIPs without tmux.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-dexto-$$"
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

tmux send-keys -t "$session" "+"
wait_screen_contains "+ Add new" "default launcher Add-new row"

tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Enter
wait_screen_contains "Executor:" "Add-new executor row"

# Radio order: claude, codex, opencode, nex, octomind, dexto, Custom Command.
# dexto is index 5 → Right x5 from the default (claude, idx 0).
for _ in 1 2 3 4 5; do
    tmux send-keys -t "$session" Right
    sleep 0.1
done
wait_screen_contains "◉ dexto" "selected dexto executor"

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

if chat.get("executor_preset_name") != "dexto":
    raise SystemExit(f"expected executor_preset_name=dexto, got {chat.get('executor_preset_name')!r}: {chat}")

if "endpoint" in chat or "endpoint_override" in chat:
    raise SystemExit(f"dexto chat must not persist an endpoint override in graph row: {chat}")

argv = chat.get("command_argv") or []
if len(argv) != 4 or argv[0] != "dexto" or argv[1] != "--agent" or argv[3] != "--auto-approve":
    raise SystemExit(
        f"expected argv ['dexto','--agent','<path>','--auto-approve'], got {argv!r}: {chat}"
    )
agent_path = Path(argv[2])
if not agent_path.is_absolute():
    raise SystemExit(f"dexto --agent path must be absolute, got {argv[2]!r}: {chat}")
if not agent_path.exists():
    raise SystemExit(f"dexto agent config was not written at {agent_path}: {chat}")
if agent_path.name != "dexto-agent.yml":
    raise SystemExit(f"unexpected dexto agent config filename: {agent_path}")

body = agent_path.read_text()
if "provider: openrouter" not in body:
    raise SystemExit(f"dexto agent config must pin provider: openrouter:\n{body}")
if "model: minimax/minimax-m3" not in body:
    raise SystemExit(
        f"dexto agent config must preserve the typed model 'minimax/minimax-m3' "
        f"(no fallback to a default):\n{body}"
    )
if "apiKey: $OPENROUTER_API_KEY" not in body:
    raise SystemExit(f"dexto agent config must reference $OPENROUTER_API_KEY:\n{body}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "dexto":
    raise SystemExit(f"expected coordinator executor_override=dexto, got {state!r}")
if state.get("endpoint_override") not in (None, ""):
    raise SystemExit(f"dexto coordinator state must not carry endpoint_override: {state!r}")

print("OK: .chat-1 launches dexto --agent " + str(agent_path) +
      " with provider: openrouter + model: minimax/minimax-m3 preserved")
PY
then
    loud_fail "dexto launcher assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu launches dexto with minimax/minimax-m3 → generated agent YAML pins provider: openrouter + model: minimax/minimax-m3 (model preserved, no endpoint)"
exit 0
