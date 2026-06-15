#!/usr/bin/env bash
# Scenario: tui_plus_nex_global_endpoint_picker (nex-openrouter-default-ui)
#
# Confirms the Nex/native Endpoint picker in the REAL `wg tui` `+` menu
# surfaces GLOBALLY-configured endpoint names (from ~/.wg/config.toml) and
# lets the user select one — even when the project's LOCAL config declares
# its own endpoints and has NOT opted into global endpoint inheritance
# (`[llm_endpoints] inherit_global`, opt-in by default). The accepted GLOBAL
# name must flow verbatim into the persisted `.chat-N` row + CoordinatorState
# + Nex argv.
#
# This drives the ACTUAL human flow (tmux keystrokes into `wg tui`), not a
# CLI substitute, and inspects persisted metadata + normalized argv.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat (so a
#      PTY is focused — the state where the `+` launcher class lives).
#   2. Configure endpoints in TWO scopes:
#        - LOCAL (project): `local-only` (is_default=true). Declaring a
#          local endpoint means the merged config does NOT inherit global
#          endpoints — so without the launcher's global-union fix the global
#          name below would be INVISIBLE in the picker.
#        - GLOBAL (~/.wg via WG_GLOBAL_DIR): `global-router` — the target the
#          user will pick. It lives ONLY in the global config.
#   3. Launch `wg tui`, `+`, Add-new, select `nex`, type a `nex:` model,
#      Tab to the Endpoint field — the picker must list `global-router`
#      (proving the global union) ALONGSIDE the local `local-only`. Filter
#      by `global` and Enter to ACCEPT the highlighted global name + launch.
#   4. Assert the new `.chat-1`:
#        - executor_preset_name == nex
#        - model == the typed `nex:` model
#        - endpoint == `global-router` (the GLOBAL name accepted from the
#          picker — NOT the local default `local-only`)
#        - command_argv == ["wg","nex","-m","qwen3-coder","-e","global-router"]
#      and CoordinatorState carries executor_override=native + the same
#      model_override + endpoint_override=global-router.
#
# Credential-free: only persisted metadata + argv are asserted; no live
# model reply is required, so the endpoint never has to be reachable.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-nex-globalep-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

history_path="$fake_home/launcher-history.jsonl"
: >"$history_path"

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

# LOCAL default endpoint. Because the project declares its OWN endpoints, the
# merged config does NOT inherit global endpoints — so the global name below
# can only appear in the picker via the launcher's global-union fix. This is
# ALSO a decoy: a default-fallback regression would persist THIS name.
LOCAL_EP_NAME="local-only"
LOCAL_EP_URL="http://127.0.0.1:18099/v1"
if ! run_wg endpoints add "$LOCAL_EP_NAME" --provider local --url "$LOCAL_EP_URL" \
    --default >ep-local.log 2>&1; then
    loud_fail "wg endpoints add $LOCAL_EP_NAME (local) failed: $(tail -20 ep-local.log)"
fi

# The TARGET endpoint, configured ONLY in the GLOBAL config (--global writes
# to $WG_GLOBAL_DIR/config.toml). The picker must surface this name so the
# user can select it.
GLOBAL_EP_NAME="global-router"
GLOBAL_EP_URL="https://global-router.example:30000/v1"
if ! run_wg endpoints add "$GLOBAL_EP_NAME" --provider openrouter --url "$GLOBAL_EP_URL" \
    --global >ep-global.log 2>&1; then
    loud_fail "wg endpoints add $GLOBAL_EP_NAME --global failed: $(tail -20 ep-global.log)"
fi

# Sanity: the global endpoint must really be in the GLOBAL config file and
# NOT in the local project config (otherwise the test would not prove the
# union surfaces a global-only name).
if ! grep -q "$GLOBAL_EP_NAME" "$fake_global/config.toml" 2>/dev/null; then
    loud_fail "global endpoint $GLOBAL_EP_NAME not written to global config $fake_global/config.toml"
fi
if grep -q "$GLOBAL_EP_NAME" "$project/.wg/config.toml" 2>/dev/null; then
    loud_fail "global endpoint $GLOBAL_EP_NAME leaked into LOCAL project config; test would not prove the union"
