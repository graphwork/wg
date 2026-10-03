#!/usr/bin/env bash
# Scenario: tui_pi_scroll_forward (let-pi-own)
#
# Live human-flow regression that a chat pane whose child owns its own
# scrollback (pi) receives WG's scroll input FORWARDED to the child, instead
# of WG synthesizing a scroll of its own copy of the pane.
#
# The bug: in chat PTY mode the wheel (and PgUp/PgDn/Home/End) drove
# `PtyPane::scroll_up/down` — WG's own tmux copy-mode / vt100 scrollback — so
# pi's own scrollback, its alternate-screen handling, and its scroll UI were
# unreachable; the human scrolled WG's copy instead.
#
# The fix (let-pi-own): a chat child that is scroll-capable (alternate screen
# and/or mouse reporting, probed directly for a raw PTY child and via tmux's
# `alternate_on`/`mouse_any_flag`/`mouse_sgr_flag` for a tmux-wrapped chat
# pane) receives the gesture in its OWN input encoding — an SGR (or legacy)
# mouse-wheel report for a wheel when it enabled mouse reporting, else its own
# PageUp/PageDown/Home/End key bytes. WG's own scrollback stays reachable as an
# explicit fallback (Ctrl+] scroll mode) and is used automatically when the
# child is not scroll-capable.
#
# Human-flow simulation: a fake `pi` on PATH emulates pi's real terminal
# negotiation (alternate screen + SGR mouse reporting — the modes real pi
# enables, verified on `pi 1.0.1`: `alternate_on=1 mouse_any_flag=1
# mouse_sgr_flag=1`). It then echoes every byte it receives (`cat -v`) so the
# inner tmux pane is a faithful byte-level record of what reached the child.
# The scenario drives the REAL `wg tui` in tmux, wheels and PgUp's the focused
# pi pane through the outer terminal, and asserts:
#   1. the fake pi's pane shows an SGR mouse-wheel report (ESC[<64;...M) — the
#      wheel was forwarded to the child, not applied to WG's buffer;
#   2. the fake pi's pane shows the PageUp key bytes (ESC[5~);
#   3. WG's forwarded-input tee (WG_PTY_DUMP) contains the same bytes — WG is
#      the writer (`mouse off` on the wrapping session rules out tmux native
#      mouse delivery);
#   4. the wrapping wg-chat session is NOT in copy-mode — WG did not fall back
#      to synthesizing its own scroll on the child's behalf.
#
# Credential-free: the fake pi only negotiates modes and echoes; no model call.
# SKIPs without tmux.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the wg tui chat PTY"
fi

WG_BIN="$(command -v wg)"

scratch=$(make_scratch)
# Isolate HOME/XDG so the pi starter profile + project config stay in the
# scratch (a shared tmux server would otherwise leak the real HOME/CWD into
# the TUI — see tui_pi_chat_pty_embedded.sh fix (a)/(b)).
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME"
cd "$scratch"

G="$scratch/.wg"

# ── Private tmux server (deterministic env; no pre-existing-server leakage) ──
TM_SOCK="wgsmoke-pi-scroll-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
tmux_kill_server() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook tmux_kill_server
session="wgsmoke-pi-scroll-$$"

# ── Fake `pi` that emulates pi's terminal negotiation then echoes input ──
# `cat -v` renders control bytes visibly (ESC -> `^[`) so `tmux capture-pane`
# is an exact record of what WG forwarded into the child.
fakedir="$scratch/fakebin"
mkdir -p "$fakedir"
ready_marker="FAKE_PI_SCROLL_READY_$$"
cat >"$fakedir/pi" <<PIEOF
#!/bin/sh
# Emulate pi's terminal negotiation: alternate screen + SGR mouse reporting.
printf '\033[?1049h'
printf '\033[?1000h\033[?1006h'
printf '\033[H\033[2J'
printf '$ready_marker\r\n'
exec cat -v
PIEOF
chmod +x "$fakedir/pi"
export PATH="$fakedir:$PATH"

# ── Config: default route (claude) — the pi chat uses its per-chat override ──
run_wg() {
    env -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
        HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" "$WG_BIN" "$@"
}
if ! run_wg --dir "$G" init >"$scratch/init.log" 2>&1; then
    loud_fail "wg --dir init failed: $(tail -5 "$scratch/init.log")"
fi

if ! run_wg --dir "$G" chat new --name piroom --executor pi \
        --model "pi:openrouter:z-ai/glm-5.2" >"$scratch/chat0.log" 2>&1; then
    loud_fail "create pi chat failed: $(cat "$scratch/chat0.log")"
fi

ptydump="$scratch/ptydump"

# ── Launch the real TUI in the private tmux server ─────────────────────────
TM new-session -d -s "$session" -x 180 -y 50 \
    "cd '$scratch' && env -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
     HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' \
     PATH='$fakedir:$PATH' WG_PTY_DUMP='$ptydump' '$WG_BIN' --dir '$G' tui"

