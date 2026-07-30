#!/usr/bin/env bash
# Default product journey: worksgood -> New chat -> bare attended Pi.
# Credential-free: a fake interactive Pi records argv/stdin while the real
# concierge, TUI, recursive chat-create path, PTY, and plugin materializer run.
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
WORKSGOOD="$CARGO_TARGET_DIR/debug/worksgood"
W="$CARGO_TARGET_DIR/debug/wg"
[[ -x "$WORKSGOOD" && -x "$W" ]] || loud_fail "candidate worksgood/wg bundle missing"

export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
export XDG_CACHE_HOME="$HOME/.cache"
export XDG_CONFIG_HOME="$HOME/.config"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER TMUX TMUX_TMPDIR
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$scratch/fakebin" "$scratch/repo"
git -C "$scratch/repo" init -q
PI_LOG="$scratch/pi.log"
export PI_LOG
cat >"$scratch/fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -u
printf 'argv=' >>"$PI_LOG"
printf ' <%s>' "$@" >>"$PI_LOG"
printf '\nchat=%s\n' "${WG_CHAT_ID:-missing}" >>"$PI_LOG"
echo "ATTENDED_PI_READY:${WG_CHAT_ID:-missing}"
while IFS= read -r line; do
    printf 'stdin=<%s>\n' "$line" >>"$PI_LOG"
    echo "ATTENDED_PI_ECHO:$line"
done
SH
chmod +x "$scratch/fakebin/pi"
export PATH="$scratch/fakebin:/usr/bin:/bin"

# Product/help boundary is explicit before any mutation.
help=$($WORKSGOOD --help)
grep -q 'Bare `worksgood` opens a route-free attended Pi chat surface' <<<"$help" \
    || loud_fail "help does not advertise the simple attended default"
grep -q 'setup.*unattended workers and evaluation.*advanced' <<<"$help" \
    || loud_fail "help does not separate advanced unattended automation"

# Dry-run proves the default never plans a route, profile, reasoning tier, or service.
dry=$($WORKSGOOD --project "$scratch/repo" --dry-run)
grep -q 'Attended Pi chat plan (read only)' <<<"$dry" || loud_fail "bare dry-run used automation planner: $dry"
grep -q 'route-free graph' <<<"$dry" || loud_fail "bare plan omitted route-free init: $dry"
grep -q 'Unattended worker/evaluator routes and services remain unchanged' <<<"$dry" \
    || loud_fail "bare plan omitted automation boundary: $dry"
[[ ! -e "$scratch/repo/.wg" && ! -e "$WG_GLOBAL_DIR" ]] \
    || loud_fail "bare dry-run wrote graph/profile/plugin state"

TM_SOCK="wgsmoke-attended-simple-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
cleanup_tmux() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux
session=attended
TM new-session -d -s "$session" -x 180 -y 50 \
    "env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' XDG_CACHE_HOME='$XDG_CACHE_HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' PATH='$PATH' PI_LOG='$PI_LOG' TERM=xterm-256color '$WORKSGOOD' --project '$scratch/repo'"
TM set-option -t "$session" remain-on-exit on
capture() { TM capture-pane -p -S - -t "$session" 2>/dev/null || true; }
wait_for() {
    local pattern=$1 tries=${2:-240}
    for _ in $(seq 1 "$tries"); do capture | grep -qE "$pattern" && return 0; sleep 0.05; done
    return 1
}
wait_for 'New chat|No chat selected' || loud_fail "bare worksgood did not open the real TUI: $(capture)"
G="$scratch/repo/.wg"
[[ -f "$G/graph.jsonl" ]] || loud_fail "route-free graph was not initialized"
[[ ! -e "$G/config.toml" && ! -e "$G/profile-selection.json" && ! -e "$G/concierge.json" ]] \
    || loud_fail "attended default wrote model/profile/concierge automation state: $(find "$G" -maxdepth 2 -type f -printf '%P\n')"
[[ ! -e "$G/service/state.json" ]] || loud_fail "attended default started a dispatcher service"
if grep -qE '"id":"\.(assign|evaluate|flip)-|"tags":\["agency"' "$G/graph.jsonl"; then
    loud_fail "attended default initialized agency/evaluator work"
fi
[[ -f "$HOME/.pi/agent/settings.json" ]] || loud_fail "compatible Pi console plugin was not ensured"
[[ ! -e "$WG_GLOBAL_DIR/active-profile" && ! -e "$WG_GLOBAL_DIR/config.toml" ]] \
    || loud_fail "attended plugin ensure selected a WG profile/global route"

