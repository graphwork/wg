#!/usr/bin/env bash
# Helpers shared by smoke-gate scenarios.
#
# Each scenario script sources this file. The contract:
#   * exit 0  → PASS
#   * exit 77 → loud SKIP (precondition missing — endpoint unreachable, no creds, ...)
#   * any other non-zero → FAIL
#
# Loud SKIPs MUST go through `loud_skip` so the banner is greppable in run logs.
#
# ── Fixture lifecycle (read this before adding a scenario) ──
# Smoke scenarios spawn `wg service daemon` processes and create temp dirs.
# Two failure modes have leaked daemons + dirs in production:
#
#   1. Per-scenario `trap` lines silently overwrote one another, so only the
#      last cleanup ran on EXIT.
#   2. `daemon_pid=$!` after `wg service start &` captured the WRAPPER PID
#      that exits as soon as it forks the real daemon. Killing the wrapper
#      did nothing — the real daemon was already re-parented to init.
#
# The contract this file enforces:
#
#   * Every scratch dir lives under `wg_smoke_root` (a single shared parent).
#   * The Rust harness assigns every scenario an unguessable
#     `WG_SMOKE_RUN_ID` and a new session. Every daemon, wrapper, observer,
#     provider and Pi child inherits that exact ownership identity.
#   * Every spawned daemon is also registered via `start_wg_daemon`, which
#     reads the canonical PID from `service/state.json` rather than `$!`.
#   * One trap, installed by this file, tears the complete ownership set down
#     on EXIT/ERR/INT/TERM/HUP, waits/reaps, and only then deletes fixtures.
#     Scenarios MUST use `add_cleanup_hook`, never replace the central trap.
#   * `wg_smoke_sweep` is a defense-in-depth reaper. It reads explicit owner
#     records and scans `/proc/*/environ` for the exact run id. It never
#     selects by command name, so unrelated user Pi sessions are invisible.

set -u

# ── Strip agent-context env vars ────────────────────────────────────
# When `wg done` runs the smoke gate from inside an agent's session, the
# agent's environment is inherited: WG_DIR pins every `wg ...` call to
# the agent's project graph (so `wg init` in a scratch dir is a no-op
# and `wg service start` reports "already running" against the parent
# project's daemon). WG_PROJECT_ROOT / WG_WORKTREE_PATH influence
# worktree-aware behaviour the same way. Unset them so smoke fixtures
# truly run in the scratch dir, not the surrounding project.
unset WG_DIR
unset WG_PROJECT_ROOT
unset WG_WORKTREE_PATH
unset WG_WORKTREE_ACTIVE
unset WG_BRANCH
unset WG_TASK_ID
unset WG_AGENT_ID
unset WG_GRAPH_ID
unset WG_WORKER_CAPABILITY
unset WG_WORKER_IPC
unset WG_WORKER_CONTROL_PROTOCOL
unset WG_WORKER_CONTROL_MODE
unset WG_WORKER_ATTEMPT_FENCE
unset WG_WORKER_ATTEMPT_ID
unset WG_WORKER_FILESYSTEM_ISOLATION
unset WG_WORKER_GENERATION

# ── Skip banner ─────────────────────────────────────────────────────
loud_skip() {
    local kind="$1"
    shift
    local reason="$*"
    echo "" 1>&2
    echo "================================================================" 1>&2
    echo "  SMOKE SKIPPED — $kind" 1>&2
    echo "  scenario: ${WG_SMOKE_SCENARIO:-?}" 1>&2
    echo "  reason:   $reason" 1>&2
    echo "================================================================" 1>&2
    exit 77
}

# ── Fail banner ─────────────────────────────────────────────────────
loud_fail() {
    local reason="$*"
    echo "" 1>&2
    echo "================================================================" 1>&2
    echo "  SMOKE FAILED" 1>&2
    echo "  scenario: ${WG_SMOKE_SCENARIO:-?}" 1>&2
    echo "  reason:   $reason" 1>&2
    echo "================================================================" 1>&2
    exit 1
}

_wg_smoke_procfs_ready() {
    [[ -d /proc && -x /proc && -r /proc/self/stat && -r /proc/self/environ ]]
}

# Process-owning smoke helpers are supported only beneath the Rust harness,
# which creates a Linux subreaper before spawn and passes this unguessable
# equality proof. Ad-hoc/direct execution cannot prove orphan adoption and
# therefore fails closed before creating any fixture.
if ! _wg_smoke_procfs_ready \
    || [[ -z "${WG_SMOKE_HARNESS_RUN_ID:-}" \
    || "${WG_SMOKE_SUBREAPER_TOKEN:-}" != "$WG_SMOKE_HARNESS_RUN_ID" ]]; then
    loud_fail "exact smoke cleanup requires the Linux Rust subreaper harness and readable /proc; direct/unsupported execution is refused"
fi

# ── wg binary discovery ─────────────────────────────────────────────
require_wg() {
    if ! command -v wg >/dev/null 2>&1; then
        loud_skip "MISSING WG BINARY" "wg not found on PATH; run 'cargo install --path .' first"
    fi
}

# ── HTTP probe ──────────────────────────────────────────────────────
endpoint_reachable() {
    local url="$1"
    if ! command -v curl >/dev/null 2>&1; then
        return 1
    fi
    curl -fsS -m 5 "$url" -o /dev/null 2>/dev/null
}

# ── Fixture root (single shared parent) ─────────────────────────────
# Pinning every smoke scratch dir under one well-known root means cleanup
# is one `find $root -maxdepth 1 -delete`, not a glob hunt across /tmp.
wg_smoke_root() {
    echo "${WG_SMOKE_ROOT:-${TMPDIR:-/tmp}/wgsmoke}"
}

