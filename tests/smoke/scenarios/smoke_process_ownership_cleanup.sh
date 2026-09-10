#!/usr/bin/env bash
# Credential-free process-lifecycle regression for the historical
# pi_threshold_compaction_same_process_kick leak. Exercises the real smoke
# helper entry point with TERM-ignoring, setsid/double-forked fixtures.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

[[ -d /proc ]] || loud_skip "MISSING PROCFS" "exact smoke ownership validation requires Linux /proc"
command -v setsid >/dev/null 2>&1 || loud_skip "MISSING SETSID" "setsid is required for escaped-session regression"
command -v timeout >/dev/null 2>&1 || loud_skip "MISSING TIMEOUT" "timeout is required for bounded abort regression"

scratch=$(make_scratch)
case_root="$scratch/cases"
mkdir -p "$case_root/bin"

# A concurrent executable literally named pi, with no ownership marker, must
# survive every cleanup pass with the same immutable start identity.
ln -s /bin/sleep "$case_root/bin/pi"
env -u WG_SMOKE_RUN_ID -u WG_SMOKE_SCENARIO "$case_root/bin/pi" 300 &
decoy_pid=$!
decoy_identity=$(_wg_smoke_proc_identity "$decoy_pid")
decoy_identity="${decoy_identity% *}" # state (R/S) is intentionally mutable
cleanup_decoy_pi() {
    kill -KILL "$decoy_pid" 2>/dev/null || true
    wait "$decoy_pid" 2>/dev/null || true
}
add_cleanup_hook cleanup_decoy_pi

# A stale-record sweep must likewise ignore a different *healthy* scenario.
# This catches the tempting but unsafe implementation that treats every owner
# record under the shared root as abandoned.
healthy_root="$scratch/healthy-sweep-root"
healthy_owner="$healthy_root/.owners/healthy"
healthy_token="wg-smoke-v2:healthy-concurrent-$BASHPID-$RANDOM"
mkdir -p "$healthy_owner"
env WG_SMOKE_RUN_ID="$healthy_token" /bin/sleep 300 &
healthy_pid=$!
healthy_identity=$(_wg_smoke_proc_identity "$healthy_pid")
healthy_identity="${healthy_identity% *}" # state is mutable
supervisor_pid=$BASHPID
supervisor_identity=$(_wg_smoke_proc_identity "$supervisor_pid")
# shellcheck disable=SC2086
set -- $supervisor_identity
cat >"$healthy_owner/owner.env" <<EOF
version=2
run_id=$healthy_token
scenario=healthy-concurrent-scenario
supervisor_pid=$supervisor_pid
supervisor_start_ticks=$4
EOF
WG_SMOKE_ROOT="$healthy_root" wg_smoke_sweep
current_healthy=$(_wg_smoke_proc_identity "$healthy_pid" 2>/dev/null || true)
current_healthy="${current_healthy% *}"
[[ "$current_healthy" == "$healthy_identity" ]] \
    || loud_fail "stale sweep killed a concurrently healthy owned scenario"
kill -KILL "$healthy_pid" 2>/dev/null || true
wait "$healthy_pid" 2>/dev/null || true
rm -rf "$healthy_root"

cat >"$case_root/fake-pi" <<'SH'
#!/usr/bin/env bash
# Parent exits immediately; the real TERM-ignoring Pi stand-in moves to a new
# session, holds cwd + log FDs, and is orphaned from the registered launcher.
set -u
setsid bash -c '
    trap "" TERM INT
    cd "$1"
    exec 8>>"$1/pi-held.log"
    echo "$$" >>"$2"
    while :; do sleep 1; done
' _ "${OWNED_CWD:?}" "${OWNED_PIDS:?}" >/dev/null 2>&1 &
exit 0
SH
chmod +x "$case_root/fake-pi"

