#!/usr/bin/env bash
# Real TUI/service/Fake-Pi regression for post-commit CreateChat response loss.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "real TUI/PTY flow requires tmux"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "graph assertions require python3"

CANDIDATE="${WG_SMOKE_CANDIDATE_BIN:-${WG_BIN:-$(command -v wg)}}"
CANDIDATE="$(readlink -f "$CANDIDATE")"
# Both daemon sockets are filesystem UDS names; keep the isolated fixture below
# sockaddr_un's path limit even when Cargo supplies a deep TMPDIR.
export WG_SMOKE_ROOT="/tmp/wgsmoke-chat-ipc"
scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
# Keep the private tmux socket path below Unix's sockaddr_un limit even when
# Cargo supplies a long per-agent TMPDIR.
export TMUX_TMPDIR="/tmp"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/fakebin"
# Every recursive TUI command and helper invocation must use the same candidate.
ln -s "$CANDIDATE" "$scratch/fakebin/wg"
export PATH="$scratch/fakebin:/usr/bin:/bin"
cd "$scratch"
G="$scratch/.wg"
PI_LOG="$scratch/pi.log"
export PI_LOG

cat >"$scratch/fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -u
printf 'chat=%s argv=' "${WG_CHAT_ID:-missing}" >>"$PI_LOG"
printf ' <%s>' "$@" >>"$PI_LOG"
printf '\n' >>"$PI_LOG"
echo "FAKE_PI_ATTENDED_READY:${WG_CHAT_ID:-missing}"
while IFS= read -r line; do
    printf 'stdin chat=%s value=<%s>\n' "${WG_CHAT_ID:-missing}" "$line" >>"$PI_LOG"
    echo "FAKE_PI_ECHO:$line"
done
SH
chmod +x "$scratch/fakebin/pi"

wg --dir "$G" init --no-agency >init.log 2>&1 || loud_fail "candidate init failed: $(cat init.log)"
wg --dir "$G" config --local \
    --model pi:openai-codex:gpt-5.6-sol --reasoning high \
    --max-coordinators 2 \
    --set-model evaluator pi:openai-codex:gpt-5.6-luna \
    --set-reasoning evaluator low --no-reload >config.log 2>&1 \
    || loud_fail "explicit Pi service contract failed: $(cat config.log)"
# Historical route-less FLIP/eval-shaped work remains in the graph. It must not
# own the attended chat lane even when a dispatcher tick is processing it.
cat >>"$G/graph.jsonl" <<'JSON'
{"kind":"task","id":".flip-implement-true-resilient","title":"historical route-less FLIP","status":"open","tags":["verification","agency"],"unplaced":true}
JSON

export WG_TEST_COORDINATOR_TICK_DELAY_MS=3600
export WG_TEST_CHAT_CREATE_RESPONSE_DELAY_MS=2600
start_wg_daemon "$scratch" --max-agents 0 --interval 1
DAEMON_LOG="$G/service/daemon.log"
for _ in $(seq 1 100); do
    grep -q 'Coordinator tick #1 starting' "$DAEMON_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -q 'Coordinator tick #1 starting' "$DAEMON_LOG" 2>/dev/null \
    || loud_fail "slow coordinator tick did not start: $(tail -40 "$DAEMON_LOG" 2>/dev/null)"

TM_SOCK="wgsmoke-chat-ipc-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
cleanup_tmux() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux
outer="tui-chat-ipc"
TM new-session -d -s "$outer" -x 180 -y 50 \
    "env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' PATH='$PATH' PI_LOG='$PI_LOG' TERM=xterm-256color '$CANDIDATE' --dir '$G' tui --no-mouse"
capture() { TM capture-pane -p -t "$outer" 2>/dev/null || true; }
wait_for() {
    local pattern="$1" tries="${2:-160}"
    for _ in $(seq 1 "$tries"); do capture | grep -qE "$pattern" && return 0; sleep 0.05; done
    return 1
}
chat_count() {
    python3 - "$G/graph.jsonl" <<'PY'
import json,sys
print(sum(1 for line in open(sys.argv[1]) if (lambda x:x.get("id","").startswith(".chat-"))(json.loads(line))))
PY
}

wait_for 'New chat|No chat' || loud_fail "TUI did not reach empty attended surface: $(capture)"
TM send-keys -t "$outer" n
wait_for 'Pi \(choose model in Pi\)' || loud_fail "New chat did not offer bare attended Pi: $(capture)"
start_ms=$(date +%s%3N)
# Double-submit is swallowed by the launcher's creating gate.
TM send-keys -t "$outer" Enter Enter
for _ in $(seq 1 200); do
    grep -q '^chat=.chat-0 ' "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -q '^chat=.chat-0 ' "$PI_LOG" 2>/dev/null \
    || loud_fail "committed chat did not reconcile/start through real TUI: $(capture) pi=$(cat "$PI_LOG" 2>/dev/null) graph=$(cat "$G/graph.jsonl" 2>/dev/null) daemon=$(tail -80 "$DAEMON_LOG" 2>/dev/null)"
