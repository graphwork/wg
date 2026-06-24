# Open-issue triage — graphwork/wg — 2026-06-24

Triage of all 15 open issues on `graphwork/wg` against **current `main`**
(`4893fded`). Each issue gets a disposition backed by concrete evidence (a
commit, a `file:line`, or a proven absence). Issues that can be **proven fixed**
are closed with a citing comment; everything else is left open and the
follow-up work is enumerated below.

Auth: `gh` authenticated as `ekg` with `repo` scope (close/comment allowed).

## Disposition summary

| # | Title | Disposition | Action |
|---|-------|-------------|--------|
| 4  | Coordinator tick silently fails to spawn agents | **FIXED** | closed |
| 5  | Orphaned in-progress tasks after daemon restart — no startup reconciliation | **FIXED** | closed |
| 6  | Daemon deadlock from hanging inline eval LLM calls | **FIXED** | closed |
| 7  | Feature: `wg heal` command for graph cleanup | **PARTIAL** | keep open |
| 8  | Feature: web dashboard for multi-repo workgraph monitoring | **STILL-VALID** | keep open |
| 9  | Feature: multi-executor task routing | **FIXED** (core) | closed |
| 26 | `wg gc`: garbage collect stale agent records and log files | **PARTIAL** | keep open |
| 35 | Add `--reason` flag to `wg resume` | **STILL-VALID** | keep open |
| 39 | `wg claim` is not atomic under concurrent claimers | **FIXED** | closed |
| 40 | Clarify/guarantee `wg add --id` dedup semantics | **FIXED** | closed |
| 41 | Daemon re-spawned a single task 113x — no per-task respawn cap | **FIXED** | closed |
| 44 | Dispatcher should not let stale lifecycle/system tasks starve user work | **PARTIAL** | keep open |
| 45 | Zero-output watchdog can false-kill active Claude workers | **PARTIAL** | keep open |
| 46 | Container epics should not block child tasks forever | **STILL-VALID** | keep open |
| 47 | Make non-cascading publish of root/epic tasks obvious | **PARTIAL** | keep open |

**Closed (proven fixed):** #4, #5, #6, #9, #39, #40, #41
**Left open (partial / still-valid):** #7, #8, #26, #35, #44, #45, #46, #47

---

## FIXED — closed with citing comment

### #4 — Coordinator tick silently fails to spawn agents — **FIXED**

The original failure (March 2026, pre-handler-first) was a spawn that logged
`Spawning agent for: X` and then silently did nothing — no agent dir, no error,
tick never completes. Current `main` has rebuilt the dispatch path so spawn
failures are **loud and recorded**, and the inline-eval deadlock that wedged the
tick is gone (see #6):

- Every spawn arm logs failures and records them on the task:
  `src/commands/service/coordinator.rs:4189` — `eprintln!("[dispatcher] Failed to
  spawn for {}: {}", task.id, e)` followed by `record_spawn_failure(...)`
  (`coordinator.rs:4191`). Same for the shell / eval / assignment / `plan_spawn`
  arms (`coordinator.rs:4008, 4060, 4087, 4157`).
- `record_spawn_failure` (`coordinator.rs:3865`) writes a `Spawn failed
  (attempt N/M): …` log entry to the task and increments `task.spawn_failures`,
  so a failing spawn is visible in `wg show` and trips the circuit breaker.
- Each spawn now emits a provenance line tracing the routing decision back to
  the config knob (`plan.provenance.log_line`, `coordinator.rs:4180`) —
  "eliminates silent-routing bugs."

Evidence the path was silent before: the single-source-of-truth `plan_spawn`
dispatch + provenance logging landed in `a7d0ab02 (spawn-single-source)` /
`3923c0bf (worker)`; `spawn_failures` accounting in `eb9617e7
(verify-circuit-breaker)`.

Disposition: **FIXED** — the "silent failure / no error logged / tick never
completes" symptom is structurally resolved. (If a *new* silent-spawn is ever
observed on current main, it should be re-filed with the daemon.log excerpt.)

### #5 — Orphaned in-progress tasks after daemon restart — **FIXED**

The issue asked for a startup reconciliation sweep that unclaims in-progress
tasks whose agents are dead. Current `main` does this **every coordinator
tick** (stronger than the proposed once-at-startup):

