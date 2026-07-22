#!/usr/bin/env bash
# Scenario: smoke_cleanup_survives_panic
#
# Pins the regression behind smoke-tests-leak: production smoke runs left
# 70+ orphaned `wg service daemon` processes and 200+ /tmp scratch dirs
# behind because per-scenario `trap` handlers did not fire on SIGKILL /
# panic / signal. The defense-in-depth that this scenario protects:
#
#   1. wg_smoke_sweep finds and kills `wg service daemon` processes whose
#      `--dir` lives under the smoke root, even after the parent shell that
#      spawned them died abruptly (no trap, no atexit).
#   2. wg_smoke_sweep removes leftover scratch dirs under the root.
#
# Strategy:
#   * Spawn a child bash that initialises a WG dir and starts a real
#     `wg service daemon`, then SIGKILLs itself before its trap can run.
#     The daemon survives, re-parented to init.
#   * Confirm pre-condition: daemon PID is alive, scratch dir exists.
#   * Run wg_smoke_sweep against the same root.
#   * Assert: daemon is dead and the scratch dir is gone.
#
# We run the leaked daemon under a private sub-root so we never touch
# fixtures that other parallel scenarios may be using.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# Normal trap paths: pass, fail, explicit SIGTERM and timeout must all reap
# their exact owned tmux session. These children use the real helper contract;
# only the later SIGKILL case intentionally disables it.
if [[ -n "${WG_SMOKE_TMUX_BIN:-}" ]]; then
    for mode in pass fail sigterm timeout; do
        session="wg-smoke-trap-${mode}-$$"
        child_script='set -u; . "$1"; tmux new-session -d -s "$2" -- sleep 300; echo "$2"; case "$3" in pass) exit 0;; fail) exit 9;; sigterm) kill -TERM $$;; timeout) sleep 30;; esac'
        if [[ "$mode" == timeout ]]; then
            timeout 1 bash -c "$child_script" _ "$HERE/_helpers.sh" "$session" "$mode" >/dev/null 2>&1 || true
        else
            bash -c "$child_script" _ "$HERE/_helpers.sh" "$session" "$mode" >/dev/null 2>&1 || true
        fi
        if "$WG_SMOKE_TMUX_BIN" has-session -t "$session" 2>/dev/null; then
            "$WG_SMOKE_TMUX_BIN" kill-session -t "$session" >/dev/null 2>&1 || true
            loud_fail "helper trap left owned tmux session alive on $mode"
        fi
    done
fi

# Sub-root unique to this scenario so the sweep we drive only reaches
# fixtures we own. The sub_root is itself under the global smoke root, so
# the central wg_smoke_cleanup trap will reap whatever survives.
mkdir -p "$(wg_smoke_root)"
sub_root="$(mktemp -d "$(wg_smoke_root)/leak_under_test.XXXXXX")"
register_scratch "$sub_root"

leak_log="$sub_root/leak.log"

# Run a child bash that spawns the daemon and then SIGKILLs itself.
# `bash -c '... ; kill -KILL $$'` makes $$ resolve to the child's PID.
# We disable the helper's trap inside the child by overwriting EXIT; the
# whole point is to simulate the trap NOT firing.
WG_SMOKE_ROOT="$sub_root" WG_SMOKE_SCENARIO="leakchild" \
    bash -c '
        set -u
        # shellcheck disable=SC1090
        . "$1"
        require_wg
        # Defeat the helper trap so this child mimics a panic/SIGKILL with
        # no cleanup. The whole regression is "trap did not fire".
        trap - EXIT INT TERM HUP
        scratch=$(make_scratch)
        cd "$scratch"
        wg init -x shell >init.log 2>&1 \
            || wg init -x claude >init.log 2>&1 \
            || { echo "INIT_FAILED" >&2; exit 1; }
        # Spawn the daemon directly (bypasses start_wg_daemon to keep the
        # child surface minimal). Wait for state.json so the parent can
        # read the PID.
        ( wg service start --max-agents 0 --no-chat-agent >daemon.log 2>&1 ) &
        wrap_pid=$!
        wg_dir=""
        for cand in .wg .wg; do
            if [[ -d "$scratch/$cand" ]]; then
                wg_dir="$scratch/$cand"
                break
            fi
        done
        for _ in $(seq 1 60); do
            if [[ -f "$wg_dir/service/state.json" ]]; then
                break
            fi
            sleep 0.2
        done
        # Create one helper-owned tmux session. The helper wrapper stamps exact
        # wg-smoke-v1 options, so the parent sweep can reap it after this owner
        # dies even though the normal trap is disabled.
        if [[ -n "${WG_SMOKE_TMUX_BIN:-}" ]]; then
            smoke_tmux="wg-smoke-leak-$BASHPID"
            tmux new-session -d -s "$smoke_tmux" -- sleep 300
            echo "TMUX_SESSION:$smoke_tmux"
        fi
        # Echo the canonical daemon PID for the parent to pick up.
        if ! grep -oE "\"pid\"[[:space:]]*:[[:space:]]*[0-9]+" \
                "$wg_dir/service/state.json" 2>/dev/null \
                | head -1 | grep -oE "[0-9]+\$"; then
            echo "NO_PID_IN_STATE" >&2
            exit 1
        fi
        wait "$wrap_pid" 2>/dev/null || true
        # Now the regression simulation: SIGKILL ourselves.
        kill -KILL $$
    ' _ "$HERE/_helpers.sh" >"$leak_log" 2>&1 || true

