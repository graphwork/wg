#!/usr/bin/env bash
# Scenario: tui_pi_chat_pty_embedded
#
# Pins fix-tui-pi: creating a Pi chat from the TUI launcher MUST attach the
# pane to a live PTY running the interactive `pi` binary, NOT route through
# `wg pi-handler` / `pi --mode rpc` on pipes (which leaves the pane stuck on
# "Booting agent...").
#
# Human-flow simulation: a fake `pi` binary is placed on PATH that prints a
# unique marker line to its PTY stdout and then idles. We launch `wg tui` in
# tmux, create a Pi chat through the launcher, and assert:
#   1. The fake `pi` marker appears in the tmux pane (live PTY output), and
#   2. No `pi --mode rpc` process was spawned for the visible chat pane.
#
# The previous regression (`.chat-39`) spawned `wg pi-handler --chat chat-N
# -m openrouter:...` which forked `pi --mode rpc` on pipes; the pane never
# showed child output. This scenario catches that by requiring the fake
# marker — pipe-stdio RPC would never reach the TUI pane.
#
# ── Hardening (harden-tui-pi): determinism on a box with a live tmux server ──
# This scenario was fragile on any host that already had a tmux server
# running. Two fixes (mirrored from true_e2e_pi_glm_chat_deepseek_agency.sh)
# make it deterministic regardless of pre-existing tmux / HOME state:
#
#   (a) PRIVATE tmux server (`tmux -L <sock>`). `wg tui` resolves the chat
#       pane's executor + the project graph from its inherited HOME/CWD. A
#       SHARED tmux server reuses the env of whichever client started it
#       first, so a developer/CI box with a tmux server already running
#       leaks that env's HOME (a *different* ~/.wg/config) and CWD into the
#       TUI — `wg tui` then loaded the wrong config / graph and spawned the
#       wrong executor entirely (observed: a real `claude` pane instead of
#       the fake pi, so the marker never appeared and the smoke FAILed). A
#       dedicated throwaway server on a private socket inherits THIS
#       scenario's isolated HOME + PATH + CWD every time. Torn down via
#       `add_cleanup_hook`.
#
#   (b) Pin the Pi routing PROJECT-LOCAL. The old setup used
#       `wg init --executor shell`, which (i) flooded the chat pane with
#       `dispatcher.executor = "shell"` deprecation warnings that scrolled
#       the single marker line off-screen, and (ii) left the coordinator
#       config on a non-pi executor, so the pane only attached to pi by
#       grace of the per-chat `CoordinatorState` written by `wg chat new`.
#       Instead we activate the real `pi` starter profile and copy it into
#       the project config, so the Pi route is the coordinator's own routing
#       (`[dispatcher].model = pi:openrouter/z-ai/glm-5.2`) and nothing
#       depends on a daemon-written CoordinatorState handoff. No `executor`
#       key means no deprecation flood corrupting the pane.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

WG_BIN="$(command -v wg)"

scratch=$(make_scratch)
# Fully isolate HOME/XDG so `wg profile use pi` writes the starter profile and
# active-profile pointer into the scratch (never the real ~/.wg), and so the
# TUI launched in the private tmux server resolves THIS scenario's config —
# not whatever HOME a pre-existing tmux server might leak.
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME"
cd "$scratch"

G="$scratch/.wg"

# ── Private tmux server — see header fix (a). Registered for teardown. ──
TM_SOCK="wgsmoke-tui-pi-pty-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
tmux_kill_server() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook tmux_kill_server
session="wgsmoke-tui-pi-pty-$$"

# ── Fake `pi` binary ──────────────────────────────────────────────────
# Emits a unique marker to stdout (the PTY), then idles reading stdin so the
# pane stays alive. It must NOT take a `--mode rpc` branch — if it sees
# `--mode rpc` on its argv it writes a sentinel to a side file so the test
# can detect the forbidden RPC routing.
fakedir="$scratch/fakebin"
mkdir -p "$fakedir"
marker="FAKE_PI_PTY_OK_$$"
rpc_sentinel="$scratch/pi_mode_rpc_spawned"
cat >"$fakedir/pi" <<PIEOF
#!/usr/bin/env bash
# fake pi for tui_pi_chat_pty_embedded smoke
echo "$marker"
# If invoked with --mode rpc, record that the forbidden path was taken.
for a in "\$@"; do
    if [[ "\$a" == "rpc" ]]; then
        echo "rpc" >"$rpc_sentinel"
    fi
