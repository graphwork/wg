# Service start readiness & the single-writer daemon fence

Status: implemented (`service-start-readiness`).

This document explains why `wg service start` used to fail its readiness
probe whenever the persistent chat agent was enabled, how that is fixed,
and how the process-level single-writer invariant composes with the
task-level dispatch idempotency owned by `make-dispatch-idempotent`.

## Symptom (2026-10-08, this repo)

```
$ wg service start
readiness timeout after 8000ms: Service IPC response timed out after 0s; the
daemon is alive but unresponsive - restart with wg service start --force
```

A 1.5s process poll showed `wg service start`, `service supervise` and
`service daemon` all alive for the whole 8s window, then all three gone — the
daemon ran fine but never answered the readiness probe, and the failure path
tore the healthy daemon down. After repeated attempts, **two** daemons were
left re-parented to init (`ppid=1`), both ticking on the same socket. The
proven workaround was `wg service start --no-chat-agent`, which reached ready
immediately.

## Why chat-agent startup blows an 8s budget

`run_daemon` binds the socket early but the IPC accept loop only starts after
all pre-loop startup work. The readiness challenge (`IpcRequest::Readiness`) is
answered by that loop, so any pre-loop work longer than the probe window
(8s total, 350ms per probe) fails readiness even though the process is healthy.

The dominant pre-loop cost was the **synchronous boot spawn of the persistent
chat supervisors**:

- `enumerate_chat_supervisors_for_boot` scans the graph for every live
  `.chat-N` / legacy `.coordinator-N` chat-loop task. A long-lived graph
  accumulates many of them, including ancient panes.
- For each, `CoordinatorAgent::spawn` runs `Config::load_or_default` +
  `plan_spawn`, and (on the `claude` handler) shells out to `claude --version`.
  Cost is roughly `N × latency`, all serialized on the main thread.
- The spawned supervisor threads then churn on session-lock recovery, flood the
  shared `DaemonLogger`, and the dispatcher re-prioritizes ancient chat tasks —
  visible as `[coordinator-N stdout] …` and `[session-lock] recovering stale
  lock …` noise, plus repeated `Priority bump: .chat-N (age: 4xx h) -> 100`.

So enabling the chat agent added an unbounded `N`-dependent serial startup
prefix in front of the readiness responder.

### Should the probe window scale with the chat agent?

**No.** A longer probe window is a band-aid: the cost grows with the number of
chat panes, so the window would need to grow without bound, and it would hide
the real defect (readiness gated behind unrelated work). The fix is structural:

1. **Decouple readiness from chat-agent startup.** Chat-supervisor boot runs on
   a background thread (`wg-chat-boot`) and its results are drained by the main
   loop through a channel. The IPC accept loop starts immediately, so readiness
   is answered long before any LLM session is spawned. `STARTUP_TIMEOUT` (8s)
   remains a *probe* bound, not a claim about daemon health.
2. **Never tear down a live daemon.** If the probe window expires but the
   spawned instance is alive by process identity (`state.json` PID + recorded
   process-birth identity), `run_start` reports it as running with a degraded
   note instead of killing it. Only a daemon that is actually gone (exit before
   readiness / birth mismatch) fails the start.

## Single-writer fence (at most one daemon per socket)

Two kernel-backed advisory locks (`src/commands/service/lock.rs`) replace the
best-effort `state.json` check:

- **`service/daemon.lock`** — held by the daemon for its whole lifetime. The
  carrier file records `pid` + OS process-birth identity + socket. `flock(2)`
  is the authority and is released automatically when the process dies (even on
  `SIGKILL`), so a crashed predecessor never wedges a restart, while a *live*
  second daemon can never bind the same socket.
- **`service/start.lock`** — serializes concurrent `wg service start`
  invocations. The loser observes the winner's daemon lock and refuses loudly
  rather than stacking a second supervisor+daemon pair.

`--force` is **kill-and-confirm**: it asks the current owner to shut down via
IPC, signals it (process-birth identity verified, never a reused PID), and waits
until the lock is free and the identity is gone before allowing a replacement to
start. It refuses loudly if it cannot confirm the old daemon exited.

Failed starts reap their own supervisor+daemon tree with the *scoped* kill (a
detached in-flight worker is never collateral) and also reap a daemon that
already re-parented to init. A `daemon.lock` whose recorded owner is provably
dead is reaped on the next start.

`wg status` derives `running/stopped` from **process identity**, not the state
file alone: if `state.json` is gone but a daemon process for the graph is
observably alive, it still reports running. `Service: stopped` can no longer be
printed while a daemon is dispatching.

## Composition with `make-dispatch-idempotent`

The two tasks guard **different layers** and must not duplicate each other:

| Concern | Owner | Mechanism |
| --- | --- | --- |
| One daemon/coordinator loop per socket & graph | `service-start-readiness` (this) | `service/daemon.lock` + `service/start.lock`, process identity |
| One attempt per task across a daemon restart | `make-dispatch-idempotent` | persisted claim/lease reconciliation at startup, definition-hash task identity |

Because this change guarantees a **single coordinator loop**, the dispatch
reconciler runs in exactly one process: there is no second loop that could race
a claim or re-emit dispatch. `make-dispatch-idempotent` therefore only needs
task-level reconciliation and must **not** add a second process-level guard.
Neither task weakens the existing fences: the claim fence and the
different-task worktree guard keep failing closed; this task removes the
*stacked process* source of redundant dispatch, not the fences themselves.

## Error text

The old failure footer said `Recovery: wg service start --force` — circular,
since `--force` itself failed. It now names the actionable remedy:

```
Recovery: retry with `wg service start --no-chat-agent` if the persistent chat
agent is what is blocking startup; otherwise inspect the daemon log above (<path>).
```