leaked_pid=$(grep -oE '^[0-9]+$' "$leak_log" | tail -1 || true)
leaked_tmux=$(grep '^TMUX_SESSION:' "$leak_log" | tail -1 | cut -d: -f2- || true)
if [[ -z "$leaked_pid" ]]; then
    loud_fail "child shell did not report a daemon PID. log:
$(cat "$leak_log")"
fi

# Pre-condition: leaked daemon must be alive.
if ! kill -0 "$leaked_pid" 2>/dev/null; then
    loud_fail "expected leaked daemon $leaked_pid to be alive after child SIGKILL — child shell may have failed to spawn the daemon. log:
$(cat "$leak_log")"
fi
sub_dirs_before=$(find "$sub_root" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
if [[ "$sub_dirs_before" -lt 1 ]]; then
    loud_fail "expected at least 1 leaked scratch under $sub_root, got $sub_dirs_before"
fi

# Create a decoy user/chat session by bypassing the helper wrapper. It has a
# smoke-looking name but no ownership options and MUST survive the sweep.
decoy_tmux=""
if [[ -n "${WG_SMOKE_TMUX_BIN:-}" && -n "$leaked_tmux" ]]; then
    decoy_tmux="wg-smoke-real-chat-$$"
    "$WG_SMOKE_TMUX_BIN" new-session -d -s "$decoy_tmux" -- sleep 300
    cleanup_decoy_tmux() {
        "$WG_SMOKE_TMUX_BIN" kill-session -t "$decoy_tmux" >/dev/null 2>&1 || true
    }
    add_cleanup_hook cleanup_decoy_tmux
    "$WG_SMOKE_TMUX_BIN" has-session -t "$leaked_tmux" 2>/dev/null \
        || loud_fail "owned leaked tmux session did not survive child SIGKILL"
fi

# The actual regression bar: wg_smoke_sweep must reap the daemon, strictly
# owned tmux session, and dir without touching the decoy user/chat session.
WG_SMOKE_ROOT="$sub_root" wg_smoke_sweep

# Assertion 1: leaked daemon is dead.
sleep 0.2
if kill -0 "$leaked_pid" 2>/dev/null; then
    sleep 1
    if kill -0 "$leaked_pid" 2>/dev/null; then
        loud_fail "wg_smoke_sweep left leaked daemon $leaked_pid alive"
    fi
fi

# Assertion 2: strictly-owned tmux was reaped while the unowned decoy survives.
if [[ -n "$leaked_tmux" ]]; then
    if "$WG_SMOKE_TMUX_BIN" has-session -t "$leaked_tmux" 2>/dev/null; then
        loud_fail "wg_smoke_sweep left strictly-owned stale tmux session $leaked_tmux alive"
    fi
    "$WG_SMOKE_TMUX_BIN" has-session -t "$decoy_tmux" 2>/dev/null \
        || loud_fail "wg_smoke_sweep killed unowned real-chat decoy $decoy_tmux"
fi

# Assertion 3: scratch dirs under sub_root are gone.
sub_dirs_after=$(find "$sub_root" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
if [[ "$sub_dirs_after" -gt 0 ]]; then
    leftover=$(find "$sub_root" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | head -5)
    loud_fail "wg_smoke_sweep left $sub_dirs_after dir(s) under $sub_root, e.g.:
$leftover"
fi

echo "PASS: cleanup survives mid-test SIGKILL — reaped daemon/strictly-owned tmux/$sub_dirs_before scratch dir(s), preserved unowned chat tmux"
exit 0
