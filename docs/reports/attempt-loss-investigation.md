# Attempt-loss investigation: `attempt-lost … reason=process_identity_dead`

Status: **investigation complete, production fix proposed as a scoped follow-up**
(mandate: do not change production restart semantics without operator approval).
Author: `investigate-the-attempt` (agent-169). Binary under investigation:
`/home/bot/.cargo/bin/wg` **sha256-prefix `4ce11904920d`** — the same binary the
daemon exec-restarted into at 22:12:45 in the incident window (see §4), so the
observed behaviour and the source in this worktree are the same revision family.

---

## TL;DR

- Of the five observed losses, **four were caused by the same thing**: a daemon
  stop/restart **tree-killed the live, running workers**. The service lifecycle
  calls `kill_process_graceful(supervisor_pid)` / `kill_process_graceful(daemon_pid)`
  (and `--force` variants). Those helpers walk the **`/proc` ppid descendant
  tree** (`collect_process_descendants`) and signal every descendant. Detached
  task workers are `setsid()`-isolated into their own session, but they are still
  **children of the daemon by PPID**, so the ppid walk reaches them and kills
  them regardless of the session boundary. The restart is not "orphaning" the
  attempt — it is **killing the worker**.
- The fifth loss (`calib-doc-list-audit-rows`, 1.7 s, `agent: (none)`) is a
  **separate** mechanism: the reconciler's `Status::InProgress` **with no
  `assigned` agent** branch has **no grace period**, so a transient
  claim-without-binding projection is converted to a lost attempt on the very
  next dispatcher tick.
- The 60de44a8 process-group reaper (`reap_orphaned_process_group`) is **ruled
  out** as the cause. It only fires for agents **already marked dead** (root PID
  already gone), refuses its own group/session, and a control experiment shows it
  leaves a live registered worker untouched.
- **Policy answer:** a daemon restart **must not** be able to kill or fail a
  running attempt. The new daemon already *adopts* a worker whose recorded PID is
  live (control experiment §5.2); the fix is to stop the restart from killing the
  worker in the first place, and to close the zero-grace `assigned: None` hole.

---

## 1. The claim → reconcile identity path, step by step

What is **recorded** at spawn:
- `spawn_agents_for_ready_tasks` (running **inside the daemon**,
  `src/commands/service/coordinator.rs:2859`) calls `spawn::spawn_agent_*` →
  `execution::spawn_agent_inner_authorized`. The wrapper is `cmd.spawn()`ed by the
  daemon itself, so the worker's PPID is the daemon PID.
- The wrapper is `setsid()`-detached in `pre_exec`
  (`src/commands/spawn/execution.rs:2562-2571`), so its **pgid == its own PID**
  and it is a session leader.
- The registry entry (`src/service/registry.rs`, `AgentEntry`) records:
  `pid` = wrapper PID, `pgid` = wrapper PID (set at
  `execution.rs:2844-2851`), `started_at` = wall-clock spawn time,
  `status` = Working, `last_heartbeat`, `task_id`, `worktree_path`.
- `claim_task_for_spawn_bound` (`execution.rs:927`) atomically, under the graph
  lock, applies `AttemptReserved` (which projects `Status::InProgress`) **and**
  sets `task.assigned = Some(agent_id)` in the same `modify_graph` write.

What is **checked** at reconcile (`reconcile_orphaned_tasks`,
`src/commands/sweep.rs:418-560`; actor `reconcile`, reason string
`process_identity_dead`):

```rust
let dominated = match &task.assigned {
    Some(agent_id) => match registry.get_agent(agent_id) {
        Some(agent) => agent.status == AgentStatus::Dead
            || (agent.is_alive() && !is_process_alive(agent.pid)),
        None => { /* Open => true; InProgress => true only after >5min                */ }
    },
    None => task.status == Status::InProgress && !chat_loop_tag, // <-- no grace
};
```

