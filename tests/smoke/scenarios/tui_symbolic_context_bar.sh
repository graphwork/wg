#!/usr/bin/env bash
# Candidate-binary real tmux/SGR flow for the approved one-row symbolic TUI.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
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
"$WG_BIN" --dir "$G" chat create --name symbolic --command cat >/dev/null
"$WG_BIN" --dir "$G" add symbolic-first -d "first exact symbolic target" >/dev/null
"$WG_BIN" --dir "$G" add symbolic-second -d "second exact symbolic target" >/dev/null
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":60,"mode":"full"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}
JSON

session="wg-symbolic-context-$$"
empty_session="wg-symbolic-empty-$$"
cleanup_sessions() {
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux kill-session -t "$empty_session" 2>/dev/null || true
    tmux kill-session -t "wg-chat-$(basename "$(dirname "$G")")-0" 2>/dev/null || true
}
add_cleanup_hook cleanup_sessions

capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
bar() {
    local row=""
    for _ in $(seq 1 40); do
        row=$(capture | grep -m1 '↯' || capture | grep ' C ' | grep ' T ' | grep -m1 ' W ' || true)
        [[ -n "$row" ]] && { printf '%s\n' "$row"; return 0; }
        sleep 0.03
    done
    return 0
}
wait_screen() {
    local needle=$1 label=${2:-"screen missing $1"}
    for _ in $(seq 1 240); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
coord() {
    local needle=$1 occurrence=${2:-first}
    capture | python3 -c 'import sys
needle, occurrence=sys.argv[1:]
hits=[]
for y,row in enumerate(sys.stdin.read().splitlines(), 1):
    start=0
    while True:
        x=row.find(needle, start)
        if x < 0:
            break
        hits.append((x+1,y))
        start=x+max(1,len(needle))
if not hits:
    raise SystemExit(1)
x,y=hits[-1] if occurrence == "last" else hits[0]
print(x,y)' "$needle" "$occurrence"
}
click_text() {
    local needle=$1 occurrence=${2:-first} xy x y
    xy=$(coord "$needle" "$occurrence") || loud_fail "click target '$needle' missing: $(capture | tr '\n' '|')"
    read -r x y <<<"$xy"
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}

start_tui() {
    tmux new-session -d -s "$session" -x 80 -y 24 \
        "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
    tmux set-option -t "$session" mouse on
}
start_tui
wait_screen "↯" "workshop lanes did not render"
wait_screen "symbolic" "current Chat title missing"
wait_screen "⊞" "live Chat must use inverse compact New-chat tile"
top=$(bar)
[[ "$top" == *"↯"* && "$top" == *"⌁"* && "$top" == *"⌂"* && "$top" == *"⌕"* && "$top" == *"⌘"* && "$top" == *"?"* ]] \
    || loud_fail "approved grammar incomplete: $top"
[[ "$top" != *"[ New chat ]"* ]] || loud_fail "live Chat rendered bracketed New-chat label: $top"

# The canonical location is always leftmost, before the coherent control group
# and the elastic current-context title.
top=$(bar)
python3 - "$top" <<'PY'
import sys
row=sys.argv[1]
assert row.find("project") < row.find("↯") < row.find("symbolic"), row
PY
if [[ -n "${WG_SMOKE_EVIDENCE_DIR:-}" ]]; then
    mkdir -p "$WG_SMOKE_EVIDENCE_DIR"
    for width in 40 80 160; do
        tmux resize-window -t "$session" -x "$width" -y 30
        sleep 0.15
        bar >"$WG_SMOKE_EVIDENCE_DIR/$width.txt"
    done
    tmux resize-window -t "$session" -x 80 -y 24
    sleep 0.15
fi

# First Task tap returns to remembered exact work; repeated tap opens the bounded selector.
click_text "⌁"
sleep 0.1
click_text "⌁"
wait_screen "Tasks — exact destination" "repeated Task lane did not open selector"
click_text "symbolic-first"
wait_screen "symbolic-first" "exact Task selector row did not commit"

# Search is an exact temporary task transaction and Enter clears query/filter coloring state.
click_text "⌕"
tmux send-keys -t "$session" -l "symbolic-second"
wait_screen "⌕/symbolic-second" "active anchored search field missing"
tmux send-keys -t "$session" Enter
wait_screen "symbolic-second" "search did not commit exact stable task"
if bar | grep -Fq "⌕/symbolic-second"; then
    loud_fail "accepted graph search state persisted after exact commit"
fi
# Up/Down remain ordinary task navigation immediately after commit.
tmux send-keys -t "$session" Up Down

# At generous width, the boundary-aware previous control is real and clickable.
tmux resize-window -t "$session" -x 120 -y 30
sleep 0.15
if bar | grep -Fq "‹"; then click_text "‹"; fi

# First Chat tap returns to the current Chat title; repeating opens existing Chat selector.
click_text "↯"
wait_screen "symbolic" "Chat lane did not restore current title"
click_text "↯"
wait_screen "Choose Chat" "repeated Chat lane did not open existing selector"
click_text ".chat-0" last
wait_screen "symbolic" "exact Chat selector row did not activate"

# Context/actions, Help, compact New Chat, and packed pulse are all pointer-owned.
click_text "⋮"
wait_screen "Close Chat" "exact Chat context action did not open"
tmux send-keys -t "$session" Escape
click_text "?"
wait_screen "Symbolic context bar" "direct Help did not explain symbols"
tmux send-keys -t "$session" Escape
graph_hash=$(sha256sum "$G/graph.jsonl")
click_text "⊞"
wait_screen "New chat" "compact New-chat tile did not open launcher"
[[ $(sha256sum "$G/graph.jsonl") == "$graph_hash" ]] || loud_fail "opening New chat mutated graph"
tmux send-keys -t "$session" Escape
if bar | grep -qE '●|○'; then
    pulse=$(bar | grep -oE '●[^ ]+|○[^ ]+' | head -1)
    click_text "$pulse"
    wait_screen "Workspace" "packed pulse did not open Workspace/system detail"
    tmux send-keys -t "$session" Escape
fi

# Audition every approved workshop glyph at the desktop/Termux width matrix
# before exercising the explicit compatibility switch.
for width in 20 32 40 60 80 120 200; do
    tmux resize-window -t "$session" -x "$width" -y 20
    sleep 0.08
    top=$(bar)
    [[ "$top" == *"↯"* && "$top" == *"⌁"* && "$top" == *"⌂"* && "$top" == *"⊞"* && "$top" != *"�"* ]] \
        || loud_fail "workshop-mode width audition failed at $width: $top :: $(capture | tr '\n' '|')"
done
tmux resize-window -t "$session" -x 120 -y 30
sleep 0.1

# Controls rows are mouse-selectable and Letters mode applies immediately with no probing.
click_text "⌘"
wait_screen "Controls — global owners" "Controls palette did not open"
tmux send-keys -t "$session" a
wait_screen "No font probing" "Appearance owner missing"
tmux send-keys -t "$session" l
sleep 0.1
top=$(bar)
[[ "$top" == *"C"* && "$top" == *"T"* && "$top" == *"W"* && "$top" == *"="* ]] \
    || loud_fail "Letters compatibility mode not immediate: $top"
[[ "$top" != *"↯"* && "$top" != *"⌁"* && "$top" != *"⌂"* ]] \
    || loud_fail "Letters mode leaked workshop lane glyphs: $top"

# Termux-like compact resize keeps exact Graph↔Chat↔Task reachability.
tmux resize-window -t "$session" -x 32 -y 20
sleep 0.15
top=$(bar)
[[ "$top" == *"C"* && "$top" == *"T"* && "$top" == *"W"* ]] \
    || loud_fail "32-column lanes disappeared: $top"
[[ "$top" != *"�"* ]] || loud_fail "unsupported/replacement glyph rendered: $top"
# Appearance was opened from Workspace detail, so first move to Chat; then W
# is a real lane change (a repeated active W correctly opens Workspace actions).
click_text "C"
wait_screen "Chat:" "compact Chat lane did not restore exact live inspector"
click_text "W"
wait_screen "symbolic-first" "compact Workspace lane did not expose Graph"
click_text "C"
wait_screen "Chat:" "compact Chat lane did not restore exact live inspector"
# Ctrl+O exits child ownership; plain Tab is then the mosh-safe binary
# Graph↔last-inspector fallback and must preserve Chat rather than choose Log.
tmux send-keys -t "$session" C-o Tab
wait_screen "symbolic-first" "compact Tab did not return to Graph"
tmux send-keys -t "$session" Tab
wait_screen "Chat:" "compact Tab did not restore exact Chat inspector"
click_text "T"
wait_screen "symbolic-second" "compact Task lane did not show remembered exact task destination"
click_text "T"
wait_screen "Tasks — exact destination" "compact repeated Task lane did not open selector"
tmux send-keys -t "$session" Escape

for width in 20 40 60 80 120 200; do
    tmux resize-window -t "$session" -x "$width" -y 20
    sleep 0.08
    top=$(bar)
    [[ "$top" == *"C"* && "$top" == *"T"* && "$top" == *"W"* && "$top" != *"�"* ]] \
        || loud_fail "letters-mode width audition failed at $width: $top :: $(capture | tr '\n' '|')"
done

# After the resize matrix, activate every reordered non-lane control again at
# its newly-derived coordinate. This rejects stale hitboxes that only worked in
# the initial 80-column frame.
tmux resize-window -t "$session" -x 200 -y 30
sleep 0.15
click_text "S"; tmux send-keys -t "$session" Escape; sleep 0.1
click_text "="; wait_screen "Controls — global owners" "resized Controls hit missed"
tmux send-keys -t "$session" a; wait_screen "No font probing" "resized Appearance owner missing"
tmux send-keys -t "$session" w; sleep 0.1
click_text "?"; wait_screen "Symbolic context bar" "resized Help hit missed"; tmux send-keys -t "$session" Escape; sleep 0.1
click_text "⋮"; sleep 0.08; tmux send-keys -t "$session" Escape; sleep 0.1
click_text "↯"; wait_screen "Chat:" "resized Chat lane missed"; sleep 0.1
click_text "⊞"; wait_screen "New chat" "resized New-chat hit missed"; tmux send-keys -t "$session" Escape; sleep 0.1
if bar | grep -qE '●|○'; then
    pulse=$(bar | grep -oE '!?[●○][^ ]+' | head -1)
    click_text "$pulse"; wait_screen "Workspace" "resized pulse hit missed"; tmux send-keys -t "$session" Escape
fi

# Empty startup remains non-mutating and uses the fully labelled action.
tmux kill-session -t "$session" 2>/dev/null || true
mkdir -p "$scratch/empty"
EG="$scratch/empty/.wg"
"$WG_BIN" --dir "$EG" init --no-agency >/dev/null
empty_hash=$(sha256sum "$EG/graph.jsonl")
tmux new-session -d -s "$empty_session" -x 40 -y 20 \
    "cd '$scratch/empty' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$EG' tui"
for _ in $(seq 1 200); do
    tmux capture-pane -p -t "$empty_session" 2>/dev/null | grep -Fq "⊞" && break
    sleep 0.03
done
tmux capture-pane -p -t "$empty_session" | grep -Fq "⊞" \
    || loud_fail "no-chat startup did not render compact New-chat control"
[[ $(sha256sum "$EG/graph.jsonl") == "$empty_hash" ]] || loud_fail "empty TUI startup mutated graph"

echo "PASS: location-first hierarchy, context-title switching, resized same-frame pointer controls, lanes/selectors/search/help/new-chat/pulse/letters/compact/no-chat"