fi

# Existing live PTY chat: the state where the `+`/launcher class of bugs
# lives. `cat` keeps the smoke credential-free.
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
wait_screen_contains "nex" "nex executor choice"

# nex is the 4th executor choice (claude, codex, opencode, nex): Right x3.
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ nex" "selected nex executor"
wait_screen_contains "Endpoint" "nex Endpoint field"

typed_model="nex:qwen3-coder"

# Tab to the model field, type the model.
tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" -l "$typed_model"
wait_screen_contains "$typed_model" "typed nex model"

# Tab to the Endpoint field — the autocomplete dropdown must list BOTH the
# local endpoint AND the global one (the union). The global name is the heart
# of this scenario.
tmux send-keys -t "$session" Tab
sleep 0.2
wait_screen_contains "$GLOBAL_EP_NAME" "endpoint picker lists the GLOBAL endpoint name"
wait_screen_contains "$LOCAL_EP_NAME" "endpoint picker also lists the local endpoint name"

# Type a fragment to filter down to the GLOBAL target, then Enter accepts the
# highlighted named suggestion and launches.
tmux send-keys -t "$session" -l "global"
sleep 0.2
wait_screen_contains "$GLOBAL_EP_NAME" "filtered global endpoint suggestion still visible"

tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$typed_model" "$GLOBAL_EP_NAME" "$LOCAL_EP_NAME" "$LOCAL_EP_URL" "$GLOBAL_EP_URL" <<'PY'
import json
import sys
from pathlib import Path

typed_model = sys.argv[1]
global_ep_name = sys.argv[2]
local_ep_name = sys.argv[3]
local_ep_url = sys.argv[4]
global_ep_url = sys.argv[5]

# Expected normalized nex argv: the `nex:` prefix is stripped to the bare
# model id; the endpoint is the accepted GLOBAL NAME, passed verbatim.
EXPECTED_ARGV = ["wg", "nex", "-m", "qwen3-coder", "-e", global_ep_name]

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "nex":
    raise SystemExit(f"expected executor_preset_name=nex, got {chat.get('executor_preset_name')!r}: {chat}")

model = chat.get("model")
if model != typed_model:
    raise SystemExit(f"chat model must be the TYPED nex model {typed_model!r}, got {model!r}: {chat}")

endpoint = chat.get("endpoint")
if endpoint != global_ep_name:
    raise SystemExit(
        f"chat endpoint must be the picker-accepted GLOBAL name {global_ep_name!r}, "
        f"got {endpoint!r}: {chat}"
    )
# It must NOT be the local default decoy (name or url), nor a raw URL.
for bad in (local_ep_name, local_ep_url, global_ep_url):
    if endpoint == bad:
        raise SystemExit(f"chat endpoint leaked a decoy/raw value {bad!r}: {chat}")

argv = chat.get("command_argv") or []
if argv != EXPECTED_ARGV:
    raise SystemExit(
        f"expected nex argv {EXPECTED_ARGV!r}, got {argv!r}. The launcher must emit "
        f"`wg nex -m <model> -e <accepted-global-name>` — not the local default: {chat}"
    )
joined = " ".join(argv)
for bad in (local_ep_name, local_ep_url, global_ep_url, "default-DECOY"):
    if bad in joined:
        raise SystemExit(f"argv leaked a decoy/raw token {bad!r}: {argv}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "native":
    raise SystemExit(f"expected coordinator executor_override=native, got {state!r}")
if state.get("endpoint_override") != global_ep_name:
    raise SystemExit(
        f"coordinator endpoint_override must be the GLOBAL name {global_ep_name!r}, "
        f"got {state.get('endpoint_override')!r}: {state}"
    )

print(
    "OK: .chat-1 + CoordinatorState carry the picker-selected GLOBAL endpoint name "
    f"{global_ep_name!r}; argv = " + " ".join(argv)
)
PY
then
    loud_fail "nex global-endpoint picker assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu picker surfaces a GLOBAL endpoint name and launches nex with -e $GLOBAL_EP_NAME (union of global + local endpoints)"
exit 0
