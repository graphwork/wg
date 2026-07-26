# Daemon silent-crash root cause + auto-restart supervisor

**Tags:** daemon, stability, lifecycle
**Task:** `fix-coordinator-daemon`
**Status:** implemented

## 1. Root cause of the silent death

Symptom: the service daemon disappears mid coordinator tick. `daemon.log` ends
at `Coordinator tick #N starting/complete` with **no** `ERROR`/`FATAL`/panic
line, the PID is gone, and dispatch halts until a manual `wg service start`.
Recurred multiple times. Not OOM (RAM free, no `dmesg` OOM).

### Why it is silent

`run_daemon` (`src/commands/service/mod.rs`) has exactly three exit classes:

1. **Graceful** — `running = false` (set by the IPC `Shutdown` handler) → it logs
   `Daemon shutting down` / `Daemon shutdown complete`, removes `state.json`,
   and returns `Ok(())`.
2. **Startup `Err`** — a bad config / socket-bind failure returns `Err`, which
   `main()` propagates to stderr (redirected to `daemon.log`) as an error line.
3. **Panic** — the `DaemonLogger::install_panic_hook` writes a `FATAL` line.

The recurring incident shows **none** of these. That rules out a panic (the hook
would log `FATAL`) and an `Err` return (stderr → log). The only remaining class
is a **signal**: `SIGTERM` / `SIGHUP` / `SIGINT` / `SIGQUIT` terminate the
process immediately with the default disposition — the panic hook does **not**
run for signals and `run_daemon` never returns, so nothing is logged. The
process simply vanishes.

`setsid()` (in `run_start`'s `pre_exec`) detaches the daemon from the launching
terminal so a close-PTY `SIGHUP` no longer reaches it (regression-pinned by
`service_daemon_survives_launch_session_hangup.sh`), but it does **not** install
any signal handlers, so any other signal source (a stray `kill`, a
process-group-wide `SIGTERM`, a cgroup/session teardown, a library that resets
`SIGPIPE`) still kills the daemon with zero log output.

The retained incident logs cannot identify the sender after the fact, so the
exact external signal is not provable. They do establish the failure class:
a default-disposition signal (or uncatchable `SIGKILL`) is the only exit path
that bypassed every existing error/panic/graceful log. The missing signal
handling made that class silent; the fix both mitigates catchable signals and
makes any uncatchable/fatal exit loud from the supervising parent.

## 2. The fix (four parts)

### 2.1 Catch + log + survive signals (the core fix)

Install async-signal-safe handlers (self-pipe → main-loop poll, reusing the same
`libc::poll` slot pattern as the graph-watcher self-pipe):

- `SIGINT`, `SIGHUP`, `SIGQUIT` → log `WARN: survived stray signal <N>` and
  **keep ticking**. A detached daemon shrugging off terminal/session signals is
  correct and **directly stops the silent-death incidents**.
- `SIGPIPE` → `SIG_IGN` (Rust's default, made explicit/defensive) + a one-shot
  `WARN` if it ever fires.
- `SIGTERM` → log `WARN: received SIGTERM, shutting down` and do a graceful
  shutdown **without** the clean-sentinel, so the supervisor restarts it
  (systemd semantics: a raw `kill` restarts; `wg service stop` stops cleanly).

No death is silent after this: every catchable signal is logged before the
process exits, and the truly fatal cases (`SIGKILL`, segfault) are covered by
the supervisor (§2.4) and the crash sentinel (§2.3).

### 2.2 Stronger panic hook

The hook now also writes the panic to raw fd 2 (`libc::write`) and `fsync`s
before the structured logger write, so a panic is visible even if the
`DaemonLogger` mutex is poisoned or its file handle is closed.

### 2.3 Crash sentinel

`.wg/service/.clean_shutdown` is written **only** at the end of a **requested**
(IPC `Shutdown`) shutdown. The supervisor consumes it: present = clean stop;
absent after child exit = crash. On startup the daemon also surfaces a loud
`WARN` when a prior run left no sentinel.

### 2.4 External auto-restart supervisor

`wg service supervise` (hidden, internal) is a tiny restart loop launched by
`wg service start` (default-on; `--no-supervise` opts out to the legacy direct
fork):

1. Fork `wg service daemon …` as its child, write `state.json` (`pid` = the
   daemon child, unchanged contract; new optional `supervisor_pid`).
2. `try_wait` poll; on child exit:
   - clean sentinel present → consume it, exit (intentional stop).
   - own `SIGTERM`/`SIGINT` received → exit (system/user stop, no restart).
   - otherwise → exponential backoff (1 s → 30 s cap) + a restart budget (N
     per 5 min window, reset after a >60 s healthy run) and re-fork, updating
     `state.json` with the new daemon PID.
3. Its own `SIGTERM`/`SIGINT` set a stopping flag (no restart) so a system
   shutdown or `wg service stop --force` (which kills the supervisor tree via
   `supervisor_pid`) terminates cleanly.

`wg service stop` (normal) is unchanged: IPC `Shutdown` → daemon writes the
sentinel → exits → supervisor sees the sentinel → exits. `--force` /
orphan-cleanup additionally kills the supervisor tree so a crash-exit is not
immediately re-spawned.

## 3. Validation mapping

| Task criterion | How it is met |
| --- | --- |
| Root cause identified + fixed | §1 (signals) + §2.1 (handlers that survive/log) |
| Future crash writes ERROR/panic to log before exit | §2.1 (signals) + §2.2 (panic→stderr) |
| Auto-restart on unexpected exit | §2.4 (supervisor); pinned by `service_daemon_auto_restart_on_crash.sh` |
