#!/usr/bin/env bash
# Real tmux/PTY regression for finish-aux-tui-snapshot-lanes.
#
# After the graph has loaded, a small LD_PRELOAD shim delays matching stat,
# open and first-read calls by 500ms (or WG_TUI_AUX_LATENCY_MS). We visit every
# reachable inspector tab and immediately press help. The visible help overlay
# must acknowledge each key in under 100ms even while auxiliary storage
# snapshots are stalled.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
command -v cc >/dev/null 2>&1 \
    || loud_skip "MISSING C COMPILER" "cc is needed to build the latency shim"

aux_latency_ms=${WG_TUI_AUX_LATENCY_MS:-500}
[[ "$aux_latency_ms" =~ ^[1-9][0-9]*$ ]] \
    || loud_fail "WG_TUI_AUX_LATENCY_MS must be a positive integer"

repo_root="$(cd "$HERE/../../.." && pwd)"
scratch=$(make_scratch)
shim_src="$scratch/fs_latency.c"
shim_so="$scratch/fs_latency.so"
control="$scratch/latency-ms"
calls="$scratch/delayed-calls"

# Keep this source local to the scenario: it is test instrumentation, not a
# shipped library. Raw syscalls read the control file and append counters so
# the shim cannot recursively intercept itself.
cat >"$shim_src" <<'C'
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/prctl.h>
#include <unistd.h>

static _Atomic unsigned char tracked[65536];

static int latency_ms(void) {
    const char *path = getenv("WG_FS_SHIM_CONTROL");
    if (!path) return 0;
    int fd = (int)syscall(SYS_openat, AT_FDCWD, path, O_RDONLY, 0);
    if (fd < 0) return 0;
    char buf[32] = {0};
    long n = syscall(SYS_read, fd, buf, sizeof(buf) - 1);
    syscall(SYS_close, fd);
    return n > 0 ? atoi(buf) : 0;
}

static int matches(const char *path) {
    const char *needle = getenv("WG_FS_SHIM_MATCH");
    return path && needle && strstr(path, needle) != NULL;
}

static void mark_and_delay(const char *kind, const char *subject) {
    const char *path = getenv("WG_FS_SHIM_CALLS");
    int ms = latency_ms();
    if (ms <= 0) return;
    if (path) {
        int fd = (int)syscall(SYS_openat, AT_FDCWD, path,
                              O_WRONLY | O_CREAT | O_APPEND, 0600);
        if (fd >= 0) {
            char line[1024];
            char thread[17] = {0};
            if (prctl(PR_GET_NAME, thread, 0, 0, 0) != 0) strcpy(thread, "unknown");
            const char *role = getpid() == syscall(SYS_gettid) ? "main" : "worker";
            int len = snprintf(line, sizeof(line), "%s %s:%s %s\n", kind, role,
                               thread, subject ? subject : "-");
            syscall(SYS_write, fd, line, (size_t)len);
            syscall(SYS_close, fd);
        }
    }
    usleep((useconds_t)ms * 1000);
}

static int real_open_call(const char *path, int flags, mode_t mode, int is64) {
    int (*fn)(const char *, int, ...) = dlsym(RTLD_NEXT, is64 ? "open64" : "open");
    if (matches(path)) mark_and_delay("open", path);
    int fd = (flags & O_CREAT) ? fn(path, flags, mode) : fn(path, flags);
    if (fd >= 0 && fd < (int)sizeof(tracked)) atomic_store(&tracked[fd], matches(path));
    return fd;
}

int open(const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    return real_open_call(path, flags, mode, 0);
}
int open64(const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    return real_open_call(path, flags, mode, 1);
}

static int real_openat_call(int dirfd, const char *path, int flags, mode_t mode, int is64) {
    int (*fn)(int, const char *, int, ...) = dlsym(RTLD_NEXT, is64 ? "openat64" : "openat");
    if (matches(path)) mark_and_delay("open", path);
    int fd = (flags & O_CREAT) ? fn(dirfd, path, flags, mode) : fn(dirfd, path, flags);
    if (fd >= 0 && fd < (int)sizeof(tracked)) atomic_store(&tracked[fd], matches(path));
    return fd;
}
int openat(int dirfd, const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    return real_openat_call(dirfd, path, flags, mode, 0);
}
int openat64(int dirfd, const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = va_arg(ap, int); va_end(ap); }
    return real_openat_call(dirfd, path, flags, mode, 1);
}