# ── Exact scenario ownership identity ──────────────────────────────
# Manifest runs receive these from src/smoke.rs before bash starts. The
# fallback remains for nested helper shells beneath that verified harness;
# unsupported or ad-hoc top-level execution was already refused above.
if [[ -z "${WG_SMOKE_RUN_ID:-}" ]]; then
    if [[ -r /proc/sys/kernel/random/uuid ]]; then
        WG_SMOKE_RUN_ID="wg-smoke-v2:$(cat /proc/sys/kernel/random/uuid)"
    else
        WG_SMOKE_RUN_ID="wg-smoke-v2:$BASHPID-$(date +%s%N)-$RANDOM"
    fi
fi
export WG_SMOKE_RUN_ID
_WG_SMOKE_OWNER_CREATED=0
if [[ -z "${WG_SMOKE_OWNER_FILE:-}" ]]; then
    _wg_owner_dir="$(wg_smoke_root)/.owners/${WG_SMOKE_SCENARIO:-adhoc}-${WG_SMOKE_RUN_ID#wg-smoke-v2:}"
    mkdir -p "$_wg_owner_dir"
    WG_SMOKE_OWNER_FILE="$_wg_owner_dir/owner.env"
    _WG_SMOKE_OWNER_CREATED=1
fi
WG_SMOKE_CLEANUP_DIAGNOSTICS="${WG_SMOKE_CLEANUP_DIAGNOSTICS:-$(dirname "$WG_SMOKE_OWNER_FILE")/cleanup-diagnostics.log}"
export WG_SMOKE_OWNER_FILE WG_SMOKE_CLEANUP_DIAGNOSTICS
if [[ ! -s "$WG_SMOKE_OWNER_FILE" ]]; then
    cat >"$WG_SMOKE_OWNER_FILE" <<EOF
version=2
run_id=$WG_SMOKE_RUN_ID
scenario=${WG_SMOKE_SCENARIO:-adhoc}
supervisor_pid=$BASHPID
EOF
fi

# ── scratch dir under the smoke root ────────────────────────────────
make_scratch() {
    local root
    root="$(wg_smoke_root)"
    mkdir -p "$root"
    local scenario="${WG_SMOKE_SCENARIO:-adhoc}"
    # `mktemp -d $root/<scenario>.XXXXXX` keeps everything under one parent
    # AND tags each dir with the scenario it belongs to so a stale leak is
    # immediately attributable to a specific scenario.
    local scratch
    scratch=$(mktemp -d "$root/${scenario}.XXXXXX")
    register_scratch "$scratch"
    echo "$scratch"
}

# ── Cleanup registries ──────────────────────────────────────────────
# IMPORTANT: registrations must survive subshells. `make_scratch` is
# called as `scratch=$(make_scratch)` (command substitution → subshell);
# bash array mutations inside a subshell do NOT propagate to the parent.
# We worked around this by writing entries to per-script registry FILES
# whose paths live in env vars inherited by every subshell, and reading
# them back at cleanup. Daemon and scratch registries both go through
# files for symmetry — that way a future scenario that calls
# `start_wg_daemon` in a subshell does not silently leak.
WG_SMOKE_CLEANUP_HOOKS=()

# Keep registry state inside the durable ownership directory. If the shell is
# SIGKILLed before its trap, the Rust exact-owner backstop removes this state
# only after descendants are gone; no side registry leaks into /tmp.
WG_SMOKE_REGISTRY_DIR="$(mktemp -d "$(dirname "$WG_SMOKE_OWNER_FILE")/registry.XXXXXX")"
export WG_SMOKE_REGISTRY_DIR
WG_SMOKE_SCRATCHES_FILE="$WG_SMOKE_REGISTRY_DIR/scratches"
WG_SMOKE_DAEMONS_FILE="$WG_SMOKE_REGISTRY_DIR/daemons"
WG_SMOKE_TMUX_FILE="$WG_SMOKE_REGISTRY_DIR/tmux"
WG_SMOKE_PROCESSES_FILE="$WG_SMOKE_REGISTRY_DIR/processes"
export WG_SMOKE_SCRATCHES_FILE WG_SMOKE_DAEMONS_FILE WG_SMOKE_TMUX_FILE WG_SMOKE_PROCESSES_FILE
: >"$WG_SMOKE_SCRATCHES_FILE"
: >"$WG_SMOKE_DAEMONS_FILE"
: >"$WG_SMOKE_TMUX_FILE"
: >"$WG_SMOKE_PROCESSES_FILE"

