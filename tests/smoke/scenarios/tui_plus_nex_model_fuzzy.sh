#!/usr/bin/env bash
# Scenario: tui_plus_nex_model_fuzzy (add-model-fuzzy)
#
# Confirms model fuzzy autocomplete composes with endpoint autocomplete on
# the Nex/native path through the REAL `wg tui` `+` menu. The user fuzzy-
# searches a model (`minimax m3`, a space-separated fragment with NO slash),
# accepts the suggestion, then uses the nex Endpoint NAME autocomplete to
# pick a configured endpoint. Both selections must flow into `.chat-N` +
# CoordinatorState + the generated `wg nex` argv, normalized for nex.
#
# This proves:
#   * the Model dropdown surfaces a model by fuzzy fragment,
#   * accepting it persists a valid WG nex model spec
#     (`openrouter:minimax/minimax-m3`, per add-model-fuzzy),
#   * model autocomplete does NOT break the nex Endpoint NAME autocomplete
#     that add-nex-endpoint shipped (Tab from Model lands on Endpoint).
#
# Flow:
#   1. Scratch WG project with a custom-command `cat` chat (PTY focused).
#   2. Configure a named endpoint `qwen-local` (the autocomplete target) and
#      a default-marked decoy `fallback-decoy`; seed a decoy recent combo.
#   3. `wg tui`, `+`, Add-new, select nex.
#   4. Tab to Model, fuzzy-type `minimax m3`, confirm the dropdown lists
#      `minimax/minimax-m3`, Tab to ACCEPT the model + advance to Endpoint.
#   5. On the Endpoint field, type `qwen` to filter, Enter to accept the
#      named endpoint + launch.
#   6. Assert `.chat-1`:
#        - executor_preset_name == nex
#        - model == openrouter:minimax/minimax-m3 (nex-normalized, exact)
#        - endpoint == qwen-local (the accepted NAME, not the default decoy)
#        - command_argv == ["wg","nex","-m","minimax/minimax-m3","-e","qwen-local"]
#      and CoordinatorState mirrors it.
#
# Credential-free: only persisted metadata + argv are asserted.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-nex-fuzzy-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

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

TARGET_EP_NAME="qwen-local"
TARGET_EP_URL="http://127.0.0.1:18099/v1"
if ! run_wg endpoints add "$TARGET_EP_NAME" --provider local --url "$TARGET_EP_URL" \
    >ep-target.log 2>&1; then
    loud_fail "wg endpoints add $TARGET_EP_NAME failed: $(tail -20 ep-target.log)"
fi

DECOY_DEFAULT_NAME="fallback-decoy"
DECOY_DEFAULT_URL="http://127.0.0.1:19999/default-DECOY"
if ! run_wg endpoints add "$DECOY_DEFAULT_NAME" --provider openrouter --url "$DECOY_DEFAULT_URL" \
    --default >ep-decoy.log 2>&1; then
    loud_fail "wg endpoints add $DECOY_DEFAULT_NAME failed: $(tail -20 ep-decoy.log)"
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

tmux send-keys -t "$session" "+"
wait_screen_contains "+ Add new" "default launcher Add-new row"

tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Enter
wait_screen_contains "Executor:" "Add-new executor row"

# nex is the 4th executor choice (claude, codex, opencode, nex): Right x3.
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ nex" "selected nex executor"

# Tab to the Model field, fuzzy-type `minimax m3` (search fragment, no
# slash), and confirm the dropdown surfaces the curated route.
tmux send-keys -t "$session" Tab
sleep 0.2
tmux send-keys -t "$session" -l "minimax m3"
sleep 0.3
wait_screen_contains "minimax/minimax-m3" "model fuzzy dropdown surfaces minimax route"

# Tab ACCEPTS the highlighted model (normalized to openrouter:minimax/...)
# AND advances to the Endpoint field — proving model autocomplete composes
# with the nex Endpoint autocomplete.
tmux send-keys -t "$session" Tab
sleep 0.2
wait_screen_contains "$TARGET_EP_NAME" "endpoint autocomplete dropdown lists the named endpoint"
wait_screen_contains "[default]" "the default endpoint is marked in the dropdown"

# Filter the endpoint list by name and Enter to accept the named suggestion
# and launch.
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

if ! python3 - "$TARGET_EP_NAME" "$DECOY_MODEL" "$DECOY_RECENT_EP" \
    "$DECOY_DEFAULT_NAME" "$DECOY_DEFAULT_URL" "$TARGET_EP_URL" <<'PY'
import json
import sys
from pathlib import Path

target_ep_name = sys.argv[1]
decoy_model = sys.argv[2]
decoy_recent_ep = sys.argv[3]
decoy_default_name = sys.argv[4]
decoy_default_url = sys.argv[5]
target_ep_url = sys.argv[6]

EXPECTED_MODEL = "openrouter:minimax/minimax-m3"
EXPECTED_ARGV = ["wg", "nex", "-m", "minimax/minimax-m3", "-e", target_ep_name]

graph = Path(".wg/graph.jsonl")
rows = [json.loads(line) for line in graph.read_text().splitlines() if line.strip()]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "nex":
    raise SystemExit(f"expected executor_preset_name=nex, got {chat.get('executor_preset_name')!r}: {chat}")

model = chat.get("model")
if model != EXPECTED_MODEL:
    raise SystemExit(
        f"chat model must be the nex-normalized fuzzy selection {EXPECTED_MODEL!r}, got {model!r}: {chat}"
    )
if model == decoy_model:
    raise SystemExit(f"chat model leaked the recent-history decoy {decoy_model!r}: {chat}")

endpoint = chat.get("endpoint")
if endpoint != target_ep_name:
    raise SystemExit(
        f"chat endpoint must be the autocomplete-accepted NAME {target_ep_name!r}, "
        f"got {endpoint!r}: {chat}"
    )
for bad in (decoy_default_name, decoy_default_url, decoy_recent_ep, target_ep_url):
    if endpoint == bad:
        raise SystemExit(f"chat endpoint leaked a decoy/raw value {bad!r}: {chat}")

argv = chat.get("command_argv") or []
if argv != EXPECTED_ARGV:
    raise SystemExit(
        f"expected nex argv {EXPECTED_ARGV!r}, got {argv!r}. The launcher must emit "
        f"`wg nex -m minimax/minimax-m3 -e <accepted-name>` from the fuzzy model + "
        f"endpoint autocomplete: {chat}"
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
if state.get("model_override") != EXPECTED_MODEL:
    raise SystemExit(f"coordinator model_override must be {EXPECTED_MODEL!r}, got {state.get('model_override')!r}: {state}")
if state.get("endpoint_override") != target_ep_name:
    raise SystemExit(
        f"coordinator endpoint_override must be the accepted name {target_ep_name!r}, "
        f"got {state.get('endpoint_override')!r}: {state}"
    )

print(
    "OK: .chat-1 + CoordinatorState carry the fuzzy-selected nex model "
    f"{EXPECTED_MODEL!r} + accepted endpoint {target_ep_name!r}; argv = " + " ".join(argv)
)
PY
then
    loud_fail "nex model-fuzzy + endpoint autocomplete assertions failed (see error above)"
fi

echo "PASS: TUI '+' menu model fuzzy ('minimax m3' -> openrouter:minimax/minimax-m3) composes with nex endpoint autocomplete (picks '$TARGET_EP_NAME'), argv = wg nex -m minimax/minimax-m3 -e $TARGET_EP_NAME"
exit 0
