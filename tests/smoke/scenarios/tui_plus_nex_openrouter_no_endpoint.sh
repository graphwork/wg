#!/usr/bin/env bash
# Scenario: tui_plus_nex_openrouter_no_endpoint (nex-optional-openrouter-endpoint)
#
# Confirms that the Nex/native endpoint is OPTIONAL in the REAL `wg tui` `+`
# menu and that leaving it BLANK for an OpenRouter model launches Nex against
# OpenRouter — NEVER a silent fallback to the local/default endpoint.
#
# This drives the ACTUAL human flow (tmux keystrokes into `wg tui`), not a
# CLI substitute, and inspects the persisted `.chat-N` graph row +
# CoordinatorState + normalized argv — not just screen text.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat (so a
#      PTY is focused — the state where the `+` launcher class lives).
#   2. Configure a DECOY local default endpoint (is_default=true, provider
#      local). If the launcher/spawn path ever fell back to the default
#      instead of routing to OpenRouter, the assertions below would catch it.
#      Also seed a DECOY recent launcher-history nex combo.
#   3. Launch `wg tui`, press `+`, choose Add-new, select `nex`.
#   4. Tab to the model field, type `minimax/minimax-m3`, and press Enter to
#      accept the OpenRouter model suggestion and LAUNCH — WITHOUT ever
#      touching the Endpoint field (it stays blank: a first-class option).
#   5. Assert the new `.chat-1`:
#        - executor_preset_name == nex
#        - model resolves to the OpenRouter route
#          (`openrouter:minimax/minimax-m3` or the bare `minimax/minimax-m3`)
#        - endpoint is blank/absent (NOT the local default decoy)
#        - command_argv == ["wg","nex","-m","openrouter:minimax/minimax-m3"]
#          — an explicit OpenRouter route and NO `-e`/`--endpoint` arg
#      and CoordinatorState carries executor_override=native + the OpenRouter
#      model_override with NO endpoint_override.
#
# Credential-free: only persisted metadata + argv are asserted; no live model
# reply is required, so OpenRouter never has to be reachable.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-nex-or-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

# Decoy recent launcher history: a DIFFERENT nex model+endpoint than what we
# launch below. A regression that submitted a "recent" combo would be caught.
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

# The DECOY local default endpoint. If the blank-endpoint launch ever fell
# back to the is_default endpoint, the chat would carry THIS name/url — the
# assertions below forbid it.
DECOY_DEFAULT_NAME="local-decoy"
DECOY_DEFAULT_URL="http://127.0.0.1:19999/local-DECOY"
if ! run_wg endpoints add "$DECOY_DEFAULT_NAME" --provider local --url "$DECOY_DEFAULT_URL" \
    --default >ep-decoy.log 2>&1; then
    loud_fail "wg endpoints add $DECOY_DEFAULT_NAME failed: $(tail -20 ep-decoy.log)"
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

# Tab to the model field and type an OpenRouter route. We deliberately do NOT
# touch the Endpoint field — leaving it blank is the whole point.
typed_model="minimax/minimax-m3"
tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" -l "$typed_model"
wait_screen_contains "$typed_model" "typed openrouter model"

# Enter from the model field accepts the highlighted OpenRouter suggestion
# (normalizing to `openrouter:minimax/minimax-m3`) and LAUNCHES — endpoint
# stays blank.
sleep 0.3
tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$DECOY_MODEL" "$DECOY_RECENT_EP" "$DECOY_DEFAULT_NAME" "$DECOY_DEFAULT_URL" <<'PY'
import json
import sys
from pathlib import Path

decoy_model = sys.argv[1]
decoy_recent_ep = sys.argv[2]
decoy_default_name = sys.argv[3]
decoy_default_url = sys.argv[4]

# Both spellings route to OpenRouter; the persisted model may be the accepted
# normalized form or the bare typed route depending on fuzzy ordering.
OK_MODELS = {"openrouter:minimax/minimax-m3", "minimax/minimax-m3"}
# The launched nex argv MUST carry an explicit OpenRouter route and NO -e.
EXPECTED_ARGV = ["wg", "nex", "-m", "openrouter:minimax/minimax-m3"]

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "nex":
    raise SystemExit(f"expected executor_preset_name=nex, got {chat.get('executor_preset_name')!r}: {chat}")

model = chat.get("model")
if model not in OK_MODELS:
    raise SystemExit(f"chat model must resolve to the OpenRouter route {OK_MODELS!r}, got {model!r}: {chat}")
if model == decoy_model:
    raise SystemExit(f"chat model leaked the recent-history decoy {decoy_model!r}: {chat}")

# Endpoint must be blank/absent — NOT the local default decoy.
endpoint = chat.get("endpoint")
if endpoint not in (None, ""):
    raise SystemExit(
        f"endpoint must be blank for a blank-endpoint OpenRouter launch, got {endpoint!r}. "
        f"A non-empty endpoint means a silent fallback to the local default: {chat}"
    )

argv = chat.get("command_argv") or []
if argv != EXPECTED_ARGV:
    raise SystemExit(
        f"expected OpenRouter nex argv {EXPECTED_ARGV!r}, got {argv!r}. The launcher must "
        f"emit `wg nex -m openrouter:<route>` with NO -e — not a local/default endpoint: {chat}"
    )
# Hard guard: no endpoint flag, no decoy tokens anywhere in the argv.
if "-e" in argv or "--endpoint" in argv:
    raise SystemExit(f"blank-endpoint launch must NOT include an endpoint flag: {argv}")
joined = " ".join(argv)
for bad in (decoy_model, decoy_recent_ep, decoy_default_name, decoy_default_url, "DECOY", "local-decoy", "19999"):
    if bad in joined:
        raise SystemExit(f"argv leaked a decoy/local-default token {bad!r}: {argv}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "native":
    raise SystemExit(f"expected coordinator executor_override=native, got {state!r}")
if state.get("model_override") not in OK_MODELS:
    raise SystemExit(
        f"coordinator model_override must be the OpenRouter route, got {state.get('model_override')!r}: {state}"
    )
ep_override = state.get("endpoint_override")
if ep_override not in (None, ""):
    raise SystemExit(
        f"coordinator endpoint_override must be unset for a blank-endpoint OpenRouter launch, "
        f"got {ep_override!r}: {state}"
    )

print(
    "OK: .chat-1 + CoordinatorState route to OpenRouter with a BLANK endpoint; "
    "argv = " + " ".join(argv) + " (no -e, no local-default fallback)"
)
PY
then
    loud_fail "nex blank-endpoint OpenRouter assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu launches nex with minimax/minimax-m3 and NO endpoint, routed to OpenRouter (argv = wg nex -m openrouter:minimax/minimax-m3, no -e, no local-default fallback)"
exit 0