# Wait for the nested wg-chat tmux session (proves the pi PTY pane spawned).
inner=""
for _ in $(seq 1 120); do
    inner="$(TM list-sessions -F '#{session_name}' 2>/dev/null | grep -E '^wg-chat-.*-chat-0$' | head -1 || true)"
    if [[ -n "$inner" ]]; then
        break
    fi
    sleep 0.25
done
if [[ -z "$inner" ]]; then
    loud_fail "nested wg-chat-*-chat-0 tmux session never appeared; pi chat pane did not spawn.\nSessions:\n$(TM list-sessions 2>&1)\nOuter pane:\n$(TM capture-pane -p -t "$session" 2>/dev/null)"
fi

# Wait for the fake pi to negotiate alt-screen + SGR mouse reporting (relayed
# by tmux to the outer world as the pane's own flags) and print its marker.
modes=""
for _ in $(seq 1 120); do
    modes="$(TM display-message -p -t "$inner" '#{alternate_on} #{mouse_any_flag} #{mouse_sgr_flag}' 2>/dev/null | tr -s ' ' | sed 's/^ *//;s/ *$//' || true)"
    inner_text="$(TM capture-pane -p -t "$inner" 2>/dev/null || true)"
    if [[ "$modes" == "1 1 1" ]] && grep -qF "$ready_marker" <<<"$inner_text"; then
        break
    fi
    sleep 0.25
done
if [[ "$modes" != "1 1 1" ]]; then
    loud_fail "fake pi did not negotiate alt-screen + SGR mouse (tmux flags: '$modes'); cannot exercise forwarding.\nInner pane:\n$(TM capture-pane -p -t "$inner" 2>/dev/null)"
fi

# Give the outer TUI a beat to finish its initial paint and focus the pane.
sleep 1.5

# ── Drive the SAME gestures a human uses ───────────────────────────────────
# Wheel: crossterm reads an SGR mouse report written straight into the outer
# TUI's tty. Sweep positions across the frame so at least one lands in the
# chat content area (a miss only scrolls the graph; it cannot forward).
wheel_seq="$(printf '\033[<64;150;20M')"
for col in 100 115 130 145 160 175; do
    for row in 20 26 32; do
        TM send-keys -t "$session" -l "$(printf '\033[<64;%s;%sM' "$col" "$row")"
        sleep 0.05
    done
done
# PageUp while the chat pane has focus (the keyboard scroll path).
TM send-keys -t "$session" PageUp
sleep 0.3
TM send-keys -t "$session" PageUp
sleep 1.0

pageup_seq="$(printf '\033[5~')"
sgr_prefix="$(printf '\033[<64;')"

# ── Assertion 1 + 2: the CHILD received the wheel report and PageUp keys ──
inner_text="$(TM capture-pane -p -t "$inner" 2>/dev/null || true)"
if ! grep -qF '^[[<64;' <<<"$inner_text"; then
    loud_fail "fake pi never received an SGR mouse-wheel report (^[[<64;...) — WG did not forward the wheel to the child.\nInner pane:\n$inner_text\nOuter pane:\n$(TM capture-pane -p -t "$session" 2>/dev/null)"
fi
if ! grep -qF '^[[5~' <<<"$inner_text"; then
    loud_fail "fake pi never received the PageUp key bytes (^[[5~) — WG did not forward the keyboard scroll to the child.\nInner pane:\n$inner_text"
fi

# ── Assertion 3: WG's forwarded-input tee proves WG is the writer ──────────
tee_hit=""
for f in "$ptydump".*.in.bin; do
    [[ -e "$f" ]] || continue
    if grep -qaF "$sgr_prefix" "$f" && grep -qaF "$pageup_seq" "$f"; then
        tee_hit="$f"
    fi
done
if [[ -z "$tee_hit" ]]; then
    loud_fail "no WG_PTY_DUMP input tee contained both the SGR wheel report and PageUp — WG did not forward them to the tmux child. Tee files:\n$(ls -l "$ptydump".*.in.bin 2>&1)\nHexdump:\n$(od -c "$ptydump".*.in.bin 2>/dev/null | head -40)"
fi

# ── Assertion 4: WG did NOT synthesize a scroll on the child's behalf ──────
in_mode="$(TM display-message -p -t "$inner" '#{pane_in_mode}' 2>/dev/null | tr -d ' \n')"
if [[ "$in_mode" != "0" ]]; then
    loud_fail "wrapping pi tmux session '$inner' is in copy-mode (pane_in_mode=$in_mode) after scrolling — WG fell back to synthesizing its own scroll instead of forwarding to the child."
fi

echo "PI_WHEEL_FORWARDED: yes"
echo "PI_PAGEUP_FORWARDED: yes"
echo "WRAPPING_TMUX_COPY_MODE: no"
echo "PASS: wg tui forwards wheel (SGR) + PageUp to the pi child and leaves the wrapping tmux session out of copy-mode ($inner)"
exit 0