cat >"$case_root/observer" <<'SH'
#!/usr/bin/env bash
trap '' TERM INT
cd "${OWNED_CWD:?}"
exec 8>>"$OWNED_CWD/observer-held.log"
echo "$$" >>"${OWNED_PIDS:?}"
while :; do sleep 1; done
SH
chmod +x "$case_root/observer"

cat >"$case_root/sanitized" <<'SH'
#!/usr/bin/env bash
# Stop while start_owned_process publishes PID/start registration, then erase
# both ownership markers. Cleanup must retain authority through registration.
kill -STOP "$$"
exec env -i OWNED_CWD="$OWNED_CWD" OWNED_PIDS="$OWNED_PIDS" bash -c '
    trap "" TERM INT
    cd "$OWNED_CWD"
    exec 8>>"$OWNED_CWD/sanitized-held.log"
    echo "$$" >>"$OWNED_PIDS"
    while :; do sleep 1; done
'
SH
chmod +x "$case_root/sanitized"

cat >"$case_root/supervisor" <<'SH'
#!/usr/bin/env bash
# TERM-ignoring supervisor plus TERM-ignoring daemon child. If the child dies
# before the supervisor, it is restarted, forcing cleanup to rescan ownership.
trap '' TERM INT
cd "${OWNED_CWD:?}"
exec 8>>"$OWNED_CWD/supervisor-held.log"
echo "$$" >>"${OWNED_PIDS:?}"
while :; do
    bash -c 'trap "" TERM INT; cd "$1"; exec 9>>"$1/daemon-held.log"; echo "$$" >>"$2"; while :; do sleep 1; done' \
        _ "$OWNED_CWD" "$OWNED_PIDS" &
    wait "$!" 2>/dev/null || true
done
SH
chmod +x "$case_root/supervisor"

cat >"$case_root/inner-case" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
helper="$1"
case_dir="$2"
mode="$3"
# shellcheck disable=SC1090
. "$helper"
owned=$(make_scratch)
pids="$case_dir/pids"
: >"$pids"
export OWNED_CWD="$owned" OWNED_PIDS="$pids"
start_owned_process fake-pi "$owned/fake-pi-launch.log" "$case_dir/../fake-pi" >/dev/null
start_owned_process observer "$owned/observer.log" "$case_dir/../observer" >/dev/null
start_owned_process sanitized "$owned/sanitized.log" "$case_dir/../sanitized" >"$case_dir/sanitized.pid"
sanitized_pid=$(<"$case_dir/sanitized.pid")
kill -CONT "$sanitized_pid"
start_owned_process supervised-daemon "$owned/supervisor.log" "$case_dir/../supervisor" >/dev/null
for _ in $(seq 1 100); do
    [[ $(wc -l <"$pids") -ge 5 ]] && break
    sleep 0.02
done
[[ $(wc -l <"$pids") -ge 5 ]]
case "$mode" in
    success) exit 0 ;;
    assertion) false ;;
    timeout) while :; do sleep 1; done ;;
    sigint) kill -INT "$$"; sleep 30 ;;
    sigterm) kill -TERM "$$"; sleep 30 ;;
    *) exit 64 ;;
esac
SH
chmod +x "$case_root/inner-case"

assert_run_gone() {
    local token="$1" pids_file="$2" mode="$3" snapshot pid
    snapshot=$(_wg_smoke_owned_snapshot "$token")
    [[ -z "$snapshot" ]] || loud_fail "$mode left scenario ownership marker alive: $snapshot"
    while read -r pid; do
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        [[ ! -e "/proc/$pid" ]] || loud_fail "$mode left PID $pid (possibly PPID 1) unreaped"
    done <"$pids_file"

    # No live marker means no orphan can retain a deleted cwd or deleted log FD.
    # Check explicitly as a diagnostic guard rather than inferring from names.
    for proc in /proc/[0-9]*; do
        pid="${proc#/proc/}"
        _wg_smoke_pid_has_run_id "$pid" "$token" || continue
        cwd=$(readlink "$proc/cwd" 2>/dev/null || true)
        deleted=$(find "$proc/fd" -maxdepth 1 -lname '* (deleted)' -print -quit 2>/dev/null || true)
        loud_fail "$mode left owned PID $pid cwd=$cwd deleted_fd=${deleted:-none}"
    done
}

