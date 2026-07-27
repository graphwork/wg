#!/usr/bin/env bash
# Installed-binary real tmux/PTY flow for selected-task persisted live progress.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
require_wg

scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config" "$scratch/leased-worktree"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
(cd "$project" && git init -q && git config user.email live@test.invalid && git config user.name Live && printf 'MAIN_ONLY\n' > main.txt && git add main.txt && git commit -qm baseline)
wg --dir "$project/.wg" init --no-agency >/dev/null
wg --dir "$project/.wg" add live-progress --id live-progress -d $'Inspect persisted activity only.\n\n## Validation\n- TUI is read-only' >/dev/null
wg --dir "$project/.wg" claim live-progress >/dev/null

fake_session="wg-live-fake-$$"
tui_session="wg-live-tui-$$"
cleanup() { tmux kill-session -t "$fake_session" 2>/dev/null || true; tmux kill-session -t "$tui_session" 2>/dev/null || true; }
add_cleanup_hook cleanup

tmux new-session -d -x 160 -y 50 -s "$fake_session" "env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' '$HERE/../../fixtures/fake-pi-watchdog' '$project/.wg' live-progress '$scratch/leased-worktree'"
send_fake() { tmux send-keys -t "$fake_session" "$1" Enter; sleep 0.18; }
send_fake 'init 0'
send_fake 'observe provider-start 1'

start_tui() {
  tmux new-session -d -x 160 -y 50 -s "$tui_session" "cd '$project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none wg --dir '$project/.wg' tui"
}
start_tui
sleep .8
tmux send-keys -t "$tui_session" 1
capture() { tmux capture-pane -p -S - -t "$tui_session" 2>/dev/null || true; }
dump() { wg --dir "$project/.wg" --json tui-dump 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))'; }
wait_for() { local needle=$1; for _ in $(seq 1 120); do dump | grep -Fq "$needle" && return 0; sleep .05; done; loud_fail "TUI never showed $needle: $(capture | tr '\n' '|')"; }
wait_for 'Phase: Waiting provider'
for label in 'Tokens:' 'Pi progress: receipt-proven' 'Worktree activity: observed/unproven' 'Tool/Test:' 'Watchdog/Resume:'; do dump | grep -Fq "$label" || loud_fail "missing accessible live row $label"; done
dump | grep -Fq 'silence-policy=300s observed-grace=120s cap=600s' || loud_fail "production 300/120/600 policy absent"

send_fake 'observe thinking-native 2'; wait_for 'Phase: Thinking'; dump | grep -Fq 'thinking=7' || loud_fail 'provided thinking count absent'
send_fake 'observe thinking-unknown 3'; wait_for 'thinking=Unknown'
send_fake 'observe output-5 4'; send_fake 'observe output-11 6'; wait_for 'Phase: Generating'; dump | grep -Eq 'rate=[0-9]' || loud_fail 'numeric output rate absent'
send_fake 'observe write-native 7'; wait_for 'Phase: Writing'
send_fake 'observe tool-native 8'; wait_for 'Phase: Tool'
send_fake 'observe test-native 9'; wait_for 'Phase: Testing'
send_fake 'observe usage-native 10'; wait_for 'receipts=1'
send_fake 'observe usage-native 11'; sleep 1.2
dump | grep -Fq 'receipts=1' || loud_fail 'usage replay double-counted'
send_fake 'observe tool-end-native 12'

# Help remains modal and owns input while persisted progress advances.
tmux send-keys -t "$tui_session" '?'; wait_for 'Keybindings'
tmux send-keys -t "$tui_session" PageDown
send_fake 'observe output-11 12'
sleep .4
dump | grep -Fq 'Keybindings' || loud_fail 'progress update displaced modal Help'
tmux send-keys -t "$tui_session" Escape; wait_for 'Phase: Generating'

# Persisted watchdog control states outrank ordinary activity.
send_fake 'observe provider-start 20'; send_fake 'tick 320'; wait_for 'Phase: Suspect'
send_fake 'tick 920'; send_fake 'observe probe 981'; send_fake 'tick 981'; wait_for 'Phase: Fencing'
send_fake 'observe launched 982'; wait_for 'Phase: Resuming'
send_fake 'observe permit 983'; send_fake 'observe output-11 984'; wait_for 'Phase: Generating'

# Resize deterministically retains provenance labels, then restores wide detail.
tmux resize-window -t "$tui_session" -x 52 -y 28; sleep .5
narrow=$(dump)
grep -Fq 'Phase:' <<<"$narrow" || loud_fail 'narrow layout lost phase'
grep -Fq 'receipt-proven' <<<"$narrow" || loud_fail 'narrow layout lost proven provenance'
grep -Fq 'observed/unproven' <<<"$narrow" || loud_fail 'narrow layout merged observed evidence'
tmux resize-window -t "$tui_session" -x 160 -y 50; sleep .4

# Passive reads/navigation do not mutate graph/lifecycle authority.
before=$(sha256sum "$project/.wg/graph.jsonl" | cut -d' ' -f1)
for _ in $(seq 1 20); do wg --dir "$project/.wg" --json tui-dump >/dev/null; done
tmux send-keys -t "$tui_session" Down Up; tmux resize-window -t "$tui_session" -x 120 -y 40; sleep .3
after=$(sha256sum "$project/.wg/graph.jsonl" | cut -d' ' -f1)
[[ "$before" == "$after" ]] || loud_fail 'TUI observation mutated graph/lifecycle state'

# Raw reasoning/tool-output canaries never enter screen or plain dump.
plain=$(dump)
! grep -Fq 'RAW_REASONING_CANARY_7f3b' <<<"$plain" || loud_fail 'reasoning canary leaked'
! grep -Fq 'HOSTILE_OUTPUT_CANARY_91ac' <<<"$plain" || loud_fail 'hostile output canary leaked'
# The context bar remains location/controls/context, not a telemetry ticker.
last=$(capture | tail -1)
! grep -Eq 'Pi progress|Worktree activity|tok/s|Watchdog/Resume' <<<"$last" || loud_fail "global context bar became telemetry: $last"

# Restart rebuilds the same persisted projection (no mtime/restart inference).
tmux send-keys -t "$tui_session" q; sleep .3; tmux kill-session -t "$tui_session" 2>/dev/null || true
start_tui; sleep .6; tmux send-keys -t "$tui_session" 1; wait_for 'Phase: Generating'
dump | grep -Fq 'receipts=1' || loud_fail 'restart lost/doubled usage projection'

# Task-generation terminal wins and freezes the compact summary.
wg --dir "$project/.wg" done live-progress >/dev/null
wait_for 'Phase: Done'
dump | grep -Fq 'Terminal summary: Done' || loud_fail 'terminal evidence summary absent'
send_fake 'observe thinking-native 200'; sleep .4
dump | grep -Fq 'Phase: Done' || loud_fail 'late event overwrote terminal phase'
! dump | grep -Fq 'RAW_REASONING_CANARY_7f3b' || loud_fail 'late reasoning leaked after terminal'

echo 'PASS: selected-task TUI renders persisted Pi/watchdog live progress with honest Unknown/proven-vs-observed labels, modal Help ownership, deterministic resize/restart, read-only refresh, and immutable terminal evidence'