`is_process_alive` is `kill(pid, 0) == 0`
(`src/service/mod.rs:78-80`). On `dominated`, the task receives
`TransitionKind::AttemptLost` with `reason = "process_identity_dead"` (or
`stale-open-claim` / `orphan-before-spawn` for the Open / no-current-attempt
shapes), then `assigned = None`, `retry_count += 1`, `started_at = None`.

The complementary, earlier paths that also produce attempt loss:
- `src/commands/service/triage.rs:202-229` (`detect_dead_reason`): process gone
  (`ProcessExited`) or PID-reuse identity mismatch (`PidReused`), after a
  **grace period** (`config.agent.reaper_grace_seconds`). Reason
  `worker_process_observed_dead`.
- `src/commands/dead_agents.rs:221+`: `exact_process_missing`.

### Every condition that can yield `process_identity_dead`

`process_identity_dead` (sweep.rs) is emitted whenever an `InProgress` attempt is
"dominated". Enumerated:

1. **Agent registered Dead** (`agent.status == Dead`).
2. **Agent alive-marked but process gone** (`agent.is_alive() && !is_process_alive(pid)`).
   This is the normal "wrapper exited" case, and it is also what fires **immediately
   after a restart tree-kill** because the registry status is still `Working`
   while the PID is already gone (no grace on this branch).
3. **Agent absent from registry**: `Open` → immediate; `InProgress` → only if
   `started_at` is > 5 min old (a partial defence); `InProgress` with no
   `started_at` → ignored.
4. **`assigned == None` while `InProgress`** (non-chat, non-compact-loop) →
   **immediate, no age/grace check** (the `calib-doc-list-audit-rows` case).
5. PID reuse: `is_process_alive` alone cannot detect it; triage's
   `verify_process_identity` (`src/service/mod.rs:631`, `actual_start >
   expected_start + 120` → false) is the only reuse guard, and it applies in
   triage, **not** in `reconcile_orphaned_tasks`.
6. A process in a different process group/session is **irrelevant** to this
   check — `is_process_alive` only tests existence by PID. That is precisely why
   the ppid-tree kill (§4) is invisible to the identity model: the worker's
   session is different, but it is still killed, and then the identity check
   correctly reports the PID gone.

There is **no start-time/identity marker in the claim→reconcile check itself**;
the only evidence used is `AgentStatus` + bare `kill(pid,0)`. The PID-reuse
start-ticks marker exists (`read_proc_start_ticks`) and is used by the heartbeat
watcher and triage, but **not** by the reconciler.

---

## 2. The five observed losses (graph + log evidence)

All times UTC. Extracted from `/home/bot/wg/.wg/graph.jsonl` (`log` arrays) and
`/home/bot/wg/.wg/service/daemon.log`.

| # | Task | Spawned | Reconciled lost | Mechanism |
|---|------|---------|-----------------|-----------|
| 1 | `calib-doc-list-audit-rows` | — | 22:06:32.777 | `InProgress` + `assigned=None`, **zero grace** |
| 2 | `calibration-batch-run` (agent-161) | 21:29:55 | 22:12:51.858 | restart tree-kill |
| 3 | `complete-the-first` (agent-163) | 22:08:40 | 22:12:51.858 | restart tree-kill |
| 4 | `survey-how-flip` (agent-164) | 22:12:55 | 22:13:02.161 | restart tree-kill |
| 5 | `plugin-wakeup-labels` (agent-165) | 22:13:00 | 22:13:02.161 | restart tree-kill |

Task log quotes:

```
calib-doc-list-audit-rows:
  22:06:32.777 reconcile  "Reconciliation: lost attempt recorded as failed
                           (was InProgress, agent: (none))"
  22:06:56.808            "Task reset for retry from failed (attempt #2)
                           — reason: recover from a claim without a worker process"

calibration-batch-run:
  21:29:55.305 agent-161  "Spawned by coordinator …"
  22:12:51.858 reconcile  "… lost attempt … (was InProgress, agent: agent-161)"

complete-the-first:
  22:08:40.769 agent-163  "Spawned by coordinator …"
  22:12:51.858 reconcile  "… lost attempt … (was InProgress, agent: agent-163)"

survey-how-flip:
  22:12:55.962 agent-164  "Spawned by coordinator …"
  22:13:02.161 reconcile  "… lost attempt … (was InProgress, agent: agent-164)"

plugin-wakeup-labels:
  22:13:00.240 agent-165  "Spawned by coordinator …"
  22:13:02.161 reconcile  "… lost attempt … (was InProgress, agent: agent-165)"
```

