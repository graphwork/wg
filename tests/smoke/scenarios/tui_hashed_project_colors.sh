#!/usr/bin/env bash
# Candidate-binary real tmux/SGR audit for rich, contrast-safe project colors.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

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
export TMUX_TMPDIR="$scratch/tmux"
unset TMUX WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER NO_COLOR
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR" "$scratch/repos"

main="$scratch/repos/project-main"
worktree="$scratch/repos/project-worktree"
other="$scratch/repos/project-other"
git init -q "$main"
git -C "$main" -c user.name=Smoke -c user.email=smoke@example.invalid commit --allow-empty -qm base
git -C "$main" worktree add -q -b color-smoke-worktree "$worktree"
git init -q "$other"
git -C "$other" -c user.name=Smoke -c user.email=smoke@example.invalid commit --allow-empty -qm base
mkdir -p "$main/nested/invocation"

prepare_graph() {
    local root=$1 theme=$2 graph="$1/.wg"
    "$WG_BIN" --dir "$graph" init --no-agency >/dev/null
    cat >"$graph/config.toml" <<TOML
[models.default]
model = "pi:openrouter:example/model"
reasoning = "high"

[tui]
color_theme = "$theme"
TOML
    "$WG_BIN" --dir "$graph" chat create --name color --command cat >/dev/null
    cat >"$graph/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":67,"mode":"full"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}
JSON
}
prepare_graph "$main" dark
prepare_graph "$worktree" light
prepare_graph "$other" dark