- `src/commands/sweep.rs:357` `reconcile_orphaned_tasks(dir, graph_path)` —
  finds `InProgress`/`Open` tasks whose assigned agent is `Dead`, alive-but-PID-
  gone, or missing, and unclaims them (resets to `Open`) under the graph lock.
- Wired into the tick: `src/commands/service/coordinator.rs:61` calls it inside
  `cleanup_and_count_alive`, logging `Reconciliation: recovered N orphaned
  task(s)`.
- Also exposed as the user-facing `wg sweep` / `wg sweep --dry-run`
  (`src/commands/sweep.rs` header).

This is exactly the proposed fix (detect in-progress task with no alive agent →
unclaim), implemented and run continuously.

Disposition: **FIXED**.

### #6 — Daemon deadlock from hanging inline eval LLM calls — **FIXED**

The issue: `spawn_eval_inline()` ran the eval LLM call synchronously in the tick,
so a hanging provider blocked the whole daemon. Current `main` **forks the eval
as a detached child and returns immediately**:

- `src/commands/service/coordinator.rs:3256-3315` — `spawn_eval_inline` builds a
  bash script, `Command::new("bash")` with null stdio, `pre_exec(setsid)` to
  detach into its own session, `cmd.spawn()`, then returns `(agent_id, pid)`.
  The LLM call runs in the child; the tick does not wait on it.
- The forked eval is registered in the agent registry
  (`register_agent_with_model`, `coordinator.rs:3306`) so it is subject to
  dead-agent detection and the zero-output watchdog (5-min kill,
  `src/commands/service/zero_output.rs:22`). A hung eval is reaped, not a
  deadlock.
- `spawn_assign_inline` (`coordinator.rs:3320`) uses the same detached-fork
  pattern, covering the assignment-call path the issue also flagged.

A hanging eval/assignment LLM call can no longer block the coordinator tick.

Disposition: **FIXED**.

### #9 — Multi-executor task routing — **FIXED (core)**