The **identical timestamps in pairs** (22:12:51.858 for #2/#3; 22:13:02.161 for
#4/#5) prove a single reconciler pass per restart event, not independent
per-task failures.

Work products confirm the workers were killed mid-turn:
- `agent-161/raw_stream.jsonl` (7.2 MB) ends on
  `tool_execution_start` for a `cargo test` — no `tool_execution_end`; the stream
  is cut mid-tool-call.
- `agent-163/raw_stream.jsonl` ends on a truncated `toolcall_delta` — again cut
  mid-turn.
- `agent-164/raw_stream.jsonl` contains **only** the session-init line (pi started
  at 22:12:56 and never produced a turn) → killed at the 22:13 restart.
- `agent-165/raw_stream.jsonl` is **0 bytes** → killed while still bootstrapping.
- `docs/reports/review-calibration-batch.md` is absent — `calibration-batch-run`
  never got to write its report (Validation criterion: report absent).

`complete-the-first` is the strongest damage case and is now recorded on this
task: before being killed it had (a) fixed `.github/workflows/release.yml` on
`wg/agent-163/complete-the-first` (commit `6da00f55`, pushed, not on `main`) and
(b) **deleted GitHub Release v0.1.0** to allow a fresh publish. The kill stranded
the fix off-main and left the world half-mutated. A lost attempt that performed a
destructive step cannot be "retried" back to consistency; see §6.

---

## 3. Reaper hypothesis (commit 60de44a8) — RULED OUT

The reaper `reap_orphaned_process_group` (`src/service/mod.rs:263`) does **not**
explain the incident:

- It is invoked only from terminal-state paths: `dead_agents::run_cleanup` and
  `disk cleanup --execute`, and only over `dead_info`, which is filtered by
  `!is_process_alive(agent.pid)` (`dead_agents.rs:114`). A live worker mid-attempt
  is never in that set.
- Its guard `process_group_signal_is_safe` refuses pgid 0/1, the caller's own
  pgid, own session id, and own PID (`src/service/mod.rs:236-250`).
- Commit 60de44a8 claims "live agents (Working + live exact PID) are structurally
  excluded" — and that held in the control experiment.

Control experiment (isolated `HOME`/`--dir`, shell worker `sleep 600`):

```
agent agent-1 pid 2734450 pgid 2734450 status working task long
$ wg dead-agents --cleanup
Dead agent cleanup (threshold: 5 minutes):
No dead agents detected.
→ worker 2734450 still alive afterwards
```

Reaper-related daemon-log lines at the loss times are **only** triage marking
already-dead agents and later disk-cache reaps, all of which run *after* the
worker PID was already gone (e.g. `[triage] Dead agent cleanup … agent-161`,
22:12:51.296; `Disk cleanup: reaped 4 owned target(s)`, 22:13:19). The reaper is a
consequence, not the cause.

Secondary note (not the incident, but a latent hazard): the reaper signals the
recorded `pgid` = wrapper PID. If a worker ever outlives its wrapper (wrapper
exits while the real handler keeps running), the reaper would kill a live group.
For the pi wrapper this does not happen because the wrapper `wait`s on the pi
child. Flagged for completeness only.

---

## 4. Daemon-restart hypothesis — CONFIRMED (with the actual kill mechanism)

The restart is the trigger for losses #2–#5. The kill mechanism is **not** the
daemon's exit and **not** the reaper; it is the service lifecycle's **ppid-tree
kill**.

### 4.1 Log timeline

```
22:12:44.188  Binary metadata changed, verifying with hash...
22:12:45.289  Detected wg binary change (old: 335903c28a64, new: 4ce11904920d), restarting service...
22:12:45.306  Shutting down coordinator agent 3/5/4 before exec-restart
22:12:45.306  Exec-replacing with: /home/bot/.cargo/bin/wg … service daemon …
22:12:45.352  Daemon starting (PID 1616959 …)          <-- exec in place, same PID
22:12:49.532  Coordinator tick #1 complete: agents_alive=2   <-- workers still alive
22:12:49.579  Supervisor: stop requested; signalling daemon to exit   <-- SIGTERM to supervisor
22:12:49.802  Daemon shutting down (clean sentinel)
22:12:49.837  Supervisor exiting
22:12:50.622  Supervisor starting (PID 2657810 …)     <-- new supervisor/daemon
22:12:51.296  [triage] Dead agent cleanup … agent-161, agent-163
22:12:51.858  reconcile lost calibration-batch-run + complete-the-first (agent-161/163)
...
22:13:01.295  Supervisor: stop requested; signalling daemon to exit
22:13:02.161  reconcile lost survey-how-flip + plugin-wakeup-labels (agent-164/165)
22:13:33.318  agent-165 completed_at (triage's grace-delayed Dead marking)
```

The workers were alive at 22:12:49.5 (`agents_alive=2`) and dead by 22:12:51 —
i.e. they died in the stop/restart window, not before it.

Note the registry records for the killed agents are consistent with daemon
children:
`agent-161 pid=2387186 pgid=2387186`, `agent-163 pid=2626370 pgid=2626370`,
`agent-165 pid=2659099 pgid=2659099` — each a session/group leader whose PPID was
the then-current daemon `1616959` (the daemon that ran from 19:34 until the
22:12 exec-restart; workers spawned after 19:34 are its PPID children).

### 4.2 The kill code path

`src/commands/service/mod.rs`:
- line 1821 — `run_start` "Kill existing daemon before starting a new one":
  `kill_process_graceful(state.pid, 5)?`
- lines 4255/4257 — `run_stop_inner` fallback: `kill_process_force(spid)` /
  `kill_process_graceful(spid, 5)` on the **supervisor** ("Kills the supervisor's
  whole descendant tree (daemon included)").
- lines 4273/4275 — same on `state.pid` (the daemon).
- lines 4299/4301 — orphan daemon/supervisor reaping.

`src/service/mod.rs`:
- `collect_process_descendants(root_pid)` (115-160) builds a `pid → ppid` map
  from `/proc/*/stat` and BFS-walks **children by PPID** — it explicitly does
  **not** consider session/process-group boundaries.
- `kill_process_graceful` (330-375) snapshots descendants, `SIGTERM`s the root
  **and every descendant**, waits, then `SIGKILL`s survivors.
- `kill_process_force` (410-425) does the same with `SIGKILL`.

So: `supervisor → daemon → worker` is a PPID chain. `kill_process_graceful(supervisor)`
(and `kill_process_graceful(daemon)`) reaches the worker through that chain and
kills it, **even though the worker called `setsid()` and lives in its own
session**. This is the exact class the task flagged as the reaper suspect; it is
not the new `reap_orphaned_process_group` reaper, but the older, broader
`kill_process_graceful` tree walk.

### 4.3 Direct reproduction of the semantics

A minimal synthetic tree (supervisor A → daemon B → worker C with `os.setsid()`)
run against the same `/proc` ppid-walk algorithm:

```
supervisor A pid = 2767840
descendants of A (ppid walk): [2767841, 2767842]
  pid 2767841 session 2767834 own-session? False
  pid 2767842 session 2767842 own-session? True     <-- detached worker
after tree SIGTERM: detached worker in its own session WAS killed
```

### 4.4 What does *not* kill a live worker (control experiments)

- `wg service stop` (kill_agents=false) on a live **standalone**-spawned worker:
  worker survived (`Service stopped … agents continue running`).
- Starting a brand-new daemon over a live registered worker: new daemon's triage
  ran (`agents_alive=1`) and left it alone; registry stayed `status=working`.

The difference is PPID. In those controls the worker had been spawned by a
standalone `wg spawn` CLI that exited, so it had already reparented to init and
was **not** a PPID descendant of the daemon. In the incident the workers were
spawned **by the daemon** (`coordinator.rs:2859`, in-process), so they *were*
descendants and were reached by the tree kill. The new daemon **already adopts**
live workers (§5.2); the restart simply destroys them first.

---

## 5. Does a restart orphan or fail an attempt? Adopt vs fail

### 5.1 Reconciler behaviour for a live worker

For a live agent the reconciler's predicate is false:
`agent.is_alive() && !is_process_alive(pid)` → `pid` alive → false; and status is
not `Dead`. So the task is left `InProgress` and dispatchable state is
preserved. **The reconciler already "adopts" a live PID.** There is no
adopt-vs-fail decision to change for the healthy case.

### 5.2 What the correct restart semantics must be

A daemon restart MUST:
1. **not signal worker PIDs or their PPID descendants** — the stop/restart path
   must kill the supervisor/daemon only, never the attached worker tree;
2. let the new daemon **re-attach** to already-running attempts: it already
   reloads `registry.json`, sees `status=Working` with a live `pid`, and
   `reconcile_orphaned_tasks` leaves them alone (verified in the control run);
   the wrapper is `setsid()`-detached precisely so it survives;
3. **fail-closed only when the process is genuinely gone** (then the attempt is
   truly unrecoverable without an explicit retry/continuation).

### 5.3 The residual ambiguity the current code gets wrong

- `reconcile_orphaned_tasks` uses only `kill(pid,0)`. It should confirm the PID is
  the **same process** using the existing `verify_process_identity` /
  `read_proc_start_ticks` marker, so PID reuse after a crash cannot silently
  adopt/fail the wrong process. (Triage already does this; the reconciler does
  not.)
- The `InProgress` + `assigned == None` branch has **no grace and no
  "is there a live owner?" probe**. A transient split-save / claim-without-binding
  projection is converted to a lost attempt in one tick (loss #1).

---

## 6. Proposed fix (scoped follow-up; NOT applied in this task)

Two independent, additive fixes. Both are production restart/reconcile-behaviour
changes, so per mandate they are proposed here and tracked as a follow-up task
rather than landed blind.

### Fix A (primary): service lifecycle must not tree-kill workers

Scope: `src/commands/service/mod.rs` (`run_start` line ~1821, `run_stop_inner`
~4255-4301) and/or `src/service/mod.rs` (`kill_process_graceful` /
`kill_process_force`).

Recommended shape: add a **session/role-aware** kill used by service lifecycle
that signals only the exact supervisor/daemon PIDs (SIGTERM, then SIGKILL on the
same PIDs), and **excludes any descendant that is a registered worker**
(session != root session is a sufficient structural test, since spawn always
`setsid`s the worker; an explicit registry-PID exclusion is even stronger).
Keep the broad ppid-tree kill for `wg kill` / hard-cancel, where killing
`setsid` descendants is the documented intent.

Regression test (credential-free, shell executor):
1. start a daemon in an isolated `HOME`/`--dir`;
2. let the daemon spawn a `sleep`-style shell worker (must be a daemon child);
3. `wg service restart` (or `stop` then `start`);
4. assert the worker PID survives and the task stays `InProgress`/adopted;
5. assert a subsequent `reconcile_orphaned_tasks`/tick does not mark it lost.

This is the test the control experiments above approximate; it needs a
deterministic way to let the daemon spawn a shell task (bypass the build-admission
deferral, e.g. an `--exec` fixture and admission disabled in the fixture).

### Fix B (defense-in-depth): close the zero-grace `assigned: None` hole

Scope: `src/commands/sweep.rs:reconcile_orphaned_tasks` (the `None =>` arm).

- For `InProgress` with `assigned == None`, do **not** immediately emit
  `AttemptLost`. Either (a) apply the same 5-minute `started_at` grace already
  used for the "absent from registry" case, or (b) emit a
  `ReconciliationIssue` (`stale-claim`) instead of `AttemptLost`, leaving it
  visibly blocked for explicit repair. Retry is cheap; destroying an attempt that
  may have a live worker (e.g. a binding write that raced) is not.
- Optionally use `verify_process_identity` in the `Some(agent)` arm so PID reuse
  is distinguished from process death.

Regression test: a unit test around `reconcile_orphaned_tasks` asserting an
`InProgress`, freshly-`started_at` task with `assigned=None` is **not** turned
into a lost attempt within the grace window (and a stale one beyond the window
is). This is small and testable via the existing `sweep.rs` test module.

### Fix C (policy/ordering, longer term): journal destructive side effects

`complete-the-first` shows that a lost attempt can leave the external world
half-mutated (deleted GitHub Release, fix stranded off-main). Independent of A/B:
destructive steps should be ordered late or journaled so a killed attempt is
recoverable. This is a design change; out of scope here, worth its own task.

### Operational question, explicitly answered

> Should a daemon restart be able to orphan and fail a running attempt, or must
> the new daemon re-attach to live workers?

**The new daemon must re-attach.** A restart is an internal infrastructure event,
not a task cancellation. The intended design already says so: workers are
`setsid()`-detached "so it survives daemon restart/crash"
(`execution.rs:2562-2564`), and `kill_agents=false` is the default. The current
stop/restart implementation violates that intent via the ppid-tree kill. The fix
is to make the implementation match the documented contract: restart preserves
live workers, the new daemon adopts them, and only a genuinely-dead PID fails the
attempt.

---

## Appendix: exact commands used

```bash
# graph evidence
cd /home/bot/wg
wg show calib-doc-list-audit-rows | plugin-wakeup-labels | survey-how-flip \
        | calibration-batch-run | complete-the-first
python3 - # dumped graph.jsonl log arrays and lifecycle for the five tasks

# daemon log evidence
grep -nE "binary change|Supervisor: stop requested|Dead agent cleanup|Reconciliation:" \
     .wg/service/daemon.log
sed -n '69407,69592p' .wg/service/daemon.log        # 22:12:45-22:13:07 sequence

# registry / proc
python3 - # printed pid/pgid/status/started_at for agent-161/163/164/165/169
ps -eo pid,ppid,pgid,sid,stat,cmd | grep agent-16x

# control experiments (isolated HOME + --dir)
wg init --executor shell --no-agency
wg add long --id long --exec 'sleep 600'
wg publish long --only
wg spawn long --executor shell
wg dead-agents --cleanup          # did NOT kill the live worker
wg service start && wg service stop   # live standalone worker survived
python3 - # synthetic supervisor->daemon->setsid-worker ppid-tree kill repro
```

## Appendix: source references

| Concern | Location |
|---|---|
| Reconciler + reason string | `src/commands/sweep.rs:418-560` (`process_identity_dead` at 528) |
| liveness probe | `src/service/mod.rs:78-80` (`is_process_alive`) |
| PID-reuse identity | `src/service/mod.rs:493-506`, `631-649`; `src/commands/service/triage.rs:202-229` |
| Dead-agent cleanup | `src/commands/dead_agents.rs:100-240` |
| In-process daemon spawn | `src/commands/service/coordinator.rs:2853-2860` |
| Worker `setsid` + pgid record | `src/commands/spawn/execution.rs:2562-2571`, `2844-2851` |
| Atomic claim binds `assigned` | `src/commands/spawn/execution.rs:927-1040` |
| ppid descendant walk | `src/service/mod.rs:115-160` |
| tree kill | `src/service/mod.rs:330-375` (graceful), `410-425` (force) |
| service lifecycle tree kill | `src/commands/service/mod.rs:1821`, `4255-4301` |
| pgid reaper (ruled out) | `src/service/mod.rs:263-296`; commit `60de44a8` |