baseline_dirs=$(find "$case_root" -mindepth 1 -maxdepth 1 -type d -name 'round-*' | wc -l)
for round in 1 2; do
    for mode in success assertion timeout sigint sigterm; do
        run_dir="$case_root/round-$round-$mode"
        root="$run_dir/root"
        owner_dir="$root/.owners/$mode"
        token="wg-smoke-v2:process-cleanup-$BASHPID-$round-$mode-$RANDOM"
        mkdir -p "$owner_dir"
        cat >"$owner_dir/owner.env" <<EOF
version=2
run_id=$token
scenario=smoke-process-ownership-$mode
supervisor_pid=$BASHPID
EOF
        args=(env WG_SMOKE_RUN_ID="$token" WG_SMOKE_SCENARIO="smoke-process-ownership-$mode" \
            WG_SMOKE_ROOT="$root" WG_SMOKE_OWNER_FILE="$owner_dir/owner.env" \
            WG_SMOKE_CLEANUP_DIAGNOSTICS="$owner_dir/cleanup-diagnostics.log" \
            WG_SMOKE_HARNESS_OWNED=0 bash "$case_root/inner-case" "$HERE/_helpers.sh" "$run_dir" "$mode")
        if [[ "$mode" == timeout ]]; then
            timeout --preserve-status --kill-after=3s 1s "${args[@]}" >/dev/null 2>&1 || true
            # If SIGKILL cut the inner trap short, exercise the explicit stale
            # ownership backstop exactly as the next global harness pass does.
            _wg_smoke_terminate_run "$token" "smoke-process-ownership-$mode" \
                "$owner_dir/cleanup-diagnostics.log" "$WG_SMOKE_HARNESS_RUN_ID" "$owner_dir" \
                || loud_fail "timeout backstop could not reap exact ownership set"
            # The timeout may SIGKILL the inner cleanup shell mid-trap. The
            # outer backstop has now proven the ownership set empty, so and
            # only so may it delete the retained fixture and owner record.
            find "$root" -mindepth 1 -maxdepth 1 ! -name '.owners' -exec rm -rf {} +
            rm -rf "$owner_dir"
        else
            "${args[@]}" >/dev/null 2>&1 || true
        fi
        assert_run_gone "$token" "$run_dir/pids" "$mode"
        # This outer orchestrator created the nested owner record, so it owns
        # record deletion after the exact process-empty assertion.
        rm -rf "$owner_dir"
        leftovers=$(find "$root" -mindepth 1 -maxdepth 1 ! -name '.owners' -print -quit 2>/dev/null || true)
        [[ -z "$leftovers" ]] || loud_fail "$mode retained fixture before/after process cleanup: $leftovers"

        current_decoy=$(_wg_smoke_proc_identity "$decoy_pid" 2>/dev/null || true)
        current_decoy="${current_decoy% *}"
        [[ "$current_decoy" == "$decoy_identity" ]] \
            || loud_fail "$mode changed/killed concurrent unrelated pi: before=$decoy_identity after=$current_decoy"
        rm -rf "$run_dir"
    done
done

after_dirs=$(find "$case_root" -mindepth 1 -maxdepth 1 -type d -name 'round-*' | wc -l)
[[ "$after_dirs" -eq "$baseline_dirs" ]] \
    || loud_fail "repeated runs changed temp-root baseline: before=$baseline_dirs after=$after_dirs"

echo "PASS: exact smoke ownership reaped TERM-ignoring Pi/observer/supervised-daemon trees on success/assertion/timeout/SIGINT/SIGTERM twice; no PPID-1/deleted-FD/marker leak; unrelated pi and concurrent healthy scenario survived"
