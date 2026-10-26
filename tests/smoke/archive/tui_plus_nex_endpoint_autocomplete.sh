#!/usr/bin/env bash
# Scenario: tui_plus_nex_endpoint_autocomplete (add-nex-endpoint)
#
# Confirms the Nex/native Endpoint field's NAME autocomplete works through
# the REAL `wg tui` `+` menu: configured `[[llm_endpoints.endpoints]]`
# surface as suggestions, the user filters by name and accepts a NAMED
# endpoint, and that exact name flows into the persisted `.chat-N` row +
# CoordinatorState + Nex argv.
#
# This drives the ACTUAL human flow (tmux keystrokes into `wg tui`), not a
# CLI substitute, and inspects the persisted metadata + normalized argv —
# not just screen text.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat (so a
#      PTY is focused — the state where the `+` launcher class lives).
#   2. Configure TWO named endpoints:
#        - `qwen-local`     (the target the user will pick — NOT default)
#        - `fallback-decoy` (is_default=true — a DECOY: if the launcher
#                            ever fell back to the default instead of the
#                            user's autocomplete pick, the exact-match
#                            assertions below would catch it)
#      and seed a DECOY recent launcher-history nex combo too.
#   3. Launch `wg tui`, press `+`, choose Add-new, select `nex`.
#   4. Tab to the model field, type a `nex:` model; Tab to the Endpoint
#      field — the autocomplete dropdown now lists BOTH endpoint NAMES
#      (assert `qwen-local` is visible). Type `qwen` to filter down to the
#      target, then Enter to ACCEPT the highlighted named suggestion and
#      launch.
#   5. Assert the new `.chat-1`:
#        - executor_preset_name == nex
#        - model == the typed `nex:` model (exact)
#        - endpoint == `qwen-local` (the NAME accepted from autocomplete,
#          NOT the default-DECOY, NOT the recent-DECOY, NOT a raw URL)
#        - command_argv == ["wg","nex","-m","<model-id>","-e","qwen-local"]
#      and CoordinatorState carries executor_override=native + the same
#      model_override + endpoint_override=qwen-local.
#
# Credential-free: only persisted metadata + argv are asserted; no live
# model reply is required, so neither endpoint has to be reachable.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-nex-ep-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

# Decoy recent launcher history: a DIFFERENT nex endpoint than the named
# one we pick below. A regression that submitted a "recent" combo instead
# of the accepted suggestion would be caught by the exact-match asserts.
history_path="$fake_home/launcher-history.jsonl"
DECOY_MODEL="nex:stale-recent-DECOY"
DECOY_RECENT_EP="recent-DECOY-endpoint"
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

# The TARGET named endpoint the user will pick via autocomplete. NOT the
# default — so accepting it proves a real autocomplete selection, not a
# default-fallback.
TARGET_EP_NAME="qwen-local"
TARGET_EP_URL="http://127.0.0.1:18099/v1"
if ! run_wg endpoints add "$TARGET_EP_NAME" --provider local --url "$TARGET_EP_URL" \
    >ep-target.log 2>&1; then
    loud_fail "wg endpoints add $TARGET_EP_NAME failed: $(tail -20 ep-target.log)"
fi

# The DECOY default endpoint: distinct name + URL from the target. The
# created chat must carry the autocomplete-accepted NAME, never this
# default.
DECOY_DEFAULT_NAME="fallback-decoy"
DECOY_DEFAULT_URL="http://127.0.0.1:19999/default-DECOY"
if ! run_wg endpoints add "$DECOY_DEFAULT_NAME" --provider openrouter --url "$DECOY_DEFAULT_URL" \
    --default >ep-decoy.log 2>&1; then
    loud_fail "wg endpoints add $DECOY_DEFAULT_NAME failed: $(tail -20 ep-decoy.log)"
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
wait_screen_contains "Endpoint" "nex Endpoint field"

typed_model="nex:qwen3-coder"