# Parse the actual SGR state attached to a requested visible token and report
# nominal terminal RGB. Indexed and named colors use the same canonical values
# as the renderer's compatibility contract.
cat >"$scratch/style.py" <<'PY'
import re,sys
raw=open(sys.argv[1],"rb").read().decode("utf-8","replace")
target=sys.argv[2]
base=[(0,0,0),(128,0,0),(0,128,0),(128,128,0),(0,0,128),(128,0,128),(0,128,128),(192,192,192),(128,128,128),(255,0,0),(0,255,0),(255,255,0),(0,0,255),(255,0,255),(0,255,255),(255,255,255)]
def idx_rgb(i):
    if i < 16: return base[i]
    if i >= 232:
        v=8+(i-232)*10; return (v,v,v)
    v=i-16
    c=lambda n: 0 if n == 0 else 55+n*40
    return (c(v//36),c((v%36)//6),c(v%6))
def parse_line(line):
    fg=bg=None; underline=reverse=False; plain=[]; styles=[]; pos=0
    for match in re.finditer(r"\x1b\[([0-9;]*)m",line):
        text=line[pos:match.start()]
        for ch in text:
            plain.append(ch); styles.append((fg,bg,underline,reverse))
        values=[int(x) if x else 0 for x in match.group(1).split(";")]
        i=0
        while i < len(values):
            p=values[i]
            if p == 0: fg=bg=None; underline=reverse=False
            elif p == 4: underline=True
            elif p == 24: underline=False
            elif p == 7: reverse=True
            elif p == 27: reverse=False
            elif 30 <= p <= 37: fg=base[p-30]
            elif 90 <= p <= 97: fg=base[p-90+8]
            elif 40 <= p <= 47: bg=base[p-40]
            elif 100 <= p <= 107: bg=base[p-100+8]
            elif p in (38,48) and i+2 < len(values) and values[i+1] == 5:
                color=idx_rgb(values[i+2]); fg=color if p == 38 else fg; bg=color if p == 48 else bg; i+=2
            elif p in (38,48) and i+4 < len(values) and values[i+1] == 2:
                color=tuple(values[i+2:i+5]); fg=color if p == 38 else fg; bg=color if p == 48 else bg; i+=4
            i+=1
        pos=match.end()
    for ch in line[pos:]:
        plain.append(ch); styles.append((fg,bg,underline,reverse))
    return "".join(plain),styles
for line in raw.splitlines():
    plain,styles=parse_line(line)
    at=plain.find(target)
    if at >= 0:
        fg,bg,underline,reverse=styles[at]
        if reverse and fg is not None and bg is not None: fg,bg=bg,fg
        def out(c): return "none" if c is None else ",".join(map(str,c))
        print(out(fg),out(bg),int(underline),int(reverse))
        raise SystemExit(0)
raise SystemExit(1)
PY

sessions=()
cleanup_sessions() {
    for session in "${sessions[@]}"; do tmux kill-session -t "$session" 2>/dev/null || true; done
}
add_cleanup_hook cleanup_sessions

launch() {
    local name=$1 root=$2 graph=$3 width=$4 capability=$5 extra=$6
    sessions+=("$name")
    tmux new-session -d -s "$name" -x "$width" -y 24 \
        "cd '$root' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TERM=xterm-256color WG_TUI_APPEARANCE=auto WG_TUI_COLOR_CAPABILITY='$capability' $extra '$WG_BIN' --dir '$graph' tui"
}

capture_style() {
    local session=$1 target=$2 measurable=${3:-yes} value="" file
    file="$scratch/$session.sgr"
    for _ in $(seq 1 240); do
        tmux capture-pane -p -e -t "$session" >"$file" 2>/dev/null || true
        value=$(python3 "$scratch/style.py" "$file" "$target" 2>/dev/null || true)
        if [[ -n "$value" ]] && { [[ "$measurable" == no ]] || [[ "$value" != none\ none* ]]; }; then
            printf '%s\n' "$value"; return 0
        fi
        sleep 0.03
    done
    loud_fail "$session never rendered styled target '$target'"
}

assert_contrast() {
    local label=$1 style=$2
    python3 - "$label" $style <<'PY'
import sys
label,fg,bg=sys.argv[1:4]
if fg == "none" or bg == "none": raise SystemExit(f"{label}: missing measurable pair fg={fg} bg={bg}")
def rgb(s): return tuple(int(x)/255 for x in s.split(","))
def lum(c):
    def f(x): return x/12.92 if x <= .04045 else ((x+.055)/1.055)**2.4
    r,g,b=map(f,c); return .2126*r+.7152*g+.0722*b
lf,lb=lum(rgb(fg)),lum(rgb(bg)); ratio=(max(lf,lb)+.05)/(min(lf,lb)+.05)
if ratio < 4.5: raise SystemExit(f"{label}: contrast {ratio:.3f} fg={fg} bg={bg}")
PY
}

suffix=$$
# Desktop truecolor: same canonical Git common directory across main, linked
# worktree (light theme), and a nested invocation must produce the same pair.
launch "wg-color-root-$suffix" "$main" "$main/.wg" 120 truecolor "COLORTERM=truecolor"
launch "wg-color-nested-$suffix" "$main/nested/invocation" "$main/.wg" 120 truecolor "COLORTERM=truecolor"
launch "wg-color-worktree-$suffix" "$worktree" "$worktree/.wg" 120 truecolor "COLORTERM=truecolor"
launch "wg-color-other-$suffix" "$other" "$other/.wg" 120 truecolor "COLORTERM=truecolor"
root_style=$(capture_style "wg-color-root-$suffix" "⌁")
nested_style=$(capture_style "wg-color-nested-$suffix" "⌁")
worktree_style=$(capture_style "wg-color-worktree-$suffix" "⌁")
other_style=$(capture_style "wg-color-other-$suffix" "⌁")
[[ "$root_style" == "$nested_style" && "$root_style" == "$worktree_style" ]] \
    || loud_fail "canonical project color drifted: root=$root_style nested=$nested_style worktree=$worktree_style"
[[ "$root_style" != "$other_style" ]] || loud_fail "distinct projects collapsed to one truecolor pair"
assert_contrast "dark-theme desktop truecolor" "$root_style"
assert_contrast "light-theme linked-worktree truecolor" "$worktree_style"
assert_contrast "distinct desktop truecolor" "$other_style"

# Active lane is the exact inverse pair and underlined; exact ID/search/service
# text remain on the readable base. The failure pulse retains semantic red.
active_style=$(capture_style "wg-color-root-$suffix" "↯")
read -r base_fg base_bg _ _ <<<"$root_style"
read -r active_fg active_bg active_under _ <<<"$active_style"
[[ "$active_fg" == "$base_bg" && "$active_bg" == "$base_fg" && "$active_under" == 1 ]] \
    || loud_fail "active/focus tile lost inverse+underline semantics: base=$root_style active=$active_style"
id_style=$(capture_style "wg-color-root-$suffix" ".chat-0")
search_style=$(capture_style "wg-color-root-$suffix" "⌕")
[[ "${id_style%% *}" == "$base_fg" && "${search_style%% *}" == "$base_fg" ]] \
    || loud_fail "exact identity/search lost readable project foreground"
pulse_style=$(capture_style "wg-color-root-$suffix" "!")
read -r _ pulse_bg _ _ <<<"$pulse_style"
[[ "$pulse_bg" == "128,0,0" || "$pulse_bg" == "255,0,0" ]] \
    || loud_fail "failure pulse lost semantic red: $pulse_style"

# Resize and redraw must keep the settled identity pair with no neutral flicker.
session="wg-color-root-$suffix"
for width in 32 60 200 80 120; do
    tmux resize-window -t "$session" -x "$width" -y 24
    style=$(capture_style "$session" "⌁")
    [[ "$style" == "$root_style" ]] || loud_fail "resize $width flickered/drifted: $style != $root_style"
done
for _ in $(seq 1 20); do
    style=$(capture_style "$session" "⌁")
    [[ "$style" == "$root_style" ]] || loud_fail "settled async frame flickered: $style"
done

# Explicit compatibility modes exercise ordinary tmux, mosh-like 256 color,
# Termux portrait 16 color, and complete monochrome fallback.
launch "wg-color-mosh-$suffix" "$main" "$main/.wg" 80 256 "MOSH_IP=192.0.2.1 COLORTERM=truecolor"
mosh_style=$(capture_style "wg-color-mosh-$suffix" "⌁")
assert_contrast "mosh explicit 256-color" "$mosh_style"
launch "wg-color-termux-$suffix" "$main" "$main/.wg" 32 16 "TERMUX_VERSION=smoke COLORTERM=truecolor"
termux_style=$(capture_style "wg-color-termux-$suffix" "⌁")
assert_contrast "Termux explicit 16-color" "$termux_style"
termux_row=$(tmux capture-pane -p -t "wg-color-termux-$suffix" | grep -m1 '↯' || true)
[[ "$termux_row" == *"↯"* && "$termux_row" == *"⌁"* && "$termux_row" == *"⌂"* ]] \
    || loud_fail "Termux portrait lost one-row project grammar: $termux_row"
launch "wg-color-mono-$suffix" "$main" "$main/.wg" 80 mono "NO_COLOR=1 COLORTERM=truecolor"
mono_style=$(capture_style "wg-color-mono-$suffix" "⌁" no)
read -r mono_fg mono_bg _ _ <<<"$mono_style"
mono_row=$(tmux capture-pane -p -t "wg-color-mono-$suffix" | grep -m1 '↯' || true)
[[ "$mono_fg" == none && "$mono_bg" == none && "$mono_row" == *"↯"* && "$mono_row" == *"⌁"* && "$mono_row" == *"⌂"* ]] \
    || loud_fail "mono/NO_COLOR lost its color-free symbolic grammar: bar=$mono_style row=$mono_row"

echo "PASS: rich hash colors stable across canonical worktrees/nesting, distinct across projects, WCAG-safe in true/256/16 dark+light tmux/mosh/Termux, semantic and resize-stable"
