#!/usr/bin/env bash
# Regression: a long-running installed `wg tui` must keep creating chats after
# its installed pathname is atomically replaced (the shape of cargo install /
# package-manager upgrades). Linux reports the old running image as
# `<path> (deleted)`; std::env::current_exe() returns that non-executable name.
# The TUI must re-exec its authoritative running image, not PATH and not the
# deleted display pathname. This is a real command-mode New chat -> Pi -> Enter
# human flow in an isolated HOME/graph/private tmux server.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI launcher"
fi
if [[ ! -e /proc/self/exe ]]; then
    loud_skip "NO PROC SELF EXE" "hot-replaced running-image regression is Linux-specific"
fi

WG_BIN="$(readlink -f "$(command -v wg)")"
# `wg done` runs owned smokes before it merges the worktree. This task also
# explicitly forbids globally installing an unmerged candidate. Loud-skip only
# that bootstrap paradox; the candidate is exercised directly before done, and
# after merge/install this marker is present so the permanent gate runs fully.
if ! grep -aFq 'Failed to execute current WG image via' "$WG_BIN"; then
    loud_skip "STALE INSTALLED WG" \
        "installed wg predates authoritative self-exec; merge then install main before running this scenario"
fi
scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME" "$scratch/graph" "$scratch/fakebin"
G="$scratch/graph/.wg"
TM_SOCK="wgsmoke-pi-self-exec-$$"
TM() { tmux -L "$TM_SOCK" "$@"; }
cleanup_tmux() { tmux -L "$TM_SOCK" kill-server 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux

# The fake interactive Pi records identity/argv and then stays alive. It is a
# standalone console launch: no wg pi-handler, --mode rpc, or hermetic -e/-ne.
cat >"$scratch/fakebin/pi" <<'SH'
#!/bin/sh
evidence="$(dirname "$0")/../pi-evidence"
printf 'pid=%s\n' "$$" >"$evidence"
printf 'WG_CHAT_ID=%s\n' "${WG_CHAT_ID:-}" >>"$evidence"
printf 'WG_CHAT_REF=%s\n' "${WG_CHAT_REF:-}" >>"$evidence"
printf 'argv=' >>"$evidence"
printf ' <%s>' "$@" >>"$evidence"
printf '\nPI_HOT_UPGRADE_OK\n'
while IFS= read -r _line; do :; done
SH
chmod +x "$scratch/fakebin/pi"
# `wg` commonly means WireGuard. Put a deliberately foreign executable first
# on PATH: recursive Pi chat creation must never execute it merely because the
# authoritative WG image's installed pathname was replaced.
cat >"$scratch/fakebin/wg" <<'SH'
#!/bin/sh
echo "FOREIGN_WG_EXECUTED $*" >"$(dirname "$0")/../foreign-wg-executed"
exit 97
SH
chmod +x "$scratch/fakebin/wg"

# Minimal, non-login, mosh-like environment. The only `wg` on PATH is the
# foreign collision above; the selected Pi and base OS tools are also present.
# The TUI is entered through a symlink to an installed-binary copy so neither
# PATH nor Cargo layout can legitimately satisfy its recursive create command.
MIN_PATH="$scratch/fakebin:/usr/bin:/bin"
cp "$WG_BIN" "$scratch/wg-image"
ln -s "$scratch/wg-image" "$scratch/wg-installed-link"
env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$HOME" PATH="$MIN_PATH" \
    "$scratch/wg-installed-link" --dir "$G" init --no-agency >"$scratch/init.log" 2>&1 \
    || loud_fail "isolated init failed: $(cat "$scratch/init.log")"
# Selecting an executor is not itself an implicit provider route. Seed the
# isolated graph with an explicit Pi route before opening the TUI; opening still
# performs no selection/mutation and creation remains an explicit confirmation.
cat >"$G/config.toml" <<'TOML'
[dispatcher]
model = "pi:openrouter:z-ai/glm-5.2"
TOML

TM new-session -d -s tui -x 180 -y 50 \
    "env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME='$HOME' PATH='$MIN_PATH' MOSH_IP=192.0.2.10 PI_EVIDENCE='$scratch/pi-evidence' TERM=xterm-256color '$scratch/wg-installed-link' --dir '$G' tui --no-mouse"

for _ in $(seq 1 100); do
    [[ -S "$G/service/tui.sock" ]] && break
    sleep 0.1
done
[[ -S "$G/service/tui.sock" ]] || loud_fail "installed-copy TUI did not create tui.sock"
tui_pid=$(TM list-panes -t tui -F '#{pane_pid}')
old_exe=$(readlink "/proc/$tui_pid/exe" 2>/dev/null || true)
[[ "$old_exe" == "$scratch/wg-image" ]] \
    || loud_fail "TUI did not run the installed-copy target: pid=$tui_pid exe=$old_exe"

# Real command-mode flow. Empty-chat startup is already in command mode; `n`
# opens New chat. The default preset order is Codex, Claude, Pi, Add new, so
# two Down presses explicitly select Pi. Replace the image while the chooser is
# open, exactly before confirmation invokes the recursive `wg chat create`.
TM send-keys -t tui n
for _ in $(seq 1 50); do
    TM capture-pane -p -t tui 2>/dev/null | grep -q 'New chat' && break
    sleep 0.1
done
TM capture-pane -p -t tui | grep -q 'Pi (pi.dev)' \
    || loud_fail "New chat chooser did not offer Pi"
rows_before_confirm=$(grep -c '"id":"\.chat-' "$G/graph.jsonl" 2>/dev/null || true)
[[ "$rows_before_confirm" -eq 0 && ! -e "$scratch/pi-evidence" ]] \
    || loud_fail "opening/selecting New chat mutated state before confirmation"
TM send-keys -t tui Down Down
sleep 0.2

cp "$WG_BIN" "$scratch/wg-image.next"
mv "$scratch/wg-image.next" "$scratch/wg-image"
deleted_exe=$(readlink "/proc/$tui_pid/exe" 2>/dev/null || true)
[[ "$deleted_exe" == *' (deleted)' ]] \
    || loud_fail "atomic replacement did not produce a deleted running image: $deleted_exe"

# Double confirmation is intentional: the launcher's creating gate must make
# this idempotent rather than creating duplicate rows/sessions.
TM send-keys -t tui Enter Enter
for _ in $(seq 1 120); do
    [[ -s "$scratch/pi-evidence" ]] && break
    sleep 0.1
done
[[ -s "$scratch/pi-evidence" ]] || {
    capture=$(TM capture-pane -p -t tui 2>/dev/null || true)
    loud_fail "Pi never launched after installed image replacement. Pane:\n$capture"
}

if TM capture-pane -p -t tui | grep -q 'Failed to run wg: No such file'; then
    loud_fail "stale deleted-path ENOENT is still visible after confirmation"
fi
[[ ! -e "$scratch/foreign-wg-executed" ]] \
    || loud_fail "Pi chat launch executed an unverified foreign wg from PATH: $(cat "$scratch/foreign-wg-executed")"
rows=$(grep -c '"id":"\.chat-' "$G/graph.jsonl" 2>/dev/null || true)
[[ "$rows" -eq 1 ]] || loud_fail "double confirmation created $rows chat rows (expected exactly one)"
grep -q '^WG_CHAT_ID=\.chat-0$' "$scratch/pi-evidence" \
    || loud_fail "Pi did not receive canonical chat identity: $(cat "$scratch/pi-evidence")"
grep -q '^WG_CHAT_REF=chat-0$' "$scratch/pi-evidence" \
    || loud_fail "Pi did not receive canonical chat ref: $(cat "$scratch/pi-evidence")"
if grep '^argv=' "$scratch/pi-evidence" | grep -Eq -- ' --mode| <rpc>| <-e>| <-ne>'; then
    loud_fail "standalone TUI Pi was confused with managed/RPC plugin launch: $(cat "$scratch/pi-evidence")"
fi
pi_pid=$(sed -n 's/^pid=//p' "$scratch/pi-evidence")
kill -0 "$pi_pid" 2>/dev/null || loud_fail "recorded Pi process is not alive: pid=$pi_pid"
chat_sessions_before=$(TM list-sessions -F '#{session_name}' 2>/dev/null | grep -c '^wg-chat-' || true)
[[ "$chat_sessions_before" -eq 1 ]] \
    || loud_fail "expected one path-owned chat tmux session, got $chat_sessions_before"

# Stateful restart must reattach the exact persistent Pi process and must not
# synthesize a duplicate row or session.
TM kill-session -t tui
TM new-session -d -s tui -x 180 -y 50 \
    "env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME='$HOME' PATH='$MIN_PATH' MOSH_IP=192.0.2.11 PI_EVIDENCE='$scratch/pi-evidence' TERM=xterm-256color '$scratch/wg-installed-link' --dir '$G' tui --no-mouse"
for _ in $(seq 1 100); do
    kill -0 "$pi_pid" 2>/dev/null && TM capture-pane -p -t tui 2>/dev/null | grep -qE 'Chat 0|\.chat-0' && break
    sleep 0.1
done
rows_after=$(grep -c '"id":"\.chat-' "$G/graph.jsonl" 2>/dev/null || true)
sessions_after=$(TM list-sessions -F '#{session_name}' 2>/dev/null | grep -c '^wg-chat-' || true)
[[ "$rows_after" -eq 1 && "$sessions_after" -eq 1 ]] \
    || loud_fail "restart duplicated identity/session: rows=$rows_after sessions=$sessions_after"
kill -0 "$pi_pid" 2>/dev/null || loud_fail "restart did not reattach the original Pi pid=$pi_pid"

# Genuine missing Pi is a separate, transactional failure. The public path and
# the identical TUI recursive path must name `pi` (not "Failed to run wg"),
# create no row, and attempt no unrelated executor. A private PATH containing
# tmux but no Pi makes this independent of the developer's installed CLIs.
missing="$scratch/missing"
mkdir -p "$missing/home" "$missing/graph" "$missing/toolbin"
ln -s "$(command -v tmux)" "$missing/toolbin/tmux"
MISSING_PATH="$missing/toolbin"
env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$missing/home" PATH="$MISSING_PATH" \
    "$scratch/wg-installed-link" --dir "$missing/graph/.wg" init --no-agency >"$missing/init.log" 2>&1 \
    || loud_fail "missing-Pi graph init failed: $(cat "$missing/init.log")"
cat >"$missing/graph/.wg/config.toml" <<'TOML'
[dispatcher]
model = "pi:openrouter:z-ai/glm-5.2"
TOML
if env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$missing/home" PATH="$MISSING_PATH" \
    "$scratch/wg-installed-link" --dir "$missing/graph/.wg" chat create --exec pi --json \
    >"$missing/public.out" 2>&1; then
    loud_fail "public wg chat create accepted missing Pi"
fi
grep -qE "['\x60]pi['\x60].*(not found|missing)|Pi executable.*(not found|missing)" "$missing/public.out" \
    || loud_fail "public missing-Pi error did not name pi: $(cat "$missing/public.out")"
missing_rows=$(grep -c '"id":"\.chat-' "$missing/graph/.wg/graph.jsonl" 2>/dev/null || true)
[[ "$missing_rows" -eq 0 ]] || loud_fail "missing-Pi public failure left $missing_rows chat rows"

TM new-session -d -s missing-tui -x 180 -y 50 \
    "env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME='$missing/home' PATH='$MISSING_PATH' MOSH_IP=192.0.2.12 TERM=xterm-256color '$scratch/wg-installed-link' --dir '$missing/graph/.wg' tui --no-mouse"
for _ in $(seq 1 100); do
    [[ -S "$missing/graph/.wg/service/tui.sock" ]] && break
    sleep 0.1
done
[[ -S "$missing/graph/.wg/service/tui.sock" ]] || loud_fail "missing-Pi TUI did not start"
TM send-keys -t missing-tui n
for _ in $(seq 1 50); do
    TM capture-pane -p -t missing-tui 2>/dev/null | grep -q 'New chat' && break
    sleep 0.1
done
TM send-keys -t missing-tui Down Down Enter
missing_tui_error=""
for _ in $(seq 1 80); do
    capture=$(TM capture-pane -p -t missing-tui 2>/dev/null || true)
    if printf '%s' "$capture" | grep -q 'interactive Pi executable'; then
        missing_tui_error="$capture"
        break
    fi
    sleep 0.1
done
[[ -n "$missing_tui_error" ]] \
    || loud_fail "missing-Pi TUI confirmation did not show the Pi-specific error"
printf '%s' "$missing_tui_error" | grep -q 'no chat was created' \
    || loud_fail "missing-Pi TUI error omitted transactional result: $missing_tui_error"
if printf '%s' "$missing_tui_error" | grep -q 'Failed to run wg'; then
    loud_fail "missing-Pi TUI misidentified the missing process as wg"
fi
missing_rows_after_tui=$(grep -c '"id":"\.chat-' "$missing/graph/.wg/graph.jsonl" 2>/dev/null || true)
[[ "$missing_rows_after_tui" -eq 0 ]] \
    || loud_fail "missing-Pi TUI failure left $missing_rows_after_tui chat rows"
sessions_after_missing=$(TM list-sessions -F '#{session_name}' 2>/dev/null | grep -c '^wg-chat-' || true)
[[ "$sessions_after_missing" -eq 1 ]] \
    || loud_fail "missing Pi created an extra chat tmux session: total=$sessions_after_missing"

echo "running_image_before=$old_exe"
echo "running_image_after_replace=$deleted_exe"
echo "pi_pid=$pi_pid rows=$rows_after chat_sessions=$sessions_after"
echo "missing_pi_error=$(tr '\n' ' ' <"$missing/public.out")"
echo "PASS: hot-replaced installed/symlinked WG TUI re-execed its running image under minimal PATH; explicit Pi created/reattached exactly once; missing Pi failed transactionally"