The core problem ("the daemon currently has one dispatch path: spawn a Claude
CLI process for each ready task") is resolved by the **handler-first** model-spec
work. Each task routes to the handler derived from its model spec:

- `src/dispatch/handler_for_model.rs` — the single source of truth mapping a
  model spec to a handler subprocess (`claude` / `codex` / `nex`/`native` /
  `pi` / `opencode`). The leading token of the spec is the handler.
- `src/dispatch/plan.rs:320` `plan_spawn` resolves `{executor, model, endpoint}`
  per task — **per-task `task.model` wins over the default**
  (`plan.rs:333`) — and is the *only* place a spawn decision is made. The
  dispatcher calls it per ready task (`coordinator.rs:4172`) and spawns via the
  resolved executor (`coordinator.rs:4183`).
- The proposed "executor field on tasks" is realized as the per-task model spec
  (handler-first); the legacy `--executor`/`-x` flag is deprecated in favor of
  it (see `CLAUDE.md`, `src/dispatch/handler_for_model.rs`).
- **Scheduled execution**: `not_before` timestamp gating
  (`src/query.rs:9` `is_time_ready`, `wg add --not-before` `src/cli.rs:349`,
  `wg reschedule` `src/cli.rs:1001`).
- **Worktree isolation**: enabled by default (`82c44384
  impl-agent-worktree`).

Design: `docs/design-handler-first-model-spec.md`; landed across `cc6976df
(design-handler-first)`, `30c8b724 / 8c7cce49 (implement-handler-first)`,
`5379a732 (fix-executor-model)`.

Disposition: **FIXED** for the core multi-runtime routing. Two narrower
sub-proposals from the original issue are *not* delivered and are noted in the
follow-up list as separate, smaller asks (they are narrower than the original
blocker): (a) an arbitrary **HTTP-endpoint executor type**
(`--executor http://…` POSTing a task payload to a generic service), and
(b) **cron-recurring** schedules on tasks (only one-shot `not_before` exists).

### #39 — `wg claim` not atomic under concurrent claimers — **FIXED**

Root cause confirmed at the reporter's commit and confirmed fixed on `main`:

- At reporter `45c4177b` (2026-03-14), `claim.rs` used **unlocked**
  `load_graph` + `save_graph` (`git show 45c4177b:src/commands/claim.rs` →
  lines 82/140) — a read-modify-write with no lock held across it, so concurrent
  claimers raced last-writer-wins (exactly the reported symptom).
- Current `src/commands/claim.rs:23` performs the claim inside `modify_graph`,
  which acquires an **exclusive blocking flock** for the whole read-check-write:
  `src/parser.rs:299` `FileLock::acquire(&lock_path)` → `src/parser.rs:30-31`
  `libc::LOCK_EX`. The status check (`Open|Blocked|Incomplete` → `InProgress`,
  `claim.rs:35-110`) is a compare-and-swap; a racer that arrives after the
  first claim sees `InProgress` and is rejected with "already claimed by @…".
- Converted to `modify_graph` in `a892a17b (mu-s-wg-user)`.

Caveat noted in the close comment: the flock is a **no-op on non-Unix
(Windows)** (`src/parser.rs:34-39`); the reporters were on macOS, where the
guarantee holds.

Disposition: **FIXED**.

### #40 — `wg add --id` dedup semantics / concurrency — **FIXED**

Same mechanism as #39:

- At `45c4177b`, `add.rs` used unlocked `load_graph`/`save_graph`
  (`git show 45c4177b:src/commands/add.rs`).
- Current `src/commands/add.rs:457` runs the create inside `modify_graph`
  (exclusive `LOCK_EX` flock); the duplicate check rejects inside the lock:
  `src/commands/add.rs:492` `"Task with ID '{}' already exists"`. Two concurrent
  `wg add --id X` therefore serialize → first creates, second is **rejected**
  (atomic reject-on-exists, not last-writer-wins). The `--subtask`/peer path is
  likewise locked (`add.rs:1002`/`1007`).

The reporter's question "could two concurrent `--id X` ever produce two records
or a corrupted entry?" is answered: **no**, given the exclusive lock + atomic
rename writes. (A docs note formalizing the reject-on-exists guarantee is a
small follow-up.)

Disposition: **FIXED**.

### #41 — Daemon re-spawned a single task 113x — no per-task respawn cap — **FIXED**

Current `main` has a multi-layer per-task respawn cap that dead-letters a wedged
task instead of respawning forever:

- **Rapid-respawn dead-letter** — `check_respawn_throttle`
  (`src/commands/service/coordinator.rs:3751`): after
  `RESPAWN_MAX_RAPID = 5` (`coordinator.rs:3736`) agent deaths within
  `RESPAWN_WINDOW_SECS = 300` (`coordinator.rs:3739`), the task is set
  `Failed` with reason "rapid respawn loop detected", with exponential backoff
  between respawns before that.
- **Spawn-failure circuit breaker** — `check_spawn_circuit_breaker`
  (`coordinator.rs:3840`) + `record_spawn_failure` (`coordinator.rs:3865`)
  stop spawning once `task.spawn_failures >= config.coordinator.max_spawn_failures`.
- **Zero-output circuit breaker** — `MAX_ZERO_OUTPUT_RESPAWNS = 2`
  (`src/commands/service/zero_output.rs:25`) marks the task
  `zero-output-circuit-broken`.
- All three are wired into the dispatch loop before spawn
  (`coordinator.rs:3960`, `3967`) and on every spawn error
  (`4008/4060/4087/4157/4191`).

A single task can no longer accumulate unbounded respawns. Landed in `f26a7a7e
(debug-agent-reaping)` (rapid-respawn) + `eb9617e7 (verify-circuit-breaker)`
(spawn-failure breaker).

Disposition: **FIXED**.

---

## PARTIAL — left open, follow-up enumerated

### #7 — `wg heal` umbrella command — **PARTIAL**

The constituent recovery operations exist as separate commands, but there is **no
unified `wg heal`** and one operation is missing:

- ✅ Unclaim orphaned in-progress → `wg sweep` (`src/commands/sweep.rs`), also
  run automatically every tick (see #5).
- ✅ Purge dead agents → `wg reap` (`src/commands/reap.rs`) and
  `wg dead-agents --purge [--delete-dirs]` (`src/commands/dead_agents.rs:308`).
- ✅ Remove terminal tasks / orphaned worktrees → `wg gc`, `wg gc --worktrees`
  (`src/commands/gc.rs`, `src/commands/worktree_gc.rs`).
- ❌ **Remove orphan/dangling dependency references** (deps pointing to
  non-existent task IDs) — not available as a command; dangling deps just block
  the dependent (`src/commands/coordinate.rs:479`).
- ❔ **Dedup duplicate graph.jsonl node IDs** — the loader keep-last behaviour
  is not surfaced as an explicit heal op.

Disposition: **PARTIAL** — building blocks exist; no single `wg heal` and no
orphan-dep cleanup command.

### #26 — `wg gc` for stale agent records + log files — **PARTIAL**

- ✅ Delete agent work dirs (`.wg/agents/<id>/`) →
  `wg dead-agents --purge --delete-dirs` (`src/commands/dead_agents.rs:308`,
  CLI `src/cli.rs:1976`).
- ✅ Prune old log files → `wg cleanup nightly` removes `.wg/logs` entries
  older than 30 days (`src/commands/cleanup.rs:822`).
- ✅ Orphaned worktree GC → `wg gc --worktrees` (`src/cli.rs:1127`).
- ❌ No single `wg gc [--older-than <days>] [--dry-run]` over agent records as
  the issue proposed; the agent-dir purge is **status-based** (dead/done/failed),
  not **age-based**, and is not wired into a configurable retention policy or
  into `wg service start`.

Disposition: **PARTIAL**.

### #44 — Lifecycle/system tasks starve user work — **PARTIAL**

`sort_tasks_by_priority_with_features` (`src/commands/service/coordinator.rs:3631`)
adds starvation prevention (24h priority bump), priority inheritance, CFS-like
fair-share by `dispatch_count`, and an idle-priority gate — but the issue's two
specific asks are **not** met:

- ❌ Disabled lifecycle flags (`auto_evaluate`/`flip_enabled`/`auto_assign =
  false`) do **not** suppress dispatch of *already-queued* `.evaluate-*` /
  `.flip-*` / `.assign-*` tasks. The flags gate scaffolding (task creation), not
  dispatch: the dispatch loop runs any ready task carrying the
  `evaluation`/`flip`/`assignment` tag regardless of the flag
  (`coordinator.rs:4031-4040`).
- ❌ Real user work does **not** sort ahead of lifecycle/system tasks at *equal*
  priority — at equal priority the tie-break is `dispatch_count`
  (`coordinator.rs:3686`), not task type, so a low-dispatch stale `.flip-*` can
  sort ahead of fresh user work. The 24h starvation bump is too slow for the
  "freshly published work starved by stale backlog" scenario.

Reference (downstream patch implementing both): Speedrift `d9adb66c`.

Disposition: **PARTIAL**.

### #45 — Zero-output watchdog false-kills active Claude workers — **PARTIAL**

Mitigations present but the two requested progress signals are **not** checked:

- ✅ Detection skips the kill if the agent has active child processes
  (`src/commands/service/zero_output.rs:220-222`,
  `has_active_children(agent.pid)`).
- ✅ Detection treats non-empty `raw_stream.jsonl` / `stream.jsonl` as output
  (`zero_output.rs:202-205`); per-task circuit breaker exists.
- ❌ Detection does **not** treat non-empty `output.log` as output — it only
  inspects `raw_stream.jsonl`/`stream.jsonl`, never `agent.output_file` content.
- ❌ Detection does **not** check recorded `worktree_path` filesystem progress
  before killing.

A `claude:opus` worker writing files into its worktree with a quiet stream and
no active child at the sample instant can still be killed at the
`ZERO_OUTPUT_KILL_THRESHOLD = 5 min` (`zero_output.rs:22`) — matching the
observed ~337s false-kill. Reference (downstream patch): Speedrift `d9adb66c`.

Disposition: **PARTIAL**.

### #47 — Make non-cascading publish obvious — **PARTIAL**

- ✅ The non-cascading behaviour exists and is documented: `wg publish --only`
  "Only publish this single task (skip subgraph propagation)"
  (`src/cli.rs:748-750`).
- ❌ No discoverable `--no-cascade` alias; no warning/summary when a publish is
  about to release descendants; no scoped "publish-wave" mode.

The issue itself allows a docs-only resolution if `--only` is preferred, so this
is close to resolvable — but the "make it obvious" + cascade-warning asks are
unmet, so it is left open. Reference (downstream patch): Speedrift `d9adb66c`.

Disposition: **PARTIAL**.

---

## STILL-VALID — left open

### #8 — Web dashboard for multi-repo workgraph monitoring — **STILL-VALID**

WG ships single-repo views (`wg html` static viewer `src/html.rs`, `wg tui`,
`wg viz`, `wg server`) and cross-repo federation primitives (`wg add --repo`,
`src/commands/quickstart.rs:646`), but there is **no multi-repo ecosystem hub**
with the JSON APIs the issue specifies (`/api/status`, `/api/repos`,
`/api/graph`, `/api/next-work`, `/api/pressure`, WebSocket `/ws/status`). This
is an unbuilt feature request; keep open as a roadmap item.

Disposition: **STILL-VALID** (feature not built).

### #35 — `wg resume --reason` — **STILL-VALID**

The `Resume` command exposes only `id` and `--only` (`src/cli.rs:731-738`); there
is no `--reason` flag, and `src/commands/resume.rs` has no resume-reason handling
or agent-context injection. The proposed mechanism (tagged resume-reason log
entry + prominent context injection to break pause/resume consistency-bias
loops) is unimplemented.

Disposition: **STILL-VALID**.

### #46 — Container epics should not block children forever — **STILL-VALID**

There is **no epic/container concept** anywhere in the source (no `epic` /
`container` / grouping-node handling). Readiness requires every blocker to be
terminal: `ready_tasks` / `blocker_satisfied_for_dependent`
(`src/query.rs:307`, `:350`) treat an `Open` blocker as not-satisfied
(`is_dep_satisfied` = Done/Failed/Abandoned), so a child `--after <epic>` stays
blocked until the epic is manually `wg done`. The requested semantics (epics
hidden from ready dispatch; open epics satisfy child readiness) are unimplemented.
Reference (downstream patch): Speedrift `d9adb66c`.

Disposition: **STILL-VALID**.

---

## Follow-up resolution tasks to create (for STILL-VALID / PARTIAL)

Not created here (per task scope — enumerate only):

1. **#44 — lifecycle starvation**: (a) filter disabled-lifecycle tasks
   (`auto_evaluate`/`flip_enabled`/`auto_assign = false`) out of the dispatch
   set, not only out of scaffolding; (b) make user work outrank
   lifecycle/system (`.flip-*`/`.assign-*`/`.evaluate-*`/`.drift-*`) tasks at
   equal priority in `sort_tasks_by_priority_with_features`. Port/adapt Speedrift
   `d9adb66c`. *(user-visible dispatcher behaviour — validate with a scripted
   daemon run, not only a unit sort test.)*
2. **#45 — zero-output watchdog**: add `output.log` non-empty + recorded
   `worktree_path` filesystem-progress-since-start checks to `check_zero_output`
   before killing. Port/adapt Speedrift `d9adb66c`. Add a smoke scenario.
3. **#46 — container epics**: introduce an `epic`/`container` grouping-node
   marker; hide grouping nodes from ready leaf dispatch and treat open grouping
   nodes as dependency-satisfied for child readiness.
4. **#47 — non-cascading publish**: add a `--no-cascade` alias for `--only` and
   print a "releasing N descendants" summary/warning on cascading publish.
5. **#35 — resume `--reason`**: add `wg resume --reason "..."`, persist a tagged
   resume-reason log entry, and inject it prominently into the next dispatched
   agent's context; optional `[resume] require_reason` config.
6. **#26 — agent-record GC**: add age-based GC of `.wg/agents/<id>/` (e.g.
   `wg gc --agents --older-than <days> [--dry-run]`) and a configurable
   retention policy optionally run by `wg service start`.
7. **#7 — `wg heal` umbrella**: a single command composing sweep + reap + gc +
   orphan-dependency-reference cleanup + graph.jsonl node-ID dedup, with
   `--dry-run` / `--json`.
8. **#9 (deferred sub-asks)**: (a) an HTTP-endpoint executor type
   (POST task payload to a generic service + completion callback); (b) cron-
   recurring schedules on tasks (beyond one-shot `not_before`). File as two
   focused issues if still wanted.
9. **#8 — multi-repo hub**: a `wg hub` (or companion) exposing the cross-repo
   JSON status/next-work/pressure APIs + optional dashboard. Large; design
   first.
10. **#40 (docs)**: document the `wg add --id` reject-on-exists idempotency
    guarantee (and its Unix-flock scope) in the manual.