# Tab to the model field, type the model.
tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" -l "$typed_model"
wait_screen_contains "$typed_model" "typed nex model"

# Tab to the Endpoint field — the autocomplete dropdown now lists the
# configured endpoint NAMES (nex-only). This is the heart of the feature:
# the user sees configured endpoints by name, not a blank URL box.
tmux send-keys -t "$session" Tab
sleep 0.2
wait_screen_contains "$TARGET_EP_NAME" "endpoint autocomplete dropdown lists the named endpoint"
wait_screen_contains "[default]" "the default endpoint is marked in the dropdown"

# Type a name fragment to filter down to the target, then Enter accepts the
# highlighted named suggestion and launches. (We deliberately do NOT type a
# raw URL here — that path is covered by tui_plus_nex_chat.sh.)
tmux send-keys -t "$session" -l "qwen"
sleep 0.2
wait_screen_contains "$TARGET_EP_NAME" "filtered endpoint suggestion still visible"

tmux send-keys -t "$session" Enter

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

if ! python3 - "$typed_model" "$TARGET_EP_NAME" "$DECOY_MODEL" "$DECOY_RECENT_EP" \
    "$DECOY_DEFAULT_NAME" "$DECOY_DEFAULT_URL" "$TARGET_EP_URL" <<'PY'
import json
import sys
from pathlib import Path

typed_model = sys.argv[1]
target_ep_name = sys.argv[2]
decoy_model = sys.argv[3]
decoy_recent_ep = sys.argv[4]
decoy_default_name = sys.argv[5]
decoy_default_url = sys.argv[6]
target_ep_url = sys.argv[7]

# Expected normalized nex argv: the `nex:` prefix is stripped to the bare
# model id; the endpoint is the accepted NAME, passed verbatim. `wg nex -e
# <name>` resolves a configured name just like a URL, so the working CLI
# shape carries the name.
EXPECTED_ARGV = ["wg", "nex", "-m", "qwen3-coder", "-e", target_ep_name]

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
if endpoint != target_ep_name:
    raise SystemExit(
        f"chat endpoint must be the autocomplete-accepted NAME {target_ep_name!r}, "
        f"got {endpoint!r}: {chat}"
    )
# It must NOT be the default decoy (name or url), the recent decoy, or the
# target's raw URL — accepting a named suggestion persists the NAME.
for bad in (decoy_default_name, decoy_default_url, decoy_recent_ep, target_ep_url):
    if endpoint == bad:
        raise SystemExit(f"chat endpoint leaked a decoy/raw value {bad!r}: {chat}")

argv = chat.get("command_argv") or []
if argv != EXPECTED_ARGV:
    raise SystemExit(
        f"expected nex argv {EXPECTED_ARGV!r}, got {argv!r}. The launcher must "
        f"emit `wg nex -m <model> -e <accepted-name>` — not a default/recent/raw "
        f"value: {chat}"
    )
for forbidden in ("--chat", "--role", "--resume"):
    if forbidden in argv:
        raise SystemExit(f"nex argv must stay on the interactive stdin path; found {forbidden!r}: {argv}")
joined = " ".join(argv)
for bad in (decoy_model, decoy_recent_ep, decoy_default_name, decoy_default_url, "stale", "DECOY"):
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
if state.get("endpoint_override") != target_ep_name:
    raise SystemExit(
        f"coordinator endpoint_override must be the accepted name {target_ep_name!r}, "
        f"got {state.get('endpoint_override')!r}: {state}"
    )

print(
    "OK: .chat-1 + CoordinatorState carry the autocomplete-accepted endpoint "
    f"name {target_ep_name!r}; argv = " + " ".join(argv)
)
PY
then
    loud_fail "nex endpoint autocomplete assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu nex Endpoint autocomplete picks named endpoint '$TARGET_EP_NAME' (not the default/recent decoy), argv = wg nex -m qwen3-coder -e $TARGET_EP_NAME"
exit 0
