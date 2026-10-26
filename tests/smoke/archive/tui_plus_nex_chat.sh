#!/usr/bin/env bash
# Scenario: tui_plus_nex_chat (confirm-real-tui)
#
# Confirms the Nex/native chat path works through the REAL `wg tui` `+`
# menu the same way the OpenCode/OpenRouter path is pinned by
# tui_plus_opencode_chat.sh / tui_plus_opencode_minimax.sh.
#
# This drives the ACTUAL human flow (tmux keystrokes into `wg tui`), not a
# CLI substitute, and inspects the persisted `.chat-N` graph row +
# CoordinatorState + normalized argv — not just screen text.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat (so a
#      PTY is focused — the state where the `+` regression class lives).
#   2. Seed DECOYS so an accidental fallback would be caught:
#        - a global default endpoint distinct from what we type, and
#        - a recent launcher-history nex combo distinct from what we type.
#   3. Launch `wg tui`, press `+`, choose Add-new, select `nex`.
#   4. Confirm the Endpoint field appears (nex-only), then Tab to the model
#      field and type a Nex model, Tab to the endpoint field and type a
#      local OAI-compatible endpoint URL.
#   5. Launch, then assert the new `.chat-1`:
#        - executor_preset_name == nex
#        - model == the typed `nex:` model (exact, not the recent decoy)
#        - endpoint == the typed endpoint (exact, not the default/recent decoy)
#        - command_argv == ["wg","nex","-m","<model-id>","-e","<endpoint>"]
#          (the working CLI shape; NOT --chat/--resume, NOT a stale value)
#      and CoordinatorState carries executor_override=native + the same
#      model_override + endpoint_override.
#
# Credential-free: only persisted metadata + argv are asserted; no live
# model reply is required, so the endpoint never has to be reachable. The
# live end-to-end reply path is covered separately by
# tui_nex_chat_end_to_end.sh (live-skips when the endpoint is down).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-nex-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

# Decoy recent launcher history: a DIFFERENT nex model+endpoint than the
# one we type below. If a regression ever made the launcher submit a
# "recent" combo instead of the typed form values, the exact-match
# assertions below would catch it.
history_path="$fake_home/launcher-history.jsonl"
DECOY_MODEL="nex:stale-recent-DECOY"
DECOY_RECENT_EP="http://127.0.0.1:17777/recent-DECOY"
printf '{"timestamp":"2026-01-01T00:00:00+00:00","executor":"native","model":"%s","endpoint":"%s","source":"tui"}\n' \
    "$DECOY_MODEL" "$DECOY_RECENT_EP" >"$history_path"

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

# Decoy global default endpoint: distinct from the endpoint we type into
# the launcher. Pins the "not a default" half of the regression bar — the
# created chat must carry the TYPED endpoint, never this default.
DECOY_DEFAULT_EP="http://127.0.0.1:19999/default-DECOY"
if ! run_wg config --endpoint "$DECOY_DEFAULT_EP" >config.log 2>&1; then
    loud_fail "wg config --endpoint failed: $(tail -20 config.log)"
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

# Selecting nex must reveal the Endpoint field (nex-only). This is the
# field that distinguishes the native/Nex launcher path from opencode.
wait_screen_contains "Endpoint" "nex Endpoint field"

# The realistic user input: a `nex:` model + a local OAI-compatible
# endpoint URL. Both must be preserved verbatim in the launched metadata.
typed_model="nex:qwen3-coder"
typed_endpoint="http://127.0.0.1:18099/v1"

tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" -l "$typed_model"
wait_screen_contains "$typed_model" "typed nex model"

tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" -l "$typed_endpoint"
wait_screen_contains "$typed_endpoint" "typed nex endpoint"

tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$typed_model" "$typed_endpoint" "$DECOY_MODEL" "$DECOY_RECENT_EP" "$DECOY_DEFAULT_EP" <<'PY'
import json
import sys
from pathlib import Path

typed_model = sys.argv[1]
typed_endpoint = sys.argv[2]
decoy_model = sys.argv[3]
decoy_recent_ep = sys.argv[4]
decoy_default_ep = sys.argv[5]

# Expected normalized nex argv: the `nex:` prefix is stripped to the bare
# model id, the endpoint is passed verbatim. This is the working CLI shape.
EXPECTED_ARGV = ["wg", "nex", "-m", "qwen3-coder", "-e", typed_endpoint]

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
if model == decoy_model:
    raise SystemExit(f"chat model leaked the recent-history decoy {decoy_model!r}: {chat}")

endpoint = chat.get("endpoint")
if endpoint != typed_endpoint:
    raise SystemExit(f"chat endpoint must be the TYPED endpoint {typed_endpoint!r}, got {endpoint!r}: {chat}")
if endpoint in (decoy_recent_ep, decoy_default_ep):
    raise SystemExit(f"chat endpoint leaked a decoy (default/recent) value {endpoint!r}: {chat}")

argv = chat.get("command_argv") or []
if argv != EXPECTED_ARGV:
    raise SystemExit(
        f"expected nex argv {EXPECTED_ARGV!r}, got {argv!r}. The launcher must "
        f"emit `wg nex -m <model> -e <endpoint>` with the TYPED values, not a "
        f"stale default/recent value: {chat}"
    )
# The interactive nex PTY path must never carry session/role flags.
for forbidden in ("--chat", "--role", "--resume"):
    if forbidden in argv:
        raise SystemExit(f"nex argv must stay on the interactive stdin path; found {forbidden!r}: {argv}")
# Defensively assert no decoy token leaked into the argv.
joined = " ".join(argv)
for bad in (decoy_model, decoy_recent_ep, decoy_default_ep, "stale", "DECOY"):
    if bad in joined:
        raise SystemExit(f"argv contains a decoy/fallback token {bad!r}: {argv}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "native":
    raise SystemExit(f"expected coordinator executor_override=native, got {state!r}")
if state.get("model_override") != typed_model:
    raise SystemExit(f"coordinator model_override must be {typed_model!r}, got {state.get('model_override')!r}: {state}")
if state.get("endpoint_override") != typed_endpoint:
    raise SystemExit(f"coordinator endpoint_override must be {typed_endpoint!r}, got {state.get('endpoint_override')!r}: {state}")

print("OK: .chat-1 + CoordinatorState carry typed nex model+endpoint; argv = " + " ".join(argv))
PY
then
    loud_fail "nex launcher assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu launches a nex chat with the typed model+endpoint, argv = wg nex -m qwen3-coder -e $typed_endpoint (no default/recent fallback)"
exit 0
