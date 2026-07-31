#!/usr/bin/env bash
# Thin worksgood launcher regression: an existing graph is exactly wg tui;
# only a missing graph enters the one-time route-free bootstrap. Credential-free.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [[ -n "${WG_SMOKE_CANDIDATE_DIR:-}" ]]; then
    CARGO_TARGET_DIR="$WG_SMOKE_CANDIDATE_DIR"
else
    CARGO_TARGET_DIR="$scratch/candidate-target"
    (cd "$repo_root" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" CARGO_BUILD_JOBS=1 \
        cargo build --quiet --bin wg --bin worksgood)
fi
REAL_WORKSGOOD="$CARGO_TARGET_DIR/debug/worksgood"
REAL_W="$CARGO_TARGET_DIR/debug/wg"
[[ -x "$REAL_WORKSGOOD" && -x "$REAL_W" ]] || loud_fail "candidate worksgood/wg bundle missing"

export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_CONFIG_HOME="$HOME/.config"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER TMUX TMUX_TMPDIR
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$scratch/no-pi-bin" "$scratch/bundle"

# Use a same-bundle absolute sibling shim so the real PTY flow also records the
# exact argv crossing the worksgood -> wg boundary. The shim immediately execs
# the candidate wg; an unknown PATH wg must never be consulted.
cp "$REAL_WORKSGOOD" "$scratch/bundle/worksgood"
INVOCATION_LOG="$scratch/wg-invocations.log"
PATH_WG_SENTINEL="$scratch/PATH_WG_EXECUTED"
export REAL_W INVOCATION_LOG PATH_WG_SENTINEL
cat >"$scratch/bundle/wg" <<'SH'
#!/usr/bin/env bash
set -eu
printf '<%s>' "$@" >>"$INVOCATION_LOG"
printf '\n' >>"$INVOCATION_LOG"
exec "$REAL_W" "$@"
SH
cat >"$scratch/no-pi-bin/wg" <<'SH'
#!/usr/bin/env bash
touch "$PATH_WG_SENTINEL"
exit 91
SH
chmod +x "$scratch/bundle/worksgood" "$scratch/bundle/wg" "$scratch/no-pi-bin/wg"
WORKSGOOD="$scratch/bundle/worksgood"
W="$scratch/bundle/wg"
# Keep normal OS helpers but deliberately expose neither Pi nor the candidate wg
# through PATH. The launcher has to use its authenticated absolute sibling.
export PATH="$scratch/no-pi-bin:/usr/bin:/bin"
command -v pi >/dev/null 2>&1 && loud_fail "test PATH unexpectedly contains Pi"

TM_SOCK="wgsmoke-thin-worksgood-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
cleanup_tmux() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux

capture() { TM capture-pane -p -S - -t "$1" 2>/dev/null || true; }
wait_for() {
    local session=$1 pattern=$2 tries=${3:-1200}
    for _ in $(seq 1 "$tries"); do
        capture "$session" | grep -qE "$pattern" && return 0
        sleep 0.05
    done
    return 1
}
wait_exit() {
    local session=$1
    for _ in $(seq 1 1200); do
        [[ "$(TM list-panes -t "$session" -F '#{pane_dead}' 2>/dev/null || echo 1)" = 1 ]] && return 0
        sleep 0.05
    done
    loud_fail "tmux session did not exit: $session ($(capture "$session"))"
}
start_tui() {
    local session=$1 command=$2
    : >"$scratch/$session.out"
    TM new-session -d -s "$session" -x 200 -y 55 \
        "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' PATH='$PATH' REAL_W='$REAL_W' INVOCATION_LOG='$INVOCATION_LOG' PATH_WG_SENTINEL='$PATH_WG_SENTINEL' TERM=xterm-256color $command"
    TM set-option -t "$session" remain-on-exit on
    wait_for "$session" 'New chat|No chat selected|Workspace' \
        || loud_fail "$session did not open the real TUI: $(capture "$session")"
}
quit_tui() {
    local session=$1
    capture "$session" >"$scratch/$session.out"
    TM send-keys -t "$session" q
    wait_exit "$session"
}
snapshot_files() {
    local root=$1
    # Each real wg process appends one timestamped usage receipt. Compare that
    # effect by line-count below; byte hashes necessarily differ by timestamp.
    find "$root" -type f ! -name usage.log -printf '%P ' -exec sha256sum {} \; 2>/dev/null | sort
}
usage_count() {
    local graph=$1
    [[ -f "$graph/usage.log" ]] && wc -l <"$graph/usage.log" || printf '0\n'
}
assert_no_setup_state() {
    local graph=$1
    [[ ! -e "$graph/config.toml" ]] || loud_fail "launcher wrote config.toml"
    [[ ! -e "$graph/profile-selection.json" ]] || loud_fail "launcher wrote project profile selection"
    [[ ! -e "$graph/concierge-pending.json" ]] || loud_fail "launcher wrote concierge transaction state"
    [[ ! -e "$graph/service/state.json" ]] || loud_fail "launcher touched/started a service"
    [[ ! -e "$WG_GLOBAL_DIR/config.toml" && ! -e "$WG_GLOBAL_DIR/active-profile" ]] \
        || loud_fail "launcher wrote global route/profile state"
    [[ ! -e "$HOME/.pi" && ! -e "$XDG_CACHE_HOME/wg/worksgood-pi" ]] \
        || loud_fail "launcher discovered/prepared Pi plugin state"
}

# Help states the returning-entry boundary and points automation at setup.
help=$($WORKSGOOD --help)
grep -q 'already has `.wg`.*same setup-neutral TUI as `wg tui`' <<<"$help" \
    || loud_fail "help omits thin existing-graph boundary"
grep -q 'new repository.*minimal route-free graph bootstrap' <<<"$help" \
    || loud_fail "help omits one-time bootstrap boundary"
grep -q 'setup.*unattended workers and evaluation.*advanced' <<<"$help" \
    || loud_fail "help omits explicit advanced setup"

# Existing graph: compare real PTY invocations of wg tui, worksgood tui, and
# bare worksgood on the same graph. An intentionally invalid concierge file
# proves returning bare entry does not parse/reconcile it.
repo="$scratch/existing"
mkdir -p "$repo/nested"
git -C "$repo" init -q
"$REAL_W" --dir "$repo/.wg" init --no-agency >/dev/null
printf '{ deliberately invalid concierge state\n' >"$repo/.wg/concierge.json"
G="$(cd "$repo/.wg" && pwd)"
graph_before=$(sha256sum "$G/graph.jsonl")
: >"$INVOCATION_LOG"
usage_before=$(usage_count "$G")

start_tui direct "'$W' --dir '$G' tui"
quit_tui direct
direct_argv=$(tail -n 1 "$INVOCATION_LOG")
direct_effect=$(snapshot_files "$G")
usage_direct=$(usage_count "$G")

start_tui alias "'$WORKSGOOD' --project '$repo/nested' tui"
quit_tui alias
alias_argv=$(tail -n 1 "$INVOCATION_LOG")
alias_effect=$(snapshot_files "$G")
usage_alias=$(usage_count "$G")

start_tui bare "'$WORKSGOOD' --project '$repo/nested'"
quit_tui bare
bare_argv=$(tail -n 1 "$INVOCATION_LOG")
bare_effect=$(snapshot_files "$G")
usage_bare=$(usage_count "$G")

expected="<--dir><$G><tui>"
[[ "$direct_argv" = "$expected" && "$alias_argv" = "$expected" && "$bare_argv" = "$expected" ]] \
    || loud_fail "TUI argv drift: direct=$direct_argv alias=$alias_argv bare=$bare_argv expected=$expected"
if [[ "$direct_effect" != "$alias_effect" || "$alias_effect" != "$bare_effect" ]]; then
    printf '%s\n' "$direct_effect" >"$scratch/direct.effect"
    printf '%s\n' "$alias_effect" >"$scratch/alias.effect"
    printf '%s\n' "$bare_effect" >"$scratch/bare.effect"
    effect_diff=$(diff -u "$scratch/direct.effect" "$scratch/alias.effect" || true)
    effect_diff+=$(diff -u "$scratch/alias.effect" "$scratch/bare.effect" || true)
    loud_fail "bare/alias/direct TUI state effects differ: $effect_diff"
fi
[[ "$graph_before" = "$(sha256sum "$G/graph.jsonl")" ]] || loud_fail "opening TUI mutated graph tasks"
[[ $((usage_direct - usage_before)) -eq 1 && $((usage_alias - usage_direct)) -eq 1 && $((usage_bare - usage_alias)) -eq 1 ]] \
    || loud_fail "direct/alias/bare did not append the same single wg usage receipt: $usage_before/$usage_direct/$usage_alias/$usage_bare"
[[ "$(grep -cF "$expected" "$INVOCATION_LOG")" -eq 3 ]] \
    || loud_fail "returning entry ran commands besides the one TUI launch: $(cat "$INVOCATION_LOG")"
! grep -qE 'Initialized|Automation|Pi executable|profile|concierge' "$scratch/bare.out" \
    || loud_fail "returning bare entry printed onboarding/setup prose: $(cat "$scratch/bare.out")"
[[ ! -e "$PATH_WG_SENTINEL" ]] || loud_fail "launcher executed unknown PATH wg"
assert_no_setup_state "$G"

# Pi is absent, yet bare worksgood opened above. Only the actual New chat -> Pi
# choice reports the actionable executor error, transactionally (no chat row,
# plugin, profile, or service state).
start_tui missingpi "'$WORKSGOOD' --project '$repo'"
TM send-keys -t missingpi n
wait_for missingpi 'Pi \(choose model in Pi\)' \
    || loud_fail "New chat did not expose Pi choice: $(capture missingpi)"
TM send-keys -t missingpi Enter
wait_for missingpi 'Pi executable.*not found on PATH|no chat was created.*no fallback' \
    || loud_fail "choosing unavailable Pi did not show actionable error: $(capture missingpi)"
missing_screen=$(capture missingpi)
grep -qi 'no chat was created\|install Pi' <<<"$missing_screen" \
    || loud_fail "Pi error omitted transactional/actionable guidance: $missing_screen"
if grep -qE '"id"[[:space:]]*:[[:space:]]*"\.chat-' "$G/graph.jsonl"; then
    loud_fail "unavailable Pi choice created a chat row"
fi
TM send-keys -t missingpi Escape
TM send-keys -t missingpi q
wait_exit missingpi
assert_no_setup_state "$G"

# Fresh repository: bare worksgood performs init --no-agency exactly once and
# then uses the identical TUI command. The second entry is already the thin path.
fresh="$scratch/fresh"
mkdir -p "$fresh"
git -C "$fresh" init -q
: >"$INVOCATION_LOG"
start_tui fresh1 "'$WORKSGOOD' --project '$fresh'"
quit_tui fresh1
FG="$(cd "$fresh/.wg" && pwd)"
fresh_first_log=$(cat "$INVOCATION_LOG")
[[ "$(sed -n '1p' "$INVOCATION_LOG")" = "<--dir><$fresh/.wg><init><--no-agency>" ]] \
    || loud_fail "fresh bootstrap did not use route-free init: $fresh_first_log"
[[ "$(sed -n '2p' "$INVOCATION_LOG")" = "<--dir><$fresh/.wg><tui>" ]] \
    || loud_fail "fresh bootstrap did not open exact graph TUI: $fresh_first_log"
[[ "$(wc -l <"$INVOCATION_LOG")" -eq 2 ]] || loud_fail "fresh bootstrap ran extra commands: $fresh_first_log"
assert_no_setup_state "$FG"
if grep -qE '"id":"\.(assign|evaluate|flip)-|"tags":\["agency"' "$FG/graph.jsonl"; then
    loud_fail "route-free bootstrap initialized agency/evaluation tasks"
fi
fresh_state=$(snapshot_files "$FG")
start_tui fresh2 "'$WORKSGOOD' --project '$fresh'"
quit_tui fresh2
[[ "$(wc -l <"$INVOCATION_LOG")" -eq 3 ]] || loud_fail "second entry repeated bootstrap: $(cat "$INVOCATION_LOG")"
[[ "$(tail -n 1 "$INVOCATION_LOG")" = "<--dir><$FG><tui>" ]] \
    || loud_fail "second entry was not the thin exact-graph TUI path: $(cat "$INVOCATION_LOG")"
[[ "$fresh_state" = "$(snapshot_files "$FG")" ]] || loud_fail "second thin entry changed setup/TUI state"
assert_no_setup_state "$FG"

# The canonical explicit setup verb keeps exact route, reasoning, and service
# gates and remains a non-mutating dry-run. No-option returning entry above did
# not enter this advanced path.
mkdir -p "$scratch/with-pi"
cat >"$scratch/with-pi/pi" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x "$scratch/with-pi/pi"
M='pi:openrouter:deepseek/deepseek-v4-flash'
setup_before=$(snapshot_files "$repo")
auto=$(PATH="$scratch/with-pi:$PATH" "$WORKSGOOD" --project "$repo" setup --model "$M" --dry-run)
grep -q 'Unattended automation setup (advanced)' <<<"$auto" || loud_fail "setup did not enter advanced concierge"
grep -qF "$M" <<<"$auto" || loud_fail "setup lost exact route"
grep -q 'Worker/chat.*effort high' <<<"$(tr -d '\n' <<<"$auto")" || loud_fail "setup lost worker reasoning gate"
grep -q 'Eval/assign/FLIP/weak roles.*effort low' <<<"$(tr -d '\n' <<<"$auto")" || loud_fail "setup lost evaluation reasoning gate"
grep -q 'service_action' <<<"$auto" || loud_fail "setup plan omitted service gate"
[[ "$setup_before" = "$(snapshot_files "$repo")" ]] || loud_fail "setup dry-run mutated repository"

echo "PASS: existing bare worksgood == worksgood tui == wg tui with no setup/Pi mutation; missing Pi fails only at New chat; fresh bootstrap runs once; setup remains explicit"