# Drive the actual human flow and actual PTY keyboard path.
TM send-keys -t "$session" n
wait_for 'Pi \(choose model in Pi\)' || loud_fail "New chat did not default to bare Pi: $(capture)"
TM send-keys -t "$session" Enter
wait_for 'ATTENDED_PI_READY:.chat-0' || loud_fail "bare Pi did not attach: $(capture) log=$(cat "$PI_LOG" 2>/dev/null)"
input="hello-attended-$$+keyboard"
TM send-keys -t "$session" -l -- "$input"
TM send-keys -t "$session" Enter
for _ in $(seq 1 160); do
    grep -Fq "stdin=<$input>" "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -Fq "stdin=<$input>" "$PI_LOG" \
    || loud_fail "real TUI did not forward keyboard input to Pi: $(capture) log=$(cat "$PI_LOG")"
argv=$(grep '^argv=' "$PI_LOG")
# Stateful attended-session ownership flags are expected. Model/provider,
# reasoning, RPC, and hermetic-extension overrides are not.
if grep -Eq -- '<--model>|<--provider>|<--thinking>|<--mode>|<rpc>|<-e>|<-ne>' <<<"$argv"; then
    loud_fail "attended Pi argv contains managed/unattended model flags: $argv"
fi
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(x) for x in open(sys.argv[1]) if '"id":".chat-' in x]
assert len(rows)==1, rows
chat=rows[0]
assert chat['id']=='.chat-0', chat
assert chat.get('executor_preset_name')=='pi', chat
assert chat.get('model') in (None,''), chat
assert chat.get('reasoning') in (None,''), chat
assert chat.get('command_argv')==['pi'], chat
PY
[[ ! -e "$G/config.toml" && ! -e "$G/profile-selection.json" && ! -e "$G/service/state.json" ]] \
    || loud_fail "attended model/session activity rewrote automation state"

# Missing Pi is detected by the concierge before graph/profile/plugin/service writes.
missing_home="$scratch/missing-home"
missing_repo="$scratch/missing-repo"
mkdir -p "$missing_home" "$missing_repo" "$scratch/no-pi-bin"
git -C "$missing_repo" init -q
missing_session=missing
TM new-session -d -s "$missing_session" -x 140 -y 30 \
    "env HOME='$missing_home' WG_GLOBAL_DIR='$missing_home/.wg' XDG_CACHE_HOME='$missing_home/.cache' XDG_CONFIG_HOME='$missing_home/.config' PATH='$scratch/no-pi-bin' '$WORKSGOOD' --project '$missing_repo'"
TM set-option -t "$missing_session" remain-on-exit on
for _ in $(seq 1 160); do
    [[ "$(TM list-panes -t "$missing_session" -F '#{pane_dead}' 2>/dev/null || echo 1)" = 1 ]] && break
    sleep 0.05
done
missing_out=$(TM capture-pane -p -S - -t "$missing_session" 2>/dev/null || true)
[[ $(grep -c 'Pi executable not found' <<<"$missing_out") -eq 1 ]] \
    || loud_fail "missing Pi did not produce one concise action: $missing_out"
grep -q 'Install Pi and run `pi` to sign in/select a model' <<<"$missing_out" \
    || loud_fail "missing Pi action omitted Pi-owned install/login: $missing_out"
[[ ! -e "$missing_repo/.wg" && ! -e "$missing_home/.wg" && ! -e "$missing_home/.pi" ]] \
    || loud_fail "missing Pi wrote graph/model/profile/plugin/service state"

# Advanced automation remains explicit and exact. This dry-run is enough to
# distinguish its planner; the dedicated worksgood_one_model_setup smoke runs
# the authenticated service apply/reuse path end-to-end.
M='pi:openrouter:deepseek/deepseek-v4-flash'
auto=$($WORKSGOOD --project "$scratch/repo" setup --model "$M" --dry-run)
grep -q 'Unattended automation setup (advanced)' <<<"$auto" || loud_fail "setup did not enter advanced automation"
grep -qF "$M" <<<"$auto" || loud_fail "automation plan lost exact route"
grep -q 'Worker/chat.*effort high' <<<"$(tr -d '\n' <<<"$auto")" || loud_fail "automation plan lost effective worker reasoning"
grep -q 'Eval/assign/FLIP/weak roles.*effort low' <<<"$(tr -d '\n' <<<"$auto")" || loud_fail "automation plan lost effective evaluator reasoning"

echo "PASS: bare worksgood opened real route-free TUI -> bare Pi, forwarded keyboard input, preserved automation state; missing Pi stayed write-free; automation remained explicit"
