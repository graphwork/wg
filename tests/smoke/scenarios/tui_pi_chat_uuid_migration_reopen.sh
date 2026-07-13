#!/usr/bin/env bash
# Permanent regression for fix-pi-chat.
#
# Drive the real TUI in tmux with a Pi chat whose native transcript starts in
# the pre-UUID `chat/chat-N/pi-sessions` directory. Opening the pane must first
# register/migrate the chat, launch Pi with the UUID session directory, preserve
# the transcript, and remain usable after the TUI pane is reopened. A fake Pi
# provides a deterministic credential-free terminal UI while enforcing the real
# --session-id/--session-dir contract.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the TUI human flow"
fi

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$TMUX_TMPDIR"
G="$scratch/.wg"
WG_BIN="$(command -v wg)"
cd "$scratch"

outer="wgsmoke-fix-pi-chat-$$"
outer2="${outer}-reopen"
project_tag="$(basename "$scratch" | tr ':.' '--')"
inner="wg-chat-${project_tag}-chat-0"
cleanup_tmux() {
    tmux kill-session -t "$outer" 2>/dev/null || true
    tmux kill-session -t "$outer2" 2>/dev/null || true
    tmux kill-session -t "$inner" 2>/dev/null || true
}
add_cleanup_hook cleanup_tmux

fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
pi_log="$scratch/pi.log"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -u
PI_SMOKE_LOG="__PI_LOG__"
session_dir=""
session_id=""
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
    case "${args[$i]}" in
        --session-dir) session_dir="${args[$((i+1))]:-}" ;;
        --session-id) session_id="${args[$((i+1))]:-}" ;;
    esac
done
printf 'ARGV:%q ' "$@" >>"$PI_SMOKE_LOG"
printf '\n' >>"$PI_SMOKE_LOG"
if [[ -z "$session_dir" || -z "$session_id" ]]; then
    echo "PI_RECOVERY: missing session flags"
    exit 2
fi
mkdir -p "$session_dir"
transcript=$(find "$session_dir" -maxdepth 1 -type f -name "*_${session_id}.jsonl" -print -quit 2>/dev/null || true)
if [[ -n "$transcript" ]]; then
    payload=$(tail -1 "$transcript" 2>/dev/null || true)
    echo "PI_HISTORY_RESTORED $payload"
else
    transcript="$session_dir/2026-07-12T00-00-00-000Z_${session_id}.jsonl"
    printf '{"type":"session","version":3,"id":"%s"}\n' "$session_id" >"$transcript"
    echo "PI_RECOVERABLE_NEW_SESSION"
fi
echo "PI_PANE_READY"
while IFS= read -r line; do
    echo "PI_USABLE:$line"
done
SH
sed -i "s|__PI_LOG__|$pi_log|g" "$fakebin/pi"
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
export PI_SMOKE_LOG="$pi_log"

wg --dir "$G" init >/dev/null
cat >"$G/config.toml" <<'TOML'
[coordinator]
executor = "pi"
TOML

# Create the user-visible Pi chat, then model the exact on-disk state left by
# the old ordering bug: graph/state exist, but storage is still at chat/chat-0
# and registration has not committed sessions.json yet.
wg --dir "$G" chat new --name migration-smoke --executor pi >"$scratch/create.log" 2>&1 \
    || loud_fail "could not create Pi chat fixture: $(cat "$scratch/create.log")"
# `wg chat new` records task/state; without a daemon it intentionally has not
# registered filesystem storage yet. Seed the legacy path directly, matching
# the old TUI-before-daemon ordering.
rm -f "$G/chat/sessions.json" "$G/chat/sessions.json.tmp"
mkdir -p "$G/chat/chat-0/pi-sessions"
historical="2026-07-12T09-11-33-232Z_chat-0.jsonl"
printf '%s\n' '{"type":"session","version":3,"id":"chat-0"}' \
    '{"type":"message","message":{"role":"user","content":"PRESERVED_HISTORY"}}' \
    >"$G/chat/chat-0/pi-sessions/$historical"

launch_tui() {
    local name="$1"
    tmux new-session -d -s "$name" -x 180 -y 50 \
        "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' TMUX_TMPDIR='$TMUX_TMPDIR' PI_SMOKE_LOG='$PI_SMOKE_LOG' '$WG_BIN' --dir '$G' tui"
}
wait_marker() {
    local name="$1" marker="$2"
    for _ in $(seq 1 80); do
        capture=$(tmux capture-pane -t "$name" -p 2>/dev/null || true)
        if grep -qF "$marker" <<<"$capture"; then return 0; fi
        sleep 0.25
    done
    return 1
}

# Human flow: open the chat in the real TUI.
launch_tui "$outer"
wait_marker "$outer" "PI_HISTORY_RESTORED" \
    || loud_fail "Pi pane did not restore the migrated transcript. pi log: $(cat "$pi_log" 2>/dev/null || true) pane: $(tmux capture-pane -t "$outer" -p 2>/dev/null || true)"

canonical=$(python3 - "$G/chat/sessions.json" <<'PY'
import json, sys
r=json.load(open(sys.argv[1]))
for uuid, meta in r.get("sessions", {}).items():
    if "chat-0" in meta.get("aliases", []): print(uuid); break
PY
)
[[ -f "$G/chat/$canonical/pi-sessions/$historical" ]] \
    || loud_fail "historical Pi transcript was not preserved in UUID directory"
if grep -q -- "--session-dir $G/chat/chat-0/pi-sessions" "$pi_log"; then
    loud_fail "TUI launched Pi with migration-prone legacy session directory: $(cat "$pi_log")"
fi
if grep -qi "ENOENT\|No such file or directory" "$pi_log" \
   || tmux capture-pane -t "$outer" -p 2>/dev/null | grep -qi "ENOENT\|No such file or directory"; then
    loud_fail "raw missing-transcript ENOENT leaked into the Pi pane"
fi

# Close only the outer human TUI. The nested persistent chat pane remains; a
# second real TUI launch must reattach and still accept input.
tmux kill-session -t "$outer"
sleep 0.5
launch_tui "$outer2"
wait_marker "$outer2" "PI_PANE_READY" \
    || loud_fail "reopened TUI did not recover a usable Pi pane"
tmux send-keys -t "$outer2" "reopen-ok" Enter
wait_marker "$outer2" "PI_USABLE:reopen-ok" \
    || loud_fail "reopened Pi pane did not accept user input: $(tmux capture-pane -t "$outer2" -p 2>/dev/null || true)"
if tmux capture-pane -t "$outer2" -p 2>/dev/null | grep -qi "ENOENT\|No such file or directory"; then
    loud_fail "pane reopen repeatedly emitted a raw missing-transcript error"
fi

echo "PASS: Pi TUI chat migrated legacy transcript to UUID storage, reopened without ENOENT, and remained usable"