ssize_t read(int fd, void *buf, size_t count) {
    ssize_t (*fn)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    if (fd >= 0 && fd < (int)sizeof(tracked) && atomic_exchange(&tracked[fd], 0))
        mark_and_delay("read", "tracked-fd");
    return fn(fd, buf, count);
}

int stat(const char *path, struct stat *st) {
    int (*fn)(const char *, struct stat *) = dlsym(RTLD_NEXT, "stat");
    if (matches(path)) mark_and_delay("stat", path);
    return fn(path, st);
}
int lstat(const char *path, struct stat *st) {
    int (*fn)(const char *, struct stat *) = dlsym(RTLD_NEXT, "lstat");
    if (matches(path)) mark_and_delay("stat", path);
    return fn(path, st);
}
int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    int (*fn)(int, const char *, struct stat *, int) = dlsym(RTLD_NEXT, "fstatat");
    if (matches(path)) mark_and_delay("stat", path);
    return fn(dirfd, path, st, flags);
}
#ifdef SYS_statx
int statx(int dirfd, const char *path, int flags, unsigned mask, struct statx *st) {
    int (*fn)(int, const char *, int, unsigned, struct statx *) = dlsym(RTLD_NEXT, "statx");
    if (matches(path)) mark_and_delay("stat", path);
    return fn(dirfd, path, flags, mask, st);
}
#endif
C

cc -shared -fPIC -O2 -o "$shim_so" "$shim_src" -ldl \
    || loud_fail "failed to build latency shim"

cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg add "Auxiliary latency probe" --id auxiliary-latency-probe >add.log 2>&1 \
    || loud_fail "fixture task failed: $(tail -10 add.log)"

session="wgsmoke-aux-latency-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

tmux new-session -d -s "$session" -x 140 -y 45 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER LD_PRELOAD='$shim_so' WG_FS_SHIM_CONTROL='$control' WG_FS_SHIM_CALLS='$calls' WG_FS_SHIM_MATCH='.wg' wg tui --no-mouse --show-keys"

capture() {
    tmux capture-pane -p -t "$session" 2>/dev/null || true
}

ensure_host_command_mode() {
    local screen
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q '\[PTY\]'; then
        tmux send-keys -t "$session" C-o
        for _ in $(seq 1 30); do
            screen=$(capture)
            if ! printf '%s\n' "$screen" | grep -q '\[PTY\]'; then
                return 0
            fi
            sleep 0.005
        done
        return 1
    fi
}

loaded=0
for _ in $(seq 1 300); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'auxiliary-latency-probe'; then
        loaded=1
        break
    fi
    sleep 0.02
done
(( loaded == 1 )) || loud_fail "TUI fixture did not load before latency injection"

# The Chat tab may own an embedded PTY, where bare `?` correctly belongs to
# the child rather than the host. Establish TUI command mode before measuring:
# try help once, and use the documented Ctrl+O escape if the child consumed it.
tmux send-keys -t "$session" '?'
preflight_help=0
for _ in $(seq 1 30); do
    screen=$(capture)
    if printf '%s\n' "$screen" | grep -q 'Navigation'; then
        preflight_help=1
        break
    fi
    sleep 0.01
done
if (( preflight_help == 0 )); then
    tmux send-keys -t "$session" C-o
    tmux send-keys -t "$session" '?'
    for _ in $(seq 1 30); do
        screen=$(capture)
        if printf '%s\n' "$screen" | grep -q 'Navigation'; then
            preflight_help=1
            break
        fi
        sleep 0.01
    done
fi
(( preflight_help == 1 )) || loud_fail "could not establish TUI command mode"
tmux send-keys -t "$session" Escape

printf '%s\n' "$aux_latency_ms" >"$control"
# Let the one-second periodic refresh submit chat/service work before tab churn.
sleep 1.05