done
# Idle so the pane stays alive (interactive mode waits for stdin).
while IFS= read -r line; do :; done
PIEOF
chmod +x "$fakedir/pi"

# Put the fake pi FIRST on PATH so the TUI's spawned `pi` resolves to it.
export PATH="$fakedir:$PATH"

# ── Config: activate the real `pi` starter profile, then pin it project-local ─
# See header fix (b). The pi starter sets [dispatcher].model = pi:openrouter/
# z-ai/glm-5.2 with NO `executor` key (so the coordinator routes to pi without
# a deprecation-warning flood). We overwrite the claude default `wg init`
# writes with the activated pi config so the TUI's own Config::load resolves
# the Pi routing deterministically — no daemon / CoordinatorState handoff.
if ! wg profile init-starters >"$scratch/init-starters.log" 2>&1; then
    loud_fail "wg profile init-starters failed: $(tail -5 "$scratch/init-starters.log")"
fi
if ! wg --dir "$G" init >"$scratch/init.log" 2>&1; then
    loud_fail "wg --dir init failed: $(tail -5 "$scratch/init.log")"
fi
if ! wg --dir "$G" profile use pi --no-reload >"$scratch/profile-use.log" 2>&1; then
    loud_fail "wg profile use pi failed: $(tail -5 "$scratch/profile-use.log")"
fi
if [[ ! -f "$HOME/.wg/config.toml" ]]; then
    loud_fail "wg profile use pi did not write $HOME/.wg/config.toml"
fi
cp "$HOME/.wg/config.toml" "$G/config.toml"

# Create a Pi chat with an explicit OpenRouter model (the explicit-model
# contract — pi_model_arg must resolve a provider/model pair). This also
# writes the per-chat CoordinatorState (executor_override = pi).
if ! wg --dir "$G" chat new --name piroom --executor pi \
        --model "openrouter:z-ai/glm-5.2" >"$scratch/chat0.log" 2>&1; then
    loud_fail "create pi chat failed: $(cat "$scratch/chat0.log")"
fi

# ── Launch the TUI in the private tmux server ─────────────────────────────
TM new-session -d -s "$session" -x 180 -y 50 "$WG_BIN --dir $G tui"

# Give the TUI time to auto-enable the chat PTY pane (maybe_auto_enable_chat_pty
# runs on focus / chat-tab render). The Pi chat is the first/only chat so it is
# active by default — the embedded pi PTY auto-enables without a nudge.
saw_marker=""
for _ in $(seq 1 60); do
    capture=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    if echo "$capture" | grep -qF "$marker"; then
        saw_marker="$capture"
        break
    fi
    sleep 0.25
done

# ── Assertion 1: live PTY output (the fake-pi marker) reached the pane ──
if [[ -z "$saw_marker" ]]; then
    capture=$(TM capture-pane -t "$session" -p 2>/dev/null || true)
    loud_fail "TUI Pi chat pane did not show fake-pi PTY marker '$marker' within 15s. \
This means the pane was NOT attached to a live PTY running interactive pi — \
the 'Booting agent...' regression. Pane content:
$capture"
fi

# ── Assertion 2: no `pi --mode rpc` was spawned for the visible pane ────
# The fake pi writes the rpc sentinel ONLY if `--mode rpc` was on its argv.
# A small grace window in case a stray worker (non-TUI) is also running.
sleep 1
if [[ -f "$rpc_sentinel" ]]; then
    loud_fail "TUI Pi chat pane spawned pi with --mode rpc (forbidden). \
The visible chat pane must use the embedded PTY path, not the detached RPC \
worker. rpc_sentinel=$rpc_sentinel present."
fi

echo "FAKE_PI_MARKER_SEEN: yes"
echo "PI_MODE_RPC_SPAWNED: no"
echo "PASS: TUI Pi chat pane attached to live PTY (fake-pi marker visible), no --mode rpc"
exit 0