elapsed=$(( $(date +%s%3N) - start_ms ))
(( elapsed >= 2000 )) || loud_fail "fault injection did not cross the historical 2s response budget (${elapsed}ms)"
[[ "$(chat_count)" == 1 ]] || loud_fail "lost response created duplicate chat rows: $(cat "$G/graph.jsonl")"
wait_for 'FAKE_PI_ATTENDED_READY:.chat-0' \
    || loud_fail "reconciled chat is not visibly attached: $(capture)"
wait_for 'Chat input.*Ctrl\+O commands' \
    || loud_fail "PTY-focused command escape is not discoverable: $(capture)"

# The durable pane is not success unless the real WG TUI forwards exact input.
# Include '+' to pin the focused-child printable policy from
# docs/bugs/tui-keymap-routing.md; Enter supplies the exact line terminator.
input_one="TUI_INPUT_ONE_$$+plus"
TM send-keys -t "$outer" -l -- "$input_one"
TM send-keys -t "$outer" Enter
for _ in $(seq 1 100); do
    grep -Fq "stdin chat=.chat-0 value=<$input_one>" "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -Fq "stdin chat=.chat-0 value=<$input_one>" "$PI_LOG" \
    || loud_fail "focused WG TUI did not forward exact bytes to Pi: $(capture) pi=$(cat "$PI_LOG")"

# Ctrl+O's audited contract is a symmetric PTY/command-mode toggle, not a
# vendor keystroke: PTY mode advertises it, command mode paints available keys,
# and '?' opens the complete Help surface. Escape restores command mode; a
# second Ctrl+O must return to the same live pane and keep forwarding input.
TM send-keys -t "$outer" C-o
wait_for 'Commands.*n New chat.*Help.*Ctrl\+O ret' \
    || loud_fail "Ctrl+O did not expose command keys: $(capture)"
TM send-keys -t "$outer" -l -- '?'
wait_for 'Essential navigation' \
    || loud_fail "command-mode Help is not reachable after Ctrl+O: $(capture)"
wait_for 'Ctrl-O' || loud_fail "Help omitted the documented Ctrl-O contract"
TM send-keys -t "$outer" Escape
wait_for 'Commands.*n New chat' || loud_fail "Help dismissal did not restore command mode"
TM send-keys -t "$outer" C-o
wait_for 'Chat input.*Ctrl\+O commands' \
    || loud_fail "Ctrl+O did not return to focused Pi input: $(capture)"
input_two="TUI_INPUT_TWO_$$"
TM send-keys -t "$outer" -l -- "$input_two"
TM send-keys -t "$outer" Enter
for _ in $(seq 1 100); do
    grep -Fq "stdin chat=.chat-0 value=<$input_two>" "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -Fq "stdin chat=.chat-0 value=<$input_two>" "$PI_LOG" \
    || loud_fail "Pi input did not recover after command/help round-trip: $(capture) pi=$(cat "$PI_LOG")"

bare_line=$(grep '^chat=.chat-0 ' "$PI_LOG" || true)
[[ -n "$bare_line" ]] || loud_fail "bare Pi invocation missing: $(cat "$PI_LOG")"
if grep '^chat=.chat-0 ' "$PI_LOG" | grep -Eq -- '<--model>|<--provider>|<-m>'; then
    loud_fail "inherited attended Pi leaked the worker profile model: $bare_line"
fi
[[ $(grep -c '^chat=.chat-0 ' "$PI_LOG") -eq 1 ]] || loud_fail "bare chat launched more than once: $(cat "$PI_LOG")"

# Create a second chat through the actual New-chat model-edit UI, this time
# explicitly pinning an exact Pi route. It must remain exact rather than being
# confused with the inherited bare attended choice.
TM send-keys -t "$outer" C-o
wait_for 'Commands.*n New chat' || loud_fail "could not return to command mode for second chat"
TM send-keys -t "$outer" n
wait_for 'Pi \(choose model in Pi\)' || loud_fail "second New chat launcher did not open"
TM send-keys -t "$outer" m
TM send-keys -t "$outer" 'pi:openai-codex:gpt-5.6-sol'
TM send-keys -t "$outer" Enter Enter
for _ in $(seq 1 220); do
    grep -q '^chat=.chat-1 ' "$PI_LOG" 2>/dev/null && break
    sleep 0.05
done
grep -q '^chat=.chat-1 ' "$PI_LOG" 2>/dev/null \
    || loud_fail "explicit Pi chat did not attach: $(capture) log=$(cat "$PI_LOG" 2>/dev/null)"
[[ "$(chat_count)" == 2 ]] || loud_fail "explicit create duplicated or lost a row: $(cat "$G/graph.jsonl")"
explicit_line=$(grep '^chat=.chat-1 ' "$PI_LOG" || true)
grep -q '<--provider> <openai-codex> <--model> <gpt-5.6-sol>' <<<"$explicit_line" \
    || loud_fail "explicit per-chat Pi argv was not exact: $explicit_line"
[[ $(grep -c '^chat=.chat-1 ' "$PI_LOG") -eq 1 ]] || loud_fail "explicit chat launched more than once: $(cat "$PI_LOG")"