max_ms=0
for tab in 0 1 2 3 4 5 6 7 8; do
    ensure_host_command_mode \
        || loud_fail "could not escape embedded PTY before measuring tab $tab"
    tmux send-keys -t "$session" "$tab"
    # Explicitly force the refresh controls on panels that expose one.
    case "$tab" in
        1) tmux send-keys -t "$session" R ;;
        3|8) tmux send-keys -t "$session" r ;;
    esac

    start_ns=$(date +%s%N)
    deadline_ns=$(( start_ns + 100000000 ))
    tmux send-keys -t "$session" '?'
    acked=0
    while (( $(date +%s%N) < deadline_ns )); do
        screen=$(capture)
        if printf '%s\n' "$screen" | grep -q 'Navigation'; then
            acked=1
            break
        fi
        sleep 0.002
    done
    elapsed_ms=$(( ($(date +%s%N) - start_ns) / 1000000 ))
    if (( acked == 0 )); then
        screen=$(capture)
        delayed=$(tail -40 "$calls" 2>/dev/null || true)
        loud_fail "tab $tab did not acknowledge help under storage delay; delayed calls:\n$delayed\nscreen:\n$screen"
    fi
    (( elapsed_ms < 100 )) \
        || loud_fail "tab $tab help acknowledgement ${elapsed_ms}ms exceeded 100ms"
    (( elapsed_ms > max_ms )) && max_ms=$elapsed_ms
    tmux send-keys -t "$session" Escape
    for _ in $(seq 1 30); do
        screen=$(capture)
        if ! printf '%s\n' "$screen" | grep -q 'Navigation'; then
            break
        fi
        sleep 0.002
    done
done

# Prove all three requested syscall classes were genuinely delayed. The lane
# is intentionally serial and each preceding syscall costs 500ms, so allow the
# worker to progress through metadata probes to open + first read after the
# latency-sensitive input measurements have already completed. The loop is
# long enough for the acceptance matrix's maximum five-second injection.
observed_all=0
for _ in $(seq 1 600); do
    if grep -q '^stat ' "$calls" 2>/dev/null \
        && grep -q '^open ' "$calls" 2>/dev/null \
        && grep -q '^read ' "$calls" 2>/dev/null; then
        observed_all=1
        break
    fi
    sleep 0.05
done
(( observed_all == 1 )) \
    || loud_fail "latency shim did not observe delayed stat, open, and read calls"
if grep -Eq '^(stat|open|read) main:' "$calls" 2>/dev/null; then
    loud_fail "project storage syscall reached the TUI main thread:\n$(grep -E '^(stat|open|read) main:' "$calls")"
fi

# Static boundary: production render/input files may submit snapshot requests,
# but may not contain direct storage/subprocess calls or invoke legacy loaders.
awk '/^#\[cfg\(test\)\]/{exit} {print}' \
    "$repo_root/src/tui/viz_viewer/event.rs" >"$scratch/event-production.rs"
awk '/^#\[cfg\(test\)\]/{exit} {print}' \
    "$repo_root/src/tui/viz_viewer/render.rs" >"$scratch/render-production.rs"
if rg -n 'std::fs|std::process::Command|Config::load|File::open|read_to_string|read_dir|metadata\(|\.exists\(' \
        "$scratch/event-production.rs" "$scratch/render-production.rs" >"$scratch/static-audit.log"; then
    loud_fail "direct blocking call remains in input/render:\n$(cat "$scratch/static-audit.log")"
fi
if rg -n 'app\.(load_hud_detail|load_log_pane|load_messages_panel|load_agency_lifecycle|load_coord_log|load_activity_feed|load_settings_panel|update_log_output|update_log_stream_events|update_service_health|update_vitals|poll_chat_messages)\(' \
        "$scratch/event-production.rs" "$scratch/render-production.rs" >"$scratch/loader-audit.log"; then
    loud_fail "legacy blocking loader remains reachable from input/render:\n$(cat "$scratch/loader-audit.log")"
fi

tmux send-keys -t "$session" Escape q
echo "PASS: every reachable TUI tab acknowledged input below 100ms under ${aux_latency_ms}ms delayed stat/open/read (max=${max_ms}ms)"