# Wrap tmux only when the real binary was present before defining the function.
# Every scenario that sources this helper then gets strict ownership metadata
# and exact-session teardown without having to remember another ad-hoc trap.
WG_SMOKE_TMUX_BIN="$(type -P tmux 2>/dev/null || true)"
if [[ -n "$WG_SMOKE_TMUX_BIN" ]]; then
    tmux() {
        local -a argv=("$@") prefix=()
        local command_index=-1 session="" mode="default" endpoint=""
        local i
        for ((i=0; i<${#argv[@]}; i++)); do
            case "${argv[$i]}" in
                -L|-S)
                    prefix+=("${argv[$i]}")
                    ((i+=1))
                    [[ $i -lt ${#argv[@]} ]] && prefix+=("${argv[$i]}")
                    if [[ "${argv[$((i-1))]}" == "-L" ]]; then
                        mode="label"
                    else
                        mode="socket"
                    fi
                    endpoint="${argv[$i]:-}"
                    ;;
                new-session|new)
                    command_index=$i
                    break
                    ;;
                *)
                    # Global tmux flags other than -L/-S are retained in the
                    # command itself; owned smoke callers currently use only
                    # the two explicit server selectors above.
                    ;;
            esac
        done
        "$WG_SMOKE_TMUX_BIN" "${argv[@]}"
        local rc=$?
        if [[ $rc -ne 0 || $command_index -lt 0 ]]; then
            return "$rc"
        fi
        for ((i=command_index+1; i<${#argv[@]}; i++)); do
            if [[ "${argv[$i]}" == "-s" && $((i+1)) -lt ${#argv[@]} ]]; then
                session="${argv[$((i+1))]}"
                break
            fi
        done
        [[ -n "$session" ]] || return "$rc"
        local scratch_root
        scratch_root="$(tail -n 1 "$WG_SMOKE_SCRATCHES_FILE" 2>/dev/null || true)"
        [[ -n "$scratch_root" ]] || scratch_root="$(wg_smoke_root)"
        "$WG_SMOKE_TMUX_BIN" "${prefix[@]}" set-option -q -t "$session" \
            @wg_smoke_owned "wg-smoke-v1" >/dev/null 2>&1 || true
        "$WG_SMOKE_TMUX_BIN" "${prefix[@]}" set-option -q -t "$session" \
            @wg_smoke_owner_pid "$BASHPID" >/dev/null 2>&1 || true
        "$WG_SMOKE_TMUX_BIN" "${prefix[@]}" set-option -q -t "$session" \
            @wg_smoke_root "$scratch_root" >/dev/null 2>&1 || true
        printf '%s|%s|%s\n' "$mode" "$endpoint" "$session" >>"$WG_SMOKE_TMUX_FILE"
        return "$rc"
    }
fi

# Add a cleanup hook (function name) to run before daemon teardown. Use
# this to e.g. `tmux kill-session` before the WG dir disappears.
# (Hooks run in the parent shell; this stays as an in-memory array.)
add_cleanup_hook() {
    WG_SMOKE_CLEANUP_HOOKS+=("$1")
}

# Register a scratch dir for `rm -rf` on cleanup. Called automatically by
# `make_scratch`; expose it for callers that mint scratch dirs manually.
register_scratch() {
    printf '%s\n' "$1" >>"$WG_SMOKE_SCRATCHES_FILE"
}

# Read the kernel start identity + process group/session. PID alone is never
# durable ownership proof because it can be reused after a fast exit.
_wg_smoke_proc_identity() {
    local pid="$1" stat rest
    [[ -r "/proc/$pid/stat" ]] || return 1
    stat=$(<"/proc/$pid/stat") || return 1
    rest="${stat##*) }"
    local -a fields
    read -r -a fields <<<"$rest"
    [[ ${#fields[@]} -gt 19 ]] || return 1
    printf '%s %s %s %s %s\n' \
        "${fields[1]}" "${fields[2]}" "${fields[3]}" "${fields[19]}" "${fields[0]}"
}

if [[ "$_WG_SMOKE_OWNER_CREATED" == 1 ]]; then
    _wg_helper_pid=$BASHPID
    _wg_helper_identity=$(_wg_smoke_proc_identity "$_wg_helper_pid" 2>/dev/null || true)
    # shellcheck disable=SC2086
    set -- $_wg_helper_identity
    if [[ $# -ge 4 ]]; then
        printf 'supervisor_pid=%s\nsupervisor_start_ticks=%s\nsupervisor_process_group=%s\nsupervisor_session=%s\n' \
            "$_wg_helper_pid" "$4" "$2" "$3" >>"$WG_SMOKE_OWNER_FILE"
    fi
fi

# Register any owned process with bounded diagnostics: role, PID, PPID, PGID,
# SID, immutable /proc start ticks and state. The exact run-id environment is
# still the authority used immediately before a signal.
register_owned_pid() {
    local pid="$1" role="${2:-process}" identity
    # Registration becomes durable cleanup authority if a descendant later
    # sanitizes its environment. Publish it only while both exact run markers
    # are readable and while a complete immutable PID/start tuple exists.
    _wg_smoke_pid_has_run_id "$pid" "$WG_SMOKE_RUN_ID" \
        || loud_fail "cannot register $role pid $pid without exact local run marker"
    _wg_smoke_pid_has_harness_id "$pid" "$WG_SMOKE_HARNESS_RUN_ID" \
        || loud_fail "cannot register $role pid $pid without exact harness run marker"
    identity=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
    [[ -n "$identity" ]] || loud_fail "cannot register $role pid $pid without immutable process identity"
    printf '%s|%s|%s\n' "$role" "$pid" "$identity" >>"$WG_SMOKE_PROCESSES_FILE"
    if [[ -n "${WG_SMOKE_HARNESS_SEEN_FILE:-}" ]]; then
        # identity is ppid pgid sid start state. Publish the immutable tuple
        # immediately, before a short-lived child can become an environ-less
        # zombie that only the Rust subreaper is able to collect.
        # shellcheck disable=SC2086
        set -- $identity
        [[ $# -ge 5 ]] && printf '%s|%s|%s|%s|%s|%s|%s\n' \
            "$pid" "$1" "$2" "$3" "$4" "$5" "$role" >>"$WG_SMOKE_HARNESS_SEEN_FILE"
    fi
}

# Spawn a non-daemon fixture in its own recorded session. Usage:
#   start_owned_process <role> <log-file> <command> [args...]
# The PID is returned in WG_SMOKE_OWNED_PID and printed for subshell callers.
start_owned_process() {
    local role="$1" log="$2" identity="" ready=0 i; shift 2
    mkdir -p "$(dirname "$log")"
    command -v setsid >/dev/null 2>&1 \
        || loud_fail "cannot launch $role without setsid ownership boundary"
    # The new session leader stops before exec so PID/start registration is
    # durably published while both exact environment markers are guaranteed
    # readable. This closes the short-lived-wrapper race.
    setsid bash -c 'kill -STOP "$$"; exec "$@"' _ "$@" >>"$log" 2>&1 &
    WG_SMOKE_OWNED_PID=$!
    for i in $(seq 1 100); do
        identity=$(_wg_smoke_proc_identity "$WG_SMOKE_OWNED_PID" 2>/dev/null || true)
        if [[ -n "$identity" ]]; then
            # identity is ppid pgid sid start state.
            # shellcheck disable=SC2086
            set -- $identity
            if [[ "$5" == T ]]; then
                ready=1
                break
            fi
        fi
        sleep 0.01
    done
    if [[ "$ready" != 1 ]]; then
        kill -KILL "$WG_SMOKE_OWNED_PID" 2>/dev/null || true
        wait "$WG_SMOKE_OWNED_PID" 2>/dev/null || true
        loud_fail "cannot establish pre-exec registration boundary for $role pid $WG_SMOKE_OWNED_PID"
    fi
    register_owned_pid "$WG_SMOKE_OWNED_PID" "$role"
    kill -CONT "$WG_SMOKE_OWNED_PID" \
        || loud_fail "cannot resume registered $role pid $WG_SMOKE_OWNED_PID"
    printf '%s\n' "$WG_SMOKE_OWNED_PID"
}

# Register a daemon (real PID, WG dir) for teardown. Format:
# `<pid> <dir>` on one line; dir is read with `read pid dir` so dirs with
# spaces are not supported, but the smoke root never contains spaces.
register_wg_daemon() {
    printf '%s %s\n' "$1" "$2" >>"$WG_SMOKE_DAEMONS_FILE"
    register_owned_pid "$1" "wg-daemon"
}

# ── Find .wg or .wg under a scratch dir ──────────────────────
graph_dir_in() {
    local scratch="$1"
    if [[ -d "$scratch/.wg" ]]; then
        echo "$scratch/.wg"; return 0
    fi
    if [[ -d "$scratch/.wg" ]]; then
        echo "$scratch/.wg"; return 0
    fi
    return 1
}

# ── Read the canonical daemon PID from service/state.json ────────────
# `wg service start` forks a child that becomes the real daemon, then
# exits. The child is the only process that knows the WG dir; its
# PID is recorded in state.json. Capturing `$!` from `wg service start &`
# captures the wrapper, NOT the daemon, and on wrapper exit the daemon is
# re-parented to init — `kill $wrapper_pid` then `pkill -P $wrapper_pid`
# both find nothing, and the daemon leaks. Read state.json instead.
wait_for_daemon_pid() {
    local wg_dir="$1"
    local timeout_s="${2:-30}"
    local state="$wg_dir/service/state.json"
    local i pid
    for i in $(seq 1 $((timeout_s * 5))); do
        if [[ -f "$state" ]]; then
            pid=$(grep -oE '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$state" 2>/dev/null \
                | head -1 | grep -oE '[0-9]+$')
            if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
                echo "$pid"
                return 0
            fi
        fi
        sleep 0.2
    done
    return 1
}

# ── Start wg service daemon and register for teardown ────────────────
# Usage: start_wg_daemon <scratch> [extra wg service start args...]
# After this returns, $WG_SMOKE_DAEMON_PID and $WG_SMOKE_DAEMON_DIR hold
# the canonical daemon PID and the WG dir housing it. The daemon
# is registered so `wg_smoke_cleanup` (installed by this file) tears it
# down on script exit.
start_wg_daemon() {
    local scratch="$1"; shift
    local wg_dir
    if ! wg_dir=$(graph_dir_in "$scratch"); then
        loud_fail "no .wg/.wg dir under $scratch — run 'wg init' before start_wg_daemon"
    fi
    local wrap_log="$scratch/daemon.log"
    local launch_cwd="${WG_SMOKE_DAEMON_LAUNCH_CWD:-$scratch}"
    # Pass --dir explicitly so the daemon binds to the scratch fixture
    # regardless of any leaked WG_DIR / discovery context. Most scenarios use
    # the project as cwd for belt-and-braces discovery; a scenario may set
    # WG_SMOKE_DAEMON_LAUNCH_CWD to prove --dir authority from another cwd.
    ( cd "$launch_cwd" && wg --dir "$wg_dir" service start "$@" >"$wrap_log" 2>&1 ) &
    local wrap_pid=$!
    register_owned_pid "$wrap_pid" "wg-start-wrapper"
    local pid
    if ! pid=$(wait_for_daemon_pid "$wg_dir" 30); then
        wait "$wrap_pid" 2>/dev/null || true
        loud_fail "daemon never wrote state.json at $wg_dir/service/state.json. wrapper log:
$(tail -20 "$wrap_log" 2>/dev/null || echo '<no log>')"
    fi
    wait "$wrap_pid" 2>/dev/null || true
    register_wg_daemon "$pid" "$wg_dir"
    WG_SMOKE_DAEMON_PID="$pid"
    WG_SMOKE_DAEMON_DIR="$wg_dir"
    return 0
}

# ── Exact ownership scan / termination ──────────────────────────────
_wg_smoke_pid_has_env_value() {
    local pid="$1" key="$2" value="$3" entry fd
    { exec {fd}<"/proc/$pid/environ"; } 2>/dev/null || return 1
    while IFS= read -r -d '' entry <&"$fd"; do
        if [[ "$entry" == "$key=$value" ]]; then
            exec {fd}<&-
            return 0
        fi
    done
    exec {fd}<&-
    return 1
}

_wg_smoke_pid_has_run_id() {
    _wg_smoke_pid_has_env_value "$1" WG_SMOKE_RUN_ID "$2"
}

_wg_smoke_pid_has_harness_id() {
    _wg_smoke_pid_has_env_value "$1" WG_SMOKE_HARNESS_RUN_ID "$2"
}

_wg_smoke_ancestor_set() {
    local pid="$BASHPID" stat rest ppid out=" $BASHPID $$ "
    while [[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 && -r "/proc/$pid/stat" ]]; do
        stat=$(<"/proc/$pid/stat") || break
        rest="${stat##*) }"
        local -a fields
        read -r -a fields <<<"$rest"
        ppid="${fields[1]:-0}"
        out+="$ppid "
        pid="$ppid"
    done
    printf '%s\n' "$out"
}

# Output: pid|ppid|pgid|sid|start_ticks|state|comm. Both the local ownership
# marker and immutable harness marker are checked before the PID/start tuple is
# retained for signaling and outer-subreaper revalidation.
_wg_smoke_owned_snapshot() {
    local run_id="$1" harness_run_id="${2:-$WG_SMOKE_HARNESS_RUN_ID}" ancestors proc pid identity comm
    ancestors=$(_wg_smoke_ancestor_set)
    [[ -d /proc ]] || return 0
    for proc in /proc/[0-9]*; do
        pid="${proc#/proc/}"
        [[ "$ancestors" == *" $pid "* ]] && continue
        _wg_smoke_pid_has_run_id "$pid" "$run_id" || continue
        _wg_smoke_pid_has_harness_id "$pid" "$harness_run_id" || continue
        identity=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
        [[ -n "$identity" ]] || continue
        comm=$(<"$proc/comm" 2>/dev/null || true)
        # identity is ppid pgid sid start state
        # shellcheck disable=SC2086
        set -- $identity
        printf '%s|%s|%s|%s|%s|%s|%s\n' "$pid" "$1" "$2" "$3" "$4" "$5" "${comm//$'\n'/}"
    done
}

_wg_smoke_signal_snapshot() {
    local run_id="$1" signal="$2" snapshot="$3"
    local pid ppid pgid sid start state comm identity current_start
    while IFS='|' read -r pid ppid pgid sid start state comm; do
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        _wg_smoke_pid_has_run_id "$pid" "$run_id" || continue
        identity=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
        [[ -n "$identity" ]] || continue
        # shellcheck disable=SC2086
        set -- $identity
        current_start="$4"
        [[ "$current_start" == "$start" ]] || continue
        kill -"$signal" "$pid" 2>/dev/null || true
    done <<<"$snapshot"
}

_wg_smoke_signal_registered() {
    local owner_dir="$1" signal="$2" processes role pid recorded current
    for processes in "$owner_dir"/registry.*/processes; do
        [[ -f "$processes" ]] || continue
        while IFS='|' read -r role pid recorded; do
            [[ "$pid" =~ ^[0-9]+$ ]] || continue
            current=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
            [[ -n "$current" ]] || continue
            # identity is ppid pgid sid start state; only start is immutable.
            # shellcheck disable=SC2086
            set -- $recorded
            [[ $# -ge 4 ]] || continue
            local recorded_start="$4"
            # shellcheck disable=SC2086
            set -- $current
            [[ "$4" == "$recorded_start" ]] || continue
            kill -"$signal" "$pid" 2>/dev/null || true
        done <"$processes"
    done
}

_wg_smoke_wait_registered() {
    local owner_dir="$1" processes role pid rest
    for processes in "$owner_dir"/registry.*/processes; do
        [[ -f "$processes" ]] || continue
        while IFS='|' read -r role pid rest; do
            [[ "$pid" =~ ^[0-9]+$ ]] || continue
            wait "$pid" 2>/dev/null || true
        done <"$processes"
    done
}

# Print registered identities that still name the same kernel process. A
# registration was published only while both exact markers were readable, so
# its immutable PID/start tuple remains signal authority after env sanitization.
_wg_smoke_registered_survivors() {
    local owner_dir="$1" processes role pid recorded current
    for processes in "$owner_dir"/registry.*/processes; do
        [[ -f "$processes" ]] || continue
        while IFS='|' read -r role pid recorded; do
            [[ "$pid" =~ ^[0-9]+$ ]] || continue
            current=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
            [[ -n "$current" ]] || continue
            # identity is ppid pgid sid start state; state may legitimately change.
            # shellcheck disable=SC2086
            set -- $recorded
            [[ $# -ge 4 ]] || continue
            local recorded_start="$4"
            # shellcheck disable=SC2086
            set -- $current
            [[ "$4" == "$recorded_start" ]] || continue
            # A zombie adopted by the outer Rust subreaper cannot be collected by
            # this shell. Its registered PID/start tuple is already in the shared
            # ledger, so hand it off rather than treating it as a live survivor.
            if [[ "${WG_SMOKE_HARNESS_OWNED:-0}" == 1 && "$5" == Z ]]; then
                continue
            fi
            printf '%s|%s|%s\n' "$role" "$pid" "$current"
        done <"$processes"
    done
}

_wg_smoke_wait_registered_gone() {
    local owner_dir="$1" survivors i
    for i in $(seq 1 100); do
        survivors=$(_wg_smoke_registered_survivors "$owner_dir")
        [[ -z "$survivors" ]] && return 0
        sleep 0.02
    done
    printf '%s\n' "$survivors"
    return 1
}

_wg_smoke_remember_snapshot() {
    local snapshot="$1" local_seen="$2"
    [[ -n "$snapshot" ]] || return 0
    printf '%s\n' "$snapshot" >>"$local_seen"
    # The outer Rust subreaper owns this immutable ledger. Nested helpers append
    # exact pre-signal PID/start identities so the parent can reap a process
    # even after zombie transition makes /proc/<pid>/environ unreadable.
    if [[ -n "${WG_SMOKE_HARNESS_SEEN_FILE:-}" ]]; then
        printf '%s\n' "$snapshot" >>"$WG_SMOKE_HARNESS_SEEN_FILE"
    fi
}

# TERM, rescan to catch a respawning supervisor, KILL, rescan again, then reap
# direct/adopted children. Failure leaves bounded PID/start/PGID/SID diagnostics
# and returns non-zero so fixture directories are deliberately retained.
_wg_smoke_terminate_run() {
    local run_id="$1" scenario="$2" diag="$3" harness_run_id="${4:-$WG_SMOKE_HARNESS_RUN_ID}"
    local target_owner_dir="${5:-$(dirname "$WG_SMOKE_OWNER_FILE")}" snapshot="" registered_survivors=""
    local seen_file="$WG_SMOKE_REGISTRY_DIR/seen" i
    : >"$seen_file"
    if ! _wg_smoke_procfs_ready; then
        printf 'scenario=%s run_id=%s cleanup refused: readable Linux /proc unavailable; owner evidence retained\n' \
            "$scenario" "$run_id" >>"$diag" 2>/dev/null || true
        return 1
    fi
    for i in $(seq 1 40); do
        snapshot=$(_wg_smoke_owned_snapshot "$run_id" "$harness_run_id")
        registered_survivors=$(_wg_smoke_registered_survivors "$target_owner_dir")
        [[ -z "$snapshot" && -z "$registered_survivors" ]] && break
        _wg_smoke_remember_snapshot "$snapshot" "$seen_file"
        _wg_smoke_signal_snapshot "$run_id" TERM "$snapshot"
        _wg_smoke_signal_registered "$target_owner_dir" TERM
        sleep 0.05
    done
    for i in $(seq 1 60); do
        snapshot=$(_wg_smoke_owned_snapshot "$run_id" "$harness_run_id")
        registered_survivors=$(_wg_smoke_registered_survivors "$target_owner_dir")
        [[ -z "$snapshot" && -z "$registered_survivors" ]] && break
        _wg_smoke_remember_snapshot "$snapshot" "$seen_file"
        _wg_smoke_signal_snapshot "$run_id" KILL "$snapshot"
        _wg_smoke_signal_registered "$target_owner_dir" KILL
        sleep 0.05
    done
    _wg_smoke_wait_registered "$target_owner_dir"
    registered_survivors=$(_wg_smoke_wait_registered_gone "$target_owner_dir") || true
    snapshot=$(_wg_smoke_owned_snapshot "$run_id" "$harness_run_id")
    if [[ "${WG_SMOKE_HARNESS_OWNED:-0}" == 1 && -n "$snapshot" ]]; then
        # Marker-matched zombies have crossed the adoption boundary: only the
        # Rust subreaper can waitpid them. Every pre-signal tuple was appended
        # to its ledger above, so retain only genuinely live survivors here.
        snapshot=$(printf '%s\n' "$snapshot" | awk -F'|' '$6 != "Z"')
    fi
    [[ -z "$snapshot" && -z "$registered_survivors" ]] && return 0
    {
        printf 'scenario=%s\nrun_id=%s\nsupervisor_pid=%s\n' "$scenario" "$run_id" "$BASHPID"
        printf 'pid|ppid|process_group|session|start_ticks|state|command\n'
        sort -u "$seen_file" 2>/dev/null | tail -128
        printf 'marker_survivors:\n%s\n' "$snapshot"
        printf 'registered_survivors:\n%s\n' "$registered_survivors"
    } >"$diag"
    return 1
}

# An owner record is stale only when its complete, exact supervisor PID/start
# identity is gone. Incomplete/legacy records remain evidence regardless of
# age; never infer authority from merely sharing the global smoke root.
_wg_smoke_owner_is_abandoned() {
    local owner_file="$1" pid expected identity
    pid=$(grep '^supervisor_pid=' "$owner_file" 2>/dev/null | tail -1 | cut -d= -f2-)
    expected=$(grep '^supervisor_start_ticks=' "$owner_file" 2>/dev/null | tail -1 | cut -d= -f2-)
    # Incomplete/legacy records are retained evidence, never authority to
    # signal by marker or delete scratch merely because time passed.
    [[ "$pid" =~ ^[0-9]+$ && "$expected" =~ ^[0-9]+$ ]] || return 1
    identity=$(_wg_smoke_proc_identity "$pid" 2>/dev/null || true)
    if [[ -n "$identity" ]]; then
        # shellcheck disable=SC2086
        set -- $identity
        [[ "$4" == "$expected" ]] && return 1
    fi
    return 0
}

_wg_smoke_remove_owner_scratch() {
    local owner_dir="$1" root="$2" scratches d canonical_root canonical
    canonical_root=$(readlink -f "$root" 2>/dev/null || true)
    [[ -n "$canonical_root" ]] || return 1
    for scratches in "$owner_dir"/registry.*/scratches; do
        [[ -f "$scratches" ]] || continue
        while IFS= read -r d; do
            [[ -n "$d" && -d "$d" && ! -L "$d" ]] || continue
            canonical=$(readlink -f "$d" 2>/dev/null || true)
            case "$canonical" in
                "$canonical_root"|"$canonical_root/.owners"/*|"") continue ;;
                "$canonical_root"/*) rm -rf -- "$canonical" || return 1 ;;
            esac
        done <"$scratches"
    done
}

# ── Sweep: terminate explicit stale ownership records, then dirs ────
wg_smoke_sweep() {
    local root owner_file run_id scenario failed=0
    root="$(wg_smoke_root)"
    # The current run may own descendants whose inner crash removed or moved
    # its record. Its exact inherited identity remains sufficient authority.
    _wg_smoke_terminate_run "$WG_SMOKE_RUN_ID" "${WG_SMOKE_SCENARIO:-adhoc}" \
        "$WG_SMOKE_CLEANUP_DIAGNOSTICS" || failed=1
    if [[ -d "$root/.owners" ]]; then
        for owner_file in "$root"/.owners/*/owner.env; do
            [[ -f "$owner_file" ]] || continue
            run_id=$(grep '^run_id=' "$owner_file" 2>/dev/null | head -1 | cut -d= -f2-)
            scenario=$(grep '^scenario=' "$owner_file" 2>/dev/null | head -1 | cut -d= -f2-)
            [[ -n "$run_id" ]] || continue
            _wg_smoke_owner_is_abandoned "$owner_file" || continue
            if _wg_smoke_terminate_run "$run_id" "${scenario:-unknown}" \
                "$(dirname "$owner_file")/cleanup-diagnostics.log" "$run_id" \
                "$(dirname "$owner_file")" \
                && _wg_smoke_remove_owner_scratch "$(dirname "$owner_file")" "$root"; then
                rm -rf "$(dirname "$owner_file")"
            else
                failed=1
            fi
        done
    fi

    # Reap stale tmux sessions only when the session itself carries the exact
    # helper-written ownership tuple. Names such as "wg-*" or "smoke-*" are
    # never proof: real chat/user sessions without these options are invisible.
    local tmux_bin
    tmux_bin="${WG_SMOKE_TMUX_BIN:-$(type -P tmux 2>/dev/null || true)}"
    if [[ -n "$tmux_bin" ]]; then
        local -a server_modes=(default) server_endpoints=("")
        local socket_dir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)" socket
        if [[ -d "$socket_dir" ]]; then
            for socket in "$socket_dir"/*; do
                [[ -S "$socket" ]] || continue
                server_modes+=(socket)
                server_endpoints+=("$socket")
            done
        fi
        local index sessions session owned owner_pid owned_root stale
        for ((index=0; index<${#server_modes[@]}; index++)); do
            if [[ "${server_modes[$index]}" == socket ]]; then
                sessions=$("$tmux_bin" -S "${server_endpoints[$index]}" list-sessions \
                    -F '#{session_name}|#{@wg_smoke_owned}|#{@wg_smoke_owner_pid}|#{@wg_smoke_root}' 2>/dev/null || true)
            else
                sessions=$("$tmux_bin" list-sessions \
                    -F '#{session_name}|#{@wg_smoke_owned}|#{@wg_smoke_owner_pid}|#{@wg_smoke_root}' 2>/dev/null || true)
            fi
            while IFS='|' read -r session owned owner_pid owned_root; do
                [[ "$owned" == "wg-smoke-v1" && -n "$session" ]] || continue
                case "$owned_root" in
                    "$root"|"$root"/*) ;;
                    *) continue ;;
                esac
                stale=0
                [[ -d "$owned_root" ]] || stale=1
                if [[ ! "$owner_pid" =~ ^[0-9]+$ ]] || ! kill -0 "$owner_pid" 2>/dev/null; then
                    stale=1
                fi
                [[ $stale -eq 1 ]] || continue
                if [[ "${server_modes[$index]}" == socket ]]; then
                    "$tmux_bin" -S "${server_endpoints[$index]}" kill-session -t "$session" >/dev/null 2>&1 || true
                else
                    "$tmux_bin" kill-session -t "$session" >/dev/null 2>&1 || true
                fi
            done <<<"$sessions"
        done
    fi
    # Never delete an unregistered directory merely because it sits below the
    # smoke root. Location, age, and a smoke-looking name are not ownership.
    return "$failed"
}

# ── Single EXIT/ERR/INT/TERM/HUP trap installed by this file ─────────
wg_smoke_cleanup() {
    local rc="${1:-$?}" cleanup_failed=0
    # Disable our own trap so cleanup can't re-enter. `set +e` ensures every
    # teardown phase runs even after an assertion or interrupted syscall.
    trap - EXIT ERR INT TERM HUP
    set +e
    # User hooks first (e.g., tmux kill-session before .wg/ disappears).
    local fn
    for fn in "${WG_SMOKE_CLEANUP_HOOKS[@]:-}"; do
        [[ -n "$fn" ]] || continue
        "$fn" 2>/dev/null || true
    done
    # Exact owned tmux sessions first, while their work directories still
    # exist. Never pattern-match names: the registry is populated only after
    # the wrapper successfully writes the wg-smoke-v1 ownership option.
    local tmux_mode tmux_endpoint tmux_session
    if [[ -n "${WG_SMOKE_TMUX_BIN:-}" && -f "$WG_SMOKE_TMUX_FILE" ]]; then
        while IFS='|' read -r tmux_mode tmux_endpoint tmux_session; do
            [[ -n "$tmux_session" ]] || continue
            case "$tmux_mode" in
                label)
                    "$WG_SMOKE_TMUX_BIN" -L "$tmux_endpoint" kill-session -t "$tmux_session" >/dev/null 2>&1 || true
                    ;;
                socket)
                    "$WG_SMOKE_TMUX_BIN" -S "$tmux_endpoint" kill-session -t "$tmux_session" >/dev/null 2>&1 || true
                    ;;
                *)
                    "$WG_SMOKE_TMUX_BIN" kill-session -t "$tmux_session" >/dev/null 2>&1 || true
                    ;;
            esac
        done <"$WG_SMOKE_TMUX_FILE"
    fi
    # Daemon teardown — graceful via IPC, then SIGTERM, then SIGKILL.
    # Read entries from the persistent file (survives subshell registers).
    local pid dir had_daemons=0
    if [[ -f "$WG_SMOKE_DAEMONS_FILE" ]]; then
        while read -r pid dir; do
            [[ -n "$pid" ]] || continue
            had_daemons=1
            if [[ -n "$dir" ]]; then
                wg --dir "$dir" service stop --force >/dev/null 2>&1 || true
            fi
            if kill -0 "$pid" 2>/dev/null; then
                kill -TERM "$pid" 2>/dev/null || true
            fi
        done <"$WG_SMOKE_DAEMONS_FILE"
    fi
    # Exact ownership is the final authority. This catches worker wrappers,
    # observers, fake providers, Pi descendants, daemon supervisors, and
    # respawned/double-forked children — including those whose parent exited.
    _wg_smoke_terminate_run "$WG_SMOKE_RUN_ID" "${WG_SMOKE_SCENARIO:-adhoc}" \
        "$WG_SMOKE_CLEANUP_DIAGNOSTICS" || cleanup_failed=1

    # Only delete fixtures after the complete ownership set is gone. On
    # failure retain both scratch and bounded diagnostics for investigation.
    local d
    # Under the Rust harness, defer fixture and registry deletion to the
    # subreaper. It reaps adopted double-fork descendants first, then consumes
    # these scratch registries and removes the ownership directory.
    if [[ "$cleanup_failed" -eq 0 ]]; then
        if [[ "${WG_SMOKE_HARNESS_OWNED:-0}" != 1 ]]; then
            if [[ -f "$WG_SMOKE_SCRATCHES_FILE" ]]; then
                while read -r d; do
                    [[ -n "$d" ]] || continue
                    if [[ -d "$d" ]]; then
                        rm -rf "$d" 2>/dev/null || true
                    fi
                done <"$WG_SMOKE_SCRATCHES_FILE"
            fi
            if [[ -n "${WG_SMOKE_REGISTRY_DIR:-}" && -d "$WG_SMOKE_REGISTRY_DIR" ]]; then
                rm -rf "$WG_SMOKE_REGISTRY_DIR" 2>/dev/null || true
            fi
            if [[ "$_WG_SMOKE_OWNER_CREATED" == 1 ]]; then
                rm -rf "$(dirname "$WG_SMOKE_OWNER_FILE")" 2>/dev/null || true
            fi
        fi
    else
        echo "SMOKE CLEANUP FAILED: scenario=${WG_SMOKE_SCENARIO:-adhoc} diagnostics=$WG_SMOKE_CLEANUP_DIAGNOSTICS" >&2
        tail -132 "$WG_SMOKE_CLEANUP_DIAGNOSTICS" >&2 2>/dev/null || true
        [[ "$rc" -ne 0 ]] || rc=1
    fi
    exit "$rc"
}

trap 'wg_smoke_cleanup $?' EXIT
trap 'wg_smoke_cleanup 1' ERR
trap 'wg_smoke_cleanup 130' INT
trap 'wg_smoke_cleanup 143' TERM
trap 'wg_smoke_cleanup 129' HUP

# Resolve collision-free attempt runtime storage by authoritative task ID.
# Falls back to the historical flat path for fixtures created by old binaries;
# callers still rely on wg itself to validate the embedded full source tuple.
attempt_runtime_dir() {
    local wg_dir="$1" task_id="$2" attempt_id="$3"
    python3 - "$wg_dir" "$task_id" "$attempt_id" <<'PY'
import glob,json,os,sys
wg,task,attempt=sys.argv[1:]
for manifest in glob.glob(os.path.join(wg,'attempts','by-source-tuple','*','source-tuple.json')):
    try: key=json.load(open(manifest))
    except Exception: continue
    if key.get('task_id')==task and key.get('attempt_id')==attempt:
        print(os.path.dirname(manifest)); raise SystemExit(0)
legacy=os.path.join(wg,'attempts',attempt)
if os.path.isdir(legacy):
    print(legacy); raise SystemExit(0)
raise SystemExit(1)
PY
}

# ── Legacy: kill_tree by direct children. Kept for the rare scenario
#    that owns a non-daemon background process (e.g. tmux). Do NOT use
#    for `wg service start` — start_wg_daemon handles that correctly. ──
kill_tree() {
    local pid="$1"
    if [[ -z "$pid" ]]; then return 0; fi
    if kill -0 "$pid" 2>/dev/null; then
        pkill -P "$pid" 2>/dev/null || true
        kill "$pid" 2>/dev/null || true
        sleep 1
        kill -9 "$pid" 2>/dev/null || true
    fi
}
