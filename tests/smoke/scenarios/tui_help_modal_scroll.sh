#!/usr/bin/env bash
# Real tmux/PTY + SGR mouse flow for modal, scrollable TUI Help.
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
unset TMUX TMUX_TMPDIR WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
cat >"$G/config.toml" <<'TOML'
[dispatcher]
model = "claude:opus"
TOML
"$WG_BIN" --dir "$G" chat create --name help-flow --command cat >/dev/null
"$WG_BIN" --dir "$G" add help-first -d "first stable Help selection" >/dev/null
"$WG_BIN" --dir "$G" add help-second -d "second stable Help selection" >/dev/null
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":60,"mode":"full"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}
JSON

session="wg-help-modal-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux kill-session -t "wg-chat-$(basename "$(dirname "$G")")-0" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

tmux new-session -d -s "$session" -x 100 -y 30 \
    "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on

capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
dump() { "$WG_BIN" --dir "$G" --json tui-dump 2>/dev/null || true; }
wait_text() {
    local needle=$1 label=${2:-"missing $1"}
    for _ in $(seq 1 200); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
wait_absent() {
    local needle=$1 label=${2:-"still showing $1"}
    for _ in $(seq 1 200); do
        if ! capture | grep -Fq "$needle"; then return 0; fi
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
coord() {
    local needle=$1
    capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(), 1):
    x=row.find(needle)
    if x >= 0:
        print(x+1, y)
        raise SystemExit(0)
raise SystemExit(1)' "$needle"
}
mouse_click() {
    local x=$1 y=$2
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}
mouse_down() {
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$1" "$2")"
}
mouse_up() {
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$1" "$2")"
}
mouse_scroll() {
    # 64/65 are vertical wheel up/down; 66/67 are horizontal trackpad-equivalent scroll.
    tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%sM' "$1" "$2" "$3")"
}
range_text() {
    capture | grep -oE 'Keybindings +[0-9]+-[0-9]+/[0-9]+' | head -1
}
range_start() { range_text | sed -E 's/.* +([0-9]+)-.*/\1/'; }
range_end_total() { range_text | sed -E 's/.*-([0-9]+)\/([0-9]+)/\1 \2/'; }
state_signature() {
    dump | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("|".join(str(v.get(k)) for k in ("active_tab","focused_panel","selected_task","input_mode","coordinator_id")))'
}
wait_range_change() {
    local before=$1
    for _ in $(seq 1 100); do
        [[ "$(range_text)" != "$before" ]] && return 0
        sleep 0.03
    done
    loud_fail "Help scroll position did not change from '$before': $(capture | tr '\n' '|')"
}

wait_text ".chat-0" "TUI did not start"
wait_text "?" "painted question-mark control missing"
wide_screen=$(capture)
[[ "$wide_screen" != *"Ctrl+O→p Panel"* ]] \
    || loud_fail "persistent Panel hint still clutters wide task bar: $(printf '%s' "$wide_screen" | tr '\n' '|')"
dump >"$scratch/wide-task-bar.json"

# Click the ACTUAL painted question mark, not a direct key-handler substitute.
read -r qx qy <<<"$(coord "?")" || loud_fail "could not locate question-mark hit target"
before_modal=$(state_signature)
sleep 0.15
# Refresh-derived selection must settle before proving modal preservation.
before_modal=$(state_signature)
mouse_click "$qx" "$qy"
wait_text "Essential navigation" "question-mark click did not open Help"
wait_text "Ctrl-O → p" "Panel access is not in Help's first viewport"
wait_text "Help scrolling" "Help scrolling controls are not in the first viewport"
capture >"$scratch/help-wide.txt"
after_modal=$(state_signature)
[[ "$after_modal" == "$before_modal" ]] \
    || loud_fail "opening Help changed workspace state: before=$before_modal after=$after_modal target=$qx,$qy"

# Arrow, PageDown/PageUp, Home/End: visible title ranges prove rendered content moved.
r0=$(range_text)
tmux send-keys -t "$session" Down
wait_range_change "$r0"
r1=$(range_text)
tmux send-keys -t "$session" NPage
wait_range_change "$r1"
r2=$(range_text)
tmux send-keys -t "$session" PPage
wait_range_change "$r2"
tmux send-keys -t "$session" Home
sleep 0.08
[[ "$(range_start)" == "1" ]] || loud_fail "Home did not clamp Help to top: $(range_text)"
tmux send-keys -t "$session" Up
sleep 0.05
[[ "$(range_start)" == "1" ]] || loud_fail "Up escaped Help's top clamp: $(range_text)"
tmux send-keys -t "$session" End
sleep 0.08
read -r visible_end total <<<"$(range_end_total)"
[[ "$visible_end" == "$total" ]] || loud_fail "End did not expose Help bottom: $(range_text)"
tmux send-keys -t "$session" Down
sleep 0.05
read -r clamped_end clamped_total <<<"$(range_end_total)"
[[ "$clamped_end" == "$clamped_total" ]] || loud_fail "Down escaped Help bottom clamp: $(range_text)"

# Real terminal SGR wheel and horizontal trackpad-equivalent events are modal too.
tmux send-keys -t "$session" Home
sleep 0.05
rw0=$(range_text)
mouse_scroll 65 50 15
wait_range_change "$rw0"
rw1=$(range_text)
mouse_scroll 67 50 15
wait_range_change "$rw1"
[[ "$(state_signature)" == "$before_modal" ]] || loud_fail "Help scrolling changed underlying state"

# The changing range plus painted thumb are the visible position affordance.
capture | grep -Eq '▲|█|▼' || loud_fail "Help scrollbar affordance is not visible"

# Inside clicks cannot leak. Outside down+up dismisses and cannot activate row-1 controls.
mouse_click 50 15
sleep 0.05
capture | grep -Fq "Keybindings" || loud_fail "inside Help click dismissed/leaked"
mouse_down 1 1
mouse_up 1 1
wait_absent "Essential navigation" "outside click did not dismiss Help"
[[ "$(state_signature)" == "$before_modal" ]] || loud_fail "outside dismissal leaked to workspace"

# Keyboard Help shortcut opens the same surface from command mode; Escape restores it.
tmux send-keys -t "$session" C-o
sleep 0.05
before_keyboard=$(state_signature)
tmux send-keys -t "$session" -l "?"
wait_text "Essential navigation" "keyboard ? did not open the same Help"
tmux send-keys -t "$session" Escape
wait_absent "Essential navigation" "Escape did not dismiss Help"
[[ "$(state_signature)" == "$before_keyboard" ]] || loud_fail "Escape did not restore pre-Help focus/view"

# Resize while open: first viewport remains legible and dismissal preserves workspace state.
tmux send-keys -t "$session" -l "?"
wait_text "Essential navigation"
tmux resize-window -t "$session" -x 40 -y 18
sleep 0.15
narrow=$(capture)
printf '%s\n' "$narrow" >"$scratch/help-narrow.txt"
[[ "$narrow" == *"Ctrl-O"* && "$narrow" == *"Help scrolling"* && "$narrow" != *"�"* ]] \
    || loud_fail "narrow Help first viewport is not legible: $(printf '%s' "$narrow" | tr '\n' '|')"
tmux send-keys -t "$session" End
tmux resize-window -t "$session" -x 100 -y 30
sleep 0.15
read -r resized_end resized_total <<<"$(range_end_total)"
[[ "$resized_end" == "$resized_total" ]] || loud_fail "resize did not clamp Help scroll safely: $(range_text)"
tmux send-keys -t "$session" Escape
wait_absent "Essential navigation"
[[ "$(state_signature)" == "$before_keyboard" ]] || loud_fail "resize+close changed focus/selection/panel state"

# Wide and narrow task bars stay clean after all modal traffic.
wide_after=$(capture)
[[ "$wide_after" != *"Ctrl+O→p Panel"* ]] \
    || loud_fail "Panel hint returned after Help dismissal: $(printf '%s' "$wide_after" | tr '\n' '|')"
tmux resize-window -t "$session" -x 40 -y 18
sleep 0.1
narrow_bar=$(capture)
[[ "$narrow_bar" != *"Ctrl+O→p Panel"* ]] \
    || loud_fail "Panel hint clutters narrow task bar: $(printf '%s' "$narrow_bar" | tr '\n' '|')"

echo "PASS: actual ? hit opens modal Help; keys/wheel/trackpad scroll and clamp with range/thumb affordance; inside/outside/Escape/resize preserve workspace; task bar is clean"