python3 - "$G/graph.jsonl" <<'PY'
import json,sys
chats=[json.loads(x) for x in open(sys.argv[1]) if '"id":".chat-' in x]
assert len(chats)==2,chats
by={x['id']:x for x in chats}
assert by['.chat-0'].get('model') in (None,''),by['.chat-0']
assert by['.chat-0'].get('command_argv')==['pi'],by['.chat-0']
receipt0=by['.chat-0'].get('log',[{}])[0].get('actor','')
assert receipt0.startswith('chat-create-request:chat-create-'),by['.chat-0']
assert by['.chat-1'].get('model')=='pi:openai-codex:gpt-5.6-sol',by['.chat-1']
receipt1=by['.chat-1'].get('log',[{}])[0].get('actor','')
assert receipt1.startswith('chat-create-request:chat-create-'),by['.chat-1']
assert receipt0 != receipt1
PY

# Let both deliberately-late replies hit closed clients. They are expected
# cancellation evidence, never daemon corruption/BrokenPipe errors.
sleep 1
cancelled=$(grep -c 'client disconnected before late response; treating reply as cancelled' "$DAEMON_LOG" 2>/dev/null || true)
(( cancelled >= 2 )) || loud_fail "late-response cancellation evidence missing: $(tail -80 "$DAEMON_LOG")"
if grep -q 'Error handling connection: Broken pipe' "$DAEMON_LOG"; then
    loud_fail "late reply was still logged as daemon corruption: $(tail -80 "$DAEMON_LOG")"
fi

# A transaction that does NOT commit must fail clearly and leave no graph row,
# Pi process, or tmux owner. The cap is two, so a third actual TUI launch drives
# the server-side mutation refusal rather than a client-side form validation.
TM send-keys -t "$outer" C-o
sleep 0.15
TM send-keys -t "$outer" n
# Depending on whether the outer nested-tmux attach has already exited, `n`
# either opens the launcher or completes the queued New-chat command directly.
# In both real TUI states the mutation must reach the daemon and be refused.
sleep 0.15
if ! capture | grep -q 'Chat cap reached (2/2)'; then
    wait_for 'Pi \(choose model in Pi\)' \
        || loud_fail "failed-create launcher did not open: $(capture)"
    TM send-keys -t "$outer" Enter
fi
wait_for 'Chat cap reached \(2/2\)' \
    || loud_fail "uncommitted create did not fail clearly: $(capture)"
[[ "$(chat_count)" == 2 && "$(grep -c '^chat=' "$PI_LOG")" == 2 ]] \
    || loud_fail "failed create left graph/process residue: graph=$(cat "$G/graph.jsonl") pi=$(cat "$PI_LOG")"
[[ $(TM list-sessions -F '#{session_name}' | grep -c '^wg-chat-' || true) -eq 2 ]] \
    || loud_fail "failed create left a tmux runtime owner"
TM send-keys -t "$outer" Escape

# Restart the service while both path-owned Pi panes are live. The service may
# observe/defer to those owners, but it must not spawn beside them.
invocations_before=$(grep -c '^chat=' "$PI_LOG")
sessions_before=$(TM list-sessions -F '#{session_name}' | grep -c '^wg-chat-' || true)
[[ "$invocations_before" -eq 2 && "$sessions_before" -eq 2 ]] \
    || loud_fail "pre-restart ownership is not singular: invocations=$invocations_before sessions=$sessions_before"
unset WG_TEST_COORDINATOR_TICK_DELAY_MS WG_TEST_CHAT_CREATE_RESPONSE_DELAY_MS
wg --dir "$G" service stop --force >stop.log 2>&1 || loud_fail "service stop failed: $(cat stop.log)"
start_wg_daemon "$scratch" --max-agents 0 --interval 1
sleep 2
invocations_after=$(grep -c '^chat=' "$PI_LOG")
sessions_after=$(TM list-sessions -F '#{session_name}' | grep -c '^wg-chat-' || true)
[[ "$invocations_after" -eq 2 && "$sessions_after" -eq 2 ]] \
    || loud_fail "service restart competed with live tmux owner: invocations=$invocations_after sessions=$sessions_after log=$(cat "$PI_LOG")"
[[ "$(chat_count)" == 2 ]] || loud_fail "service restart duplicated graph chats"

# The actual Pi plugin/model bridge reports later attended model changes via
# `wg chat model --warm-pi-writeback`; verify that persisted chat observation
# does not rewrite the project worker/agency contract.
wg --dir "$G" chat model .chat-0 pi:openai-codex:gpt-5.6-luna --warm-pi-writeback >/dev/null \
    || loud_fail "managed Pi model observation write-back failed"
grep -q 'pi:openai-codex:gpt-5.6-sol' "$G/config.toml" \
    || loud_fail "attended session model observation rewrote project worker route"
grep -q 'pi:openai-codex:gpt-5.6-luna' "$G/service/coordinator-state-0.json" \
    || loud_fail "actual attended session model was not persisted on the exact chat"

echo "PASS: >2s lost replies reconciled exact chats; custom-tmux Pi input, Ctrl+O command/help round-trip, bare/explicit argv, restart, and ownership stayed exact"
